//! Typing from the phone: `Msg::Text` and `Msg::Key` → key presses on the virtual keyboard.
//!
//! Pure: produces reports, writes nothing. Every message presses and releases everything it
//! needs (Shift for capitals, the modifiers of a shortcut), so no key is ever left held,
//! even if the connection drops right after.
//!
//! The laptop's layout is US (`phonelink_core::touchpad::keymap::us`), so the keys typed are
//! exactly the characters sent. The phone pastes anything else through the clipboard.

use std::time::{Duration, Instant};

use evdev::{EventType, InputEvent};
use phonelink_core::touchpad::{
    keymap::{code, us},
    mods, Key,
};

use crate::pad::Report;

fn key(code: u16, down: bool) -> InputEvent {
    InputEvent::new(EventType::KEY.0, code, down as i32)
}

/// Press `codes` in order (one report), then release them in reverse (a second report).
fn chord(codes: &[u16]) -> [Report; 2] {
    [
        codes.iter().map(|&c| key(c, true)).collect(),
        codes.iter().rev().map(|&c| key(c, false)).collect(),
    ]
}

/// The reports that type `text`; characters the US layout can't type are skipped.
pub fn text_reports(text: &str) -> Vec<Report> {
    let mut out = Vec::new();
    for c in text.chars() {
        if let Some((k, shift)) = us(c) {
            let codes: &[u16] = if shift { &[code::LEFTSHIFT, k] } else { &[k] };
            out.extend(chord(codes));
        }
    }
    out
}

fn modifier_codes(m: u8) -> Vec<u16> {
    let mut v = Vec::new();
    for (bit, c) in [
        (mods::CTRL, code::LEFTCTRL),
        (mods::ALT, code::LEFTALT),
        (mods::SHIFT, code::LEFTSHIFT),
        (mods::SUPER, code::LEFTMETA),
    ] {
        if m & bit != 0 {
            v.push(c);
        }
    }
    v
}

/// The reports for one key with modifiers (empty if the character can't be typed).
pub fn key_reports(k: Key, m: u8) -> Vec<Report> {
    let (code, shift) = match k {
        Key::Enter => (code::ENTER, false),
        Key::Backspace => (code::BACKSPACE, false),
        Key::Tab => (code::TAB, false),
        Key::Escape => (code::ESC, false),
        Key::Left => (code::LEFT, false),
        Key::Right => (code::RIGHT, false),
        Key::Up => (code::UP, false),
        Key::Down => (code::DOWN, false),
        Key::Home => (code::HOME, false),
        Key::End => (code::END, false),
        Key::Delete => (code::DELETE, false),
        Key::PageUp => (code::PAGEUP, false),
        Key::PageDown => (code::PAGEDOWN, false),
        Key::Char(c) => match us(c) {
            Some(x) => x,
            None => return vec![],
        },
    };
    let mut codes = modifier_codes(m | if shift { mods::SHIFT } else { 0 });
    codes.push(code);
    chord(&codes).into()
}

/// At most this many key presses per second from the phone; more are dropped.
pub const MAX_PER_SECOND: u32 = 400;

/// A one-second window counter: a runaway or hostile phone can't flood the laptop with keys.
#[derive(Debug)]
pub struct RateLimit {
    window: Instant,
    used: u32,
}

impl RateLimit {
    pub fn new(now: Instant) -> Self {
        Self { window: now, used: 0 }
    }

    /// May `n` more presses go now?
    pub fn allow(&mut self, n: u32, now: Instant) -> bool {
        if now.duration_since(self.window) >= Duration::from_secs(1) {
            self.window = now;
            self.used = 0;
        }
        if self.used + n > MAX_PER_SECOND {
            return false;
        }
        self.used += n;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evdev::KeyCode;

    fn names(rs: &[Report]) -> Vec<String> {
        rs.iter()
            .map(|r| {
                r.iter()
                    .map(|e| format!("{:?}={}", KeyCode(e.code()), e.value()))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    #[test]
    fn text_presses_and_releases_each_character_with_shift_for_capitals() {
        assert_eq!(
            names(&text_reports("aB!")),
            [
                "KEY_A=1",
                "KEY_A=0",
                "KEY_LEFTSHIFT=1 KEY_B=1",
                "KEY_B=0 KEY_LEFTSHIFT=0",
                "KEY_LEFTSHIFT=1 KEY_1=1",
                "KEY_1=0 KEY_LEFTSHIFT=0",
            ]
        );
        assert!(text_reports("😀é").is_empty(), "not on the US layout: skipped");
        assert_eq!(names(&text_reports("a\n")), ["KEY_A=1", "KEY_A=0", "KEY_ENTER=1", "KEY_ENTER=0"]);
    }

    #[test]
    fn shortcuts_press_modifiers_first_and_release_them_last() {
        assert_eq!(
            names(&key_reports(Key::Char('c'), mods::CTRL)),
            ["KEY_LEFTCTRL=1 KEY_C=1", "KEY_C=0 KEY_LEFTCTRL=0"]
        );
        assert_eq!(
            names(&key_reports(Key::Left, mods::CTRL | mods::SHIFT)),
            ["KEY_LEFTCTRL=1 KEY_LEFTSHIFT=1 KEY_LEFT=1", "KEY_LEFT=0 KEY_LEFTSHIFT=0 KEY_LEFTCTRL=0"]
        );
        assert_eq!(
            names(&key_reports(Key::Char('T'), mods::CTRL | mods::ALT)),
            ["KEY_LEFTCTRL=1 KEY_LEFTALT=1 KEY_LEFTSHIFT=1 KEY_T=1", "KEY_T=0 KEY_LEFTSHIFT=0 KEY_LEFTALT=0 KEY_LEFTCTRL=0"]
        );
        assert_eq!(names(&key_reports(Key::Escape, mods::SUPER)), ["KEY_LEFTMETA=1 KEY_ESC=1", "KEY_ESC=0 KEY_LEFTMETA=0"]);
        assert!(key_reports(Key::Char('€'), mods::CTRL).is_empty());
    }

    #[test]
    fn every_report_releases_what_it_pressed() {
        for rs in [text_reports("Hello, World! ~_~"), key_reports(Key::Delete, 15)] {
            let mut held = std::collections::HashSet::new();
            for e in rs.iter().flatten() {
                if e.value() == 1 {
                    held.insert(e.code());
                } else {
                    held.remove(&e.code());
                }
            }
            assert!(held.is_empty(), "nothing stays held");
        }
    }

    #[test]
    fn the_rate_limit_allows_a_burst_then_refuses_until_the_next_second() {
        let t = Instant::now();
        let mut r = RateLimit::new(t);
        assert!(r.allow(MAX_PER_SECOND - 1, t));
        assert!(r.allow(1, t));
        assert!(!r.allow(1, t + Duration::from_millis(500)));
        assert!(r.allow(10, t + Duration::from_secs(1)));
    }
}
