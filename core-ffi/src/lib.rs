//! Kotlin-facing API of the EchoTwin core, generated with UniFFI.
//!
//! The Android app creates one [`PhoneLink`] inside its foreground service. All networking,
//! crypto, storage and reconnect logic is the shared Rust core, so the phone and the laptop
//! run identical protocol code. Methods block and should be called off the main thread
//! (`Dispatchers.IO`). Events arrive on a dedicated background thread through
//! [`EventListener`].

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc as std_mpsc, Arc, Mutex,
    },
    time::Duration,
};

use phonelink_core::{
    lan::Lan,
    message::{
        is_fresh, truncate_text, CLIPBOARD_MAX_AGE_MS, MAX_CLIPBOARD_TEXT,
        MAX_NOTIFICATION_TEXT, MAX_SNAPSHOT_ITEMS,
    },
    transfer::{Direction, SourceOpener, TransferConfig, TransferInfo, Transfers, CAP_FILES},
    AppMessage, Event, Fingerprint, MediaAction, Node, NodeConfig, NowPlaying, PairingOffer,
    PeerRecord, PhoneNotification, SmsMessage, SmsState, SmsThread, CAP_CLIPBOARD, CAP_LOCK,
    CAP_CALLS, CAP_CLIPBOARD_IMAGE, CAP_CONTACTS, CAP_LINKS, CAP_MEDIA, CAP_PRESENT, CAP_REMOTE, CAP_RING, PresentKeyKind, CAP_NOTIFICATIONS, CAP_SMS, CAP_STATUS,
};
use tokio::{runtime::Runtime, task::JoinHandle};

uniffi::setup_scaffolding!();

pub mod touchpad;
pub use touchpad::{Finger, PadError, PadEvent, PadLaptop, PadListener, TouchpadLink};

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum LinkError {
    #[error("{0}")]
    Failed(String),
}

impl From<phonelink_core::Error> for LinkError {
    fn from(e: phonelink_core::Error) -> Self {
        LinkError::Failed(e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Device {
    /// Short id shown on both screens, e.g. `3F2A-91C0-77DE-0B14`.
    pub id: String,
    pub name: String,
    pub connected: bool,
    /// Messages waiting for this device to come back.
    pub pending: u64,
    /// Connected, and its EchoTwin can be locked from here.
    pub can_lock: bool,
    /// Connected, and shares the clipboard.
    pub can_clipboard: bool,
    /// Connected, and can be rung from here.
    pub can_ring: bool,
    /// Connected, and can be used as a remote (its media players, its own buttons).
    pub can_remote: bool,
    /// Connected, and takes presentation keys.
    pub can_present: bool,
    /// Connected, and takes the per-app start keys (Google Slides, Canva, full screen).
    pub can_present_apps: bool,
    /// Connected, and can be muted from here.
    pub can_mute: bool,
    /// Connected, and shows the presentation pointer.
    pub can_point: bool,
    /// Connected, and can be unlocked from here (if its owner allows it there).
    pub can_unlock: bool,
    /// Its screen is locked (`None`: it hasn't said: an older version, or its desktop
    /// doesn't tell). Cleared when it disconnects.
    pub locked: Option<bool>,
    /// It lets this phone unlock it right now (its "Let your phone unlock" switch).
    pub unlock_allowed: bool,
    /// "wifi" or "bluetooth" while connected; empty when offline.
    pub via: String,
    /// The computer's Bluetooth address for the backup link (it tells us), if it has one.
    pub bluetooth: Option<String>,
    /// The computer's battery (0–100) while connected, if it has one and said so.
    pub battery: Option<u8>,
    pub charging: bool,
    /// The computer switched its connection off (it reconnects when switched back on).
    pub paused_by_peer: bool,
}

/// What a scanned QR code says, shown before the user confirms pairing.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PairingCode {
    pub name: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum LinkEvent {
    Paired {
        id: String,
        name: String,
    },
    /// Also sent when the link changes (Bluetooth backup → Wi-Fi). `via`: "wifi" or
    /// "bluetooth".
    Connected {
        id: String,
        name: String,
        via: String,
    },
    Disconnected {
        id: String,
        name: String,
    },
    Text {
        id: String,
        name: String,
        text: String,
    },
    /// The computer asks to clear this notification on the phone ("dismiss on both").
    DismissRequested {
        id: String,
        key: String,
    },
    /// The computer replies to this notification.
    ReplyRequested {
        id: String,
        key: String,
        text: String,
    },
    /// The computer presses action button `index` (position in `MirroredNotification::actions`).
    ActionRequested {
        id: String,
        key: String,
        index: u32,
    },
    /// The computer switched its connection off (refresh `devices()`).
    ComputerPaused {
        id: String,
    },
    /// The computer's battery changed (refresh `devices()`). `alert`: show "battery low" /
    /// "fully charged" now (same once-only rules as the computer uses for the phone).
    ComputerStatus {
        id: String,
        name: String,
        battery: Option<u8>,
        charging: bool,
        alert: Option<BatteryAlertKind>,
    },
    /// An image copied on the computer arrives as file transfer `transfer_id`: when it is
    /// done, put it on the clipboard instead of saving it in Downloads.
    ClipboardImageIncoming {
        id: String,
        name: String,
        transfer_id: String,
    },
    /// The computer sends a web link: show it as a tap-to-open notification (Android does
    /// not let a background app open it by itself). Already checked to be http(s).
    OpenUrlRequested {
        id: String,
        name: String,
        url: String,
    },
    /// The computer searches this phone's contacts (New message). Answer with
    /// `contacts_result(id, req, …)` (at most 20).
    ContactsRequested {
        id: String,
        req: u32,
        query: String,
    },
    /// What plays on the computer changed (`None`: nothing). For the Remote screen.
    ComputerMedia {
        id: String,
        media: Option<PhoneMedia>,
    },
    /// The computer's screen was locked or unlocked, or its unlock switch changed; read
    /// `devices()` again for the Lock / Unlock tile.
    ComputerLockChanged {
        id: String,
        locked: bool,
        unlock_allowed: bool,
    },
    /// The computer's album art (a small JPEG or PNG), for `ComputerMedia` whose `art_key`
    /// is `key`. Arrives before that `ComputerMedia`.
    ComputerMediaArt {
        id: String,
        key: String,
        image: Vec<u8>,
    },
    /// The computer's own buttons for the Remote screen.
    RemoteCommands {
        id: String,
        items: Vec<RemoteButton>,
    },
    /// The computer asks to decline the ringing call whose notification key is `key`.
    /// Answer with `send_action_result(id, "call", ok, message)`.
    CallDeclineRequested {
        id: String,
        key: String,
    },
    /// The computer presses `action` on the call whose notification key is `key`. Answer
    /// with `send_action_result(id, "call", ok, message)`.
    CallActionRequested {
        id: String,
        key: String,
        action: CallActionFfi,
    },
    /// The computer asks this phone to ring loudly (`on`) or to stop. Already checked to
    /// be recent.
    RingRequested {
        id: String,
        name: String,
        on: bool,
    },
    /// Text copied on the computer: put it on this phone's clipboard (if sharing is on).
    ClipboardReceived {
        id: String,
        name: String,
        text: String,
    },
    /// The computer pressed a music button.
    MediaControlRequested {
        id: String,
        command: MediaCommand,
    },
    /// The computer asks to send an SMS (now; reply with `sms_send_result`).
    SmsSendRequested {
        id: String,
        client_id: String,
        thread: u64,
        address: String,
        body: String,
    },
    /// The computer wants older messages of a conversation (reply with `sms_older`).
    SmsOlderRequested {
        id: String,
        thread: u64,
        before_ms: u64,
    },
    /// A file transfer started, progressed (at most a few times a second) or finished.
    TransferChanged {
        transfer: FileTransfer,
    },
    /// How a request this phone made went (e.g. `key` = "lock").
    RequestResult {
        id: String,
        name: String,
        key: String,
        ok: bool,
        message: String,
    },
}

/// A music button pressed on the computer.
/// Where a call on this phone is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CallPhaseFfi {
    Idle,
    Ringing,
    Active,
}

/// How the presentation pointer looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PointerShapeFfi {
    Dot,
    Ring,
    Spotlight,
}

/// A button of the phone's call notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CallActionFfi {
    Answer,
    Decline,
    End,
    Mute,
    Speaker,
    Hold,
}

impl From<CallActionFfi> for phonelink_core::CallActionKind {
    fn from(a: CallActionFfi) -> Self {
        match a {
            CallActionFfi::Answer => Self::Answer,
            CallActionFfi::Decline => Self::Decline,
            CallActionFfi::End => Self::End,
            CallActionFfi::Mute => Self::Mute,
            CallActionFfi::Speaker => Self::Speaker,
            CallActionFfi::Hold => Self::Hold,
        }
    }
}

impl From<phonelink_core::CallActionKind> for CallActionFfi {
    fn from(a: phonelink_core::CallActionKind) -> Self {
        use phonelink_core::CallActionKind as K;
        match a {
            K::Answer => Self::Answer,
            K::Decline => Self::Decline,
            K::End => Self::End,
            K::Mute => Self::Mute,
            K::Speaker => Self::Speaker,
            K::Hold => Self::Hold,
        }
    }
}

/// The computer's battery just went low or full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BatteryAlertKind {
    Low,
    Full,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
    VolumeUp,
    VolumeDown,
    SetVolume { level: u32 },
    /// Mute or unmute the computer (only for `computer_media_control`, needs `can_mute`).
    ToggleMute,
}

impl From<MediaCommand> for MediaAction {
    fn from(c: MediaCommand) -> Self {
        match c {
            MediaCommand::PlayPause => Self::PlayPause,
            MediaCommand::Next => Self::Next,
            MediaCommand::Previous => Self::Previous,
            MediaCommand::VolumeUp => Self::VolumeUp,
            MediaCommand::VolumeDown => Self::VolumeDown,
            MediaCommand::SetVolume { level } => Self::SetVolume(level),
            MediaCommand::ToggleMute => Self::ToggleMute,
        }
    }
}

/// One of the computer's own buttons for the Remote screen.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RemoteButton {
    pub id: String,
    pub name: String,
}

/// A contact found on this phone, for the computer's "New message" search.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ContactEntry {
    pub name: String,
    pub number: String,
}

/// The presentation remote's keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PresentKeyFfi {
    Next,
    Previous,
    Start,
    End,
    Blank,
    /// Ctrl+F5: Google Slides (needs `can_present_apps`).
    StartSlides,
    /// Ctrl+Alt+P: Canva (needs `can_present_apps`).
    StartCanva,
    /// F11: full screen, toggles (needs `can_present_apps`).
    StartFullscreen,
}

impl From<MediaAction> for MediaCommand {
    fn from(a: MediaAction) -> Self {
        match a {
            MediaAction::PlayPause => Self::PlayPause,
            MediaAction::Next => Self::Next,
            MediaAction::Previous => Self::Previous,
            MediaAction::VolumeUp => Self::VolumeUp,
            MediaAction::VolumeDown => Self::VolumeDown,
            MediaAction::SetVolume(level) => Self::SetVolume { level },
            MediaAction::ToggleMute => Self::ToggleMute,
        }
    }
}

/// What is playing on this phone, for the computer's Now Playing card.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PhoneMedia {
    pub app: String,
    pub package: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub position_ms: u64,
    pub playing: bool,
    pub can_previous: bool,
    pub can_next: bool,
    pub volume: u32,
    pub max_volume: u32,
    /// Names the image sent with `send_media_art`; empty for none.
    pub art_key: String,
}

impl PhoneMedia {
    fn from_wire(n: NowPlaying) -> Self {
        Self {
            app: n.app,
            package: n.package,
            title: n.title,
            artist: n.artist,
            album: n.album,
            duration_ms: n.duration_ms,
            position_ms: n.position_ms,
            playing: n.playing,
            can_previous: n.can_previous,
            can_next: n.can_next,
            volume: n.volume.min(n.max_volume),
            max_volume: n.max_volume,
            art_key: n.art_key,
        }
    }

    fn into_wire(self) -> NowPlaying {
        NowPlaying {
            app: truncate_text(&self.app, 100),
            package: truncate_text(&self.package, 200),
            title: truncate_text(&self.title, 300),
            artist: truncate_text(&self.artist, 300),
            album: truncate_text(&self.album, 300),
            duration_ms: self.duration_ms,
            position_ms: self.position_ms,
            playing: self.playing,
            can_previous: self.can_previous,
            can_next: self.can_next,
            volume: self.volume.min(self.max_volume),
            max_volume: self.max_volume,
            art_key: truncate_text(&self.art_key, 100),
        }
    }
}

/// A notification shown on this phone, to mirror to the computer.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MirroredNotification {
    pub key: String,
    pub package: String,
    pub app: String,
    pub title: String,
    pub text: String,
    pub posted_ms: u64,
    pub can_reply: bool,
    pub actions: Vec<String>,
}

