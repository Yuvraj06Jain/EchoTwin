//! Application-level messages carried in the durable `Deliver` channel.
//!
//! Same compatibility rules as `wire.rs`: variants and fields are append-only, and an
//! older build turns unknown variants into [`AppMessage::Unsupported`] instead of failing.
//! Never reorder or remove variants: postcard identifies them by position.

use serde::{Deserialize, Serialize};

use crate::Result;

/// Capability string for notification mirroring (phone → computer).
pub const CAP_NOTIFICATIONS: &str = "notifications/1";

/// Longest notification text sent over the wire; anything longer is cut on the phone.
pub const MAX_NOTIFICATION_TEXT: usize = 2000;
/// Most notifications in one snapshot.
pub const MAX_SNAPSHOT_ITEMS: usize = 100;

/// A notification shown on the phone, as mirrored to the computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneNotification {
    /// Android's notification key: stable across updates of the same notification.
    pub key: String,
    /// App package, e.g. `com.whatsapp`.
    pub package: String,
    /// App name as the user sees it, e.g. `WhatsApp`.
    pub app: String,
    pub title: String,
    pub text: String,
    /// When the phone posted it (Unix milliseconds, phone clock).
    pub posted_ms: u64,
    /// The app offers an inline Reply action.
    pub can_reply: bool,
    /// Titles of the other action buttons, in order (index = position here).
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppMessage {
    /// A plain text note (also used as a connectivity test).
    Text {
        text: String,
    },
    /// Placeholder for whatever a newer version sent that we don't understand.
    /// Never sent on the wire by this version.
    Unsupported,

    // ---- notification mirroring (phone → computer), added in protocol 1 / notifications/1
    /// New or updated notification (same `key` replaces the earlier one).
    NotificationPosted(PhoneNotification),
    /// The notification is gone from the phone.
    NotificationRemoved {
        key: String,
    },
    /// Everything currently shown on the phone; sent on every (re)connect.
    NotificationSnapshot {
        items: Vec<PhoneNotification>,
    },
    /// App icon as a small PNG, sent once per app.
    AppIcon {
        package: String,
        png: Vec<u8>,
    },

    // ---- requests computer → phone (only ever about notifications the phone sent)
    DismissNotification {
        key: String,
    },
    ReplyNotification {
        key: String,
        text: String,
    },
    /// Press the action button at `index` of `PhoneNotification::actions`.
    NotificationAction {
        key: String,
        index: u32,
    },

    /// Result of a request above, phone → computer.
    ActionResult {
        key: String,
        ok: bool,
        message: String,
    },
}

impl AppMessage {
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(postcard::to_stdvec(self)?)
    }

    pub fn decode(bytes: &[u8]) -> Self {
        postcard::from_bytes(bytes).unwrap_or(AppMessage::Unsupported)
    }
}

/// Cut `s` to at most `max` bytes on a character boundary, marking the cut with "…".
pub fn truncate_text(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max.saturating_sub('…'.len_utf8());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PhoneNotification {
        PhoneNotification {
            key: "0|com.whatsapp|1|null|10123".into(),
            package: "com.whatsapp".into(),
            app: "WhatsApp".into(),
            title: "Rahul".into(),
            text: "Are you coming to the lab? 🙂".into(),
            posted_ms: 1_762_350_000_000,
            can_reply: true,
            actions: vec!["Mark as read".into()],
        }
    }

    #[test]
    fn roundtrip_and_forward_compat() {
        let m = AppMessage::Text { text: "hi".into() };
        assert_eq!(AppMessage::decode(&m.encode().unwrap()), m);
        assert_eq!(AppMessage::decode(&[250, 1, 2]), AppMessage::Unsupported);
    }

    #[test]
    fn every_notification_message_roundtrips() {
        let all = vec![
            AppMessage::NotificationPosted(sample()),
            AppMessage::NotificationRemoved { key: "k".into() },
            AppMessage::NotificationSnapshot {
                items: vec![sample(), sample()],
            },
            AppMessage::AppIcon {
                package: "com.whatsapp".into(),
                png: vec![0x89, b'P', b'N', b'G'],
            },
            AppMessage::DismissNotification { key: "k".into() },
            AppMessage::ReplyNotification {
                key: "k".into(),
                text: "On my way".into(),
            },
            AppMessage::NotificationAction {
                key: "k".into(),
                index: 0,
            },
            AppMessage::ActionResult {
                key: "k".into(),
                ok: false,
                message: "Reply not available".into(),
            },
        ];
        for m in all {
            assert_eq!(AppMessage::decode(&m.encode().unwrap()), m);
        }
    }

    #[test]
    fn existing_variants_keep_their_wire_positions() {
        // Older builds identify Text as variant 0: it must never move.
        assert_eq!(
            AppMessage::Text {
                text: String::new()
            }
            .encode()
            .unwrap()[0],
            0
        );
        assert_eq!(
            AppMessage::NotificationPosted(sample()).encode().unwrap()[0],
            2
        );
    }

    #[test]
    fn truncation_respects_characters() {
        assert_eq!(truncate_text("short", 10), "short");
        let t = truncate_text("ééééééé", 7);
        assert!(t.len() <= 7 && t.ends_with('…'), "{t}");
        assert_eq!(
            truncate_text(&"x".repeat(5000), MAX_NOTIFICATION_TEXT).len(),
            MAX_NOTIFICATION_TEXT
        );
    }
}
