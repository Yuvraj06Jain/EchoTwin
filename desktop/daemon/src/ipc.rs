//! Local control protocol: one JSON object per line, over a Unix socket that only the
//! owning user can open.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use crate::notifications::{AppInfo, NotificationInfo};

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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WatchEvent {
    Paired {
        id: String,
        name: String,
    },
    Connected {
        id: String,
        name: String,
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
    },
    Notifications {
        items: Vec<NotificationInfo>,
        apps: Vec<AppInfo>,
    },
    AppIcons {
        icons: BTreeMap<String, String>,
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
