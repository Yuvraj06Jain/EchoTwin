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
    lan::Lan,
    transfer::{Direction, PathOpener, TransferConfig, TransferInfo, TransferState, Transfers, CAP_FILES},
    message::{
        is_fresh, CLIPBOARD_MAX_AGE_MS, LOCK_MAX_AGE_MS, LOCK_RESULT_KEY, MAX_CLIPBOARD_TEXT,
        MAX_NOTIFICATION_TEXT,
    },
    node::Timings,
    AppMessage, Event, Fingerprint, Node, NodeConfig, PeerRecord, CAP_CLIPBOARD, CAP_LOCK,
    CAP_MEDIA, CAP_NOTIFICATIONS, CAP_SMS, CAP_STATUS, PhoneStatus,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{broadcast, Notify},
    task::JoinHandle,
};
use tracing::{debug, info, warn};

use crate::{
    clipboard::ClipboardSync,
    config::Config,
    lock::Buses,
    media::MediaStore,
    sms::SmsStore,
    ipc::{DeviceInfo, Reply, Request, TransferView, WatchEvent},
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
    /// D-Bus addresses for locking and the clipboard; default = the real buses. Tests
    /// point these at private buses with stand-in services.
    pub buses: Buses,
    /// Where received files are saved.
    pub download_dir: PathBuf,
}

/// Settings that can change while running (mirrored to config.toml).
struct Settings {
    notifications: bool,
    sync_dismissals: bool,
    muted: HashSet<String>,
    clipboard: bool,
    allow_lock: bool,
    bluetooth: bool,
    connection: bool,
    battery_saver: bool,
}

/// Keep-alive and search timers for the battery saver switch.
fn timings(battery_saver: bool) -> Timings {
    if battery_saver {
        Timings::battery_saver()
    } else {
        Timings::default()
    }
}

/// A change in signal bars alone is announced at most this often per phone (they wobble
/// constantly); battery, charging and network changes go out at once.
const SIGNAL_QUIET: Duration = Duration::from_secs(10);

/// When a phone's status was last announced, and whether a held-back change waits.
struct StatusSent {
    at: std::time::Instant,
    held: bool,
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
    /// What each connected phone supports (from its Hello).
    caps: Mutex<HashMap<Fingerprint, Vec<String>>>,
    clipboard: ClipboardSync,
    buses: Buses,
    media: Mutex<MediaStore>,
    transfers: Transfers,
    sms: Mutex<SmsStore>,
    /// Why the Bluetooth backup isn't ready (empty: ready).
    bt_status: Mutex<String>,
    bt_wake: Arc<Notify>,
    /// LAN advertising (stopped while the connection is switched off).
    lan: Mutex<Option<Lan>>,
    discovery: bool,
    /// Each connected phone's battery and network.
    phone_status: Mutex<HashMap<Fingerprint, PhoneStatus>>,
    status_sent: Mutex<HashMap<Fingerprint, StatusSent>>,
    battery_root: PathBuf,
}

