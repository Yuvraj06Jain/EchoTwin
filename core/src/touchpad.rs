//! The touchpad protocol: the phone's Touchpad tab driving the laptop's
//! `phonelink-touchpad` service, on a [`crate::Node`] session of its own (its own identity,
//! pairing and port, never the main EchoTwin session).
//!
//! The phone does not interpret gestures. It streams the raw fingers it sees, and the laptop
//! replays them on a virtual touchpad, so the laptop's own touchpad stack (libinput, KWin,
//! InputActions) decides what a tap, drag, scroll or 3-finger swipe means.
//!
//! * [`Msg`] travels on the control stream with `Node::send_fleeting`: in order while
//!   connected, never queued (a button press must not replay after a reconnect).
//! * [`TouchFrame`] travels as an unreliable QUIC datagram (`Node::send_datagram`). Every
//!   frame is a full snapshot of the fingers that are down, so a lost frame is corrected by
//!   the next one; the receiver ignores frames older than the newest it applied.
//!
//! Compatibility: variants and fields are only ever appended.
//!
//! Ported from the TouchTwin project (`touchtwin-core/src/proto.rs`); [`Msg::Button`] and
//! the touchpad pairing code ([`to_pair_uri`], [`offer_from_pair_uri`]) are new.

use serde::{Deserialize, Serialize};

/// Both sides support the touchpad protocol below.
pub const CAP_TOUCHPAD: &str = "touchpad/1";

/// Most fingers a frame may carry (libinput understands up to five).
pub const MAX_CONTACTS: usize = 5;

/// Touch coordinates run from 0 to this value on both axes; [`PadInfo`] gives the
/// physical size, so the laptop knows the aspect ratio and resolution.
pub const COORD_MAX: u16 = 10_000;

/// Reliable messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Msg {
    /// Phone → laptop, at the start of every session and whenever the pad changes size
    /// (rotation, split screen). The laptop (re)creates its virtual touchpad to match.
    PadInfo(PadInfo),
    /// Phone → laptop: the pad screen was closed; lift every finger now.
    Release,
    /// Laptop → phone: the virtual touchpad is ready (`ok`) or could not be created.
    PadStatus { ok: bool, message: String },
    /// Phone → laptop: the Left click / Right click button went down or up. While a button
    /// is held the phone keeps sending touch frames (empty ones too), so the laptop's
    /// watchdog can tell "held" from "connection lost".
    Button { button: Button, down: bool },
}

/// The pad's two physical buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Button {
    Left,
    Right,
}

/// What the touchpad service's QR code starts with. Not the main `phonelink:pair?`, so a
/// touchpad code scanned in EchoTwin's main pairing (or the reverse) is refused.
pub const PAIR_URI_PREFIX: &str = "phonelink:touchpad?";

/// The touchpad pairing code for `offer`.
pub fn to_pair_uri(offer: &crate::PairingOffer) -> String {
    let uri = offer.to_uri();
    let query = uri.split_once('?').map(|(_, q)| q).unwrap_or_default();
    format!("{PAIR_URI_PREFIX}{query}")
}

/// Read a touchpad pairing code (from [`to_pair_uri`]).
pub fn offer_from_pair_uri(uri: &str) -> crate::Result<crate::PairingOffer> {
    let uri = uri.trim();
    let Some(query) = uri.strip_prefix(PAIR_URI_PREFIX) else {
        return Err(crate::Error::BadOffer(if uri.starts_with("phonelink:pair?") {
            "this is EchoTwin's main pairing code; for the touchpad run `phonelink touchpad pair` on the laptop".into()
        } else {
            "not an EchoTwin touchpad code".into()
        }));
    };
    crate::PairingOffer::from_uri(&format!("phonelink:pair?{query}"))
}

/// Longest [`Msg::PadStatus`] message, so the whole `Msg` fits one fleeting frame.
pub const MAX_STATUS_LEN: usize = 200;

impl Msg {
    /// A `PadStatus`, its message cut to [`MAX_STATUS_LEN`] bytes on a char boundary.
    pub fn status(ok: bool, message: &str) -> Self {
        let mut end = message.len().min(MAX_STATUS_LEN);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        Msg::PadStatus { ok, message: message[..end].to_string() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PadInfo {
    /// Physical size of the touch area, in tenths of a millimetre.
    pub width_dmm: u16,
    pub height_dmm: u16,
}

impl PadInfo {
    /// Sizes outside 2 cm – 40 cm are not a phone screen; clamp them.
    pub fn sanitized(self) -> Self {
        Self {
            width_dmm: self.width_dmm.clamp(200, 4000),
            height_dmm: self.height_dmm.clamp(200, 4000),
        }
    }
}

/// One finger. `slot` is stable while the finger is down (0..MAX_CONTACTS); `id` is new
/// for every touch, so lifting and putting a finger back down is seen as a new touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub slot: u8,
    pub id: u16,
    pub x: u16,
    pub y: u16,
}

/// Snapshot of all fingers on the pad.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TouchFrame {
    /// Increases by one per frame (wrapping), within one session.
    pub seq: u32,
    pub contacts: Vec<Contact>,
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("Msg always serialises")
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        postcard::from_bytes(bytes).ok()
    }
}

