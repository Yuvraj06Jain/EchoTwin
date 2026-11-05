//! The long-running service: owns the node, keeps paired phones connected, advertises
//! anonymously on the LAN, raises desktop notifications, and serves the control socket.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
    net::SocketAddr,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{bail, Context};
use base64::Engine;
use phonelink_core::{
    lan::Lan, message::MAX_NOTIFICATION_TEXT, AppMessage, Event, Fingerprint, Node, NodeConfig,
    PeerRecord, CAP_NOTIFICATIONS,
};
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
    notifications::{self, Effect, NotificationInfo, PopupCmd, Popups, Store},
};

const MAX_PAIR_TTL: Duration = Duration::from_secs(300);
const MAX_REQUEST_LINE: usize = 1 << 20;

pub struct DaemonOptions {
    pub data_dir: PathBuf,
    pub socket: PathBuf,
    pub config: Config,
    /// Where settings changed at runtime are saved (`None`: don't persist).
    pub config_path: Option<PathBuf>,
    /// Desktop popups; `None` starts the real D-Bus one. Tests pass a recorder.
    pub popups: Option<Popups>,
}

/// Settings that can change while running (mirrored to config.toml).
struct Settings {
    notifications: bool,
    sync_dismissals: bool,
    muted: HashSet<String>,
}

