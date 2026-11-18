//! Local control protocol: one JSON object per line, over a Unix socket that only the
//! owning user can open.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use crate::media::MediaInfo;
pub use crate::sms::{SmsMessageView, SmsThreadView};
pub use phonelink_core::SmsState;
pub use crate::notifications::{AppInfo, NotificationInfo};
pub use phonelink_core::transfer::{Direction, TransferState};
pub use phonelink_core::MediaAction;

/// A file transfer, for the UI and CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferView {
    pub id: String,
    pub device: String,
    pub device_name: String,
    pub direction: Direction,
    pub name: String,
    pub size: u64,
    pub done: u64,
    pub state: TransferState,
    pub message: String,
    /// Received and saved: where.
    pub path: Option<String>,
    pub updated_ms: u64,
    /// Bytes per second while moving (0 otherwise).
    #[serde(default)]
    pub rate: u64,
    /// Seconds left at the current speed, when known.
    #[serde(default)]
    pub eta_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Status,
    Devices,
    /// Open the pairing window. Replies `PairStarted`, then `Paired` or `PairExpired`.
    /// Closing the connection cancels pairing.
    Pair {
        ttl_secs: u64,
    },
    Unpair {
        device: String,
    },
    /// `device` may be omitted when exactly one device is paired.
    SendText {
        device: Option<String>,
        text: String,
    },
    /// Stream `Event` replies until the connection is closed.
    Watch,
    /// Turn desktop notifications on or off (saved to config.toml).
    SetNotifications {
        enabled: bool,
    },
    /// Notifications mirrored from the phone (newest first) and the apps that sent them.
    Notifications,
    /// App icons as `data:` URLs, by package.
    AppIcons,
    /// Remove one from this computer (and from the phone if "dismiss on both" is on).
    DismissNotification {
        id: String,
    },
    ClearNotifications,
    ReplyNotification {
        id: String,
        text: String,
    },
    NotificationAction {
        id: String,
        index: u32,
    },
    SetSyncDismissals {
        enabled: bool,
    },
    SetAppMuted {
        package: String,
        muted: bool,
    },
    /// Try to reach every paired device right now (skip the back-off, search the network).
    Reconnect,
    SetClipboardSync {
        enabled: bool,
    },
    SetAllowLock {
        enabled: bool,
    },
    /// What's playing on the connected phones.
    Media,
    /// Control the phone's player. `device` may be omitted with one phone connected.
    MediaControl {
        device: Option<String>,
        action: MediaAction,
    },
    /// File transfers, newest first.
    Transfers,
    /// Send files (absolute paths) to the phone. `notify`: show a popup (e.g. when sent
    /// from the file manager, where nothing else shows that it worked).
    SendFiles {
        device: Option<String>,
        paths: Vec<String>,
        #[serde(default)]
        notify: bool,
    },
    CancelTransfer {
        id: String,
    },
    /// Remove finished transfers from the list (the files stay).
    ClearTransfers,
    /// Open a received file, or (`reveal`) show it in the file manager.
    OpenTransfer {
        id: String,
        #[serde(default)]
        reveal: bool,
    },
    /// SMS conversations from the phone(s), most recent first.
    SmsThreads,
    /// Send an SMS in a conversation (`thread` from `SmsThreads`).
    SendSms {
        thread: String,
        text: String,
    },
    /// Start a new conversation: send `text` to `address` (a phone number). Replies
    /// `SmsQueued`; the result arrives as an `sms_send_result` event.
    SendSmsTo {
        device: Option<String>,
        address: String,
        text: String,
    },
    /// Search the phone's contacts (names and numbers; nothing is kept here). At most 20.
    Contacts {
        query: String,
    },
    /// Mark a conversation read on this computer.
    SmsMarkRead {
        thread: String,
    },
    /// Ask the phone for older messages of a conversation.
    SmsLoadOlder {
        thread: String,
    },
    SetBluetoothBackup {
        enabled: bool,
    },
    /// The connection switch (saved). Off: disconnect and stop searching.
    SetConnection {
        on: bool,
    },
    /// The battery saver switch (saved): slower keep-alive and searching.
    SetBatterySaver {
        on: bool,
    },
    /// Where received files go: an absolute folder (made if missing; must be writable), or
    /// `None` for the default (`~/Downloads/EchoTwin`). Saved in config.toml.
    SetDownloadDir {
        path: Option<String>,
    },
    /// Turn the battery saver on by itself at or below this battery level (0 = never, ≤ 50).
    SetAutoSaver {
        below: u8,
    },
    /// One of the simple on/off settings (`config::SWITCHES`), saved to config.toml.
    SetSwitch {
        name: String,
        on: bool,
    },
    /// Open a web link on the phone (as a tap-to-open notification there). Queued while
    /// the phone is away. Replies `Sent`.
    OpenUrl {
        device: Option<String>,
        url: String,
    },
    /// Your own buttons for the phone's Remote screen (names and commands).
    Commands,
    /// Replace them (validated, saved); connected phones get the new names.
    SetCommands {
        items: Vec<crate::config::CommandEntry>,
    },
    /// Decline the call ringing on the phone. Replies `Done` once asked; the phone's
    /// answer arrives as an `action_result` event.
    DeclineCall {
        device: Option<String>,
    },
    /// Press a button of the phone's call: "answer", "decline", "end", "mute", "speaker" or
    /// "hold" (only one the call offers right now). Replies `Done` once asked.
    CallAction {
        device: Option<String>,
        action: String,
    },
    /// The app can't show the pointer (no overlay on this desktop): tell the phone once.
    PointerUnavailable {
        reason: String,
    },
    /// The app shows the call card for phone `id`: no KDE notification needed.
    CallPopupShown {
        id: String,
    },
    /// The same as pressing button `key` on our popup `id` (e.g. "note-copy").
    PopupAction {
        id: String,
        key: String,
    },
    /// Ring the connected phone loudly (even on silent), or stop it.
    Ring {
        device: Option<String>,
        on: bool,
    },
    /// Put a notification's one-time code on this computer's clipboard.
    CopyCode {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Short human-comparable id, e.g. `3F2A-91C0-77DE-0B14`.
    pub id: String,
    pub fingerprint: String,
    pub name: String,
    pub connected: bool,
    /// Messages waiting for this device to come back.
    pub pending: usize,
    pub last_addrs: Vec<String>,
    /// Features the connected phone supports (empty while offline).
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// "wifi" or "bluetooth" while connected.
    #[serde(default)]
    pub via: Option<String>,
    /// The phone's battery and network while connected (if it reports them).
    #[serde(default)]
    pub status: Option<phonelink_core::PhoneStatus>,
    /// The phone switched its connection off (it reconnects when switched back on).
    #[serde(default)]
    pub paused_by_peer: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WatchEvent {
    /// A call on the phone: `phase` is "ringing", "active" or "idle" (ended).
    /// Where the phone's presentation pointer is (up to 30 a second; the app's overlay draws
    /// it). `x`, `y`: 0..=10000 across the screen; `shape`: "dot", "ring", "spotlight";
    /// `color`: 0xRRGGBB; `size`: 0..=2.
    Pointer {
        x: u16,
        y: u16,
        visible: bool,
        shape: String,
        color: u32,
        size: u8,
    },
    CallChanged {
        id: String,
        name: String,
        phase: String,
        caller: String,
        /// Buttons the phone's call notification offers: "answer", "decline", "end",
        /// "mute", "speaker", "hold".
        #[serde(default)]
        actions: Vec<String>,
        /// This computer's clock when the call became active (for the timer).
        #[serde(default)]
        active_ms: Option<u64>,
        /// Show the card at the top of the screen (ringing, and `call_popup` on).
        #[serde(default)]
        card: bool,
    },
    Paired {
        id: String,
        name: String,
    },
    /// Also sent when the link changes (Bluetooth backup ↔ Wi-Fi).
    Connected {
        id: String,
        name: String,
        #[serde(default)]
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
    NotificationPosted {
        item: NotificationInfo,
    },
    /// `deleted`: gone from the list; otherwise only marked as cleared on the phone.
    NotificationRemoved {
        id: String,
        deleted: bool,
    },
    /// Many changes at once (snapshot, clear all, mute): fetch the list again.
    NotificationsChanged,
    /// Outcome of a reply / action / dismissal on the phone.
    ActionResult {
        id: String,
        ok: bool,
        message: String,
    },
    AppIcon {
        package: String,
    },
    /// The phone sent its clipboard; it is now on this computer's clipboard.
    ClipboardReceived {
        id: String,
        name: String,
    },
    /// The phone locked this computer.
    LockedByPhone {
        id: String,
        name: String,
    },
    /// What's playing on this phone changed (or it stopped / disconnected): ask `Media`.
    MediaChanged {
        id: String,
    },
    /// The phone's battery or network changed (see `DeviceInfo::status`).
    PhoneStatusChanged {
        id: String,
    },
    /// This computer's battery level or charging changed (ask `Status`).
    ComputerBatteryChanged,
    /// The battery saver went on or off; `auto`: by itself, because the battery is low.
    SaverChanged {
        active: bool,
        auto: bool,
    },
    /// The connection switch on this computer changed.
    ConnectionChanged {
        on: bool,
    },
    /// The phone switched its connection off.
    PeerPaused {
        id: String,
        name: String,
    },
    TransferChanged {
        item: TransferView,
    },
    /// Conversations changed (`thread`: just that one): ask `SmsThreads`.
    SmsChanged {
        thread: Option<String>,
    },
    /// What happened to an SMS sent from here.
    SmsSendResult {
        client_id: String,
        state: SmsState,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Status {
        name: String,
        id: String,
        fingerprint: String,
        port: u16,
        devices: Vec<DeviceInfo>,
        /// Whether desktop notifications are on (absent from older daemons).
        #[serde(default)]
        notifications: bool,
        #[serde(default)]
        sync_dismissals: bool,
        #[serde(default)]
        clipboard_sync: bool,
        #[serde(default)]
        allow_lock: bool,
        #[serde(default)]
        bluetooth_backup: bool,
        /// Empty when the backup is ready; otherwise why not ("off", no adapter, …).
        #[serde(default)]
        bluetooth_status: String,
        /// The connection switch (absent from older daemons = on).
        #[serde(default = "yes")]
        connection: bool,
        /// The battery saver switch (absent from older daemons = on).
        #[serde(default = "yes")]
        battery_saver: bool,
        /// The simple on/off settings by name (`config::SWITCHES`).
        #[serde(default)]
        switches: BTreeMap<String, bool>,
        /// The level at which the saver turns on by itself (0 = never).
        #[serde(default)]
        auto_saver_below: u8,
        /// The saver is on right now: switched on, or on by itself because the battery is low.
        #[serde(default)]
        saver_active: bool,
        /// This computer's battery (0–100), if it has one.
        #[serde(default)]
        battery: Option<u8>,
        #[serde(default)]
        charging: bool,
        /// The call on the phone, if there is one.
        #[serde(default)]
        call: Option<CallView>,
        /// Where received files go (absolute).
        #[serde(default)]
        download_dir: String,
    },
    Notifications {
        items: Vec<NotificationInfo>,
        apps: Vec<AppInfo>,
    },
    Commands {
        items: Vec<crate::config::CommandEntry>,
    },
    Contacts {
        items: Vec<phonelink_core::Contact>,
    },
    AppIcons {
        icons: BTreeMap<String, String>,
    },
    Media {
        items: Vec<MediaInfo>,
    },
    Transfers {
        items: Vec<TransferView>,
    },
    FilesQueued {
        ids: Vec<String>,
    },
    SmsThreads {
        threads: Vec<SmsThreadView>,
    },
    SmsQueued {
        client_id: String,
    },
    Devices {
        devices: Vec<DeviceInfo>,
    },
    PairStarted {
        uri: String,
        id: String,
        addrs: Vec<String>,
        expires_in_secs: u64,
    },
    Paired {
        device: DeviceInfo,
    },
    PairExpired,
    Sent {
        message_id: String,
        delivered_now: bool,
    },
    Done,
    Event {
        event: WatchEvent,
    },
    Error {
        message: String,
    },
}

fn yes() -> bool {
    true
}

/// A call on the phone, as the window and the widget show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallView {
    pub id: String,
    pub name: String,
    /// "ringing" or "active".
    pub phase: String,
    pub caller: String,
    pub actions: Vec<String>,
    pub active_ms: Option<u64>,
}
