//! Local control socket: `phonelink touchpad pair` / `devices` / `unpair` (forwarded to
//! `phonelink-touchpad`) talk to the running service.
//!
//! `/run/phonelink-touchpad/control.sock` (`PHONELINK_TOUCHPAD_SOCKET` overrides it). Any local program may
//! connect and list devices, but pairing or forgetting a phone needs polkit's approval for
//! the connecting process (your password). One JSON request line in, JSON reply lines out.

use std::{os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};

use anyhow::{bail, Context};
use qrcode::{render::unicode::Dense1x2, QrCode};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::broadcast,
    task::JoinHandle,
};
use phonelink_core::{touchpad, Fingerprint, Node};
use tracing::{debug, info, warn};

use crate::{netaddrs, polkit};

/// How long a pairing QR code works.
const PAIRING_TTL: Duration = Duration::from_secs(120);

#[derive(Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Request {
    Pair,
    Devices,
    Unpair { who: String },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
enum Reply {
    PairingCode { uri: String, seconds: u64 },
    Paired { name: String },
    PairingExpired,
    Devices { devices: Vec<Device> },
    Unpaired { name: String },
    Error { message: String },
}

#[derive(Serialize, Deserialize)]
struct Device {
    name: String,
    fingerprint: String,
    connected: bool,
}

fn socket_path() -> PathBuf {
    std::env::var_os("PHONELINK_TOUCHPAD_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/phonelink-touchpad/control.sock"))
}

pub async fn serve(
    node: Node,
    port: u16,
    paired: broadcast::Sender<String>,
) -> anyhow::Result<JoinHandle<()>> {
    let path = socket_path();
    let dir = path.parent().expect("has a parent");
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
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
            let (node, paired) = (node.clone(), paired.subscribe());
            tokio::spawn(async move {
                if let Err(e) = handle(stream, node, port, paired).await {
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

async fn handle(
    stream: UnixStream,
    node: Node,
    port: u16,
    mut paired: broadcast::Receiver<String>,
) -> anyhow::Result<()> {
    let cred = stream.peer_cred()?;
    let (r, mut w) = stream.into_split();
    let mut line = String::new();
    BufReader::new(r.take(4096)).read_line(&mut line).await?;
    let req: Request = serde_json::from_str(&line)?;

    if matches!(req, Request::Pair | Request::Unpair { .. }) {
        let allowed = match cred.pid() {
            Some(pid) => polkit::authorize(pid, cred.uid()).await.unwrap_or_else(|e| {
                warn!("polkit check failed: {e:#}");
                false
            }),
            None => false,
        };
        if !allowed {
            info!(uid = cred.uid(), pid = ?cred.pid(), "refused: not authorised to manage phones");
            return send(&mut w, &Reply::Error {
                message: "not authorised (pairing and unpairing need your password)".into(),
            })
            .await;
        }
    }

    match req {
        Request::Pair => {
            let offer = node.start_pairing(PAIRING_TTL, netaddrs::lan_addrs(port));
            if offer.addrs.is_empty() {
                node.cancel_pairing();
                return send(&mut w, &Reply::Error {
                    message: "this laptop has no network address; connect to Wi-Fi first".into(),
                })
                .await;
            }
            send(&mut w, &Reply::PairingCode { uri: touchpad::to_pair_uri(&offer), seconds: PAIRING_TTL.as_secs() })
                .await?;
            let reply = match tokio::time::timeout(PAIRING_TTL, paired.recv()).await {
                Ok(Ok(name)) => Reply::Paired { name },
                _ => {
                    node.cancel_pairing();
                    Reply::PairingExpired
                }
            };
            send(&mut w, &reply).await
        }
        Request::Devices => {
            let devices = node
                .peers()
                .into_iter()
                .map(|p| Device {
                    connected: node.is_connected(&p.fingerprint),
                    name: p.name,
                    fingerprint: p.fingerprint.to_hex(),
                })
                .collect();
            send(&mut w, &Reply::Devices { devices }).await
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

async fn request(req: &Request) -> anyhow::Result<tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>> {
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
    Ok(BufReader::new(r).lines())
}

async fn next(lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>) -> anyhow::Result<Reply> {
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
    let mut lines = request(&Request::Pair).await?;
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

pub async fn client_devices() -> anyhow::Result<()> {
    let mut lines = request(&Request::Devices).await?;
    match next(&mut lines).await? {
        Reply::Devices { devices } if devices.is_empty() => {
            println!("No phones paired with the touchpad yet. Run: phonelink touchpad pair")
        }
        Reply::Devices { devices } => {
            for d in devices {
                let fp: Fingerprint = Fingerprint::from_hex(&d.fingerprint)?;
                println!(
                    "{}  {}  {}",
                    if d.connected { "●" } else { "○" },
                    fp.short(),
                    d.name
                );
            }
        }
        Reply::Error { message } => bail!("{message}"),
        _ => bail!("unexpected reply"),
    }
    Ok(())
}

pub async fn client_unpair(who: &str) -> anyhow::Result<()> {
    let mut lines = request(&Request::Unpair { who: who.into() }).await?;
    match next(&mut lines).await? {
        Reply::Unpaired { name } => println!("Forgot {name}."),
        Reply::Error { message } => bail!("{message}"),
        _ => bail!("unexpected reply"),
    }
    Ok(())
}