impl MirroredNotification {
    fn into_wire(self) -> PhoneNotification {
        PhoneNotification {
            key: self.key,
            package: self.package,
            app: truncate_text(&self.app, 100),
            title: truncate_text(&self.title, 300),
            text: truncate_text(&self.text, MAX_NOTIFICATION_TEXT),
            posted_ms: self.posted_ms,
            can_reply: self.can_reply,
            actions: self
                .actions
                .into_iter()
                .take(3)
                .map(|a| truncate_text(&a, 60))
                .collect(),
        }
    }
}

/// One SMS read from the phone's message store.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PhoneSms {
    pub id: u64,
    pub from_me: bool,
    pub body: String,
    pub date_ms: u64,
    /// received | sent | sending | failed
    pub state: String,
    pub read: bool,
}

/// One conversation with its most recent messages (oldest first).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PhoneSmsThread {
    pub id: u64,
    pub address: String,
    pub name: String,
    pub messages: Vec<PhoneSms>,
    pub has_older: bool,
}

fn sms_wire(m: PhoneSms) -> SmsMessage {
    SmsMessage {
        id: m.id,
        from_me: m.from_me,
        body: truncate_text(&m.body, phonelink_core::message::SMS_MAX_BODY),
        date_ms: m.date_ms,
        state: match m.state.as_str() {
            "sent" => SmsState::Sent,
            "sending" => SmsState::Sending,
            "failed" => SmsState::Failed,
            _ => SmsState::Received,
        },
        read: m.read,
    }
}

fn thread_wire(t: PhoneSmsThread) -> SmsThread {
    let mut messages: Vec<SmsMessage> = t.messages.into_iter().map(sms_wire).collect();
    messages.sort_by_key(|m| m.date_ms);
    let cut = messages.len().saturating_sub(phonelink_core::message::SMS_MAX_MESSAGES);
    let has_older = t.has_older || cut > 0;
    SmsThread {
        id: t.id,
        address: truncate_text(&t.address, 200),
        name: truncate_text(&t.name, 200),
        messages: messages.split_off(cut),
        has_older,
    }
}

/// A file transfer, for the phone's notification and screen.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FileTransfer {
    pub id: String,
    pub device_id: String,
    pub device_name: String,
    /// Phone → computer.
    pub outgoing: bool,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub done: u64,
    /// queued | waiting | active | done | failed | cancelled
    pub state: String,
    pub message: String,
    /// Received and complete: the file in the app's private folder, to publish to
    /// Downloads (then it may be deleted).
    pub path: Option<String>,
    /// Bytes per second while moving (0 otherwise).
    pub rate: u64,
    /// Seconds left at the current speed, when known.
    pub eta_secs: Option<u64>,
}

fn file_transfer(node: &Node, t: &TransferInfo) -> FileTransfer {
    use phonelink_core::transfer::TransferState as S;
    let state = match t.state {
        S::Queued => "queued",
        S::Waiting => "waiting",
        S::Active => "active",
        S::Done => "done",
        S::Failed => "failed",
        S::Cancelled => "cancelled",
    }
    .to_string();
    FileTransfer {
        id: t.id.clone(),
        device_id: t.peer.short(),
        device_name: name_of(node, &t.peer),
        outgoing: t.direction == Direction::Outgoing,
        name: t.name.clone(),
        mime: t.mime.clone(),
        size: t.size,
        done: t.done,
        state,
        message: t.message.clone(),
        path: t.path.clone(),
        rate: t.rate,
        eta_secs: t.eta_secs,
    }
}

/// Implemented in Kotlin: opens a file being sent (a content URI) and hands over its file
/// descriptor; the core closes it. Return -1 if it can't be opened any more.
#[uniffi::export(with_foreign)]
pub trait FileSource: Send + Sync {
    fn open(&self, transfer_id: String, source: String) -> i32;
}

struct KotlinFiles(Arc<dyn FileSource>);

impl SourceOpener for KotlinFiles {
    fn open(&self, id: &str, source: &str) -> std::io::Result<std::fs::File> {
        let fd = self.0.open(id.to_string(), source.to_string());
        if fd < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "the phone can no longer read this file",
            ));
        }
        // SAFETY: Kotlin detached this descriptor for us (ParcelFileDescriptor.detachFd), so
        // nothing else owns or closes it.
        Ok(unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) })
    }
}

/// Implemented in Kotlin. Called on a background thread, one event at a time, in order.
#[uniffi::export(with_foreign)]
pub trait EventListener: Send + Sync {
    fn on_event(&self, event: LinkEvent);
}

#[derive(uniffi::Object)]
pub struct PhoneLink {
    rt: Runtime,
    node: Node,
    lan: Mutex<Option<Lan>>,
    keepers: Arc<Mutex<HashMap<Fingerprint, JoinHandle<()>>>>,
    caps: Caps,
    power: Power,
    files: Transfers,
    /// Queue notifications for offline computers (off: only connected ones get them).
    queue_notifications: AtomicBool,
}

/// What each connected computer supports (from its Hello).
type Caps = Arc<Mutex<HashMap<Fingerprint, Vec<String>>>>;

/// What each connected computer last reported about itself.
#[derive(Debug, Clone, Copy, Default)]
struct PeerLive {
    battery: Option<u8>,
    charging: bool,
    /// (screen locked, may this phone unlock it), once it said.
    lock: Option<(bool, bool)>,
}

type Power = Arc<Mutex<HashMap<Fingerprint, PeerLive>>>;

fn has_cap(caps: &Caps, fp: &Fingerprint, cap: &str) -> bool {
    caps.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(fp)
        .is_some_and(|c| c.iter().any(|x| x == cap))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn keep_connected(
    node: &Node,
    keepers: &Mutex<HashMap<Fingerprint, JoinHandle<()>>>,
    fp: Fingerprint,
) {
    let mut k = keepers.lock().unwrap_or_else(|e| e.into_inner());
    if k.get(&fp).is_some_and(|h| !h.is_finished()) {
        return;
    }
    k.insert(fp, node.maintain(fp, vec![]));
}

fn device(node: &Node, caps: &Caps, power: &Power, p: &PeerRecord) -> Device {
    let connected = node.is_connected(&p.fingerprint);
    let live = power
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&p.fingerprint)
        .copied()
        .filter(|_| connected)
        .unwrap_or_default();
    let (battery, charging) = (live.battery, live.charging);
    Device {
        id: p.fingerprint.short(),
        name: p.name.clone(),
        connected,
        pending: node.pending_count(&p.fingerprint).unwrap_or(0) as u64,
        can_lock: connected && has_cap(caps, &p.fingerprint, CAP_LOCK),
        can_clipboard: connected && has_cap(caps, &p.fingerprint, CAP_CLIPBOARD),
        can_ring: connected && has_cap(caps, &p.fingerprint, CAP_RING),
        can_remote: connected && has_cap(caps, &p.fingerprint, CAP_REMOTE),
        can_present: connected && has_cap(caps, &p.fingerprint, CAP_PRESENT),
        can_present_apps: connected && has_cap(caps, &p.fingerprint, phonelink_core::CAP_PRESENT2),
        can_mute: connected && has_cap(caps, &p.fingerprint, phonelink_core::CAP_MUTE),
        can_point: connected && has_cap(caps, &p.fingerprint, phonelink_core::CAP_POINTER),
        can_unlock: connected && has_cap(caps, &p.fingerprint, phonelink_core::CAP_UNLOCK),
        locked: live.lock.map(|l| l.0),
        unlock_allowed: live.lock.is_some_and(|l| l.1),
        via: via(node.transport(&p.fingerprint)),
        bluetooth: p.bluetooth.clone(),
        battery,
        charging,
        paused_by_peer: node.peer_paused(&p.fingerprint),
    }
}

fn via(t: Option<phonelink_core::Transport>) -> String {
    match t {
        Some(phonelink_core::Transport::Quic) => "wifi",
        Some(phonelink_core::Transport::Stream) => "bluetooth",
        None => "",
    }
    .to_string()
}

/// Implemented in Kotlin around a connected Bluetooth socket.
#[uniffi::export(with_foreign)]
pub trait StreamSink: Send + Sync {
    /// Write to the socket (blocking). False if the socket failed.
    fn write(&self, data: Vec<u8>) -> bool;
    /// Close the socket: the session ended, or Wi-Fi took over. (Not `close`: that name
    /// clashes with Kotlin's AutoCloseable in the generated bindings.)
    fn close_link(&self);
}

/// The phone's end of a Bluetooth link: Kotlin feeds it what the socket reads.
#[derive(uniffi::Object)]
pub struct StreamPipe {
    tx: Mutex<Option<tokio::sync::mpsc::Sender<Vec<u8>>>>,
}

#[uniffi::export]
impl StreamPipe {
    /// Bytes read from the socket. Blocks while the core catches up. False once the link
    /// is closed (stop reading then).
    pub fn received(&self, data: Vec<u8>) -> bool {
        let tx = self.tx.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match tx {
            Some(tx) => tx.blocking_send(data).is_ok(),
            None => false,
        }
    }