impl TouchFrame {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("TouchFrame always serialises")
    }

    /// Decodes and validates: too many fingers, slots out of range, duplicate slots or
    /// coordinates beyond [`COORD_MAX`] are refused (the peer is paired, but be strict).
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let f: TouchFrame = postcard::from_bytes(bytes).ok()?;
        if f.contacts.len() > MAX_CONTACTS {
            return None;
        }
        let mut seen = 0u8;
        for c in &f.contacts {
            if c.slot as usize >= MAX_CONTACTS || c.x > COORD_MAX || c.y > COORD_MAX {
                return None;
            }
            if seen & (1 << c.slot) != 0 {
                return None;
            }
            seen |= 1 << c.slot;
        }
        Some(f)
    }
}

/// `a` is newer than `b` in wrapping sequence order.
pub fn seq_newer(a: u32, b: u32) -> bool {
    let d = a.wrapping_sub(b);
    d != 0 && d < u32::MAX / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(slot: u8, x: u16) -> Contact {
        Contact { slot, id: slot as u16 + 1, x, y: 5 }
    }

    #[test]
    fn frames_roundtrip_and_stay_small() {
        let f = TouchFrame {
            seq: 123_456,
            contacts: (0..5).map(|s| c(s, 9_999)).collect(),
        };
        let b = f.encode();
        assert!(b.len() < 64, "five fingers fit easily in a datagram: {}", b.len());
        assert_eq!(TouchFrame::decode(&b), Some(f));
    }

    #[test]
    fn bad_frames_are_refused() {
        let six = TouchFrame { seq: 1, contacts: (0..6).map(|s| c(s % 5, 1)).collect() };
        assert_eq!(TouchFrame::decode(&six.encode()), None);
        let dup = TouchFrame { seq: 1, contacts: vec![c(1, 1), c(1, 2)] };
        assert_eq!(TouchFrame::decode(&dup.encode()), None);
        let far = TouchFrame { seq: 1, contacts: vec![c(0, COORD_MAX + 1)] };
        assert_eq!(TouchFrame::decode(&far.encode()), None);
        let slot = TouchFrame { seq: 1, contacts: vec![c(5, 1)] };
        assert_eq!(TouchFrame::decode(&slot.encode()), None);
        assert_eq!(TouchFrame::decode(b"\xff\xff"), None);
    }

    #[test]
    fn sequence_order_wraps() {
        assert!(seq_newer(2, 1));
        assert!(!seq_newer(1, 2));
        assert!(!seq_newer(7, 7));
        assert!(seq_newer(0, u32::MAX));
        assert!(seq_newer(3, u32::MAX - 3));
    }

    #[test]
    fn msgs_roundtrip() {
        for m in [
            Msg::PadInfo(PadInfo { width_dmm: 700, height_dmm: 1500 }),
            Msg::Release,
            Msg::PadStatus { ok: false, message: "no uinput".into() },
            Msg::Button { button: Button::Left, down: true },
            Msg::Button { button: Button::Right, down: false },
        ] {
            assert_eq!(Msg::decode(&m.encode()), Some(m));
        }
    }

    #[test]
    fn button_is_appended_last() {
        // postcard writes the variant index first: existing variants keep theirs.
        assert_eq!(Msg::PadInfo(PadInfo { width_dmm: 1, height_dmm: 1 }).encode()[0], 0);
        assert_eq!(Msg::Release.encode()[0], 1);
        assert_eq!(Msg::status(true, "").encode()[0], 2);
        assert_eq!(Msg::Button { button: Button::Left, down: true }.encode(), [3, 0, 1]);
        assert_eq!(Msg::Button { button: Button::Right, down: false }.encode(), [3, 1, 0]);
    }

    #[test]
    fn a_long_status_fits_one_fleeting_frame() {
        let m = Msg::status(false, &"é".repeat(400));
        let Msg::PadStatus { message, .. } = &m else { unreachable!() };
        assert!(message.len() <= MAX_STATUS_LEN && message.chars().all(|c| c == 'é'));
        assert!(m.encode().len() <= crate::wire::MAX_FLEETING_LEN);
    }

    #[test]
    fn touchpad_and_main_pairing_codes_never_mix() {
        let offer = crate::PairingOffer {
            fingerprint: crate::Fingerprint([3; 32]),
            secret: [7; 32],
            addrs: vec!["192.168.1.5:47830".parse().unwrap()],
            name: "Laptop".into(),
        };
        let tp = to_pair_uri(&offer);
        assert!(tp.starts_with(PAIR_URI_PREFIX));
        let back = offer_from_pair_uri(&tp).unwrap();
        assert_eq!((back.fingerprint, back.secret, back.addrs.clone()), (offer.fingerprint, offer.secret, offer.addrs.clone()));
        // A main code in the Touchpad tab, and a touchpad code in the main pairing: refused.
        let e = offer_from_pair_uri(&offer.to_uri()).unwrap_err().to_string();
        assert!(e.contains("phonelink touchpad pair"), "{e}");
        let e = crate::PairingOffer::from_uri(&tp).unwrap_err().to_string();
        assert!(e.contains("Touchpad tab"), "{e}");
        assert!(offer_from_pair_uri("hello").is_err());
    }
}