/// Run until `shutdown` resolves.
pub async fn run(opts: DaemonOptions, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    // Reading copied pictures needs the desktop session's display, which a service started
    // before the desktop may lack: take it from the user's session (systemd) if missing.
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        for (k, v) in crate::popups::session_env() {
            if matches!(k.as_str(), "WAYLAND_DISPLAY" | "DISPLAY" | "XDG_RUNTIME_DIR") && std::env::var_os(&k).is_none() {
                std::env::set_var(k, v);
            }
        }
    }
    std::fs::create_dir_all(&opts.data_dir)?;
    std::fs::set_permissions(&opts.data_dir, std::fs::Permissions::from_mode(0o700))?;

    let listener = bind_control_socket(&opts.socket).await?;
    let (node, events) = start_node(&opts).await?;
    let port = node.local_addr()?.port();
    let (watchers, _) = broadcast::channel(256);
    let (copies_tx, copies) = tokio::sync::mpsc::unbounded_channel();
    let (transfers, transfer_events) = Transfers::start(
        node.clone(),
        TransferConfig {
            db_path: Some(opts.data_dir.join("transfers.sqlite3")),
            incoming_dir: opts.download_dir.clone(),
            reserve_bytes: 512 * 1024 * 1024,
        },
        Arc::new(PathOpener),
    )?;
    let state = Arc::new(State {
        node: node.clone(),
        port,
        config: opts.config.clone(),
        config_path: opts.config_path.clone(),
        settings: Mutex::new(Settings {
            notifications: opts.config.notifications,
            sync_dismissals: opts.config.sync_dismissals,
            muted: opts.config.muted_apps.iter().cloned().collect(),
            clipboard: opts.config.clipboard_sync,
            allow_lock: opts.config.allow_lock,
            bluetooth: opts.config.bluetooth_backup,
            connection: opts.config.connection,
            battery_saver: opts.config.battery_saver,
        }),
        store: Mutex::new(Store::default()),
        popups: opts.popups.clone().unwrap_or_else(Popups::start),
        icons_dir: opts.data_dir.join("icons"),
        watchers,
        keepers: Mutex::new(HashMap::new()),
        advert_refresh: Arc::new(Notify::new()),
        caps: Mutex::new(HashMap::new()),
        clipboard: ClipboardSync::start(
            opts.buses.session.clone(),
            copies_tx,
            Some(Box::new(crate::clipboard::SystemImages::new())),
            opts.data_dir.join("clipboard"),
        ),
        buses: opts.buses.clone(),
        media: Mutex::new(MediaStore::default()),
        transfers,
        sms: Mutex::new(SmsStore::default()),
        bt_status: Mutex::new("starting".into()),
        bt_wake: Arc::new(Notify::new()),
        phone_status: Mutex::new(HashMap::new()),
        status_sent: Mutex::new(HashMap::new()),
        lan: Mutex::new(None),
        discovery: opts.config.discovery,
        battery_root: opts
            .config
            .power_supply_dir
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(crate::battery::DEFAULT_ROOT)),
    });
    info!(
        name = %opts.config.device_name(),
        id = %node.fingerprint().short(),
        port,
        socket = %opts.socket.display(),
        "phonelinkd running"
    );

    if !opts.config.connection {
        node.set_paused(true);
        info!("connection is switched off; turn it on in the app or with `phonelink on`");
    }
    for peer in node.peers() {
        state.keep_connected(peer.fingerprint);
    }
    state.update_lan();

    tokio::spawn(event_pump(state.clone(), events));
    tokio::spawn(network_watch(state.clone()));
    tokio::spawn(share_copies(state.clone(), copies));
    tokio::spawn(transfer_updates(state.clone(), transfer_events));
    tokio::spawn(bluetooth_backup(state.clone()));
    tokio::spawn(battery_watch(state.clone()));
    let server = tokio::spawn(serve(state.clone(), listener));

    shutdown.await;
    info!("shutting down");
    server.abort();
    node.shutdown().await;
    if let Some(lan) = lock(&state.lan).take() {
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
        cfg.timings = timings(opts.config.battery_saver);
        cfg.capabilities.extend(
            [CAP_NOTIFICATIONS, CAP_LOCK, CAP_CLIPBOARD, phonelink_core::CAP_CLIPBOARD_IMAGE, CAP_MEDIA, CAP_FILES, CAP_SMS, CAP_STATUS]
                .map(String::from),
        );
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

fn device_info(
    node: &Node,
    p: &PeerRecord,
    capabilities: Vec<String>,
    status: Option<PhoneStatus>,
) -> DeviceInfo {
    DeviceInfo {
        id: p.fingerprint.short(),
        fingerprint: p.fingerprint.to_hex(),
        name: p.name.clone(),
        connected: node.is_connected(&p.fingerprint),
        pending: node.pending_count(&p.fingerprint).unwrap_or(0),
        last_addrs: p.last_addrs.iter().map(|a| a.to_string()).collect(),
        capabilities,
        via: via(node.transport(&p.fingerprint)),
        status,
        paused_by_peer: false,
    }
}

fn via(t: Option<phonelink_core::Transport>) -> Option<String> {
    t.map(|t| match t {
        phonelink_core::Transport::Quic => "wifi".to_string(),
        phonelink_core::Transport::Stream => "bluetooth".to_string(),
    })
}

fn hex_id(id: &phonelink_core::MsgId) -> String {
    id.0.iter().take(6).map(|b| format!("{b:02x}")).collect()
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
    /// Advertise on the LAN while discovery is on and the connection switched on.
    fn update_lan(&self) {
        let want = self.discovery && !self.node.is_paused();
        let mut lan = lock(&self.lan);
        if !want {
            if let Some(l) = lan.take() {
                l.shutdown();
            }
            return;
        }
        if lan.is_none() {
            match Lan::new() {
                Ok(l) => {
                    l.advertise(self.node.clone(), self.port, self.advert_refresh.clone());
                    *lan = Some(l);
                }
                Err(e) => warn!("LAN discovery unavailable, relying on remembered addresses: {e}"),
            }
        }
    }

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

    /// EchoTwin's own popups (paired, connected, a note). Same click-to-open behaviour.
    fn status_popup(&self, enabled: bool, id: String, title: String, body: String) {
        self.popup_to(enabled, id, title, body, crate::popups::Tab::Notifications)
    }

    fn popup_to(&self, enabled: bool, id: String, title: String, body: String, tab: crate::popups::Tab) {
        if !enabled {
            return;
        }
        self.popups.send(PopupCmd::Show {
            id,
            app: "EchoTwin".into(),
            title: notifications::escape_markup(&title),
            body: notifications::escape_markup(&body),
            icon: None,
            tab,
        });
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
            tab: crate::popups::Tab::Notifications,
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

    fn device_info(&self, p: &PeerRecord) -> DeviceInfo {
        let caps = lock(&self.caps)
            .get(&p.fingerprint)
            .cloned()
            .unwrap_or_default();
        let status = lock(&self.phone_status).get(&p.fingerprint).cloned();
        let mut info = device_info(&self.node, p, caps, status);
        info.paused_by_peer = self.node.peer_paused(&p.fingerprint);
        info
    }

    fn clipboard_on(&self) -> bool {
        lock(&self.settings).clipboard
    }

    /// Whether to announce a phone's new status now. A signal-only change within
    /// [`SIGNAL_QUIET`] of the last announcement is held back and announced when the quiet
    /// time ends (unless a real change went out meanwhile).
    fn announce_status(self: &Arc<Self>, peer: Fingerprint, signal_only: bool) -> bool {
        let now = std::time::Instant::now();
        let mut sent = lock(&self.status_sent);
        if signal_only {
            if let Some(s) = sent.get_mut(&peer).filter(|s| now < s.at + SIGNAL_QUIET) {
                if !s.held {
                    s.held = true;
                    let (state, at) = (self.clone(), s.at + SIGNAL_QUIET);
                    tokio::spawn(async move {
                        tokio::time::sleep_until(at.into()).await;
                        state.announce_held_status(peer);
                    });
                }
                return false;
            }
        }
        sent.insert(peer, StatusSent { at: now, held: false });
        true
    }

    fn announce_held_status(&self, peer: Fingerprint) {
        {
            let mut sent = lock(&self.status_sent);
            let Some(s) = sent.get_mut(&peer).filter(|s| s.held) else {
                return;
            };
            *s = StatusSent {
                at: std::time::Instant::now(),
                held: false,
            };
        }
        self.broadcast(WatchEvent::PhoneStatusChanged { id: peer.short() });
    }

    /// Connected devices that support `cap`.
    /// Tell a phone this computer's battery (not queued: only "now" matters).
    fn send_battery(&self, peer: &Fingerprint, battery: Option<(u8, bool)>) {
        let msg = AppMessage::ComputerStatus {
            battery: battery.map(|b| b.0),
            charging: battery.is_some_and(|b| b.1),
        };
        if let Ok(body) = msg.encode() {
            let _ = self.node.send(peer, body);
        }
    }

    fn connected_with(&self, cap: &str) -> Vec<Fingerprint> {
        lock(&self.caps)
            .iter()
            .filter(|(fp, caps)| caps.iter().any(|c| c == cap) && self.node.is_connected(fp))
            .map(|(fp, _)| *fp)
            .collect()
    }

    /// The phone asked to lock this computer.
    async fn lock_requested(self: &Arc<Self>, peer: Fingerprint, sent_ms: u64) -> WatchEvent {
        let name = self.name_of(&peer);
        let (ok, message) = if !lock(&self.settings).allow_lock {
            (false, "Locking from the phone is switched off on this computer".to_string())
        } else if !is_fresh(sent_ms, now_ms(), LOCK_MAX_AGE_MS) {
            // Waited in a queue (or the clocks are far apart): locking now would surprise.
            (false, "The request arrived too late, so the computer was not locked".to_string())
        } else {
            let buses = self.buses.clone();
            match tokio::task::spawn_blocking(move || crate::lock::lock_screen(&buses)).await {
                Ok(Ok(how)) => {
                    info!(device = %name, "locked by the phone ({how})");
                    (true, "Computer locked".to_string())
                }
                Ok(Err(e)) => {
                    warn!("could not lock the screen: {e}");
                    (false, "This computer could not lock its screen".to_string())
                }
                Err(e) => (false, format!("Could not lock: {e}")),
            }
        };
        let result = AppMessage::ActionResult {
            key: LOCK_RESULT_KEY.into(),
            ok,
            message,
        };
        let _ = self.to_phone(&peer, result);
        WatchEvent::LockedByPhone {
            id: peer.short(),
            name,
        }
    }

    /// `<device id>/<thread>` → (device, thread).
    fn sms_thread(&self, key: &str) -> Option<(Fingerprint, u64)> {
        self.node
            .peers()
            .iter()
            .find_map(|p| crate::sms::parse_key(&p.fingerprint, key).map(|t| (p.fingerprint, t)))
    }

    /// Ask the phone to send an SMS now. Not queued: a text sent hours later, when the
    /// phone happens to reconnect, would be a surprise.
    fn send_sms(&self, thread: &str, text: &str) -> Result<String, String> {
        let text = text.trim_end();
        if text.trim().is_empty() {
            return Err("the message is empty".into());
        }
        if text.len() > phonelink_core::message::SMS_MAX_BODY {
            return Err("the message is too long".into());
        }
        let (fp, id) = self.sms_thread(thread).ok_or("no such conversation")?;
        if !self.node.is_connected(&fp) {
            return Err(format!("{} is not connected; the message was not sent", self.name_of(&fp)));
        }
        if !self.connected_with(CAP_SMS).contains(&fp) {
            return Err(format!("update EchoTwin on {} to send SMS", self.name_of(&fp)));
        }
        let address = lock(&self.sms).address(&fp, id).unwrap_or_default();
        if address.is_empty() {
            return Err("this conversation has no number to send to".into());
        }
        if address.contains(',') || address.contains(';') {
            return Err("group messages can't be sent from the computer yet".into());
        }
        let client_id: String = phonelink_core::MsgId::random().0[..8].iter().map(|b| format!("{b:02x}")).collect();
        lock(&self.sms).add_pending(fp, id, client_id.clone(), text.to_string(), now_ms());
        let msg = AppMessage::SmsSend {
            client_id: client_id.clone(),
            thread: id,
            address,
            body: text.to_string(),
        };
        if let Err(e) = self.to_phone(&fp, msg) {
            lock(&self.sms).send_result(&fp, &client_id, phonelink_core::SmsState::Failed, &e);
            return Err(e);
        }
        self.broadcast(WatchEvent::SmsChanged {
            thread: Some(thread.to_string()),
        });
        Ok(client_id)
    }

    fn transfer_view(&self, t: &TransferInfo) -> TransferView {
        TransferView {
            id: t.id.clone(),
            device: t.peer.short(),
            device_name: self.name_of(&t.peer),
            direction: t.direction,
            name: t.name.clone(),
            size: t.size,
            done: t.done,
            state: t.state,
            message: t.message.clone(),
            path: t.path.clone(),
            updated_ms: t.updated_ms,
            rate: t.rate,
            eta_secs: t.eta_secs,
        }
    }

    /// Queue files (absolute paths to regular files) for a phone.
    /// An image copied here: send it at full quality (as a file) to connected phones that
    /// put such images on their clipboard, and tell them which transfer it is.
    fn send_clipboard_image(&self, path: &std::path::Path) {
        let Ok(meta) = std::fs::metadata(path) else { return };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Clipboard image.png".into());
        let mime = crate::open::guess_mime(&name);
        for peer in self.connected_with(phonelink_core::CAP_CLIPBOARD_IMAGE) {
            if !self.node.peer_supports(&peer, CAP_FILES) {
                continue;
            }
            match self.transfers.send_file(peer, path.to_string_lossy().into_owned(), &name, meta.len(), mime) {
                Ok(transfer_id) => {
                    let msg = AppMessage::ClipboardImage {
                        transfer_id,
                        copied_ms: now_ms(),
                    };
                    if let Err(e) = self.to_phone(&peer, msg) {
                        debug!("could not announce the clipboard image: {e}");
                    }
                }
                Err(e) => debug!("could not send the clipboard image: {e}"),
            }
        }
    }

    fn send_files(&self, device: Option<&str>, paths: &[String], notify: bool) -> Result<Vec<String>, String> {
        if paths.is_empty() {
            return Err("no files given".into());
        }
        let peer = self.resolve(device)?;
        let mut files = vec![];
        for p in paths {
            let path = PathBuf::from(p);
            if !path.is_absolute() {
                return Err(format!("{p}: give the full path"));
            }
            let meta = std::fs::metadata(&path).map_err(|e| format!("{p}: {e}"))?;
            if meta.is_dir() {
                return Err(format!("{p} is a folder; folders can't be sent yet (zip it first)"));
            }
            if !meta.is_file() {
                return Err(format!("{p} is not a regular file"));
            }
            std::fs::File::open(&path).map_err(|e| format!("{p}: {e}"))?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            files.push((path, name, meta.len()));
        }
        let mut ids = vec![];
        for (path, name, size) in &files {
            let mime = crate::open::guess_mime(name);
            let id = self
                .transfers
                .send_file(peer.fingerprint, path.to_string_lossy().into_owned(), name, *size, mime)
                .map_err(|e| e.to_string())?;
            ids.push(id);
        }
        if notify {
            let what = match files.as_slice() {
                [(_, name, _)] => format!("“{name}”"),
                many => format!("{} files", many.len()),
            };
            let when = if self.node.is_connected(&peer.fingerprint) {
                "Sending now."
            } else {
                "They go as soon as it connects."
            };
            self.popup_to(
                true,
                format!("send/{}", ids[0]),
                format!("Sending {what} to {}", peer.name),
                when.into(),
                crate::popups::Tab::Files,
            );
        }
        Ok(ids)
    }

    /// The phone to send a media button to: connected and supporting media/1.
    fn media_target(&self, query: Option<&str>) -> Result<Fingerprint, String> {
        let fp = match query {
            Some(q) => self.resolve(Some(q))?.fingerprint,
            None => {
                let connected = self.connected_with(CAP_MEDIA);
                match connected.as_slice() {
                    [one] => *one,
                    [] => return Err("no phone with music controls is connected".into()),
                    _ => return Err("several phones connected; say which one".into()),
                }
            }
        };
        if !self.node.is_connected(&fp) {
            return Err(format!("{} is not connected", self.name_of(&fp)));
        }
        if !self.connected_with(CAP_MEDIA).contains(&fp) {
            return Err(format!(
                "update EchoTwin on {} for music controls",
                self.name_of(&fp)
            ));
        }
        Ok(fp)
    }

    fn devices(&self) -> Vec<DeviceInfo> {
        self.node
            .peers()
            .iter()
            .map(|p| self.device_info(p))
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
                state.status_popup(
                    notify,
                    format!("status/{}", peer.short()),
                    "Device paired".into(),
                    format!("{name} can now connect to this computer."),
                );
                WatchEvent::Paired {
                    id: peer.short(),
                    name,
                }
            }
            Event::PeerConnected {
                peer,
                name,
                capabilities,
                transport,
                ..
            } => {
                let wants_status = capabilities.iter().any(|c| c == CAP_STATUS);
                lock(&state.caps).insert(peer, capabilities);
                if wants_status {
                    state.send_battery(&peer, crate::battery::read(&state.battery_root));
                }
                state.transfers.nudge(); // files waiting for this phone go now
                let blip = last_disconnect
                    .get(&peer)
                    .is_some_and(|t| t.elapsed() < RECONNECT_QUIET);
                state.status_popup(
                    notify && !blip,
                    format!("status/{}", peer.short()),
                    format!("{name} connected"),
                    "EchoTwin is active.".into(),
                );
                WatchEvent::Connected {
                    id: peer.short(),
                    name,
                    via: via(Some(transport)).unwrap_or_default(),
                }
            }
            Event::PeerPaused { peer } => WatchEvent::PeerPaused {
                id: peer.short(),
                name: state.name_of(&peer),
            },
            Event::PeerDisconnected { peer } => {
                last_disconnect.insert(peer, std::time::Instant::now());
                lock(&state.caps).remove(&peer);
                lock(&state.phone_status).remove(&peer);
                lock(&state.status_sent).remove(&peer);
                if lock(&state.media).forget(&peer) {
                    state.broadcast(WatchEvent::MediaChanged { id: peer.short() });
                }
                WatchEvent::Disconnected {
                    id: peer.short(),
                    name: state.name_of(&peer),
                }
            }
            Event::Message { peer, id, body } => match AppMessage::decode(&body) {
                msg @ AppMessage::FileCancel { .. } => {
                    state.transfers.on_message(&peer, &msg);
                    continue;
                }
                AppMessage::SmsSnapshot { threads } => {
                    lock(&state.sms).snapshot(peer, &threads);
                    WatchEvent::SmsChanged { thread: None }
                }
                AppMessage::SmsThreadUpdate(t) => {
                    let key = crate::sms::thread_key(&peer, t.id);
                    lock(&state.sms).update(peer, t);
                    WatchEvent::SmsChanged { thread: Some(key) }
                }
                AppMessage::SmsOlder {
                    thread,
                    messages,
                    has_older,
                } => {
                    lock(&state.sms).older(peer, thread, messages, has_older);
                    WatchEvent::SmsChanged {
                        thread: Some(crate::sms::thread_key(&peer, thread)),
                    }
                }
                AppMessage::SmsSendResult {
                    client_id,
                    state: sms_state,
                    message,
                } => {
                    let thread = lock(&state.sms).send_result(&peer, &client_id, sms_state, &message);
                    state.broadcast(WatchEvent::SmsChanged {
                        thread: thread.map(|t| crate::sms::thread_key(&peer, t)),
                    });
                    WatchEvent::SmsSendResult {
                        client_id,
                        state: sms_state,
                        error: message,
                    }
                }
                // Only ever computer → phone.
                AppMessage::SmsLoadOlder { .. } | AppMessage::SmsSend { .. } => continue,
                AppMessage::Text { text } => {
                    let name = state.name_of(&peer);
                    state.status_popup(
                        notify,
                        format!("note/{}", hex_id(&id)),
                        format!("Message from {name}"),
                        text.clone(),
                    );
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
                AppMessage::LockComputer { sent_ms } => {
                    // Locking can take a moment (D-Bus); don't hold up other messages.
                    let state = state.clone();
                    tokio::spawn(async move {
                        let ev = state.lock_requested(peer, sent_ms).await;
                        state.broadcast(ev);
                    });
                    continue;
                }
                AppMessage::Clipboard { text, copied_ms } => {
                    if !state.clipboard_on() {
                        debug!("clipboard from the phone ignored: sharing is off");
                        continue;
                    }
                    if !is_fresh(copied_ms, now_ms(), CLIPBOARD_MAX_AGE_MS) {
                        debug!("stale clipboard from the phone ignored");
                        continue;
                    }
                    if text.is_empty() || text.len() > MAX_CLIPBOARD_TEXT {
                        continue;
                    }
                    let name = state.name_of(&peer);
                    state.clipboard.set(text);
                    state.status_popup(
                        notify,
                        format!("clipboard/{}", peer.short()),
                        format!("Copied from {name}"),
                        "Paste it anywhere on this computer.".into(),
                    );
                    WatchEvent::ClipboardReceived {
                        id: peer.short(),
                        name,
                    }
                }
                AppMessage::NowPlaying(now) => {
                    let changed =
                        lock(&state.media).update(peer, now, std::time::Instant::now());
                    if !changed {
                        continue;
                    }
                    WatchEvent::MediaChanged { id: peer.short() }
                }
                AppMessage::MediaArt { key, image } => {
                    if !lock(&state.media).set_art(peer, key, image) {
                        debug!("album art rejected (not a small JPEG/PNG)");
                        continue;
                    }
                    WatchEvent::MediaChanged { id: peer.short() }
                }
                AppMessage::PhoneStatus(mut s) => {
                    s.battery = s.battery.filter(|b| *b <= 100);
                    s.signal = s.signal.min(4);
                    s.network = phonelink_core::message::truncate_text(&s.network, 16);
                    // Always stored, so the next status request shows the latest bars.
                    let prev = lock(&state.phone_status).insert(peer, s.clone());
                    if prev.as_ref() == Some(&s) {
                        continue;
                    }
                    let signal_only = prev.is_some_and(|p| {
                        (p.battery, p.charging, &p.network) == (s.battery, s.charging, &s.network)
                    });
                    if !state.announce_status(peer, signal_only) {
                        continue;
                    }
                    WatchEvent::PhoneStatusChanged { id: peer.short() }
                }
                // Only ever computer → phone.
                AppMessage::MediaControl { .. }
                | AppMessage::ComputerStatus { .. }
                | AppMessage::ClipboardImage { .. } => continue,
                AppMessage::Unsupported => {
                    debug!("message type not supported by this version; ignored");
                    continue;
                }
            },
        };
        let _ = state.watchers.send(watch);
    }
}

/// Keeps the EchoTwin Bluetooth profile registered with BlueZ while the backup is on (and
/// Bluetooth exists), tells phones where to find this computer, and serves the connections.
/// Re-reads this computer's battery every 30 s and tells connected phones when it changed.
async fn battery_watch(state: Arc<State>) {
    let mut last = crate::battery::read(&state.battery_root);
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let now = crate::battery::read(&state.battery_root);
        if now != last {
            for peer in state.connected_with(CAP_STATUS) {
                state.send_battery(&peer, now);
            }
            last = now;
        }
    }
}