struct State {
    node: Node,
    port: u16,
    config: Config,
    config_path: Option<PathBuf>,
    settings: Mutex<Settings>,
    store: Mutex<Store>,
    popups: Popups,
    icons_dir: PathBuf,
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
        settings: Mutex::new(Settings {
            notifications: opts.config.notifications,
            sync_dismissals: opts.config.sync_dismissals,
            muted: opts.config.muted_apps.iter().cloned().collect(),
        }),
        store: Mutex::new(Store::default()),
        popups: opts.popups.clone().unwrap_or_else(Popups::start),
        icons_dir: opts.data_dir.join("icons"),
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
        let mut cfg = NodeConfig::new(name.clone(), Some(opts.data_dir.clone()), bind);
        cfg.capabilities.push(CAP_NOTIFICATIONS.to_string());
        match Node::start(cfg).await {
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

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The phone's own key inside our `<device>/<key>` id.
fn phone_key(id: &str) -> &str {
    id.split_once('/').map(|(_, k)| k).unwrap_or(id)
}

impl State {
    fn notifications_on(&self) -> bool {
        lock(&self.settings).notifications
    }

    fn sync_on(&self) -> bool {
        lock(&self.settings).sync_dismissals
    }

    fn is_muted(&self, package: &str) -> bool {
        lock(&self.settings).muted.contains(package)
    }

    /// Change a setting in config.toml (when there is one).
    fn persist(&self, change: impl FnOnce(&mut Config)) -> Reply {
        let Some(path) = &self.config_path else {
            return Reply::Done;
        };
        let mut cfg = Config::load(path).unwrap_or_else(|_| self.config.clone());
        change(&mut cfg);
        match save_config(path, &cfg) {
            Ok(()) => Reply::Done,
            Err(e) => Reply::Error {
                message: format!("saved for now, but could not write {}: {e}", path.display()),
            },
        }
    }

    fn maybe_popup(&self, info: &NotificationInfo) {
        let fresh = now_ms().saturating_sub(info.posted_ms) <= notifications::POPUP_MAX_AGE_MS;
        if !fresh || !self.notifications_on() || self.is_muted(&info.package) {
            return;
        }
        self.popups.send(PopupCmd::Show {
            id: info.id.clone(),
            app: info.app.clone(),
            title: notifications::escape_markup(&info.title),
            body: notifications::escape_markup(&info.text),
            icon: notifications::icon_path(&self.icons_dir, &info.package),
        });
    }

    fn broadcast(&self, ev: WatchEvent) {
        let _ = self.watchers.send(ev);
    }

    /// Send a request about a notification back to the phone it came from.
    fn to_phone(&self, device: &Fingerprint, msg: AppMessage) -> Result<bool, String> {
        let body = msg.encode().map_err(|e| e.to_string())?;
        self.node.send(device, body).map_err(|e| e.to_string())?;
        Ok(self.node.is_connected(device))
    }

    fn visible_notifications(&self) -> (Vec<NotificationInfo>, Vec<notifications::AppInfo>) {
        let store = lock(&self.store);
        let settings = lock(&self.settings);
        let items = store
            .list()
            .into_iter()
            .filter(|n| !settings.muted.contains(&n.package))
            .collect();
        let apps = store.apps(&|p| settings.muted.contains(p));
        (items, apps)
    }

    fn app_icons(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let Ok(entries) = std::fs::read_dir(&self.icons_dir) else {
            return out;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(package) = name.strip_suffix(".png") else {
                continue;
            };
            if !notifications::safe_package(package) {
                continue;
            }
            if let Ok(png) = std::fs::read(e.path()) {
                if notifications::is_png(&png) {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
                    out.insert(package.to_string(), format!("data:image/png;base64,{b64}"));
                }
            }
        }
        out
    }

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
        let notify = state.notifications_on();
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
                AppMessage::NotificationPosted(n) => {
                    let name = state.name_of(&peer);
                    let effect = lock(&state.store).post(peer, &name, &n);
                    let Effect::Posted(info) = effect else {
                        continue;
                    };
                    state.maybe_popup(&info);
                    WatchEvent::NotificationPosted { item: info }
                }
                AppMessage::NotificationRemoved { key } => {
                    let sync = state.sync_on();
                    match lock(&state.store).removed_on_phone(peer, &key, sync) {
                        Effect::Removed { id, deleted } => {
                            state.popups.send(PopupCmd::Close { id: id.clone() });
                            WatchEvent::NotificationRemoved { id, deleted }
                        }
                        _ => continue,
                    }
                }
                AppMessage::NotificationSnapshot { items } => {
                    let name = state.name_of(&peer);
                    let sync = state.sync_on();
                    let effects = lock(&state.store).snapshot(peer, &name, &items, sync);
                    for e in effects {
                        if let Effect::Removed { id, .. } = e {
                            state.popups.send(PopupCmd::Close { id });
                        }
                    }
                    WatchEvent::NotificationsChanged
                }
                AppMessage::AppIcon { package, png } => {
                    match notifications::save_icon(&state.icons_dir, &package, &png) {
                        Ok(true) => WatchEvent::AppIcon { package },
                        Ok(false) => {
                            debug!("ignored an invalid app icon");
                            continue;
                        }
                        Err(e) => {
                            warn!("could not save app icon: {e}");
                            continue;
                        }
                    }
                }
                AppMessage::ActionResult { key, ok, message } => WatchEvent::ActionResult {
                    id: notifications::make_id(&peer, &key),
                    ok,
                    message,
                },
                // Requests only ever go computer → phone; ignore them in this direction.
                AppMessage::DismissNotification { .. }
                | AppMessage::ReplyNotification { .. }
                | AppMessage::NotificationAction { .. } => continue,
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
                lock(&state.settings).notifications = enabled;
                let reply = state.persist(|c| c.notifications = enabled);
                write_reply(&mut w, &reply).await?;
            }
            Request::SetSyncDismissals { enabled } => {
                lock(&state.settings).sync_dismissals = enabled;
                let reply = state.persist(|c| c.sync_dismissals = enabled);
                write_reply(&mut w, &reply).await?;
            }
            Request::SetAppMuted { package, muted } => {
                let reply = if !notifications::safe_package(&package) {
                    Reply::Error {
                        message: "invalid app".into(),
                    }
                } else {
                    let list: Vec<String> = {
                        let mut settings = lock(&state.settings);
                        if muted {
                            settings.muted.insert(package.clone());
                        } else {
                            settings.muted.remove(&package);
                        }
                        let mut v: Vec<String> = settings.muted.iter().cloned().collect();
                        v.sort();
                        v
                    };
                    state.broadcast(WatchEvent::NotificationsChanged);
                    state.persist(|c| c.muted_apps = list)
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::Notifications => {
                let (items, apps) = state.visible_notifications();
                write_reply(&mut w, &Reply::Notifications { items, apps }).await?;
            }
            Request::AppIcons => {
                write_reply(
                    &mut w,
                    &Reply::AppIcons {
                        icons: state.app_icons(),
                    },
                )
                .await?;
            }
            Request::DismissNotification { id } => {
                let removed = lock(&state.store).dismiss(&id);
                let reply = match removed {
                    None => Reply::Error {
                        message: "that notification is no longer here".into(),
                    },
                    Some((device, info)) => {
                        state.popups.send(PopupCmd::Close { id: id.clone() });
                        state.broadcast(WatchEvent::NotificationRemoved {
                            id: id.clone(),
                            deleted: true,
                        });
                        if state.sync_on() && !info.removed_on_phone {
                            let msg = AppMessage::DismissNotification {
                                key: phone_key(&id).to_string(),
                            };
                            if let Err(e) = state.to_phone(&device, msg) {
                                warn!("could not ask the phone to dismiss: {e}");
                            }
                        }
                        Reply::Done
                    }
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::ClearNotifications => {
                let all = lock(&state.store).clear();
                let sync = state.sync_on();
                for (device, info) in all {
                    state.popups.send(PopupCmd::Close {
                        id: info.id.clone(),
                    });
                    if sync && !info.removed_on_phone {
                        let msg = AppMessage::DismissNotification {
                            key: phone_key(&info.id).to_string(),
                        };
                        let _ = state.to_phone(&device, msg);
                    }
                }
                state.broadcast(WatchEvent::NotificationsChanged);
                write_reply(&mut w, &Reply::Done).await?;
            }
            Request::ReplyNotification { id, text } => {
                let found = lock(&state.store).get(&id).cloned();
                let text = text.trim().to_string();
                let reply = match found {
                    None => Reply::Error {
                        message: "that notification is no longer here".into(),
                    },
                    Some((_, info)) if !info.can_reply => Reply::Error {
                        message: format!("{} doesn't allow replies from notifications", info.app),
                    },
                    Some(_) if text.is_empty() => Reply::Error {
                        message: "the reply is empty".into(),
                    },
                    Some(_) if text.len() > MAX_NOTIFICATION_TEXT => Reply::Error {
                        message: "the reply is too long".into(),
                    },
                    Some((device, _)) => {
                        let msg = AppMessage::ReplyNotification {
                            key: phone_key(&id).to_string(),
                            text,
                        };
                        match state.to_phone(&device, msg) {
                            Ok(now) => Reply::Sent {
                                message_id: id.clone(),
                                delivered_now: now,
                            },
                            Err(message) => Reply::Error { message },
                        }
                    }
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::NotificationAction { id, index } => {
                let found = lock(&state.store).get(&id).cloned();
                let reply = match found {
                    None => Reply::Error {
                        message: "that notification is no longer here".into(),
                    },
                    Some((_, info)) if index as usize >= info.actions.len() => Reply::Error {
                        message: "no such button".into(),
                    },
                    Some((device, _)) => {
                        let msg = AppMessage::NotificationAction {
                            key: phone_key(&id).to_string(),
                            index,
                        };
                        match state.to_phone(&device, msg) {
                            Ok(now) => Reply::Sent {
                                message_id: id.clone(),
                                delivered_now: now,
                            },
                            Err(message) => Reply::Error { message },
                        }
                    }
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
                    notifications: state.notifications_on(),
                    sync_dismissals: state.sync_on(),
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
                            lock(&state.store).forget_device(&p.fingerprint);
                            state.broadcast(WatchEvent::NotificationsChanged);
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
