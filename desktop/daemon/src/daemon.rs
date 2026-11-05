//! The long-running service: owns the node, keeps paired phones connected, advertises
//! anonymously on the LAN, raises desktop notifications, and serves the control socket.

use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{bail, Context};
use phonelink_core::{lan::Lan, AppMessage, Event, Fingerprint, Node, NodeConfig, PeerRecord};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{broadcast, Notify},
    task::JoinHandle,
};
use tracing::{debug, info, warn};

use crate::{
    config::Config,
    ipc::{DeviceInfo, Reply, Request, WatchEvent},
    netaddrs,
};

const MAX_PAIR_TTL: Duration = Duration::from_secs(300);
const MAX_REQUEST_LINE: usize = 1 << 20;

pub struct DaemonOptions {
    pub data_dir: PathBuf,
    pub socket: PathBuf,
    pub config: Config,
    /// Where settings changed at runtime are saved (`None`: don't persist).
    pub config_path: Option<PathBuf>,
}

struct State {
    node: Node,
    port: u16,
    config: Config,
    config_path: Option<PathBuf>,
    notifications: std::sync::atomic::AtomicBool,
    watchers: broadcast::Sender<WatchEvent>,
    keepers: Mutex<HashMap<Fingerprint, JoinHandle<()>>>,
    advert_refresh: Arc<Notify>,
}

/// Run until `shutdown` resolves.
pub async fn run(opts: DaemonOptions, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    std::fs::create_dir_all(&opts.data_dir)?;
    std::fs::set_permissions(&opts.data_dir, std::fs::Permissions::from_mode(0o700))?;

    let listener = bind_control_socket(&opts.socket).await?;
    let (node, events) = start_node(&opts).await?;
    let port = node.local_addr()?.port();
    let (watchers, _) = broadcast::channel(256);
    let state = Arc::new(State {
        node: node.clone(),
        port,
        config: opts.config.clone(),
        config_path: opts.config_path.clone(),
        notifications: std::sync::atomic::AtomicBool::new(opts.config.notifications),
        watchers,
        keepers: Mutex::new(HashMap::new()),
        advert_refresh: Arc::new(Notify::new()),
    });
    info!(
        name = %opts.config.device_name(),
        id = %node.fingerprint().short(),
        port,
        socket = %opts.socket.display(),
        "phonelinkd running"
    );

    for peer in node.peers() {
        state.keep_connected(peer.fingerprint);
    }
    let lan = if opts.config.discovery {
        match Lan::new() {
            Ok(lan) => {
                lan.advertise(node.clone(), port, state.advert_refresh.clone());
                Some(lan)
            }
            Err(e) => {
                warn!("LAN discovery unavailable, relying on remembered addresses: {e}");
                None
            }
        }
    } else {
        None
    };

    tokio::spawn(event_pump(state.clone(), events));
    let server = tokio::spawn(serve(state.clone(), listener));

    shutdown.await;
    info!("shutting down");
    server.abort();
    node.shutdown().await;
    if let Some(lan) = lan {
        lan.shutdown();
    }
    let _ = std::fs::remove_file(&opts.socket);
    Ok(())
}

async fn start_node(
    opts: &DaemonOptions,
) -> anyhow::Result<(Node, tokio::sync::mpsc::UnboundedReceiver<Event>)> {
    let name = opts.config.device_name();
    let port = opts.config.port;
    // Dual-stack first (IPv4 + IPv6), plain IPv4 if IPv6 is disabled on this machine.
    let mut last_err = None;
    for bind in [
        SocketAddr::from(([0u16; 8], port)),
        SocketAddr::from(([0u8; 4], port)),
    ] {
        match Node::start(NodeConfig::new(
            name.clone(),
            Some(opts.data_dir.clone()),
            bind,
        ))
        .await
        {
            Ok(x) => return Ok(x),
            Err(e) => {
                debug!(%bind, "bind failed: {e}");
                last_err = Some(e);
            }
        }
    }
    Err(last_err
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow::anyhow!("bind failed")))
    .with_context(|| {
        format!("could not listen on UDP port {port} (is another phonelinkd running?)")
    })
}

async fn bind_control_socket(path: &Path) -> anyhow::Result<UnixListener> {
    if let Some(dir) = path.parent() {
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    if path.exists() {
        if UnixStream::connect(path).await.is_ok() {
            bail!("phonelinkd is already running ({})", path.display());
        }
        std::fs::remove_file(path)?; // stale socket from a crash
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

fn save_config(path: &Path, cfg: &Config) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string(cfg)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn my_uid() -> u32 {
    std::fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(u32::MAX)
}

async fn serve(state: Arc<State>, listener: UnixListener) {
    let uid = my_uid();
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        // Defence in depth on top of the 0600 file mode: the kernel tells us who connected.
        match stream.peer_cred() {
            Ok(cred) if cred.uid() == uid => {}
            other => {
                warn!("refused control connection from another user: {other:?}");
                continue;
            }
        }
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(state, stream).await {
                debug!("control client: {e}");
            }
        });
    }
}

