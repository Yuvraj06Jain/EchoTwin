//! PhoneLink core: everything both the Linux daemon and the Android app share.
//!
//! * [`identity`]: per-device Ed25519 key + self-signed certificate, identified by its SHA-256 fingerprint.
//! * [`pairing`]: QR-code pairing; a one-time secret shown only on the laptop screen.
//! * [`tls`]: mutual TLS 1.3 over QUIC that only admits pinned fingerprints.
//! * [`node`]: endpoint, sessions, durable at-least-once message delivery, auto-reconnect.
//! * [`discovery`]: rotating opaque tokens so the LAN never learns who you are.
//! * [`limiter`]: handshake flood protection for hostile networks.

pub mod discovery;
pub mod error;
pub mod identity;
pub mod lan;
pub mod limiter;
pub mod message;
pub mod node;
pub mod outbox;
pub mod pairing;
pub mod probe;
pub mod tls;
pub mod transfer;
pub mod trust;
mod util;
pub mod wire;

pub use error::{Error, Result};
pub use identity::{Fingerprint, Identity};
pub use message::{
    AppMessage, MediaAction, NowPlaying, PhoneNotification, SmsMessage, SmsState, SmsThread,
    CAP_CLIPBOARD, CAP_LOCK, CAP_MEDIA, CAP_NOTIFICATIONS, CAP_SMS,
};
pub use node::{Event, Node, NodeConfig, Transport};
pub use pairing::PairingOffer;
pub use trust::PeerRecord;
pub use wire::{MsgId, PROTOCOL_VERSION};