async fn bluetooth_backup(state: Arc<State>) {
    use crate::bluetooth;
    let (tx, mut incoming) = tokio::sync::mpsc::unbounded_channel::<bluetooth::Incoming>();
    let accept_state = state.clone();
    tokio::spawn(async move {
        while let Some(conn) = incoming.recv().await {
            let state = accept_state.clone();
            tokio::spawn(async move {
                let std_stream = std::os::unix::net::UnixStream::from(conn.fd);
                let stream = std_stream
                    .set_nonblocking(true)
                    .and_then(|_| tokio::net::UnixStream::from_std(std_stream));
                match stream {
                    Ok(s) => match state.node.accept_stream(s).await {
                        Ok(peer) => info!(device = %state.name_of(&peer), "connected over Bluetooth"),
                        Err(e) => info!(from = %conn.device, "Bluetooth connection refused: {e}"),
                    },
                    Err(e) => warn!("unusable Bluetooth socket: {e}"),
                }
            });
        }
    });
    let mut registered: Option<bluetooth::Registered> = None;
    let set_status = |s: &str| {
        let mut cur = lock(&state.bt_status);
        if *cur != s {
            if s.is_empty() {
                info!("Bluetooth backup ready");
            } else {
                info!("Bluetooth backup not available: {s}");
            }
            *cur = s.to_string();
        }
    };
    loop {
        let wanted = lock(&state.settings).bluetooth && !state.node.is_paused();
        if !wanted {
            if let Some(r) = registered.take() {
                let _ = tokio::task::spawn_blocking(move || r.unregister()).await;
            }
            state.node.set_bluetooth_address(None);
            set_status("off");
        } else if let Some(r) = registered.take() {
            // Still there? (Bluetooth switched off, adapter unplugged…)
            let (r, addr) = tokio::task::spawn_blocking(move || {
                let a = r.current_address();
                (r, a)
            })
            .await
            .expect("bluetooth check");
            match addr {
                Some(a) => {
                    state.node.set_bluetooth_address(Some(a));
                    registered = Some(r);
                }
                None => {
                    state.node.set_bluetooth_address(None);
                    set_status("no Bluetooth adapter found (is Bluetooth on?)");
                }
            }
        } else {
            let (bus, tx) = (state.buses.system.clone(), tx.clone());
            match tokio::task::spawn_blocking(move || bluetooth::register(&bus, tx)).await {
                Ok(Ok(r)) => {
                    state.node.set_bluetooth_address(Some(r.address.clone()));
                    set_status("");
                    registered = Some(r);
                }
                Ok(Err(e)) => {
                    state.node.set_bluetooth_address(None);
                    set_status(&e);
                }
                Err(e) => set_status(&e.to_string()),
            }
        }
        tokio::select! {
            _ = state.bt_wake.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(30)) => {}
        }
    }
}