fn device_info(node: &Node, p: &PeerRecord) -> DeviceInfo {
    DeviceInfo {
        id: p.fingerprint.short(),
        fingerprint: p.fingerprint.to_hex(),
        name: p.name.clone(),
        connected: node.is_connected(&p.fingerprint),
        pending: node.pending_count(&p.fingerprint).unwrap_or(0),
        last_addrs: p.last_addrs.iter().map(|a| a.to_string()).collect(),
    }
}

fn notify_desktop(enabled: bool, summary: String, body: String) {
    if !enabled {
        return;
    }
    tokio::task::spawn_blocking(move || {
        if let Err(e) = notify_rust::Notification::new()
            .appname("PhoneLink")
            .summary(&summary)
            .body(&body)
            .icon("smartphone")
            .show()
        {
            debug!("desktop notification failed: {e}");
        }
    });
}

impl State {
    fn keep_connected(&self, fp: Fingerprint) {
        let mut keepers = self.keepers.lock().unwrap_or_else(|e| e.into_inner());
        if keepers.get(&fp).is_some_and(|h| !h.is_finished()) {
            return;
        }
        keepers.insert(fp, self.node.maintain(fp, vec![]));
    }

    fn name_of(&self, fp: &Fingerprint) -> String {
        self.node
            .peer(fp)
            .map(|p| p.name)
            .unwrap_or_else(|| "Unknown device".into())
    }

    fn devices(&self) -> Vec<DeviceInfo> {
        self.node
            .peers()
            .iter()
            .map(|p| device_info(&self.node, p))
            .collect()
    }

    /// Accepts the short id (`3F2A-91C0-…`, any case, dashes optional), a fingerprint
    /// hex prefix of at least 4 characters, or the device name.
    fn resolve(&self, query: Option<&str>) -> Result<PeerRecord, String> {
        let peers = self.node.peers();
        let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) else {
            return match peers.as_slice() {
                [only] => Ok(only.clone()),
                [] => Err("no paired devices; run `phonelink pair` first".into()),
                _ => Err(
                    "several devices are paired; say which one (see `phonelink devices`)".into(),
                ),
            };
        };
        let norm = q.replace('-', "").to_lowercase();
        let hits: Vec<&PeerRecord> = peers
            .iter()
            .filter(|p| {
                p.name.eq_ignore_ascii_case(q)
                    || (norm.len() >= 4 && p.fingerprint.to_hex().starts_with(&norm))
            })
            .collect();
        match hits.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(format!("no paired device matches '{q}'")),
            _ => Err(format!("'{q}' matches several devices; be more specific")),
        }
    }
}

/// A phone that drops and returns within this window (network hand-over, duplicate
/// connection being resolved) doesn't get a second "connected" popup.
const RECONNECT_QUIET: Duration = Duration::from_secs(10);
/// The phone always connects right after pairing; dialling it at the same moment would
/// only create a duplicate connection that then has to be resolved.
const DIAL_AFTER_PAIRING: Duration = Duration::from_secs(4);

async fn event_pump(state: Arc<State>, mut events: tokio::sync::mpsc::UnboundedReceiver<Event>) {
    let mut last_disconnect: HashMap<Fingerprint, std::time::Instant> = HashMap::new();
    while let Some(ev) = events.recv().await {
        let notify = state
            .notifications
            .load(std::sync::atomic::Ordering::Relaxed);
        let watch = match ev {
            Event::Paired { peer, name } => {
                let delayed = state.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(DIAL_AFTER_PAIRING).await;
                    if delayed.node.peer(&peer).is_some() {
                        delayed.keep_connected(peer);
                    }
                });
                state.advert_refresh.notify_one();
                notify_desktop(
                    notify,
                    "Device paired".into(),
                    format!("{name} can now connect to this computer."),
                );
                WatchEvent::Paired {
                    id: peer.short(),
                    name,
                }
            }
            Event::PeerConnected { peer, name, .. } => {
                let blip = last_disconnect
                    .get(&peer)
                    .is_some_and(|t| t.elapsed() < RECONNECT_QUIET);
                notify_desktop(
                    notify && !blip,
                    format!("{name} connected"),
                    "PhoneLink is active.".into(),
                );
                WatchEvent::Connected {
                    id: peer.short(),
                    name,
                }
            }
            Event::PeerDisconnected { peer } => {
                last_disconnect.insert(peer, std::time::Instant::now());
                WatchEvent::Disconnected {
                    id: peer.short(),
                    name: state.name_of(&peer),
                }
            }
            Event::Message { peer, body, .. } => match AppMessage::decode(&body) {
                AppMessage::Text { text } => {
                    let name = state.name_of(&peer);
                    notify_desktop(notify, format!("Message from {name}"), text.clone());
                    WatchEvent::Text {
                        id: peer.short(),
                        name,
                        text,
                    }
                }
                AppMessage::Unsupported => {
                    debug!("message type not supported by this version; ignored");
                    continue;
                }
            },
        };
        let _ = state.watchers.send(watch);
    }
}

async fn write_reply(w: &mut (impl AsyncWriteExt + Unpin), reply: &Reply) -> anyhow::Result<()> {
    let mut line = serde_json::to_vec(reply)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await?;
    Ok(())
}

