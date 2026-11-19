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
    /// Phone → laptop: the scroll strip moved, in hi-res wheel units (120 = one notch,
    /// positive = up / away from you). At most [`MAX_SCROLL`] either way per message.
    Scroll { v120: i32 },
    /// Laptop → phone: the laptop's own touchpad is in use (or no longer), so the phone's
    /// input is ignored meanwhile.
    LaptopTouchpad { in_use: bool },
    /// Phone → laptop: type these characters, as if on the laptop's keyboard. Only
    /// characters [`keymap::us`] can type; at most [`MAX_TEXT`] bytes.
    Text { text: String },
    /// Phone → laptop: one key with modifiers ([`mods`] bits), pressed and released.
    Key { key: Key, mods: u8 },
    /// Laptop → phone: typing is switched off on the laptop.
    KeyboardOff,
}

/// Longest [`Msg::Text`] in bytes (longer text goes in several messages).
pub const MAX_TEXT: usize = 200;

/// Modifier bits for [`Msg::Key`].
pub mod mods {
    pub const CTRL: u8 = 1;
    pub const ALT: u8 = 2;
    pub const SHIFT: u8 = 4;
    pub const SUPER: u8 = 8;
}

/// Keys the phone can press. Append-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Key {
    Enter,
    Backspace,
    Tab,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Delete,
    PageUp,
    PageDown,
    /// A character key (with Ctrl: shortcuts like Ctrl+C). Must be typeable on the US layout.
    Char(char),
}