/// Every change to a transfer goes to the UI; a received file (or a failed send) also
/// gets a popup that opens the Files tab.
async fn transfer_updates(state: Arc<State>, mut events: tokio::sync::mpsc::UnboundedReceiver<TransferInfo>) {
    let mut announced: HashSet<String> = HashSet::new();
    while let Some(t) = events.recv().await {
        let view = state.transfer_view(&t);
        if t.state.finished() && announced.insert(t.id.clone()) {
            let notify = state.notifications_on();
            let device = view.device_name.clone();
            match (t.direction, t.state) {
                (Direction::Incoming, TransferState::Done) => state.popup_to(
                    notify,
                    format!("file/{}", t.id),
                    format!("Received “{}”", t.name),
                    format!("From {device}. Saved in Downloads/EchoTwin."),
                    crate::popups::Tab::Files,
                ),
                (Direction::Outgoing, TransferState::Failed) => state.popup_to(
                    notify,
                    format!("file/{}", t.id),
                    format!("Couldn't send “{}”", t.name),
                    t.message.clone(),
                    crate::popups::Tab::Files,
                ),
                _ => {}
            }
        }
        if announced.len() > 1000 {
            announced.clear();
        }
        state.broadcast(WatchEvent::TransferChanged { item: view });
    }
}

