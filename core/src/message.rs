//! Application-level messages carried in the durable `Deliver` channel.
//!
//! Same compatibility rules as `wire.rs`: variants and fields are append-only, and an
//! older build turns unknown variants into [`AppMessage::Unsupported`] instead of failing.
//! Never reorder or remove variants: postcard identifies them by position.

use serde::{Deserialize, Serialize};

use crate::Result;

/// Capability string for notification mirroring (phone → computer).
pub const CAP_NOTIFICATIONS: &str = "notifications/1";

/// Lock the computer from the phone (phone → computer).
pub const CAP_LOCK: &str = "lock/1";
/// Clipboard text in both directions.
pub const CAP_CLIPBOARD: &str = "clipboard/1";

/// What is playing on the phone, and controlling it from the computer.
pub const CAP_MEDIA: &str = "media/1";
/// Largest album-art image sent (JPEG or PNG, bytes).
pub const MAX_ART_BYTES: usize = 64 * 1024;

/// Longest clipboard text sent (bytes). Longer copies are not sent at all: a cut-off
/// paste would be worse than none.
pub const MAX_CLIPBOARD_TEXT: usize = 64 * 1024;
/// A lock request older than this (it waited in a queue) is ignored: locking long after the
/// tap would be a surprise. Generous for clock differences between the two devices.
pub const LOCK_MAX_AGE_MS: u64 = 2 * 60 * 1000;
/// Clipboard text older than this is not applied (it would overwrite something newer).
pub const CLIPBOARD_MAX_AGE_MS: u64 = 2 * 60 * 1000;

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

/// The phone's current media session, as shown on the computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NowPlaying {
    /// Player app name as the user sees it, e.g. `Spotify`.
    pub app: String,
    pub package: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// 0 when unknown (live streams).
    pub duration_ms: u64,
    /// Position when this was sent; the computer advances it itself while `playing`.
    pub position_ms: u64,
    pub playing: bool,
    pub can_previous: bool,
    pub can_next: bool,
    /// Media volume on the phone: `0..=max_volume`.
    pub volume: u32,
    pub max_volume: u32,
    /// Identifies the album art sent in `MediaArt`; empty when there is none.
    pub art_key: String,
}

/// A button pressed on the computer for the phone's player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaAction {
    PlayPause,
    Next,
    Previous,
    VolumeUp,
    VolumeDown,
    /// Absolute media volume, `0..=max_volume`.
    SetVolume(u32),
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

    /// Result of a request above, phone → computer. Also computer → phone for
    /// `LockComputer` (`key` = [`LOCK_RESULT_KEY`]).
    ActionResult {
        key: String,
        ok: bool,
        message: String,
    },

    // ---- lock/1 (phone → computer)
    /// Lock the computer's screen now. `sent_ms`: phone clock, to drop stale requests.
    LockComputer {
        sent_ms: u64,
    },

    // ---- clipboard/1 (both ways)
    /// Text copied on the sending device. `copied_ms`: sender's clock.
    Clipboard {
        text: String,
        copied_ms: u64,
    },

    // ---- media/1
    /// Phone → computer: what's playing now (`None`: nothing). Sent on every change and
    /// when a computer connects.
    NowPlaying(Option<NowPlaying>),
    /// Phone → computer: album art for `NowPlaying::art_key` (JPEG or PNG).
    MediaArt {
        key: String,
        image: Vec<u8>,
    },
    /// Computer → phone: control the player / media volume.
    MediaControl {
        action: MediaAction,
    },
}

/// Album art must be a small JPEG or PNG.
pub fn is_art_image(data: &[u8]) -> bool {
    data.len() <= MAX_ART_BYTES
        && (data.starts_with(&[0xFF, 0xD8, 0xFF]) || data.starts_with(b"\x89PNG\r\n\x1a\n"))
}

/// `ActionResult::key` for the answer to `LockComputer`.
pub const LOCK_RESULT_KEY: &str = "lock";

/// Is a timestamp from the other device recent enough (`max_age`), allowing for clocks
/// that differ by up to `max_age` in either direction?
pub fn is_fresh(sent_ms: u64, now_ms: u64, max_age: u64) -> bool {
    sent_ms.abs_diff(now_ms) <= max_age
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
        // Appended after ActionResult (9), never moved.
        assert_eq!(AppMessage::LockComputer { sent_ms: 1 }.encode().unwrap()[0], 10);
        assert_eq!(
            AppMessage::Clipboard {
                text: String::new(),
                copied_ms: 0
            }
            .encode()
            .unwrap()[0],
            11
        );
    }

    #[test]
    fn media_messages_roundtrip_and_keep_their_positions() {
        let now = NowPlaying {
            app: "Spotify".into(),
            package: "com.spotify.music".into(),
            title: "Kesariya".into(),
            artist: "Arijit Singh".into(),
            album: "Brahmastra".into(),
            duration_ms: 268_000,
            position_ms: 61_500,
            playing: true,
            can_previous: true,
            can_next: true,
            volume: 9,
            max_volume: 15,
            art_key: "a1".into(),
        };
        let all = [
            AppMessage::NowPlaying(Some(now)),
            AppMessage::NowPlaying(None),
            AppMessage::MediaArt {
                key: "a1".into(),
                image: vec![0xFF, 0xD8, 0xFF, 0xE0],
            },
            AppMessage::MediaControl {
                action: MediaAction::SetVolume(7),
            },
            AppMessage::MediaControl {
                action: MediaAction::PlayPause,
            },
        ];
        for m in all {
            assert_eq!(AppMessage::decode(&m.encode().unwrap()), m);
        }
        assert_eq!(AppMessage::NowPlaying(None).encode().unwrap()[0], 12);
        assert!(is_art_image(&[0xFF, 0xD8, 0xFF, 0xE0, 1, 2]));
        assert!(is_art_image(b"\x89PNG\r\n\x1a\n...."));
        assert!(!is_art_image(b"<svg onload=alert(1)>"));
        assert!(!is_art_image(&[0xFF; MAX_ART_BYTES + 1]));
        // The JSON form used by the computer's control socket.
        assert_eq!(serde_json::to_string(&MediaAction::PlayPause).unwrap(), "\"play_pause\"");
        assert_eq!(serde_json::to_string(&MediaAction::SetVolume(4)).unwrap(), "{\"set_volume\":4}");
    }

    #[test]
    fn lock_and_clipboard_roundtrip() {
        for m in [
            AppMessage::LockComputer {
                sent_ms: 1_762_350_000_000,
            },
            AppMessage::Clipboard {
                text: "copied ✓\nline 2".into(),
                copied_ms: 1_762_350_000_000,
            },
        ] {
            assert_eq!(AppMessage::decode(&m.encode().unwrap()), m);
        }
    }

    #[test]
    fn freshness_allows_small_clock_differences_both_ways() {
        let now = 1_000_000;
        assert!(is_fresh(now, now, LOCK_MAX_AGE_MS));
        assert!(is_fresh(now - 60_000, now, LOCK_MAX_AGE_MS));
        assert!(is_fresh(now + 60_000, now, LOCK_MAX_AGE_MS), "phone clock slightly ahead");
        assert!(!is_fresh(now - 10 * 60_000, now, LOCK_MAX_AGE_MS), "queued for 10 min");
        assert!(!is_fresh(0, now, LOCK_MAX_AGE_MS));
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