/// Largest scroll step in one [`Msg::Scroll`] (ten notches); more is clamped.
pub const MAX_SCROLL: i32 = 1200;

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
    /// `text` as `Text` messages of at most [`MAX_TEXT`] bytes each (split on char boundaries).
    pub fn texts(text: &str) -> Vec<Self> {
        let mut out = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let mut end = rest.len().min(MAX_TEXT);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            out.push(Msg::Text { text: rest[..end].to_string() });
            rest = &rest[end..];
        }
        out
    }

    /// A `Scroll`, clamped to ±[`MAX_SCROLL`].
    pub fn scroll(v120: i32) -> Self {
        Msg::Scroll { v120: v120.clamp(-MAX_SCROLL, MAX_SCROLL) }
    }

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
            Msg::Scroll { v120: -360 },
            Msg::LaptopTouchpad { in_use: true },
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
        assert_eq!(Msg::scroll(1).encode()[0], 4);
        assert_eq!(Msg::LaptopTouchpad { in_use: false }.encode(), [5, 0]);
    }

    #[test]
    fn keyboard_messages_are_appended_after_scroll() {
        assert_eq!(Msg::Text { text: "a".into() }.encode()[0], 6);
        assert_eq!(Msg::Key { key: Key::Enter, mods: 0 }.encode(), [7, 0, 0]);
        assert_eq!(Msg::KeyboardOff.encode(), [8]);
        for m in [Msg::Key { key: Key::Char('c'), mods: mods::CTRL }, Msg::Text { text: "Hi!".into() }] {
            assert_eq!(Msg::decode(&m.encode()), Some(m));
        }
    }

    #[test]
    fn long_text_is_split_on_char_boundaries() {
        let text = "é".repeat(150) + "abc";
        let parts = Msg::texts(&text);
        assert_eq!(parts.len(), 2);
        let mut joined = String::new();
        for p in &parts {
            let Msg::Text { text } = p else { unreachable!() };
            assert!(text.len() <= MAX_TEXT);
            joined.push_str(text);
            assert!(p.encode().len() <= crate::wire::MAX_FLEETING_LEN);
        }
        assert_eq!(joined, text);
        assert!(Msg::texts("").is_empty());
    }

    #[test]
    fn scroll_is_clamped() {
        assert_eq!(Msg::scroll(99_999), Msg::Scroll { v120: MAX_SCROLL });
        assert_eq!(Msg::scroll(-99_999), Msg::Scroll { v120: -MAX_SCROLL });
        assert_eq!(Msg::scroll(-240), Msg::Scroll { v120: -240 });
    }

    #[test]
    fn control_requests_and_replies_keep_their_json_shape() {
        use control::*;
        let r: Request = serde_json::from_str(r#"{"cmd":"set_enabled","on":false}"#).unwrap();
        assert!(matches!(r, Request::SetEnabled { on: false }));
        let r: Request = serde_json::from_str(r#"{"cmd":"pair"}"#).unwrap();
        assert!(matches!(r, Request::Pair));
        let st = Reply::Status { enabled: true, typing: false, devices: vec![] };
        assert_eq!(serde_json::to_string(&st).unwrap(), r#"{"reply":"status","enabled":true,"typing":false,"devices":[]}"#);
        // From a service that predates typing: allowed.
        let old: Reply = serde_json::from_str(r#"{"reply":"status","enabled":true,"devices":[]}"#).unwrap();
        assert!(matches!(old, Reply::Status { typing: true, .. }));
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

/// The touchpad service's local control socket: one JSON request line in, JSON reply lines
/// out. Shared by the service, `phonelink touchpad …` and the desktop window.
pub mod control {
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    /// Where the service listens (`PHONELINK_TOUCHPAD_SOCKET` overrides it).
    pub fn socket_path() -> PathBuf {
        std::env::var_os("PHONELINK_TOUCHPAD_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/run/phonelink-touchpad/control.sock"))
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "cmd", rename_all = "snake_case")]
    pub enum Request {
        /// Show a pairing code (polkit: your password). Leaving the connection cancels it.
        Pair,
        Devices,
        /// Forget a phone (polkit: your password).
        Unpair { who: String },
        /// Whether the touchpad is switched on, and the paired phones.
        Status,
        /// Switch the touchpad on or off (polkit: allowed for your active desktop session).
        SetEnabled { on: bool },
        /// Allow or refuse typing from the phone (polkit: like `SetEnabled`).
        SetTyping { on: bool },
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "reply", rename_all = "snake_case")]
    pub enum Reply {
        PairingCode { uri: String, seconds: u64 },
        Paired { name: String },
        PairingExpired,
        Devices { devices: Vec<Device> },
        Unpaired { name: String },
        Error { message: String },
        Status {
            enabled: bool,
            /// Typing from the phone allowed (older services don't say: allowed).
            #[serde(default = "super::yes")]
            typing: bool,
            devices: Vec<Device>,
        },
        Enabled { on: bool },
        Typing { on: bool },
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Device {
        pub name: String,
        /// Full hex fingerprint.
        pub fingerprint: String,
        pub connected: bool,
    }
}

/// The US keyboard layout: which Linux key (input-event-codes.h) and whether Shift types a
/// character. The laptop's layout is US, so pressing these keys types exactly this text;
/// anything else (emoji, other scripts) is pasted instead.
pub mod keymap {
    /// Linux key codes used for typing (`KEY_*`).
    pub mod code {
        pub const ESC: u16 = 1;
        pub const BACKSPACE: u16 = 14;
        pub const TAB: u16 = 15;
        pub const ENTER: u16 = 28;
        pub const LEFTCTRL: u16 = 29;
        pub const LEFTSHIFT: u16 = 42;
        pub const LEFTALT: u16 = 56;
        pub const SPACE: u16 = 57;
        pub const HOME: u16 = 102;
        pub const UP: u16 = 103;
        pub const PAGEUP: u16 = 104;
        pub const LEFT: u16 = 105;
        pub const RIGHT: u16 = 106;
        pub const END: u16 = 107;
        pub const DOWN: u16 = 108;
        pub const PAGEDOWN: u16 = 109;
        pub const DELETE: u16 = 111;
        pub const LEFTMETA: u16 = 125;
    }

    const LETTERS: [u16; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44,
    ];
    const DIGITS: [u16; 10] = [11, 2, 3, 4, 5, 6, 7, 8, 9, 10];

    /// The key and Shift for `c` on a US keyboard, or `None` if it has no key.
    pub fn us(c: char) -> Option<(u16, bool)> {
        Some(match c {
            'a'..='z' => (LETTERS[(c as u8 - b'a') as usize], false),
            'A'..='Z' => (LETTERS[(c as u8 - b'A') as usize], true),
            '0'..='9' => (DIGITS[(c as u8 - b'0') as usize], false),
            ' ' => (code::SPACE, false),
            '\n' => (code::ENTER, false),
            '\t' => (code::TAB, false),
            '-' => (12, false),
            '_' => (12, true),
            '=' => (13, false),
            '+' => (13, true),
            '[' => (26, false),
            '{' => (26, true),
            ']' => (27, false),
            '}' => (27, true),
            ';' => (39, false),
            ':' => (39, true),
            '\'' => (40, false),
            '"' => (40, true),
            '`' => (41, false),
            '~' => (41, true),
            '\\' => (43, false),
            '|' => (43, true),
            ',' => (51, false),
            '<' => (51, true),
            '.' => (52, false),
            '>' => (52, true),
            '/' => (53, false),
            '?' => (53, true),
            '!' => (2, true),
            '@' => (3, true),
            '#' => (4, true),
            '$' => (5, true),
            '%' => (6, true),
            '^' => (7, true),
            '&' => (8, true),
            '*' => (9, true),
            '(' => (10, true),
            ')' => (11, true),
            _ => return None,
        })
    }

    /// Can every character of `text` be typed?
    pub fn typeable(text: &str) -> bool {
        text.chars().all(|c| us(c).is_some())
    }

    /// `text` in runs: (typeable, the run). Typeable runs are typed; the others pasted.
    pub fn runs(text: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in text.chars() {
            let t = us(c).is_some();
            match out.last_mut() {
                Some((kind, run)) if *kind == t => run.push(c),
                _ => out.push((t, c.to_string())),
            }
        }
        out
    }

    /// Every key code typing may press (for the virtual keyboard's key list).
    pub fn all_codes() -> Vec<u16> {
        let mut v: Vec<u16> = (32u8..127).filter_map(|b| us(b as char)).map(|(k, _)| k).collect();
        v.extend([
            code::ENTER, code::TAB, code::ESC, code::BACKSPACE, code::LEFTCTRL, code::LEFTSHIFT,
            code::LEFTALT, code::LEFTMETA, code::HOME, code::END, code::UP, code::DOWN, code::LEFT,
            code::RIGHT, code::PAGEUP, code::PAGEDOWN, code::DELETE,
        ]);
        v.sort_unstable();
        v.dedup();
        v
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_printable_ascii_character_has_a_key() {
            for b in 32u8..127 {
                assert!(us(b as char).is_some(), "{:?}", b as char);
            }
            assert_eq!(us('a'), Some((30, false)));
            assert_eq!(us('Q'), Some((16, true)));
            assert_eq!(us('m'), Some((50, false)));
            assert_eq!(us('0'), Some((11, false)));
            assert_eq!(us('!'), Some((2, true)));
            assert_eq!(us('?'), Some((53, true)));
            assert_eq!(us('\n'), Some((code::ENTER, false)));
        }

        #[test]
        fn shifted_symbols_share_their_key_with_the_unshifted_one() {
            for (plain, shifted) in [('1', '!'), ('-', '_'), ('[', '{'), (';', ':'), ('\'', '"'), ('/', '?'), ('`', '~')] {
                let (a, s1) = us(plain).unwrap();
                let (b, s2) = us(shifted).unwrap();
                assert_eq!((a, s1, s2), (b, false, true), "{plain} {shifted}");
            }
        }

        #[test]
        fn other_scripts_are_split_out_for_pasting() {
            assert!(us('é').is_none() && us('😀').is_none() && us('₹').is_none() && us('न').is_none());
            assert!(typeable("Hello, world!") && !typeable("Hi 😀"));
            assert_eq!(
                runs("Hi 😀😀 ok"),
                vec![(true, "Hi ".into()), (false, "😀😀".into()), (true, " ok".into())]
            );
            assert!(runs("").is_empty());
        }

        #[test]
        fn no_key_code_is_used_twice_for_different_unshifted_characters() {
            let mut seen = std::collections::HashMap::new();
            for b in 32u8..127 {
                if let Some((k, false)) = us(b as char) {
                    assert!(seen.insert(k, b as char).is_none(), "key {k} for two characters");
                }
            }
        }
    }
}

fn yes() -> bool {
    true
}
