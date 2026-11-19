//! Local control socket: `phonelink touchpad …` (through `phonelink-touchpad`) and the
//! EchoTwin window talk to the running service here.
//!
//! `/run/phonelink-touchpad/control.sock` (`PHONELINK_TOUCHPAD_SOCKET` overrides it); the
//! request and reply types are `phonelink_core::touchpad::control`. Any local program may
//! connect, list phones and read the switch. Pairing or forgetting a phone needs polkit's
//! approval for the connecting process (your password); switching the touchpad on or off
//! needs polkit too, which allows it without a password for your active desktop session
//! only. One JSON request line in, JSON reply lines out. A client that leaves while its
//! pairing code is shown cancels the pairing.

use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, Context};
use phonelink_core::{
    touchpad::{
        self,
        control::{socket_path, Device, Reply, Request},
    },
    Fingerprint, Node,
};
use qrcode::{render::unicode::Dense1x2, QrCode};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{unix::OwnedReadHalf, UnixListener, UnixStream},
    sync::{broadcast, watch},
    task::JoinHandle,
};
use tracing::{debug, info, warn};

use crate::{netaddrs, polkit};

/// How long a pairing QR code works.
const PAIRING_TTL: Duration = Duration::from_secs(120);

/// The touchpad's on/off switch and the typing switch, saved in the state directory
/// (`settings.json`).
pub struct Switch {
    path: PathBuf,
    tx: watch::Sender<bool>,
    typing: std::sync::atomic::AtomicBool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Settings {
    enabled: bool,
    #[serde(default = "yes")]
    typing: bool,
}

fn yes() -> bool {
    true
}

impl Switch {
    /// Load it (on when there is no file yet, or it can't be read).
    pub fn load(dir: &std::path::Path) -> Self {
        let path = dir.join("settings.json");
        let saved = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok());
        let enabled = saved.as_ref().is_none_or(|s| s.enabled);
        let typing = saved.as_ref().is_none_or(|s| s.typing);
        Self { path, tx: watch::channel(enabled).0, typing: typing.into() }
    }

    /// Typing from the phone allowed?
    pub fn typing(&self) -> bool {
        self.typing.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn on(&self) -> bool {
        *self.tx.borrow()
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.tx.subscribe()
    }

    fn save(&self, s: &Settings) -> anyhow::Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(s)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn set(&self, on: bool) -> anyhow::Result<()> {
        self.save(&Settings { enabled: on, typing: self.typing() })?;
        self.tx.send_replace(on);
        Ok(())
    }

    fn set_typing(&self, on: bool) -> anyhow::Result<()> {
        self.save(&Settings { enabled: self.on(), typing: on })?;
        self.typing.store(on, std::sync::atomic::Ordering::Relaxed);
        // Wake the service so it can drop (or not) its keyboard.
        self.tx.send_modify(|_| {});
        Ok(())
    }
}

/// What every control client shares.
pub struct Shared {
    pub node: Node,
    pub port: u16,
    pub paired: broadcast::Sender<String>,
    pub switch: Switch,
}

pub async fn serve(shared: Arc<Shared>) -> anyhow::Result<JoinHandle<()>> {
    let path = socket_path();
    let dir = path.parent().expect("has a parent");
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))?;
    }
    if path.exists() {
        if UnixStream::connect(&path).await.is_ok() {
            bail!("the touchpad service is already running ({})", path.display());
        }
        std::fs::remove_file(&path)?; // stale socket from a crash
    }
    let listener = UnixListener::bind(&path)?;
    // Everyone may connect: what they may do is decided per request (polkit).
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))?;
    Ok(tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(e) = handle(stream, shared).await {
                    debug!("control client: {e}");
                }
            });
        }
    }))
}

async fn send(w: &mut (impl AsyncWriteExt + Unpin), reply: &Reply) -> anyhow::Result<()> {
    let mut line = serde_json::to_vec(reply)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    Ok(())
}

fn devices(node: &Node) -> Vec<Device> {
    node.peers()
        .into_iter()
        .map(|p| Device {
            connected: node.is_connected(&p.fingerprint),
            name: p.name,
            fingerprint: p.fingerprint.to_hex(),
        })
        .collect()
}

/// Resolves when the client closes its end (it sends nothing after its request).
async fn client_left(r: &mut OwnedReadHalf) {
    let mut buf = [0u8; 64];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {} // ignore anything else it sends
        }
    }
}

