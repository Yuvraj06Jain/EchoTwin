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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WatchEvent {
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
    },
    Notifications {
        items: Vec<NotificationInfo>,
        apps: Vec<AppInfo>,
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