    /// The socket closed or failed.
    pub fn closed(&self) {
        self.tx.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

fn name_of(node: &Node, fp: &Fingerprint) -> String {
    node.peer(fp)
        .map(|p| p.name)
        .unwrap_or_else(|| "Unknown device".into())
}

/// Where the phone's listening port is remembered (inside its private data dir).
const PORT_FILE: &str = "port.txt";

#[uniffi::export]
impl PhoneLink {
    /// Start the node. `data_dir` should be the app's private files directory.
    #[uniffi::constructor]
    pub fn start(
        data_dir: String,
        device_name: String,
        listener: Arc<dyn EventListener>,
        files: Arc<dyn FileSource>,
    ) -> Result<Arc<Self>, LinkError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("phonelink")
            .enable_all()
            .build()
            .map_err(|e| LinkError::Failed(e.to_string()))?;
        let dir = PathBuf::from(data_dir);
        let (node, mut events) = rt.block_on(async {
            // A stable port, so the computer can always dial this phone back at its last
            // address (after the app restarts, a reboot, …). Random on first start, then saved.
            let saved: Option<u16> = std::fs::read_to_string(dir.join(PORT_FILE))
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .filter(|p| *p >= 1024);
            let mut ports: Vec<u16> = saved.into_iter().collect();
            ports.extend(
                (0..4).map(|_| rand::Rng::gen_range(&mut rand::thread_rng(), 40000..60000)),
            );
            ports.push(0); // last resort: any free port
            let mut last = None;
            for port in ports {
                for bind in [
                    SocketAddr::from(([0u16; 8], port)),
                    SocketAddr::from(([0u8; 4], port)),
                ] {
                    let mut cfg = NodeConfig::new(device_name.clone(), Some(dir.clone()), bind);
                    cfg.capabilities.extend(
                        [
                            CAP_NOTIFICATIONS,
                            CAP_LOCK,
                            CAP_CLIPBOARD,
                            CAP_CLIPBOARD_IMAGE,
                            CAP_MEDIA,
                            CAP_FILES,
                            CAP_SMS,
                            CAP_STATUS,
                            CAP_LINKS,
                            CAP_RING,
                            CAP_CALLS,
                            CAP_REMOTE,
                            CAP_PRESENT,
                            CAP_CONTACTS,
                            phonelink_core::CAP_MUTE,
                            phonelink_core::CAP_PRESENT2,
                            phonelink_core::CAP_REMOTE_ART,
                            phonelink_core::CAP_CALLS2,
                            phonelink_core::CAP_POINTER,
                            phonelink_core::CAP_UNLOCK,
                            phonelink_core::CAP_LOCKSTATE,
                        ]
                        .map(String::from),
                    );
                    match Node::start(cfg).await {
                        Ok((node, events)) => {
                            if let Ok(addr) = node.local_addr() {
                                if Some(addr.port()) != saved {
                                    let _ = std::fs::write(
                                        dir.join(PORT_FILE),
                                        addr.port().to_string(),
                                    );
                                }
                            }
                            return Ok((node, events));
                        }
                        Err(e) => last = Some(e),
                    }
                }
            }
            Err(last.expect("at least one bind attempted"))
        })?;

        // Files: received ones go to a private folder first; the app publishes them to
        // Downloads/EchoTwin once complete.
        let (transfers, mut transfer_events) = {
            let _guard = rt.enter();
            Transfers::start(
                node.clone(),
                TransferConfig {
                    db_path: Some(dir.join("transfers.sqlite3")),
                    incoming_dir: dir.join("incoming"),
                    reserve_bytes: 256 * 1024 * 1024,
                },
                Arc::new(KotlinFiles(files)),
            )?
        };

        let keepers = Arc::new(Mutex::new(HashMap::new()));
        for p in node.peers() {
            let _guard = rt.enter();
            keep_connected(&node, &keepers, p.fingerprint);
        }

        // Callbacks into Kotlin happen on one plain thread, never on a runtime worker.
        let (tx, rx) = std_mpsc::channel::<LinkEvent>();
        std::thread::Builder::new()
            .name("phonelink-events".into())
            .spawn(move || {
                while let Ok(ev) = rx.recv() {
                    listener.on_event(ev);
                }
            })
            .map_err(|e| LinkError::Failed(e.to_string()))?;
        let caps: Caps = Arc::new(Mutex::new(HashMap::new()));
        let power: Power = Arc::new(Mutex::new(HashMap::new()));
        let pump_power = power.clone();
        let mut alerts: HashMap<Fingerprint, phonelink_core::alerts::BatteryAlerts> = HashMap::new();
        let (pump_node, pump_keepers, pump_caps) = (node.clone(), keepers.clone(), caps.clone());
        let pump_files = transfers.clone();
        let (file_tx, file_node) = (tx.clone(), node.clone());
        rt.spawn(async move {
            while let Some(t) = transfer_events.recv().await {
                if file_tx
                    .send(LinkEvent::TransferChanged {
                        transfer: file_transfer(&file_node, &t),
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
        rt.spawn(async move {
            while let Some(ev) = events.recv().await {
                let out = match ev {
                    Event::Paired { peer, name } => {
                        keep_connected(&pump_node, &pump_keepers, peer);
                        LinkEvent::Paired {
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
                        pump_caps
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(peer, capabilities);
                        pump_files.nudge(); // files waiting for this computer go now
                        LinkEvent::Connected {
                            id: peer.short(),
                            name,
                            via: via(Some(transport)),
                        }
                    }
                    // Datagrams belong to the separate TouchpadLink node.
                    Event::Datagram { .. } => continue,
                    Event::PeerPaused { peer } => LinkEvent::ComputerPaused { id: peer.short() },
                    Event::PeerDisconnected { peer } => {
                        pump_caps
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&peer);
                        pump_power
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&peer);
                        LinkEvent::Disconnected {
                            id: peer.short(),
                            name: name_of(&pump_node, &peer),
                        }
                    }
                    Event::Message { peer, body, .. } => match AppMessage::decode(&body) {
                        msg @ AppMessage::FileCancel { .. } => {
                            pump_files.on_message(&peer, &msg);
                            continue;
                        }
                        AppMessage::Text { text } => LinkEvent::Text {
                            id: peer.short(),
                            name: name_of(&pump_node, &peer),
                            text,
                        },
                        AppMessage::DismissNotification { key } => LinkEvent::DismissRequested {
                            id: peer.short(),
                            key,
                        },
                        AppMessage::ReplyNotification { key, text } => LinkEvent::ReplyRequested {
                            id: peer.short(),
                            key,
                            text,
                        },
                        AppMessage::NotificationAction { key, index } => {
                            LinkEvent::ActionRequested {
                                id: peer.short(),
                                key,
                                index,
                            }
                        }
                        AppMessage::Clipboard { text, copied_ms } => {
                            // A copy that waited in a queue would paste over something newer.
                            if text.is_empty()
                                || text.len() > MAX_CLIPBOARD_TEXT
                                || !is_fresh(copied_ms, now_ms(), CLIPBOARD_MAX_AGE_MS)
                            {
                                continue;
                            }
                            LinkEvent::ClipboardReceived {
                                id: peer.short(),
                                name: name_of(&pump_node, &peer),
                                text,
                            }
                        }
                        AppMessage::CallDecline { key } => LinkEvent::CallDeclineRequested { id: peer.short(), key },
                        AppMessage::CallAction { key, action } => {
                            LinkEvent::CallActionRequested { id: peer.short(), key, action: action.into() }
                        }
                        AppMessage::ContactsQuery { req, query } => LinkEvent::ContactsRequested {
                            id: peer.short(),
                            req,
                            query: truncate_text(&query, phonelink_core::message::MAX_CONTACT_QUERY),
                        },
                        AppMessage::MediaArt { key, image } => {
                            // From a computer: its album art (phones send theirs the other way).
                            let ok = has_cap(&pump_caps, &peer, phonelink_core::CAP_REMOTE_ART)
                                && key.len() <= 64
                                && phonelink_core::message::is_art_image(&image);
                            if !ok {
                                continue;
                            }
                            LinkEvent::ComputerMediaArt { id: peer.short(), key, image }
                        }
                        AppMessage::ComputerNowPlaying(n) => LinkEvent::ComputerMedia {
                            id: peer.short(),
                            media: n.map(PhoneMedia::from_wire),
                        },
                        AppMessage::RemoteCommands { items } => LinkEvent::RemoteCommands {
                            id: peer.short(),
                            items: items
                                .into_iter()
                                .take(phonelink_core::message::MAX_COMMANDS)
                                .filter(|c| c.id.len() <= 16)
                                .map(|c| RemoteButton {
                                    id: c.id,
                                    name: truncate_text(&c.name, phonelink_core::message::MAX_COMMAND_NAME * 4),
                                })
                                .collect(),
                        },
                        AppMessage::Ring { on, sent_ms } => {
                            if !is_fresh(sent_ms, now_ms(), phonelink_core::message::RING_MAX_AGE_MS) {
                                continue;
                            }
                            LinkEvent::RingRequested {
                                id: peer.short(),
                                name: name_of(&pump_node, &peer),
                                on,
                            }
                        }
                        AppMessage::OpenUrl { url, .. } => {
                            if !phonelink_core::message::is_web_link(&url) {
                                continue;
                            }
                            LinkEvent::OpenUrlRequested {
                                id: peer.short(),
                                name: name_of(&pump_node, &peer),
                                url,
                            }
                        }
                        AppMessage::ClipboardImage { transfer_id, copied_ms } => {
                            // An old copy that waited in a queue: just a received file.
                            if !is_fresh(copied_ms, now_ms(), CLIPBOARD_MAX_AGE_MS) {
                                continue;
                            }
                            LinkEvent::ClipboardImageIncoming {
                                id: peer.short(),
                                name: name_of(&pump_node, &peer),
                                transfer_id,
                            }
                        }
                        AppMessage::SmsSend {
                            client_id,
                            thread,
                            address,
                            body,
                        } => LinkEvent::SmsSendRequested {
                            id: peer.short(),
                            client_id,
                            thread,
                            address,
                            body,
                        },
                        AppMessage::SmsLoadOlder { thread, before_ms } => LinkEvent::SmsOlderRequested {
                            id: peer.short(),
                            thread,
                            before_ms,
                        },
                        AppMessage::ComputerStatus { battery, charging } => {
                            let battery = battery.filter(|b| *b <= 100);
                            {
                                let mut p = pump_power.lock().unwrap_or_else(|e| e.into_inner());
                                let live = p.entry(peer).or_default();
                                live.battery = battery;
                                live.charging = charging;
                            }
                            let alert = alerts.entry(peer).or_default().update(battery, charging).map(|a| match a {
                                phonelink_core::alerts::BatteryAlert::Low(_) => BatteryAlertKind::Low,
                                phonelink_core::alerts::BatteryAlert::Full => BatteryAlertKind::Full,
                            });
                            LinkEvent::ComputerStatus {
                                id: peer.short(),
                                name: name_of(&pump_node, &peer),
                                battery,
                                charging,
                                alert,
                            }
                        }
                        AppMessage::LockState { locked, unlock_allowed } => {
                            pump_power.lock().unwrap_or_else(|e| e.into_inner()).entry(peer).or_default().lock =
                                Some((locked, unlock_allowed));
                            LinkEvent::ComputerLockChanged { id: peer.short(), locked, unlock_allowed }
                        }
                        AppMessage::MediaControl { action } => LinkEvent::MediaControlRequested {
                            id: peer.short(),
                            command: action.into(),
                        },
                        AppMessage::ActionResult { key, ok, message } => LinkEvent::RequestResult {
                            id: peer.short(),
                            name: name_of(&pump_node, &peer),
                            key,
                            ok,
                            message,
                        },
                        // Phone → computer messages never arrive here; unknown ones are skipped.
                        _ => continue,
                    },
                };
                if tx.send(out).is_err() {
                    return;
                }
            }
        });

        Ok(Arc::new(Self {
            rt,
            node,
            lan: Mutex::new(None),
            keepers,
            caps,
            power,
            files: transfers,
            queue_notifications: AtomicBool::new(false),
        }))
    }

    /// The UDP port this phone listens on (stable across restarts).
    pub fn local_port(&self) -> u16 {
        self.node.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// This phone's short id (to compare with what the laptop shows).
    pub fn id(&self) -> String {
        self.node.fingerprint().short()
    }

    /// Start browsing the LAN for paired laptops. On Android, hold a
    /// `WifiManager.MulticastLock` while this runs, or multicast packets are dropped.
    pub fn start_discovery(&self) -> Result<(), LinkError> {
        let mut lan = self.lan.lock().unwrap_or_else(|e| e.into_inner());
        if lan.is_some() {
            return Ok(());
        }
        let _guard = self.rt.enter();
        let l = Lan::new()?;
        l.browse(self.node.clone())?;
        *lan = Some(l);
        Ok(())
    }

    pub fn stop_discovery(&self) {
        if let Some(l) = self.lan.lock().unwrap_or_else(|e| e.into_inner()).take() {
            l.shutdown();
        }
    }

    pub fn devices(&self) -> Vec<Device> {
        self.node
            .peers()
            .iter()
            .map(|p| device(&self.node, &self.caps, &self.power, p))
            .collect()
    }

    /// Parse a scanned QR code without contacting anything.
    pub fn read_pairing_code(&self, uri: String) -> Result<PairingCode, LinkError> {
        let offer = PairingOffer::from_uri(&uri)?;
        Ok(PairingCode {
            name: offer.name,
            id: offer.fingerprint.short(),
        })
    }

    /// Pair with the laptop whose QR code was scanned. Blocks for up to ~10 s.
    pub fn pair(&self, uri: String) -> Result<Device, LinkError> {
        let offer = PairingOffer::from_uri(&uri)?;
        let rec = self.rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(20), self.node.pair_with(&offer))
                .await
                .map_err(|_| {
                    LinkError::Failed(
                        "the laptop did not answer; are you on the same network?".into(),
                    )
                })?
                .map_err(LinkError::from)
        })?;
        {
            let _guard = self.rt.enter();
            keep_connected(&self.node, &self.keepers, rec.fingerprint);
        }
        Ok(device(&self.node, &self.caps, &self.power, &rec))
    }

    /// Queue a text for `device_id` (or the only paired device). Returns true if the
    /// device is connected right now; otherwise it is delivered when it reconnects.
    pub fn send_text(&self, device_id: Option<String>, text: String) -> Result<bool, LinkError> {
        let fp = self.resolve(device_id)?;
        let body = AppMessage::Text { text }.encode()?;
        self.node.send(&fp, body)?;
        Ok(self.node.is_connected(&fp))
    }

    pub fn unpair(&self, device_id: String) -> Result<(), LinkError> {
        let fp = self.resolve(Some(device_id))?;
        if let Some(h) = self
            .keepers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&fp)
        {
            h.abort();
        }
        self.files.forget_peer(&fp);
        self.node.unpair(&fp)?;
        Ok(())
    }

    /// Send a file to the computer (`device_id`, or the only paired one). `source` is
    /// given back to `FileSource::open` (a content URI). It goes when the computer is
    /// connected over Wi-Fi, and resumes after interruptions. Returns the transfer id.
    pub fn send_file(
        &self,
        device_id: Option<String>,
        source: String,
        name: String,
        size: u64,
        mime: String,
    ) -> Result<String, LinkError> {
        let fp = self.resolve(device_id)?;
        let id = {
            let _guard = self.rt.enter();
            self.files.send_file(fp, source, &name, size, &mime)?
        };
        Ok(id)
    }

    /// Stop a transfer on both devices.
    pub fn cancel_transfer(&self, id: String) -> Result<(), LinkError> {
        self.files.cancel(&id)?;
        Ok(())
    }

    /// File transfers, newest first.
    pub fn transfers(&self) -> Vec<FileTransfer> {
        self.files
            .list()
            .iter()
            .map(|t| file_transfer(&self.node, t))
            .collect()
    }

    /// Forget finished transfers (received files stay where they were saved).
    pub fn clear_transfers(&self) -> Result<(), LinkError> {
        self.files.clear_finished()?;
        Ok(())
    }

    /// Mirror a new or updated notification to every paired computer.
    pub fn post_notification(&self, notification: MirroredNotification) -> Result<(), LinkError> {
        self.send_notification(AppMessage::NotificationPosted(notification.into_wire()))
    }

    /// The connection switch: `true` = off (disconnect, stop searching, refuse the
    /// computer until switched back on). Survives nothing: call again after `start`.
    pub fn set_paused(&self, paused: bool) {
        let _guard = self.rt.enter();
        self.node.set_paused(paused);
    }

    pub fn is_paused(&self) -> bool {
        self.node.is_paused()
    }

    /// Whether notifications are queued for computers that are offline (default off:
    /// you were with your phone, so you already saw them).
    pub fn set_queue_notifications(&self, on: bool) {
        self.queue_notifications.store(on, Ordering::Relaxed);
    }

    /// The notification was cleared on the phone.
    pub fn remove_notification(&self, key: String) -> Result<(), LinkError> {
        self.send_notification(AppMessage::NotificationRemoved { key })
    }

    /// Everything currently shown; call when a computer (re)connects.
    pub fn send_snapshot(&self, notifications: Vec<MirroredNotification>) -> Result<(), LinkError> {
        let items = notifications
            .into_iter()
            .take(MAX_SNAPSHOT_ITEMS)
            .map(|n| n.into_wire())
            .collect();
        self.send_all(AppMessage::NotificationSnapshot { items })
    }

    /// An app's icon as a small PNG (sent once per app).
    pub fn send_app_icon(&self, package: String, png: Vec<u8>) -> Result<(), LinkError> {
        self.send_notification(AppMessage::AppIcon { package, png })
    }

    /// Tell the computer `device_id` how its request about `key` went.
    pub fn send_action_result(
        &self,
        device_id: String,
        key: String,
        ok: bool,
        message: String,
    ) -> Result<(), LinkError> {
        let fp = self.resolve(Some(device_id))?;
        let body = AppMessage::ActionResult { key, ok, message }.encode()?;
        self.node.send(&fp, body)?;
        Ok(())
    }

    /// Lock the computer's screen now. Only while it is connected: a lock that arrived later
    /// would be a surprise (the computer also ignores late ones). The outcome arrives as
    /// `RequestResult` with key "lock". Returns the computer's name.
    pub fn lock_computer(&self, device_id: Option<String>) -> Result<String, LinkError> {
        let fp = self.connected_one(device_id, CAP_LOCK, "locking from the phone")?;
        let body = AppMessage::LockComputer { sent_ms: now_ms() }.encode()?;
        self.node.send(&fp, body)?;
        Ok(name_of(&self.node, &fp))
    }

    /// Unlock the computer's screen. Call only after the phone's fingerprint or screen lock
    /// confirmed it is the owner. Sent only while connected (never queued); the computer
    /// answers with `RequestResult` key `unlock` (it refuses unless allowed there). Returns
    /// its name.
    pub fn unlock_computer(&self, device_id: Option<String>) -> Result<String, LinkError> {
        let fp = self.connected_one(device_id, phonelink_core::CAP_UNLOCK, "unlocking from the phone")?;
        let body = AppMessage::UnlockComputer { sent_ms: now_ms() }.encode()?;
        if !self.node.send_fleeting(&fp, body)? {
            return Err(LinkError::Failed(format!("{} is not connected", name_of(&self.node, &fp))));
        }
        Ok(name_of(&self.node, &fp))
    }

    /// Tell connected computers about a call (ringing, ongoing, ended). `caller` may be
    /// empty; `key` is the call notification's key (for Decline). Returns how many got it.
    pub fn send_call_state(&self, phase: CallPhaseFfi, caller: String, key: String) -> Result<u32, LinkError> {
        let state = match phase {
            CallPhaseFfi::Idle => phonelink_core::CallPhase::Idle,
            CallPhaseFfi::Ringing => phonelink_core::CallPhase::Ringing,
            CallPhaseFfi::Active => phonelink_core::CallPhase::Active,
        };
        let caller = phonelink_core::message::truncate_text(&caller, phonelink_core::message::MAX_CALLER);
        self.send_connected(CAP_CALLS, AppMessage::CallState { state, caller, key })
    }

    /// The presentation pointer: `x`, `y` in 0..=10000 across the screen, `visible` false
    /// when the finger lifts. Sent only if the computer is connected right now (never
    /// queued: a late pointer is worse than none). Returns whether it went out.
    #[allow(clippy::too_many_arguments)]
    pub fn pointer(
        &self,
        device_id: Option<String>,
        x: u16,
        y: u16,
        visible: bool,
        shape: PointerShapeFfi,
        color: u32,
        size: u8,
    ) -> Result<bool, LinkError> {
        let fp = self.connected_one(device_id, phonelink_core::CAP_POINTER, "the pointer")?;
        let shape = match shape {
            PointerShapeFfi::Dot => phonelink_core::PointerShape::Dot,
            PointerShapeFfi::Ring => phonelink_core::PointerShape::Ring,
            PointerShapeFfi::Spotlight => phonelink_core::PointerShape::Spotlight,
        };
        let max = phonelink_core::message::POINTER_MAX;
        let msg = AppMessage::Pointer { x: x.min(max), y: y.min(max), visible, shape, color: color & 0xFF_FFFF, size: size.min(2) };
        Ok(self.node.send_fleeting(&fp, msg.encode()?)?)
    }

    /// Tell connected computers about a call and the buttons its notification offers:
    /// `CallInfo` to computers with `calls/2`, the older `CallState` to `calls/1` ones.
    /// Returns how many got it.
    pub fn send_call_info(&self, phase: CallPhaseFfi, caller: String, key: String, actions: Vec<CallActionFfi>) -> Result<u32, LinkError> {
        let phase = match phase {
            CallPhaseFfi::Idle => phonelink_core::CallPhase::Idle,
            CallPhaseFfi::Ringing => phonelink_core::CallPhase::Ringing,
            CallPhaseFfi::Active => phonelink_core::CallPhase::Active,
        };
        let caller = phonelink_core::message::truncate_text(&caller, phonelink_core::message::MAX_CALLER);
        let mut kinds: Vec<phonelink_core::CallActionKind> = Vec::new();
        for a in actions {
            let k = a.into();
            if !kinds.contains(&k) && kinds.len() < phonelink_core::message::MAX_CALL_ACTIONS {
                kinds.push(k);
            }
        }
        let info = AppMessage::CallInfo { key: key.clone(), caller: caller.clone(), phase, actions: kinds }.encode()?;
        let old = AppMessage::CallState { state: phase, caller, key }.encode()?;
        let mut n = 0;
        for p in self.node.peers() {
            if !self.node.is_connected(&p.fingerprint) {
                continue;
            }
            let body = if has_cap(&self.caps, &p.fingerprint, phonelink_core::CAP_CALLS2) {
                &info
            } else if has_cap(&self.caps, &p.fingerprint, CAP_CALLS) {
                &old
            } else {
                continue;
            };
            self.node.send(&p.fingerprint, body.clone())?;
            n += 1;
        }
        Ok(n)
    }

    /// The answer to `ContactsRequested` `req` (cut to 20 contacts).
    pub fn contacts_result(&self, device_id: String, req: u32, items: Vec<ContactEntry>) -> Result<(), LinkError> {
        let fp = self.resolve(Some(device_id))?;
        let items = items
            .into_iter()
            .take(phonelink_core::message::MAX_CONTACTS)
            .map(|c| phonelink_core::Contact { name: truncate_text(&c.name, 100), number: truncate_text(&c.number, 30) })
            .collect();
        self.node.send(&fp, AppMessage::ContactsResult { req, items }.encode()?)?;
        Ok(())
    }

    /// Control what plays on the computer (Remote screen).
    pub fn computer_media_control(&self, device_id: Option<String>, command: MediaCommand) -> Result<(), LinkError> {
        let fp = if command == MediaCommand::ToggleMute {
            self.connected_one(device_id, phonelink_core::CAP_MUTE, "muting the computer")?
        } else {
            self.connected_one(device_id, CAP_REMOTE, "the remote")?
        };
        self.node.send(&fp, AppMessage::ComputerMediaControl { action: command.into() }.encode()?)?;
        Ok(())
    }

    /// Press one of the computer's own buttons. The answer arrives as `RequestResult` with
    /// key `cmd:<id>`.
    pub fn run_command(&self, device_id: Option<String>, id: String) -> Result<(), LinkError> {
        let fp = self.connected_one(device_id, CAP_REMOTE, "the remote")?;
        self.node.send(&fp, AppMessage::RunCommand { id, sent_ms: now_ms() }.encode()?)?;
        Ok(())
    }

    /// A presentation key (Next, Previous, Start, End, Blank). Errors arrive as
    /// `RequestResult` with key `present`.
    pub fn present_key(&self, device_id: Option<String>, key: PresentKeyFfi) -> Result<(), LinkError> {
        let cap = match key {
            PresentKeyFfi::StartSlides | PresentKeyFfi::StartCanva | PresentKeyFfi::StartFullscreen => phonelink_core::CAP_PRESENT2,
            _ => CAP_PRESENT,
        };
        let fp = self.connected_one(device_id, cap, "the presentation remote")?;
        let key = match key {
            PresentKeyFfi::Next => PresentKeyKind::Next,
            PresentKeyFfi::Previous => PresentKeyKind::Previous,
            PresentKeyFfi::Start => PresentKeyKind::Start,
            PresentKeyFfi::End => PresentKeyKind::End,
            PresentKeyFfi::Blank => PresentKeyKind::Blank,
            PresentKeyFfi::StartSlides => PresentKeyKind::StartSlides,
            PresentKeyFfi::StartCanva => PresentKeyKind::StartCanva,
            PresentKeyFfi::StartFullscreen => PresentKeyKind::StartFullscreen,
        };
        self.node.send(&fp, AppMessage::PresentKey { key, sent_ms: now_ms() }.encode()?)?;
        Ok(())
    }

    /// Ring the computer (to find it), or stop. Returns its name.
    pub fn ring_computer(&self, device_id: Option<String>, on: bool) -> Result<String, LinkError> {
        let fp = self.connected_one(device_id, CAP_RING, "ringing")?;
        self.node.send(&fp, AppMessage::Ring { on, sent_ms: now_ms() }.encode()?)?;
        Ok(name_of(&self.node, &fp))
    }

    /// "Open on computer": open a web link in the computer's browser. Returns its name.
    pub fn open_url(&self, device_id: Option<String>, url: String) -> Result<String, LinkError> {
        let url = url.trim().to_string();
        if !phonelink_core::message::is_web_link(&url) {
            return Err(LinkError::Failed("only web links (http or https) can be opened".into()));
        }
        let fp = self.connected_one(device_id, CAP_LINKS, "opening links")?;
        self.node.send(&fp, AppMessage::OpenUrl { url, sent_ms: now_ms() }.encode()?)?;
        Ok(name_of(&self.node, &fp))
    }

    /// Send text copied on the phone to every connected computer that shares the clipboard.
    /// Returns how many got it.
    pub fn send_clipboard(&self, text: String) -> Result<u32, LinkError> {
        if text.trim().is_empty() {
            return Err(LinkError::Failed("the clipboard is empty".into()));
        }
        if text.len() > MAX_CLIPBOARD_TEXT {
            return Err(LinkError::Failed("that is too much text to send".into()));
        }
        let targets: Vec<Fingerprint> = self
            .node
            .peers()
            .iter()
            .map(|p| p.fingerprint)
            .filter(|fp| self.node.is_connected(fp) && has_cap(&self.caps, fp, CAP_CLIPBOARD))
            .collect();
        if targets.is_empty() {
            return Err(LinkError::Failed("no computer is connected".into()));
        }
        let body = AppMessage::Clipboard {
            text,
            copied_ms: now_ms(),
        }
        .encode()?;
        for fp in &targets {
            self.node.send(fp, body.clone())?;
        }
        Ok(targets.len() as u32)
    }

    /// This phone's battery and network, for connected computers that show it. Not queued;
    /// call on `Connected` and whenever something changes. `network`: "5G", "4G", "3G",
    /// "2G", "Mobile data", "Wi-Fi" or "" (none); `signal` 0–4. Returns how many got it.
    pub fn send_phone_status(
        &self,
        battery: Option<u8>,
        charging: bool,
        network: String,
        signal: u8,
    ) -> Result<u32, LinkError> {
        let status = phonelink_core::PhoneStatus {
            battery: battery.filter(|b| *b <= 100),
            charging,
            network: truncate_text(&network, 16),
            signal: signal.min(4),
        };
        self.send_connected(CAP_STATUS, AppMessage::PhoneStatus(status))
    }

    /// What is playing now (`None`: nothing), for connected computers that show music.
    /// Not queued for computers that are offline: they get the current state when they
    /// connect (call again on `Connected`). Returns how many got it.
    pub fn send_now_playing(&self, media: Option<PhoneMedia>) -> Result<u32, LinkError> {
        self.send_connected(CAP_MEDIA, AppMessage::NowPlaying(media.map(PhoneMedia::into_wire)))
    }

    /// Run the session with the computer over a Bluetooth socket the app connected (the
    /// backup when Wi-Fi is gone). Same encryption and pinned keys as over Wi-Fi. Feed the
    /// socket's bytes to the returned pipe; the core writes through `sink` and closes it
    /// when the session ends or Wi-Fi takes over. `Connected` (via "bluetooth") follows
    /// on success.
    pub fn open_stream(
        &self,
        device_id: String,
        sink: Arc<dyn StreamSink>,
    ) -> Result<Arc<StreamPipe>, LinkError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let fp = self.resolve(Some(device_id))?;
        if self.node.transport(&fp) == Some(phonelink_core::Transport::Quic) {
            return Err(LinkError::Failed("already connected over Wi-Fi".into()));
        }
        let (core_end, app_end) = tokio::io::duplex(64 * 1024);
        let (mut app_r, mut app_w) = tokio::io::split(app_end);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        // socket → core
        self.rt.spawn(async move {
            while let Some(d) = rx.recv().await {
                if app_w.write_all(&d).await.is_err() {
                    break;
                }
            }
            let _ = app_w.shutdown().await;
        });
        // core → socket; when the core is done, close the socket
        self.rt.spawn(async move {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match app_r.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let (s, d) = (sink.clone(), buf[..n].to_vec());
                        let ok = tokio::task::spawn_blocking(move || s.write(d)).await.unwrap_or(false);
                        if !ok {
                            break;
                        }
                    }
                }
            }
            let _ = tokio::task::spawn_blocking(move || sink.close_link()).await;
        });
        let node = self.node.clone();
        self.rt.spawn(async move {
            if let Err(e) = node.connect_stream(fp, core_end).await {
                tracing::debug!("Bluetooth session not established: {e}");
            }
        });
        Ok(Arc::new(StreamPipe {
            tx: Mutex::new(Some(tx)),
        }))
    }

    /// All conversations (most recent first), for computers that just connected. Not queued
    /// for computers that are offline: they get a fresh one when they connect.
    pub fn sms_snapshot(&self, threads: Vec<PhoneSmsThread>) -> Result<u32, LinkError> {
        let threads: Vec<SmsThread> = threads
            .into_iter()
            .take(phonelink_core::message::SMS_MAX_THREADS)
            .map(thread_wire)
            .collect();
        let ids = threads.iter().map(|t| t.id).collect();
        let n = self.send_connected(CAP_SMS, AppMessage::SmsSnapshot { threads: ids })?;
        for t in threads {
            self.send_connected(CAP_SMS, AppMessage::SmsThreadUpdate(t))?;
        }
        Ok(n)
    }

    /// One conversation changed (a message arrived or was sent).
    pub fn sms_thread_update(&self, thread: PhoneSmsThread) -> Result<u32, LinkError> {
        self.send_connected(CAP_SMS, AppMessage::SmsThreadUpdate(thread_wire(thread)))
    }

    /// The answer to `SmsOlderRequested`.
    pub fn sms_older(
        &self,
        device_id: String,
        thread: u64,
        messages: Vec<PhoneSms>,
        has_older: bool,
    ) -> Result<(), LinkError> {
        let fp = self.resolve(Some(device_id))?;
        let mut messages: Vec<SmsMessage> = messages.into_iter().map(sms_wire).collect();
        messages.sort_by_key(|m| m.date_ms);
        let msg = AppMessage::SmsOlder {
            thread,
            messages,
            has_older,
        };
        self.node.send(&fp, msg.encode()?)?;
        Ok(())
    }

    /// How an `SmsSendRequested` went.
    pub fn sms_send_result(
        &self,
        device_id: String,
        client_id: String,
        sent: bool,
        message: String,
    ) -> Result<(), LinkError> {
        let fp = self.resolve(Some(device_id))?;
        let msg = AppMessage::SmsSendResult {
            client_id,
            state: if sent { SmsState::Sent } else { SmsState::Failed },
            message,
        };
        self.node.send(&fp, msg.encode()?)?;
        Ok(())
    }

    /// Album art for `PhoneMedia::art_key`: a JPEG or PNG of at most 64 KB.
    pub fn send_media_art(&self, key: String, image: Vec<u8>) -> Result<u32, LinkError> {
        if !phonelink_core::message::is_art_image(&image) {
            return Err(LinkError::Failed("album art must be a JPEG or PNG under 64 KB".into()));
        }
        self.send_connected(CAP_MEDIA, AppMessage::MediaArt { key, image })
    }

    /// Call from `ConnectivityManager.NetworkCallback`: retry immediately instead of
    /// waiting out the reconnect backoff.
    pub fn on_network_changed(&self) {
        self.node.nudge();
    }

    pub fn shutdown(&self) {
        self.stop_discovery();
        self.rt.block_on(self.node.shutdown());
    }
}