async fn handle_client(state: Arc<State>, stream: UnixStream) -> anyhow::Result<()> {
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r);
    let mut line = String::new();
    loop {
        line.clear();
        if lines.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        if line.len() > MAX_REQUEST_LINE {
            bail!("request too large");
        }
        let req: Request = match serde_json::from_str(line.trim()) {
            Ok(r) => r,
            Err(e) => {
                write_reply(
                    &mut w,
                    &Reply::Error {
                        message: format!("bad request: {e}"),
                    },
                )
                .await?;
                continue;
            }
        };
        match req {
            Request::SetNotifications { enabled } => {
                state
                    .notifications
                    .store(enabled, std::sync::atomic::Ordering::Relaxed);
                let reply = match &state.config_path {
                    Some(path) => {
                        let mut cfg = Config::load(path).unwrap_or_else(|_| state.config.clone());
                        cfg.notifications = enabled;
                        match save_config(path, &cfg) {
                            Ok(()) => Reply::Done,
                            Err(e) => Reply::Error {
                                message: format!(
                                    "saved for now, but could not write {}: {e}",
                                    path.display()
                                ),
                            },
                        }
                    }
                    None => Reply::Done,
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::Status => {
                let node = &state.node;
                let reply = Reply::Status {
                    name: state.config.device_name(),
                    id: node.fingerprint().short(),
                    fingerprint: node.fingerprint().to_hex(),
                    port: state.port,
                    devices: state.devices(),
                    notifications: state
                        .notifications
                        .load(std::sync::atomic::Ordering::Relaxed),
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::Devices => {
                write_reply(
                    &mut w,
                    &Reply::Devices {
                        devices: state.devices(),
                    },
                )
                .await?
            }
            Request::Unpair { device } => {
                let reply = match state.resolve(Some(&device)) {
                    Ok(p) => match state.node.unpair(&p.fingerprint) {
                        Ok(_) => {
                            if let Some(h) = state
                                .keepers
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .remove(&p.fingerprint)
                            {
                                h.abort();
                            }
                            state.advert_refresh.notify_one();
                            Reply::Done
                        }
                        Err(e) => Reply::Error {
                            message: e.to_string(),
                        },
                    },
                    Err(message) => Reply::Error { message },
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::SendText { device, text } => {
                let reply = match state.resolve(device.as_deref()) {
                    Ok(p) => {
                        let body = AppMessage::Text { text }.encode()?;
                        match state.node.send(&p.fingerprint, body) {
                            Ok(id) => Reply::Sent {
                                message_id: format!("{id:?}"),
                                delivered_now: state.node.is_connected(&p.fingerprint),
                            },
                            Err(e) => Reply::Error {
                                message: e.to_string(),
                            },
                        }
                    }
                    Err(message) => Reply::Error { message },
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::Watch => {
                let mut rx = state.watchers.subscribe();
                loop {
                    line.clear();
                    tokio::select! {
                        ev = rx.recv() => match ev {
                            Ok(event) => write_reply(&mut w, &Reply::Event { event }).await?,
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(_) => return Ok(()),
                        },
                        n = lines.read_line(&mut line) => if n.unwrap_or(0) == 0 { return Ok(()) },
                    }
                }
            }
            Request::Pair { ttl_secs } => {
                let ttl = Duration::from_secs(ttl_secs.max(10)).min(MAX_PAIR_TTL);
                let mut rx = state.watchers.subscribe(); // before opening: can't miss the event
                let addrs = netaddrs::lan_addrs(state.port);
                if addrs.is_empty() {
                    let message = "this computer has no LAN address; connect to Wi-Fi (or your phone's hotspot) first";
                    write_reply(
                        &mut w,
                        &Reply::Error {
                            message: message.into(),
                        },
                    )
                    .await?;
                    continue;
                }
                let offer = state.node.start_pairing(ttl, addrs.clone());
                write_reply(
                    &mut w,
                    &Reply::PairStarted {
                        uri: offer.to_uri(),
                        id: state.node.fingerprint().short(),
                        addrs: addrs.iter().map(|a| a.to_string()).collect(),
                        expires_in_secs: ttl.as_secs(),
                    },
                )
                .await?;
                let deadline = tokio::time::sleep(ttl);
                tokio::pin!(deadline);
                let outcome = loop {
                    line.clear();
                    tokio::select! {
                        _ = &mut deadline => break Reply::PairExpired,
                        ev = rx.recv() => {
                            if let Ok(WatchEvent::Paired { id, .. }) = ev {
                                if let Some(p) = state.node.peers().iter().find(|p| p.fingerprint.short() == id) {
                                    break Reply::Paired { device: device_info(&state.node, p) };
                                }
                            }
                        }
                        n = lines.read_line(&mut line) => if n.unwrap_or(0) == 0 {
                            // The user hit Ctrl-C: never leave the window open unattended.
                            state.node.cancel_pairing();
                            return Ok(());
                        },
                    }
                };
                state.node.cancel_pairing();
                write_reply(&mut w, &outcome).await?;
            }
        }
    }
}