async fn handle(stream: UnixStream, shared: Arc<Shared>) -> anyhow::Result<()> {
    let cred = stream.peer_cred()?;
    let (mut r, mut w) = stream.into_split();
    // Read the request line byte by byte, so nothing after it is swallowed by a buffer
    // (the rest of the connection only tells us when the client leaves).
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while line.len() < 4096 {
        if r.read(&mut byte).await? == 0 || byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
    }
    let req: Request = serde_json::from_slice(&line)?;
    let node = &shared.node;

    let action = match req {
        Request::Pair | Request::Unpair { .. } => Some(polkit::MANAGE),
        Request::SetEnabled { .. } | Request::SetTyping { .. } => Some(polkit::SWITCH),
        Request::Devices | Request::Status => None,
    };
    if let Some(action) = action {
        let allowed = match cred.pid() {
            Some(pid) => polkit::authorize(pid, cred.uid(), action).await.unwrap_or_else(|e| {
                warn!("polkit check failed: {e:#}");
                false
            }),
            None => false,
        };
        if !allowed {
            info!(uid = cred.uid(), pid = ?cred.pid(), action, "refused: not authorised");
            let message = if action == polkit::MANAGE {
                "not authorised (pairing and unpairing need your password)"
            } else {
                "not authorised (only your own desktop session can switch the touchpad)"
            };
            return send(&mut w, &Reply::Error { message: message.into() }).await;
        }
    }

    match req {
        Request::Pair => {
            let mut paired = shared.paired.subscribe();
            let offer = node.start_pairing(PAIRING_TTL, netaddrs::lan_addrs(shared.port));
            if offer.addrs.is_empty() {
                node.cancel_pairing();
                return send(&mut w, &Reply::Error {
                    message: "this laptop has no network address; connect to Wi-Fi first".into(),
                })
                .await;
            }
            send(&mut w, &Reply::PairingCode {
                uri: touchpad::to_pair_uri(&offer),
                seconds: PAIRING_TTL.as_secs(),
            })
            .await?;
            let reply = tokio::select! {
                got = tokio::time::timeout(PAIRING_TTL, paired.recv()) => match got {
                    Ok(Ok(name)) => Reply::Paired { name },
                    _ => {
                        node.cancel_pairing();
                        Reply::PairingExpired
                    }
                },
                _ = client_left(&mut r) => {
                    node.cancel_pairing();
                    info!("pairing cancelled: the client left");
                    return Ok(());
                }
            };
            send(&mut w, &reply).await
        }
        Request::Devices => send(&mut w, &Reply::Devices { devices: devices(node) }).await,
        Request::Status => {
            send(&mut w, &Reply::Status {
                enabled: shared.switch.on(),
                typing: shared.switch.typing(),
                devices: devices(node),
            })
            .await
        }
        Request::SetEnabled { on } => {
            let reply = match shared.switch.set(on) {
                Ok(()) => {
                    info!(on, "touchpad switched");
                    Reply::Enabled { on }
                }
                Err(e) => Reply::Error { message: format!("could not save the setting: {e}") },
            };
            send(&mut w, &reply).await
        }
        Request::SetTyping { on } => {
            let reply = match shared.switch.set_typing(on) {
                Ok(()) => {
                    info!(on, "typing switched");
                    Reply::Typing { on }
                }
                Err(e) => Reply::Error { message: format!("could not save the setting: {e}") },
            };
            send(&mut w, &reply).await
        }
        Request::Unpair { who } => {
            let who_l = who.to_lowercase();
            let matches: Vec<_> = node
                .peers()
                .into_iter()
                .filter(|p| p.name.to_lowercase() == who_l || p.fingerprint.to_hex().starts_with(&who_l))
                .collect();
            let reply = match matches.as_slice() {
                [p] => {
                    node.unpair(&p.fingerprint)?;
                    info!(name = %p.name, "unpaired");
                    Reply::Unpaired { name: p.name.clone() }
                }
                [] => Reply::Error { message: format!("no paired phone matches {who:?}") },
                _ => Reply::Error {
                    message: format!("{who:?} matches several phones; use more of the fingerprint"),
                },
            };
            send(&mut w, &reply).await
        }
    }
}

// ---------------------------------------------------------------- client side

type Lines = tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>;