impl PhoneLink {
    /// Queue a message for every paired computer (durably: it arrives after a reconnect).
    fn send_all(&self, msg: AppMessage) -> Result<(), LinkError> {
        let body = msg.encode()?;
        for p in self.node.peers() {
            self.node.send(&p.fingerprint, body.clone())?;
        }
        Ok(())
    }

    /// Notification traffic: queued, or only to connected computers, per the setting.
    fn send_notification(&self, msg: AppMessage) -> Result<(), LinkError> {
        if self.queue_notifications.load(Ordering::Relaxed) {
            self.send_all(msg)
        } else {
            self.send_connected(CAP_NOTIFICATIONS, msg).map(|_| ())
        }
    }

    /// Send to every connected computer supporting `cap`; returns how many.
    fn send_connected(&self, cap: &str, msg: AppMessage) -> Result<u32, LinkError> {
        let body = msg.encode()?;
        let mut n = 0;
        for p in self.node.peers() {
            if self.node.is_connected(&p.fingerprint) && has_cap(&self.caps, &p.fingerprint, cap) {
                self.node.send(&p.fingerprint, body.clone())?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// The device to send a "right now" request to: connected and supporting `cap`.
    fn connected_one(
        &self,
        device_id: Option<String>,
        cap: &str,
        what: &str,
    ) -> Result<Fingerprint, LinkError> {
        let fp = match device_id {
            Some(id) => self.resolve(Some(id))?,
            None => {
                let peers = self.node.peers();
                let connected: Vec<_> = peers
                    .iter()
                    .filter(|p| self.node.is_connected(&p.fingerprint))
                    .collect();
                match connected.as_slice() {
                    [one] => one.fingerprint,
                    [] if peers.is_empty() => return Err(LinkError::Failed("no paired computer".into())),
                    [] => {
                        return Err(LinkError::Failed(format!(
                            "{} is not connected",
                            peers[0].name
                        )))
                    }
                    _ => return Err(LinkError::Failed("several computers connected; choose one".into())),
                }
            }
        };
        if !self.node.is_connected(&fp) {
            return Err(LinkError::Failed(format!("{} is not connected", name_of(&self.node, &fp))));
        }
        if !has_cap(&self.caps, &fp, cap) {
            return Err(LinkError::Failed(format!(
                "update EchoTwin on {} to use {what}",
                name_of(&self.node, &fp)
            )));
        }
        Ok(fp)
    }

    fn resolve(&self, device_id: Option<String>) -> Result<Fingerprint, LinkError> {
        let peers = self.node.peers();
        match device_id {
            None => match peers.as_slice() {
                [only] => Ok(only.fingerprint),
                [] => Err(LinkError::Failed("no paired devices".into())),
                _ => Err(LinkError::Failed(
                    "several devices paired; choose one".into(),
                )),
            },
            Some(id) => peers
                .iter()
                .find(|p| p.fingerprint.short().eq_ignore_ascii_case(&id))
                .map(|p| p.fingerprint)
                .ok_or_else(|| LinkError::Failed(format!("no paired device {id}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Test file source: sources are plain paths.
    struct Paths;
    impl FileSource for Paths {
        fn open(&self, _id: String, source: String) -> i32 {
            use std::os::fd::IntoRawFd;
            std::fs::File::open(source).map(|f| f.into_raw_fd()).unwrap_or(-1)
        }
    }

    struct Collect(Mutex<mpsc::Sender<LinkEvent>>);
    impl EventListener for Collect {
        fn on_event(&self, event: LinkEvent) {
            let _ = self.0.lock().unwrap().send(event);
        }
    }

    fn wait(rx: &mpsc::Receiver<LinkEvent>, f: impl Fn(&LinkEvent) -> bool) -> LinkEvent {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let ev = rx.recv_timeout(left).expect("timed out waiting for event");
            if f(&ev) {
                return ev;
            }
        }
    }

    /// The exact call sequence the Android app makes, against a real laptop node.
    #[test]
    fn android_call_sequence_against_a_laptop() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (laptop, mut lrx) = rt
            .block_on(Node::start(NodeConfig::new(
                "Kubuntu",
                None,
                "127.0.0.1:0".parse().unwrap(),
            )))
            .unwrap();
        let uri = laptop
            .start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()])
            .to_uri();

        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let phone = PhoneLink::start(
            dir.path().to_string_lossy().into(),
            "Galaxy S24 FE".into(),
            Arc::new(Collect(Mutex::new(tx))),
            Arc::new(Paths),
        )
        .unwrap();

        let code = phone.read_pairing_code(uri.clone()).unwrap();
        assert_eq!(
            code,
            PairingCode {
                name: "Kubuntu".into(),
                id: laptop.fingerprint().short()
            }
        );
        assert!(phone
            .read_pairing_code("https://example.com".into())
            .is_err());

        let dev = phone.pair(uri).unwrap();
        assert_eq!(dev.name, "Kubuntu");
        wait(&rx, |e| matches!(e, LinkEvent::Connected { .. }));
        assert!(phone.devices()[0].connected);

        assert!(phone.send_text(None, "from phone".into()).unwrap());
        let got = rt.block_on(async {
            loop {
                if let Some(Event::Message { body, .. }) = lrx.recv().await {
                    return AppMessage::decode(&body);
                }
            }
        });
        assert_eq!(
            got,
            AppMessage::Text {
                text: "from phone".into()
            }
        );

        let phone_fp = laptop.peers()[0].fingerprint;
        laptop
            .send(
                &phone_fp,
                AppMessage::Text {
                    text: "from laptop".into(),
                }
                .encode()
                .unwrap(),
            )
            .unwrap();
        let ev = wait(&rx, |e| matches!(e, LinkEvent::Text { .. }));
        assert_eq!(
            ev,
            LinkEvent::Text {
                id: dev.id.clone(),
                name: "Kubuntu".into(),
                text: "from laptop".into()
            }
        );

        phone.on_network_changed(); // must be harmless while connected
        phone.unpair(dev.id).unwrap();
        assert!(phone.devices().is_empty());
        phone.shutdown();
        rt.block_on(laptop.shutdown());
    }

    /// Battery and network: phone → computer when it supports it, and the computer's battery
    /// shows on the phone's device card.
    #[test]
    fn status_both_ways() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (laptop, mut lrx) = rt
            .block_on(Node::start({
                let mut c = NodeConfig::new("Kubuntu", None, "127.0.0.1:0".parse().unwrap());
                c.capabilities.push(CAP_STATUS.into());
                c
            }))
            .unwrap();
        let uri = laptop
            .start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()])
            .to_uri();
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let phone = PhoneLink::start(
            dir.path().to_string_lossy().into(),
            "S24".into(),
            Arc::new(Collect(Mutex::new(tx))),
            Arc::new(Paths),
        )
        .unwrap();
        phone.pair(uri).unwrap();
        wait(&rx, |e| matches!(e, LinkEvent::Connected { .. }));

        let n = phone
            .send_phone_status(Some(250), true, "5G".into(), 9)
            .unwrap();
        assert_eq!(n, 1);
        let got = rt.block_on(async {
            loop {
                if let Some(Event::Message { body, .. }) = lrx.recv().await {
                    return AppMessage::decode(&body);
                }
            }
        });
        assert_eq!(
            got,
            AppMessage::PhoneStatus(phonelink_core::PhoneStatus {
                battery: None,
                charging: true,
                network: "5G".into(),
                signal: 4
            }),
            "out-of-range values are dropped or clamped"
        );

        assert_eq!(phone.devices()[0].battery, None);
        let phone_fp = laptop.peers()[0].fingerprint;
        laptop
            .send(
                &phone_fp,
                AppMessage::ComputerStatus {
                    battery: Some(76),
                    charging: true,
                }
                .encode()
                .unwrap(),
            )
            .unwrap();
        wait(&rx, |e| matches!(e, LinkEvent::ComputerStatus { battery: Some(76), alert: None, .. }));
        let d = &phone.devices()[0];
        assert_eq!((d.battery, d.charging), (Some(76), true));
        for (b, alert) in [(Some(14), Some(BatteryAlertKind::Low)), (Some(12), None)] {
            laptop
                .send(&phone_fp, AppMessage::ComputerStatus { battery: b, charging: false }.encode().unwrap())
                .unwrap();
            wait(&rx, |e| matches!(e, LinkEvent::ComputerStatus { battery, alert: a, .. } if *battery == b && *a == alert));
        }

        // The connection switch: the laptop is told, the phone stays off until switched on.
        phone.set_paused(true);
        assert!(phone.is_paused());
        rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(10), async {
                while !matches!(lrx.recv().await, Some(Event::PeerPaused { .. })) {}
            })
            .await
            .expect("laptop told the phone switched off")
        });
        wait(&rx, |e| matches!(e, LinkEvent::Disconnected { .. }));
        phone.set_paused(false);
        wait(&rx, |e| matches!(e, LinkEvent::Connected { .. }));

        rt.block_on(laptop.shutdown());
        wait(&rx, |e| matches!(e, LinkEvent::Disconnected { .. }));
        assert_eq!(phone.devices()[0].battery, None, "forgotten when it goes offline");
        phone.shutdown();
    }

    /// The calls MirrorService makes, against a real laptop node.
    #[test]
    fn notification_mirroring_call_sequence() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (laptop, mut lrx) = rt
            .block_on(Node::start({
                let mut c = NodeConfig::new("Kubuntu", None, "127.0.0.1:0".parse().unwrap());
                c.capabilities.push(CAP_NOTIFICATIONS.into());
                c
            }))
            .unwrap();
        let uri = laptop
            .start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()])
            .to_uri();
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let phone = PhoneLink::start(
            dir.path().to_string_lossy().into(),
            "S24".into(),
            Arc::new(Collect(Mutex::new(tx))),
            Arc::new(Paths),
        )
        .unwrap();
        phone.pair(uri).unwrap();
        wait(&rx, |e| matches!(e, LinkEvent::Connected { .. }));

        let next_msg = |lrx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>| {
            rt.block_on(async {
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        if let Some(Event::Message { body, .. }) = lrx.recv().await {
                            return AppMessage::decode(&body);
                        }
                    }
                })
                .await
                .expect("laptop got a message")
            })
        };

        let long = "x".repeat(5000);
        phone
            .post_notification(MirroredNotification {
                key: "k1".into(),
                package: "com.whatsapp".into(),
                app: "WhatsApp".into(),
                title: "Rahul".into(),
                text: long,
                posted_ms: 1,
                can_reply: true,
                actions: vec!["a".into(), "b".into(), "c".into(), "d".into()],
            })
            .unwrap();
        let AppMessage::NotificationPosted(n) = next_msg(&mut lrx) else {
            panic!("expected a posted notification")
        };
        assert_eq!(
            n.text.len(),
            phonelink_core::message::MAX_NOTIFICATION_TEXT,
            "long text truncated"
        );
        assert_eq!(n.actions.len(), 3, "at most three buttons");

        // A snapshot of the most notifications, each as long as allowed, still fits one frame.
        let biggest: Vec<_> = (0..MAX_SNAPSHOT_ITEMS + 5)
            .map(|i| {
                MirroredNotification {
                    key: format!("0|com.whatsapp|{i}|null|10{}", "k".repeat(150)),
                    package: "com.whatsapp".repeat(10),
                    app: "é".repeat(200),
                    title: "é".repeat(400),
                    text: "é".repeat(MAX_NOTIFICATION_TEXT),
                    posted_ms: u64::MAX,
                    can_reply: true,
                    actions: vec!["é".repeat(100); 5],
                }
                .into_wire()
            })
            .take(MAX_SNAPSHOT_ITEMS)
            .collect();
        let size = AppMessage::NotificationSnapshot { items: biggest }.encode().unwrap().len();
        assert!(size < phonelink_core::wire::MAX_BODY_LEN, "snapshot of {size} bytes doesn't fit a frame");

        phone.send_snapshot(vec![]).unwrap();
        assert_eq!(
            next_msg(&mut lrx),
            AppMessage::NotificationSnapshot { items: vec![] }
        );
        phone
            .send_app_icon("com.whatsapp".into(), vec![1, 2, 3])
            .unwrap();
        assert!(matches!(next_msg(&mut lrx), AppMessage::AppIcon { .. }));
        phone.remove_notification("k1".into()).unwrap();
        assert_eq!(
            next_msg(&mut lrx),
            AppMessage::NotificationRemoved { key: "k1".into() }
        );

        // The laptop asks the phone to reply; the phone reports back.
        let phone_fp = laptop.peers()[0].fingerprint;
        laptop
            .send(
                &phone_fp,
                AppMessage::ReplyNotification {
                    key: "k1".into(),
                    text: "On my way".into(),
                }
                .encode()
                .unwrap(),
            )
            .unwrap();
        let ev = wait(&rx, |e| matches!(e, LinkEvent::ReplyRequested { .. }));
        let LinkEvent::ReplyRequested { id, key, text } = ev else {
            unreachable!()
        };
        assert_eq!((key.as_str(), text.as_str()), ("k1", "On my way"));
        phone
            .send_action_result(id, key, true, "Reply sent".into())
            .unwrap();
        assert_eq!(
            next_msg(&mut lrx),
            AppMessage::ActionResult {
                key: "k1".into(),
                ok: true,
                message: "Reply sent".into()
            }
        );

        laptop
            .send(
                &phone_fp,
                AppMessage::DismissNotification { key: "k9".into() }
                    .encode()
                    .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            wait(&rx, |e| matches!(e, LinkEvent::DismissRequested { .. })),
            LinkEvent::DismissRequested { .. }
        ));
        laptop
            .send(
                &phone_fp,
                AppMessage::NotificationAction {
                    key: "k9".into(),
                    index: 1,
                }
                .encode()
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            wait(&rx, |e| matches!(e, LinkEvent::ActionRequested { .. })),
            LinkEvent::ActionRequested { index: 1, .. }
        ));

        // Laptop away: with the default setting nothing is queued for it.
        let laptop_fp = laptop.fingerprint();
        rt.block_on(laptop.shutdown());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while phone.node.is_connected(&laptop_fp) {
            assert!(std::time::Instant::now() < deadline, "phone saw the disconnect");
            std::thread::sleep(Duration::from_millis(50));
        }
        let queued = || phone.node.pending_count(&laptop_fp).unwrap();
        let before = queued();
        let note = |key: &str| MirroredNotification {
            key: key.into(),
            package: "com.whatsapp".into(),
            app: "WhatsApp".into(),
            title: "Away".into(),
            text: "seen on the phone".into(),
            posted_ms: 2,
            can_reply: false,
            actions: vec![],
        };
        phone.post_notification(note("k2")).unwrap();
        phone.remove_notification("k2".into()).unwrap();
        phone.send_app_icon("com.x".into(), vec![1]).unwrap();
        assert_eq!(queued(), before, "nothing queued while the setting is off");
        phone.set_queue_notifications(true);
        phone.post_notification(note("k3")).unwrap();
        phone.remove_notification("k3".into()).unwrap();
        assert_eq!(queued(), before + 2, "queued when the setting is on");

        phone.shutdown();
    }

    /// After the app restarts, the phone listens where the laptop remembers it.
    #[test]
    fn phone_port_is_stable_across_restarts() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (laptop, mut lrx) = rt
            .block_on(Node::start(NodeConfig::new(
                "Kubuntu",
                None,
                "127.0.0.1:0".parse().unwrap(),
            )))
            .unwrap();
        let uri = laptop
            .start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()])
            .to_uri();
        let dir = tempfile::tempdir().unwrap();
        let start = || {
            let (tx, _rx) = mpsc::channel();
            PhoneLink::start(
                dir.path().to_string_lossy().into(),
                "S24".into(),
                Arc::new(Collect(Mutex::new(tx))),
            Arc::new(Paths),
            )
            .unwrap()
        };
        let phone = start();
        let first_port = phone.local_port();
        assert!((40000..60000).contains(&first_port), "port {first_port}");
        phone.pair(uri).unwrap();
        rt.block_on(async {
            while !matches!(lrx.recv().await, Some(Event::PeerConnected { .. })) {}
        });
        phone.shutdown();
        drop(phone);

        let again = start();
        assert_eq!(again.local_port(), first_port, "same port after a restart");
        let remembered = laptop.peers()[0].last_addrs[0];
        assert_eq!(
            remembered.port(),
            again.local_port(),
            "the laptop can dial the phone back"
        );
        again.shutdown();
        rt.block_on(laptop.shutdown());
    }

    /// Lock and clipboard, as the Android app calls them, against a laptop node that
    /// supports both, then against one that doesn't (an older EchoTwin on the computer).
    #[test]
    fn lock_and_clipboard_call_sequence() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let start_laptop = |caps: &[&str]| {
            let mut cfg = NodeConfig::new("Kubuntu", None, "127.0.0.1:0".parse().unwrap());
            cfg.capabilities.extend(caps.iter().map(|c| c.to_string()));
            rt.block_on(Node::start(cfg)).unwrap()
        };
        let (laptop, mut lrx) = start_laptop(&[CAP_LOCK, CAP_CLIPBOARD, CAP_LINKS, CAP_RING, CAP_CALLS, CAP_REMOTE, CAP_PRESENT, phonelink_core::CAP_MUTE, phonelink_core::CAP_PRESENT2, phonelink_core::CAP_REMOTE_ART, phonelink_core::CAP_CALLS2, phonelink_core::CAP_POINTER, phonelink_core::CAP_UNLOCK, phonelink_core::CAP_LOCKSTATE]);
        let uri = laptop
            .start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()])
            .to_uri();
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let phone = PhoneLink::start(
            dir.path().to_string_lossy().into(),
            "Galaxy S24 FE".into(),
            Arc::new(Collect(Mutex::new(tx))),
            Arc::new(Paths),
        )
        .unwrap();
        // Not paired yet: a clear error, nothing sent.
        assert!(phone.lock_computer(None).is_err());
        assert!(phone.send_clipboard("x".into()).is_err());
        phone.pair(uri).unwrap();
        wait(&rx, |e| matches!(e, LinkEvent::Connected { .. }));
        let d = &phone.devices()[0];
        assert!(d.can_lock && d.can_clipboard, "{d:?}");

        let next_from_phone = |lrx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>| {
            rt.block_on(async {
                loop {
                    if let Some(Event::Message { body, .. }) = lrx.recv().await {
                        return AppMessage::decode(&body);
                    }
                }
            })
        };

        assert_eq!(phone.lock_computer(None).unwrap(), "Kubuntu");
        let got = next_from_phone(&mut lrx);
        assert!(
            matches!(got, AppMessage::LockComputer { sent_ms } if sent_ms.abs_diff(now_ms()) < 10_000),
            "{got:?}"
        );
        let phone_fp = laptop.peers()[0].fingerprint;
        let to_phone = |m: AppMessage| laptop.send(&phone_fp, m.encode().unwrap()).unwrap();
        to_phone(AppMessage::ActionResult {
            key: "lock".into(),
            ok: true,
            message: "Computer locked".into(),
        });
        let ev = wait(&rx, |e| matches!(e, LinkEvent::RequestResult { .. }));
        assert_eq!(
            ev,
            LinkEvent::RequestResult {
                id: laptop.fingerprint().short(),
                name: "Kubuntu".into(),
                key: "lock".into(),
                ok: true,
                message: "Computer locked".into()
            }
        );

        assert_eq!(phone.send_clipboard("from the phone".into()).unwrap(), 1);
        assert!(matches!(
            next_from_phone(&mut lrx),
            AppMessage::Clipboard { text, .. } if text == "from the phone"
        ));
        assert!(phone.send_clipboard("  ".into()).is_err());
        assert!(phone
            .send_clipboard("x".repeat(MAX_CLIPBOARD_TEXT + 1))
            .is_err());

        // Links: bad ones never leave the phone; a web link reaches the laptop, timestamped.
        assert!(phone.open_url(None, "file:///etc/passwd".into()).is_err());
        assert_eq!(phone.open_url(None, " https://example.org/a ".into()).unwrap(), "Kubuntu");
        assert!(matches!(
            next_from_phone(&mut lrx),
            AppMessage::OpenUrl { url, sent_ms } if url == "https://example.org/a" && sent_ms.abs_diff(now_ms()) < 10_000
        ));
        // Remote: controls, buttons and presentation keys go to the laptop; what plays there
        // and its button names come back as events.
        phone.computer_media_control(None, MediaCommand::Next).unwrap();
        assert!(matches!(next_from_phone(&mut lrx), AppMessage::ComputerMediaControl { action: MediaAction::Next }));
        phone.run_command(None, "ab12cd34".into()).unwrap();
        assert!(matches!(next_from_phone(&mut lrx), AppMessage::RunCommand { id, sent_ms } if id == "ab12cd34" && sent_ms.abs_diff(now_ms()) < 10_000));
        phone.present_key(None, PresentKeyFfi::Blank).unwrap();
        assert!(matches!(next_from_phone(&mut lrx), AppMessage::PresentKey { key: PresentKeyKind::Blank, .. }));
        assert!(phone.devices()[0].can_remote && phone.devices()[0].can_present);
        let d = &phone.devices()[0];
        assert!(d.can_mute && d.can_present_apps && d.can_point && d.can_unlock);
        assert_eq!(phone.devices()[0].locked, None, "not said yet");
        to_phone(AppMessage::LockState { locked: true, unlock_allowed: true });
        let ev = wait(&rx, |e| matches!(e, LinkEvent::ComputerLockChanged { .. }));
        assert!(matches!(ev, LinkEvent::ComputerLockChanged { locked: true, unlock_allowed: true, .. }));
        let d2 = &phone.devices()[0];
        assert_eq!((d2.locked, d2.unlock_allowed), (Some(true), true));
        assert_eq!(phone.unlock_computer(None).unwrap(), "Kubuntu");
        assert!(matches!(next_from_phone(&mut lrx), AppMessage::UnlockComputer { sent_ms } if sent_ms.abs_diff(now_ms()) < 10_000));
        // The pointer: clamped, masked, not queued.
        let queued_before = phone.node.pending_count(&laptop.fingerprint()).unwrap();
        assert!(phone.pointer(None, 20_000, 5_000, true, PointerShapeFfi::Ring, 0xAAFF3B30, 9).unwrap());
        assert!(matches!(
            next_from_phone(&mut lrx),
            AppMessage::Pointer { x: 10_000, y: 5_000, visible: true, shape: phonelink_core::PointerShape::Ring, color: 0xFF3B30, size: 2 }
        ));
        // Earlier messages may still wait for their ack; the pointer adds nothing to the queue.
        assert!(phone.node.pending_count(&laptop.fingerprint()).unwrap() <= queued_before, "never queued");
        phone.computer_media_control(None, MediaCommand::ToggleMute).unwrap();
        assert!(matches!(next_from_phone(&mut lrx), AppMessage::ComputerMediaControl { action: MediaAction::ToggleMute }));
        for (ffi, wire) in [
            (PresentKeyFfi::StartSlides, PresentKeyKind::StartSlides),
            (PresentKeyFfi::StartCanva, PresentKeyKind::StartCanva),
            (PresentKeyFfi::StartFullscreen, PresentKeyKind::StartFullscreen),
        ] {
            phone.present_key(None, ffi).unwrap();
            assert!(matches!(next_from_phone(&mut lrx), AppMessage::PresentKey { key, .. } if key == wire));
        }
        // The laptop's album art: a bad image is dropped, a good one becomes an event.
        to_phone(AppMessage::MediaArt { key: "bad".into(), image: b"not a picture".to_vec() });
        let jpeg = [&[0xFF, 0xD8, 0xFF, 0xE0][..], &[0u8; 64]].concat();
        to_phone(AppMessage::MediaArt { key: "k1".into(), image: jpeg.clone() });
        let ev = wait(&rx, |e| matches!(e, LinkEvent::ComputerMediaArt { .. }));
        assert!(matches!(ev, LinkEvent::ComputerMediaArt { key, image, .. } if key == "k1" && image == jpeg), "the bad one was dropped");
        to_phone(AppMessage::RemoteCommands {
            items: vec![phonelink_core::RemoteCommand { id: "ab12cd34".into(), name: "Suspend".into() }],
        });
        let ev = wait(&rx, |e| matches!(e, LinkEvent::RemoteCommands { .. }));
        assert!(matches!(ev, LinkEvent::RemoteCommands { items, .. } if items == vec![RemoteButton { id: "ab12cd34".into(), name: "Suspend".into() }]));
        let playing = NowPlaying {
            app: "VLC".into(), package: String::new(), title: "Lo-fi".into(), artist: "Chillhop".into(), album: String::new(),
            duration_ms: 0, position_ms: 0, playing: true, can_previous: false, can_next: true, volume: 0, max_volume: 0, art_key: String::new(),
        };
        to_phone(AppMessage::ComputerNowPlaying(Some(playing)));
        let ev = wait(&rx, |e| matches!(e, LinkEvent::ComputerMedia { .. }));
        assert!(matches!(ev, LinkEvent::ComputerMedia { media: Some(m), .. } if m.title == "Lo-fi" && m.playing));
        to_phone(AppMessage::ComputerNowPlaying(None));
        assert!(matches!(wait(&rx, |e| matches!(e, LinkEvent::ComputerMedia { .. })), LinkEvent::ComputerMedia { media: None, .. }));

        // Contacts: a search from the laptop becomes an event; the answer goes back, cut to 20.
        to_phone(AppMessage::ContactsQuery { req: 3, query: "mo".into() });
        let ev = wait(&rx, |e| matches!(e, LinkEvent::ContactsRequested { .. }));
        let LinkEvent::ContactsRequested { id, req, query } = ev else { unreachable!() };
        assert_eq!((req, query.as_str()), (3, "mo"));
        let many = (0..30).map(|i| ContactEntry { name: format!("Mo {i}"), number: format!("+91 9000000{i:03}") }).collect();
        phone.contacts_result(id, req, many).unwrap();
        assert!(matches!(next_from_phone(&mut lrx), AppMessage::ContactsResult { req: 3, items } if items.len() == 20 && items[0].name == "Mo 0"));

        // Calls: the state reaches the laptop (long names cut); Decline comes back as an event.
        let long = "M".repeat(300);
        assert_eq!(phone.send_call_state(CallPhaseFfi::Ringing, long, "k1".into()).unwrap(), 1);
        assert!(matches!(
            next_from_phone(&mut lrx),
            AppMessage::CallState { state: phonelink_core::CallPhase::Ringing, caller, key } if caller.len() <= 100 && key == "k1"
        ));
        to_phone(AppMessage::CallDecline { key: "k1".into() });
        let ev = wait(&rx, |e| matches!(e, LinkEvent::CallDeclineRequested { .. }));
        assert!(matches!(ev, LinkEvent::CallDeclineRequested { key, .. } if key == "k1"));
        // calls/2: the buttons go along (no repeats, at most six); a press comes back.
        let many = vec![CallActionFfi::Answer, CallActionFfi::Decline, CallActionFfi::Answer, CallActionFfi::Mute];
        assert_eq!(phone.send_call_info(CallPhaseFfi::Ringing, "Mom".into(), "k2".into(), many).unwrap(), 1);
        use phonelink_core::CallActionKind as K;
        assert!(matches!(
            next_from_phone(&mut lrx),
            AppMessage::CallInfo { phase: phonelink_core::CallPhase::Ringing, actions, key, .. } if actions == vec![K::Answer, K::Decline, K::Mute] && key == "k2"
        ));
        to_phone(AppMessage::CallAction { key: "k2".into(), action: K::Answer });
        let ev = wait(&rx, |e| matches!(e, LinkEvent::CallActionRequested { .. }));
        assert!(matches!(ev, LinkEvent::CallActionRequested { key, action: CallActionFfi::Answer, .. } if key == "k2"));

        // Ringing both ways; an old ring request (queued somewhere) is ignored.
        assert_eq!(phone.ring_computer(None, true).unwrap(), "Kubuntu");
        assert!(matches!(next_from_phone(&mut lrx), AppMessage::Ring { on: true, .. }));
        to_phone(AppMessage::Ring { on: true, sent_ms: now_ms() - 3_600_000 });
        to_phone(AppMessage::Ring { on: false, sent_ms: now_ms() });
        let ev = wait(&rx, |e| matches!(e, LinkEvent::RingRequested { .. }));
        assert!(matches!(ev, LinkEvent::RingRequested { on: false, .. }), "{ev:?}");

        // Laptop → phone: only a plain web link becomes a notification.
        to_phone(AppMessage::OpenUrl { url: "javascript:alert(1)".into(), sent_ms: now_ms() });
        to_phone(AppMessage::OpenUrl { url: "https://example.org/x".into(), sent_ms: 0 });
        let ev = wait(&rx, |e| matches!(e, LinkEvent::OpenUrlRequested { .. }));
        assert!(matches!(ev, LinkEvent::OpenUrlRequested { url, name, .. } if url == "https://example.org/x" && name == "Kubuntu"));

        // Laptop → phone: fresh copies arrive; a stale one (queued for ages) does not.
        to_phone(AppMessage::Clipboard {
            text: "old".into(),
            copied_ms: now_ms() - 3_600_000,
        });
        to_phone(AppMessage::Clipboard {
            text: "ssh lab-pc".into(),
            copied_ms: now_ms(),
        });
        let ev = wait(&rx, |e| matches!(e, LinkEvent::ClipboardReceived { .. }));
        assert!(matches!(ev, LinkEvent::ClipboardReceived { text, .. } if text == "ssh lab-pc"));

        // An image copied on the laptop: the phone learns which transfer to put on its
        // clipboard; an old announcement is ignored (the file is then just a download).
        to_phone(AppMessage::ClipboardImage {
            transfer_id: "old".into(),
            copied_ms: now_ms() - 3_600_000,
        });
        to_phone(AppMessage::ClipboardImage {
            transfer_id: "t-7".into(),
            copied_ms: now_ms(),
        });
        let ev = wait(&rx, |e| matches!(e, LinkEvent::ClipboardImageIncoming { .. }));
        assert!(matches!(ev, LinkEvent::ClipboardImageIncoming { transfer_id, .. } if transfer_id == "t-7"));

        // A computer running an older EchoTwin: the phone says so instead of sending.
        phone.shutdown();
        drop(phone);
        rt.block_on(laptop.shutdown());
        let (old_laptop, _olrx) = start_laptop(&[]);
        let uri = old_laptop
            .start_pairing(Duration::from_secs(60), vec![old_laptop.local_addr().unwrap()])
            .to_uri();
        let dir2 = tempfile::tempdir().unwrap();
        let (tx2, rx2) = mpsc::channel();
        let phone2 = PhoneLink::start(
            dir2.path().to_string_lossy().into(),
            "Galaxy S24 FE".into(),
            Arc::new(Collect(Mutex::new(tx2))),
            Arc::new(Paths),
        )
        .unwrap();
        phone2.pair(uri).unwrap();
        wait(&rx2, |e| matches!(e, LinkEvent::Connected { .. }));
        let d = &phone2.devices()[0];
        assert!(d.connected && !d.can_lock && !d.can_clipboard);
        let err = phone2.lock_computer(None).unwrap_err().to_string();
        assert!(err.contains("update EchoTwin"), "{err}");
        assert!(phone2.send_clipboard("x".into()).is_err());
        assert!(!d.can_unlock && phone2.unlock_computer(None).is_err(), "no unlock without unlock/1");
        phone2.shutdown();
        rt.block_on(old_laptop.shutdown());
    }

    /// Music, as the Android app calls it: state and art go to a computer that shows
    /// music (and not to one that doesn't), and its buttons come back as events.
    #[test]
    fn media_call_sequence() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut cfg = NodeConfig::new("Kubuntu", None, "127.0.0.1:0".parse().unwrap());
        cfg.capabilities.push(CAP_MEDIA.into());
        let (laptop, mut lrx) = rt.block_on(Node::start(cfg)).unwrap();
        let uri = laptop
            .start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()])
            .to_uri();
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let phone = PhoneLink::start(
            dir.path().to_string_lossy().into(),
            "Galaxy S24 FE".into(),
            Arc::new(Collect(Mutex::new(tx))),
            Arc::new(Paths),
        )
        .unwrap();
        let media = PhoneMedia {
            app: "YouTube Music".into(),
            package: "com.google.android.apps.youtube.music".into(),
            title: "Tum Hi Ho".into(),
            artist: "Arijit Singh".into(),
            album: "Aashiqui 2".into(),
            duration_ms: 262_000,
            position_ms: 5_000,
            playing: true,
            can_previous: false,
            can_next: true,
            volume: 20, // more than the maximum: clamped
            max_volume: 15,
            art_key: "tumhiho".into(),
        };
        // Not connected yet: nothing goes anywhere (and nothing is queued).
        assert_eq!(phone.send_now_playing(Some(media.clone())).unwrap(), 0);
        phone.pair(uri).unwrap();
        wait(&rx, |e| matches!(e, LinkEvent::Connected { .. }));
        assert_eq!(phone.devices()[0].pending, 0, "no stale music state queued");

        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];
        assert_eq!(phone.send_media_art("tumhiho".into(), jpeg.clone()).unwrap(), 1);
        assert!(phone.send_media_art("x".into(), b"GIF89a".to_vec()).is_err());
        assert_eq!(phone.send_now_playing(Some(media)).unwrap(), 1);
        assert_eq!(phone.send_now_playing(None).unwrap(), 1);
        let got: Vec<AppMessage> = rt.block_on(async {
            let mut v = vec![];
            while v.len() < 3 {
                if let Some(Event::Message { body, .. }) = lrx.recv().await {
                    v.push(AppMessage::decode(&body));
                }
            }
            v
        });
        assert_eq!(got[0], AppMessage::MediaArt { key: "tumhiho".into(), image: jpeg });
        let AppMessage::NowPlaying(Some(n)) = &got[1] else { panic!("{:?}", got[1]) };
        assert_eq!((n.title.as_str(), n.volume, n.max_volume, n.can_previous), ("Tum Hi Ho", 15, 15, false));
        assert_eq!(got[2], AppMessage::NowPlaying(None));

        let phone_fp = laptop.peers()[0].fingerprint;
        for action in [MediaAction::PlayPause, MediaAction::SetVolume(7)] {
            laptop
                .send(&phone_fp, AppMessage::MediaControl { action }.encode().unwrap())
                .unwrap();
        }
        let ev = wait(&rx, |e| matches!(e, LinkEvent::MediaControlRequested { .. }));
        assert!(matches!(ev, LinkEvent::MediaControlRequested { command: MediaCommand::PlayPause, .. }));
        let ev = wait(&rx, |e| matches!(e, LinkEvent::MediaControlRequested { .. }));
        assert!(matches!(ev, LinkEvent::MediaControlRequested { command: MediaCommand::SetVolume { level: 7 }, .. }));

        // Computer gone: music updates are not queued for later (it gets fresh state
        // when it reconnects instead of a backlog of old songs).
        rt.block_on(laptop.shutdown());
        wait(&rx, |e| matches!(e, LinkEvent::Disconnected { .. }));
        assert_eq!(phone.send_now_playing(None).unwrap(), 0);
        assert_eq!(phone.devices()[0].pending, 0);
        phone.shutdown();
    }

    /// Files, as the Android app drives them: send through the FileSource callback,
    /// receive into the private folder (path in the event), cancel, list.
    #[test]
    fn file_transfer_call_sequence() {
        use phonelink_core::transfer::{PathOpener, TransferConfig, Transfers};
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut cfg = NodeConfig::new("Kubuntu", None, "127.0.0.1:0".parse().unwrap());
        cfg.capabilities.push(CAP_FILES.into());
        let (laptop, mut lrx) = rt.block_on(Node::start(cfg)).unwrap();
        let ldir = tempfile::tempdir().unwrap();
        let (lfiles, _lev) = {
            let _g = rt.enter();
            Transfers::start(
                laptop.clone(),
                TransferConfig {
                    db_path: None,
                    incoming_dir: ldir.path().to_path_buf(),
                    reserve_bytes: 0,
                },
                Arc::new(PathOpener),
            )
            .unwrap()
        };
        let uri = laptop
            .start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()])
            .to_uri();
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let phone = PhoneLink::start(
            dir.path().to_string_lossy().into(),
            "Galaxy S24 FE".into(),
            Arc::new(Collect(Mutex::new(tx))),
            Arc::new(Paths),
        )
        .unwrap();
        phone.pair(uri).unwrap();
        wait(&rx, |e| matches!(e, LinkEvent::Connected { .. }));
        rt.block_on(async {
            while !matches!(lrx.recv().await, Some(Event::PeerConnected { .. })) {}
        });

        // Phone → laptop: the "content URI" here is a path the test FileSource opens.
        let src = tempfile::tempdir().unwrap();
        let photo = src.path().join("IMG_1.jpg");
        std::fs::write(&photo, vec![9u8; 300_000]).unwrap();
        let id = phone
            .send_file(None, photo.to_string_lossy().into(), "IMG_1.jpg".into(), 300_000, "image/jpeg".into())
            .unwrap();
        let ev = wait(&rx, |e| matches!(e, LinkEvent::TransferChanged { transfer } if transfer.id == id && transfer.state == "done"));
        let LinkEvent::TransferChanged { transfer } = ev else { unreachable!() };
        assert!(transfer.outgoing && transfer.device_name == "Kubuntu");
        assert_eq!(std::fs::read(ldir.path().join("IMG_1.jpg")).unwrap(), vec![9u8; 300_000]);

        // A source the app can no longer open fails with a clear message.
        let gone = phone
            .send_file(None, "/no/longer/there.mp4".into(), "there.mp4".into(), 10, "video/mp4".into())
            .unwrap();
        let ev = wait(&rx, |e| matches!(e, LinkEvent::TransferChanged { transfer } if transfer.id == gone && transfer.state == "failed"));
        let LinkEvent::TransferChanged { transfer } = ev else { unreachable!() };
        assert!(transfer.message.contains("Share it again"), "{}", transfer.message);

        // Laptop → phone: lands in the private folder, path in the event.
        let doc = src.path().join("notes.pdf");
        std::fs::write(&doc, b"%PDF-1.7 hello").unwrap();
        let phone_fp = laptop.peers()[0].fingerprint;
        let lid = lfiles
            .send_file(phone_fp, doc.to_string_lossy().into(), "notes.pdf", 14, "application/pdf")
            .unwrap();
        let ev = wait(&rx, |e| matches!(e, LinkEvent::TransferChanged { transfer } if transfer.id == lid && transfer.state == "done"));
        let LinkEvent::TransferChanged { transfer } = ev else { unreachable!() };
        assert!(!transfer.outgoing);
        let path = PathBuf::from(transfer.path.unwrap());
        assert!(path.starts_with(dir.path().join("incoming")), "{path:?}");
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-1.7 hello");
        assert_eq!(transfer.mime, "application/pdf");

        let list = phone.transfers();
        assert_eq!(list.len(), 3);
        phone.clear_transfers().unwrap();
        assert!(phone.transfers().is_empty());
        assert!(path.exists(), "clearing the list keeps the file");
        phone.shutdown();
        rt.block_on(laptop.shutdown());
    }

    /// SMS as the Android app drives it: snapshot and updates go to the computer, its
    /// send and history requests come back as events, and the answers return.
    #[test]
    fn sms_call_sequence() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut cfg = NodeConfig::new("Kubuntu", None, "127.0.0.1:0".parse().unwrap());
        cfg.capabilities.push(CAP_SMS.into());
        let (laptop, mut lrx) = rt.block_on(Node::start(cfg)).unwrap();
        let uri = laptop
            .start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()])
            .to_uri();
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let phone = PhoneLink::start(
            dir.path().to_string_lossy().into(),
            "Galaxy S24 FE".into(),
            Arc::new(Collect(Mutex::new(tx))),
            Arc::new(Paths),
        )
        .unwrap();
        phone.pair(uri).unwrap();
        wait(&rx, |e| matches!(e, LinkEvent::Connected { .. }));
        let sms = |id: u64, from_me: bool, body: &str, date_ms: u64| PhoneSms {
            id,
            from_me,
            body: body.into(),
            date_ms,
            state: if from_me { "sent" } else { "received" }.into(),
            read: true,
        };
        // 35 messages in one conversation: only the newest 30 go, with "has older".
        let many: Vec<PhoneSms> = (0..35).rev().map(|i| sms(i, i % 2 == 0, &format!("m{i}"), 1000 + i)).collect();
        let threads = vec![
            PhoneSmsThread { id: 3, address: "+919800004410".into(), name: "Mom".into(), messages: many, has_older: false },
            PhoneSmsThread { id: 4, address: "AX-HDFCBK".into(), name: "AX-HDFCBK".into(), messages: vec![sms(90, false, "debited", 50)], has_older: false },
        ];
        assert_eq!(phone.sms_snapshot(threads).unwrap(), 1);
        let got: Vec<AppMessage> = rt.block_on(async {
            let mut v = vec![];
            while v.len() < 3 {
                if let Some(Event::Message { body, .. }) = lrx.recv().await {
                    v.push(AppMessage::decode(&body));
                }
            }
            v
        });
        assert_eq!(got[0], AppMessage::SmsSnapshot { threads: vec![3, 4] });
        let AppMessage::SmsThreadUpdate(t) = &got[1] else { panic!("{:?}", got[1]) };
        assert_eq!((t.messages.len(), t.has_older), (30, true));
        assert_eq!(t.messages.first().unwrap().body, "m5", "oldest first, newest 30 kept");

        // The computer asks to send, and for older messages.
        let phone_fp = laptop.peers()[0].fingerprint;
        for m in [
            AppMessage::SmsSend { client_id: "c9".into(), thread: 3, address: "+919800004410".into(), body: "On my way".into() },
            AppMessage::SmsLoadOlder { thread: 3, before_ms: 1005 },
        ] {
            laptop.send(&phone_fp, m.encode().unwrap()).unwrap();
        }
        let ev = wait(&rx, |e| matches!(e, LinkEvent::SmsSendRequested { .. }));
        let LinkEvent::SmsSendRequested { id, client_id, address, body, .. } = ev else { unreachable!() };
        assert_eq!((client_id.as_str(), address.as_str(), body.as_str()), ("c9", "+919800004410", "On my way"));
        let ev = wait(&rx, |e| matches!(e, LinkEvent::SmsOlderRequested { .. }));
        assert!(matches!(ev, LinkEvent::SmsOlderRequested { thread: 3, before_ms: 1005, .. }));

        phone.sms_send_result(id.clone(), "c9".into(), false, "No service".into()).unwrap();
        phone.sms_older(id, 3, vec![sms(2, false, "m2", 1002), sms(1, true, "m1", 1001)], true).unwrap();
        let got: Vec<AppMessage> = rt.block_on(async {
            let mut v = vec![];
            while v.len() < 2 {
                if let Some(Event::Message { body, .. }) = lrx.recv().await {
                    v.push(AppMessage::decode(&body));
                }
            }
            v
        });
        assert!(matches!(&got[0], AppMessage::SmsSendResult { state: SmsState::Failed, message, .. } if message == "No service"));
        let AppMessage::SmsOlder { messages, has_older: true, .. } = &got[1] else { panic!("{:?}", got[1]) };
        assert_eq!(messages.iter().map(|m| m.id).collect::<Vec<_>>(), vec![1, 2], "oldest first");
        phone.shutdown();
        rt.block_on(laptop.shutdown());
    }

    /// The Bluetooth backup as the Android app runs it, with a socket pair standing in for
    /// the RFCOMM socket: the laptop's Bluetooth address arrives over Wi-Fi; with Wi-Fi gone,
    /// the session runs over the "socket"; when the session ends, the app's socket is closed.
    #[test]
    fn bluetooth_backup_call_sequence() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ldir = tempfile::tempdir().unwrap();
        let start_laptop = || {
            rt.block_on(Node::start(NodeConfig::new(
                "Kubuntu",
                Some(ldir.path().to_path_buf()),
                "127.0.0.1:0".parse().unwrap(),
            )))
            .unwrap()
        };
        let (laptop, _lrx) = start_laptop();
        laptop.set_bluetooth_address(Some("AA:BB:CC:DD:EE:FF".into()));
        let uri = laptop
            .start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()])
            .to_uri();
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let phone = PhoneLink::start(
            dir.path().to_string_lossy().into(),
            "Galaxy S24 FE".into(),
            Arc::new(Collect(Mutex::new(tx))),
            Arc::new(Paths),
        )
        .unwrap();
        phone.pair(uri).unwrap();
        let ev = wait(&rx, |e| matches!(e, LinkEvent::Connected { .. }));
        assert!(matches!(ev, LinkEvent::Connected { ref via, .. } if via == "wifi"));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while phone.devices()[0].bluetooth.is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(phone.devices()[0].bluetooth.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
        let id = phone.devices()[0].id.clone();

        // While on Wi-Fi, no Bluetooth session is started.
        struct Sock(Mutex<UnixStream>, Arc<std::sync::atomic::AtomicBool>);
        impl StreamSink for Sock {
            fn write(&self, data: Vec<u8>) -> bool {
                self.0.lock().unwrap().write_all(&data).is_ok()
            }
            fn close_link(&self) {
                self.1.store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = self.0.lock().unwrap().shutdown(std::net::Shutdown::Both);
            }
        }
        let (a, _b) = UnixStream::pair().unwrap();
        let closed0 = Arc::new(std::sync::atomic::AtomicBool::new(false));
        assert!(phone
            .open_stream(id.clone(), Arc::new(Sock(Mutex::new(a), closed0)))
            .is_err());

        // Wi-Fi gone: the laptop comes back somewhere the phone can't reach over the network.
        rt.block_on(laptop.shutdown());
        drop(laptop);
        wait(&rx, |e| matches!(e, LinkEvent::Disconnected { .. }));
        let (laptop, mut lrx) = start_laptop();

        // The app connected its Bluetooth socket (here: one end of a socket pair).
        let (phone_sock, laptop_sock) = UnixStream::pair().unwrap();
        let reader = phone_sock.try_clone().unwrap();
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pipe = phone
            .open_stream(id, Arc::new(Sock(Mutex::new(phone_sock), closed.clone())))
            .unwrap();
        // The app's reading thread.
        let pipe2 = pipe.clone();
        let reading = std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = vec![0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if !pipe2.received(buf[..n].to_vec()) {
                            break;
                        }
                    }
                }
            }
            pipe2.closed();
        });
        // The laptop's Bluetooth server.
        laptop_sock.set_nonblocking(true).unwrap();
        let l2 = laptop.clone();
        rt.spawn(async move {
            let s = tokio::net::UnixStream::from_std(laptop_sock).unwrap();
            let _ = l2.accept_stream(s).await;
        });
        let ev = wait(&rx, |e| matches!(e, LinkEvent::Connected { .. }));
        assert!(matches!(ev, LinkEvent::Connected { ref via, .. } if via == "bluetooth"), "{ev:?}");
        assert_eq!(phone.devices()[0].via, "bluetooth");
        assert!(phone.send_text(None, "over bluetooth".into()).unwrap());
        let got = rt.block_on(async {
            loop {
                if let Some(Event::Message { body, .. }) = lrx.recv().await {
                    return AppMessage::decode(&body);
                }
            }
        });
        assert_eq!(got, AppMessage::Text { text: "over bluetooth".into() });

        // The laptop goes away: the core closes the app's socket, the reading thread ends.
        rt.block_on(laptop.shutdown());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !closed.load(std::sync::atomic::Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(closed.load(std::sync::atomic::Ordering::SeqCst), "socket closed by the core");
        reading.join().unwrap();
        assert!(!pipe.received(vec![1, 2, 3]), "the pipe refuses data once closed");
        phone.shutdown();
    }
}