/// Text copied on this computer goes to every connected phone that can take it. Only
/// while connected: queueing copies for later would paste stale text over newer ones.
async fn share_copies(
    state: Arc<State>,
    mut copies: tokio::sync::mpsc::UnboundedReceiver<crate::clipboard::Copied>,
) {
    while let Some(copied) = copies.recv().await {
        let text = match copied {
            crate::clipboard::Copied::Text(t) => t,
            crate::clipboard::Copied::Image(path) => {
                if state.clipboard_on() {
                    state.send_clipboard_image(&path);
                }
                continue;
            }
        };
        if !state.clipboard_on() || text.trim().is_empty() || text.len() > MAX_CLIPBOARD_TEXT {
            continue;
        }
        let msg = AppMessage::Clipboard {
            text,
            copied_ms: now_ms(),
        };
        for peer in state.connected_with(CAP_CLIPBOARD) {
            if let Err(e) = state.to_phone(&peer, msg.clone()) {
                debug!("could not share the clipboard: {e}");
            }
        }
    }
}

/// Retry at once when this computer's addresses change (new Wi-Fi, the phone's hotspot
/// restarted) or it wakes from suspend. Every 10 s: suspend is still caught (wall clock
/// vs monotonic clock), and an address change waits at most that long.
async fn network_watch(state: Arc<State>) {
    let now = || (std::time::SystemTime::now(), std::time::Instant::now());
    let (wall, mono) = now();
    let mut watch = netaddrs::NetWatch::new(netaddrs::lan_addrs(state.port), wall, mono);
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let (wall, mono) = now();
        if watch.check(netaddrs::lan_addrs(state.port), wall, mono) {
            info!("network changed or woke from sleep; looking for paired devices now");
            state.node.nudge();
        }
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
            Request::SetClipboardSync { enabled } => {
                lock(&state.settings).clipboard = enabled;
                let reply = state.persist(|c| c.clipboard_sync = enabled);
                write_reply(&mut w, &reply).await?;
            }
            Request::SetBluetoothBackup { enabled } => {
                lock(&state.settings).bluetooth = enabled;
                state.bt_wake.notify_one();
                let reply = state.persist(|c| c.bluetooth_backup = enabled);
                write_reply(&mut w, &reply).await?;
            }
            Request::SetConnection { on } => {
                lock(&state.settings).connection = on;
                state.node.set_paused(!on);
                state.update_lan();
                state.bt_wake.notify_one();
                state.broadcast(WatchEvent::ConnectionChanged { on });
                let reply = state.persist(|c| c.connection = on);
                write_reply(&mut w, &reply).await?;
            }
            Request::SetBatterySaver { on } => {
                lock(&state.settings).battery_saver = on;
                let reply = match state.node.set_timings(timings(on)) {
                    Ok(()) => state.persist(|c| c.battery_saver = on),
                    Err(e) => Reply::Error {
                        message: e.to_string(),
                    },
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::SetAllowLock { enabled } => {
                lock(&state.settings).allow_lock = enabled;
                let reply = state.persist(|c| c.allow_lock = enabled);
                write_reply(&mut w, &reply).await?;
            }
            Request::Media => {
                let at = std::time::Instant::now();
                let items = {
                    let media = lock(&state.media);
                    state
                        .node
                        .peers()
                        .iter()
                        .filter_map(|p| media.view(&p.fingerprint, &p.name, at))
                        .collect()
                };
                write_reply(&mut w, &Reply::Media { items }).await?;
            }
            Request::MediaControl { device, action } => {
                let reply = match state.media_target(device.as_deref()) {
                    Err(message) => Reply::Error { message },
                    Ok(fp) => {
                        let action = match action {
                            phonelink_core::MediaAction::SetVolume(v) => {
                                let max = lock(&state.media).max_volume(&fp).unwrap_or(u32::MAX);
                                phonelink_core::MediaAction::SetVolume(v.min(max))
                            }
                            other => other,
                        };
                        match state.to_phone(&fp, AppMessage::MediaControl { action }) {
                            Ok(_) => Reply::Done,
                            Err(message) => Reply::Error { message },
                        }
                    }
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::Transfers => {
                let items = state.transfers.list().iter().map(|t| state.transfer_view(t)).collect();
                write_reply(&mut w, &Reply::Transfers { items }).await?;
            }
            Request::SendFiles {
                device,
                paths,
                notify,
            } => {
                let reply = match state.send_files(device.as_deref(), &paths, notify) {
                    Ok(ids) => Reply::FilesQueued { ids },
                    Err(message) => Reply::Error { message },
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::CancelTransfer { id } => {
                let reply = match state.transfers.cancel(&id) {
                    Ok(_) => Reply::Done,
                    Err(e) => Reply::Error {
                        message: e.to_string(),
                    },
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::ClearTransfers => {
                let reply = match state.transfers.clear_finished() {
                    Ok(_) => Reply::Done,
                    Err(e) => Reply::Error {
                        message: e.to_string(),
                    },
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::OpenTransfer { id, reveal } => {
                let reply = match state.transfers.get(&id) {
                    Some(t) if t.direction == Direction::Incoming && t.state == TransferState::Done => {
                        match t.path.map(PathBuf::from).filter(|p| p.exists()) {
                            Some(p) => {
                                let ok = tokio::task::spawn_blocking(move || {
                                    if reveal {
                                        crate::open::reveal_file(&p)
                                    } else {
                                        crate::open::open_file(&p)
                                    }
                                })
                                .await
                                .unwrap_or(false);
                                if ok {
                                    Reply::Done
                                } else {
                                    Reply::Error {
                                        message: "could not open it on this desktop".into(),
                                    }
                                }
                            }
                            None => Reply::Error {
                                message: "the file was moved or deleted".into(),
                            },
                        }
                    }
                    Some(_) => Reply::Error {
                        message: "only received files can be opened".into(),
                    },
                    None => Reply::Error {
                        message: "no such transfer".into(),
                    },
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::SmsThreads => {
                let threads = {
                    let sms = lock(&state.sms);
                    state.node.peers().iter().flat_map(|p| sms.view(&p.fingerprint)).collect()
                };
                write_reply(&mut w, &Reply::SmsThreads { threads }).await?;
            }
            Request::SendSms { thread, text } => {
                let reply = match state.send_sms(&thread, &text) {
                    Ok(client_id) => Reply::SmsQueued { client_id },
                    Err(message) => Reply::Error { message },
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::SmsMarkRead { thread } => {
                let reply = match state.sms_thread(&thread) {
                    Some((fp, id)) => {
                        lock(&state.sms).mark_read(&fp, id, now_ms());
                        Reply::Done
                    }
                    None => Reply::Error {
                        message: "no such conversation".into(),
                    },
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::SmsLoadOlder { thread } => {
                let reply = match state.sms_thread(&thread) {
                    Some((fp, id)) if state.connected_with(CAP_SMS).contains(&fp) => {
                        let before_ms = lock(&state.sms).oldest(&fp, id).unwrap_or(u64::MAX);
                        match state.to_phone(&fp, AppMessage::SmsLoadOlder { thread: id, before_ms }) {
                            Ok(_) => Reply::Done,
                            Err(message) => Reply::Error { message },
                        }
                    }
                    Some((fp, _)) => Reply::Error {
                        message: format!("{} is not connected", state.name_of(&fp)),
                    },
                    None => Reply::Error {
                        message: "no such conversation".into(),
                    },
                };
                write_reply(&mut w, &reply).await?;
            }
            Request::Reconnect => {
                info!("reconnect requested");
                state.node.nudge();
                write_reply(&mut w, &Reply::Done).await?;
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
                // Read settings first: two `lock()` temporaries in one expression would
                // deadlock (the first guard lives until the end of the statement).
                let (allow_lock, bluetooth_backup, battery_saver) = {
                    let s = lock(&state.settings);
                    (s.allow_lock, s.bluetooth, s.battery_saver)
                };
                let bluetooth_status = lock(&state.bt_status).clone();
                let reply = Reply::Status {
                    name: state.config.device_name(),
                    id: node.fingerprint().short(),
                    fingerprint: node.fingerprint().to_hex(),
                    port: state.port,
                    devices: state.devices(),
                    notifications: state.notifications_on(),
                    sync_dismissals: state.sync_on(),
                    clipboard_sync: state.clipboard_on(),
                    allow_lock,
                    bluetooth_backup,
                    bluetooth_status,
                    connection: !node.is_paused(),
                    battery_saver,
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
                            lock(&state.media).forget(&p.fingerprint);
                            state.transfers.forget_peer(&p.fingerprint);
                            lock(&state.sms).forget(&p.fingerprint);
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
                                    break Reply::Paired { device: state.device_info(p) };
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