async fn request(req: &Request) -> anyhow::Result<(Lines, tokio::net::unix::OwnedWriteHalf)> {
    let path = socket_path();
    let stream = UnixStream::connect(&path).await.with_context(|| {
        format!(
            "the touchpad service is not running ({}). Install it with packaging/install-touchpad.sh, or start it with: sudo systemctl start phonelink-touchpad",
            path.display()
        )
    })?;
    let (r, mut w) = stream.into_split();
    let mut line = serde_json::to_vec(req)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    // The write half stays open: closing it would tell the service we left.
    Ok((BufReader::new(r).lines(), w))
}

async fn next(lines: &mut Lines) -> anyhow::Result<Reply> {
    let line = lines.next_line().await?.context("the touchpad service closed the connection")?;
    Ok(serde_json::from_str(&line)?)
}

fn qr(uri: &str) -> anyhow::Result<String> {
    let code = QrCode::new(uri.as_bytes())?;
    // Inverted colours: renders correctly on dark terminal themes.
    Ok(code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

/// `show_uri`: also print the code as text (for `fake_touchpad_phone` and debugging).
pub async fn client_pair(show_uri: bool) -> anyhow::Result<()> {
    let (mut lines, _w) = request(&Request::Pair).await?;
    loop {
        match next(&mut lines).await? {
            Reply::PairingCode { uri, seconds } => {
                println!("{}", qr(&uri)?);
                if show_uri {
                    println!("{uri}");
                }
                println!("Scan this in the Touchpad tab of the EchoTwin app (valid for {seconds} s).");
            }
            Reply::Paired { name } => {
                println!("Paired with {name}.");
                return Ok(());
            }
            Reply::PairingExpired => bail!("the code expired; run `phonelink touchpad pair` again"),
            Reply::Error { message } => bail!("{message}"),
            _ => bail!("unexpected reply"),
        }
    }
}

fn print_devices(devices: &[Device]) -> anyhow::Result<()> {
    if devices.is_empty() {
        println!("No phones paired with the touchpad yet. Run: phonelink touchpad pair");
    }
    for d in devices {
        let fp: Fingerprint = Fingerprint::from_hex(&d.fingerprint)?;
        println!("{}  {}  {}", if d.connected { "●" } else { "○" }, fp.short(), d.name);
    }
    Ok(())
}

pub async fn client_devices() -> anyhow::Result<()> {
    let (mut lines, _w) = request(&Request::Devices).await?;
    match next(&mut lines).await? {
        Reply::Devices { devices } => print_devices(&devices),
        Reply::Error { message } => bail!("{message}"),
        _ => bail!("unexpected reply"),
    }
}

pub async fn client_status() -> anyhow::Result<()> {
    let (mut lines, _w) = request(&Request::Status).await?;
    match next(&mut lines).await? {
        Reply::Status { enabled, typing, devices } => {
            println!("Touchpad: {}", if enabled { "on" } else { "off (the phone's Touchpad tab is refused)" });
            println!("Typing:   {}", if typing { "on" } else { "off (the phone's keyboard is refused)" });
            print_devices(&devices)
        }
        Reply::Error { message } => bail!("{message}"),
        _ => bail!("unexpected reply"),
    }
}

pub async fn client_set_enabled(on: bool) -> anyhow::Result<()> {
    let (mut lines, _w) = request(&Request::SetEnabled { on }).await?;
    match next(&mut lines).await? {
        Reply::Enabled { on } => {
            println!("Touchpad {}.", if on { "on" } else { "off" });
            Ok(())
        }
        Reply::Error { message } => bail!("{message}"),
        _ => bail!("unexpected reply"),
    }
}

pub async fn client_set_typing(on: bool) -> anyhow::Result<()> {
    let (mut lines, _w) = request(&Request::SetTyping { on }).await?;
    match next(&mut lines).await? {
        Reply::Typing { on } => {
            println!("Typing from the phone {}.", if on { "on" } else { "off" });
            Ok(())
        }
        Reply::Error { message } => bail!("{message}"),
        _ => bail!("unexpected reply"),
    }
}

pub async fn client_unpair(who: &str) -> anyhow::Result<()> {
    let (mut lines, _w) = request(&Request::Unpair { who: who.into() }).await?;
    match next(&mut lines).await? {
        Reply::Unpaired { name } => println!("Forgot {name}."),
        Reply::Error { message } => bail!("{message}"),
        _ => bail!("unexpected reply"),
    }
    Ok(())
}
