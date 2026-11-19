//! Turns the phone's finger snapshots into the evdev events a real multitouch touchpad
//! sends (protocol B), so libinput, KWin and InputActions treat the phone exactly like the
//! laptop's own touchpad.
//!
//! Pure: produces event lists, writes nothing. [`crate::uinput`] sends them to the kernel.
//!
//! Ported from TouchTwin (`daemon/src/pad.rs`); [`Buttons`] (the Left/Right click buttons)
//! is new.

use evdev::{AbsoluteAxisCode as Abs, EventType, InputEvent, KeyCode as Key};
use phonelink_core::touchpad::{Button, Contact, PadInfo, COORD_MAX, MAX_CONTACTS};

/// Device units per millimetre on both axes.
pub const UNITS_PER_MM: i32 = 40;

/// The virtual touchpad's coordinate space, derived from the phone's pad size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub max_x: i32,
    pub max_y: i32,
}

impl Geometry {
    pub fn from_pad(info: PadInfo) -> Self {
        let info = info.sanitized();
        // tenths of a mm × units/mm ÷ 10
        Self {
            max_x: info.width_dmm as i32 * UNITS_PER_MM / 10,
            max_y: info.height_dmm as i32 * UNITS_PER_MM / 10,
        }
    }

    fn map(&self, c: &Contact) -> (i32, i32) {
        let sx = |v: u16, max: i32| (v.min(COORD_MAX) as i64 * max as i64 / COORD_MAX as i64) as i32;
        (sx(c.x, self.max_x), sx(c.y, self.max_y))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Touch {
    /// The phone's id for this touch; a new id in the same slot is a new touch.
    phone_id: u16,
    tracking_id: i32,
    x: i32,
    y: i32,
}

/// One `SYN_REPORT` worth of events (the sender appends the SYN_REPORT).
pub type Report = Vec<InputEvent>;

pub struct PadState {
    geometry: Geometry,
    slots: [Option<Touch>; MAX_CONTACTS],
    /// The slot the kernel last saw selected (ABS_MT_SLOT is stateful).
    current_slot: i32,
    next_tracking_id: i32,
    /// Which BTN_TOOL_* is down (number of fingers, 0 = none).
    tool: usize,
}

const TOOLS: [Key; 5] = [
    Key::BTN_TOOL_FINGER,
    Key::BTN_TOOL_DOUBLETAP,
    Key::BTN_TOOL_TRIPLETAP,
    Key::BTN_TOOL_QUADTAP,
    Key::BTN_TOOL_QUINTTAP,
];

fn abs(code: Abs, value: i32) -> InputEvent {
    InputEvent::new(EventType::ABSOLUTE.0, code.0, value)
}

fn key(code: Key, down: bool) -> InputEvent {
    InputEvent::new(EventType::KEY.0, code.0, down as i32)
}

impl PadState {
    pub fn new(geometry: Geometry) -> Self {
        Self {
            geometry,
            slots: [None; MAX_CONTACTS],
            current_slot: -1,
            next_tracking_id: 0,
            tool: 0,
        }
    }

    pub fn fingers_down(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// Move to the phone's latest snapshot. Usually one report; two when a slot was lifted
    /// and touched again between frames (the lift must reach libinput first).
    pub fn apply(&mut self, contacts: &[Contact]) -> Vec<Report> {
        let mut wanted: [Option<Contact>; MAX_CONTACTS] = [None; MAX_CONTACTS];
        for c in contacts {
            if (c.slot as usize) < MAX_CONTACTS {
                wanted[c.slot as usize] = Some(*c);
            }
        }
        let mut reports = Vec::new();

        // A slot whose touch changed identity: lift the old touch in its own report first.
        let replaced: Vec<usize> = (0..MAX_CONTACTS)
            .filter(|&s| matches!((&self.slots[s], &wanted[s]), (Some(t), Some(w)) if t.phone_id != w.id))
            .collect();
        if !replaced.is_empty() {
            let mut r = Vec::new();
            for s in replaced {
                self.select(&mut r, s);
                r.push(abs(Abs::ABS_MT_TRACKING_ID, -1));
                self.slots[s] = None;
            }
            self.finish(&mut r);
            reports.push(r);
        }

        let mut r = Vec::new();
        for (s, want) in wanted.iter().enumerate() {
            match (self.slots[s], want) {
                (None, None) => {}
                (Some(_), None) => {
                    self.select(&mut r, s);
                    r.push(abs(Abs::ABS_MT_TRACKING_ID, -1));
                    self.slots[s] = None;
                }
                (None, Some(c)) => {
                    let (x, y) = self.geometry.map(c);
                    let tracking_id = self.next_tracking_id;
                    self.next_tracking_id = (self.next_tracking_id + 1) & 0xffff;
                    self.select(&mut r, s);
                    r.push(abs(Abs::ABS_MT_TRACKING_ID, tracking_id));
                    r.push(abs(Abs::ABS_MT_POSITION_X, x));
                    r.push(abs(Abs::ABS_MT_POSITION_Y, y));
                    self.slots[s] = Some(Touch { phone_id: c.id, tracking_id, x, y });
                }
                (Some(t), Some(c)) => {
                    let (x, y) = self.geometry.map(c);
                    if (x, y) != (t.x, t.y) {
                        self.select(&mut r, s);
                        if x != t.x {
                            r.push(abs(Abs::ABS_MT_POSITION_X, x));
                        }
                        if y != t.y {
                            r.push(abs(Abs::ABS_MT_POSITION_Y, y));
                        }
                        self.slots[s] = Some(Touch { x, y, ..t });
                    }
                }
            }
        }
        if !r.is_empty() {
            self.finish(&mut r);
            reports.push(r);
        }
        reports
    }

    /// Lift every finger (connection lost, phone silent, pad closed).
    pub fn release_all(&mut self) -> Vec<Report> {
        self.apply(&[])
    }

    fn select(&mut self, r: &mut Report, slot: usize) {
        if self.current_slot != slot as i32 {
            r.push(abs(Abs::ABS_MT_SLOT, slot as i32));
            self.current_slot = slot as i32;
        }
    }

    /// Single-touch emulation (ABS_X/Y, BTN_TOUCH) and the finger-count tool keys, as a
    /// real touchpad sends them at the end of each report.
    fn finish(&mut self, r: &mut Report) {
        let fingers = self.fingers_down();
        if fingers != self.tool {
            if self.tool > 0 {
                r.push(key(TOOLS[self.tool - 1], false));
            }
            if self.tool == 0 {
                r.push(key(Key::BTN_TOUCH, true));
            }
            if fingers > 0 {
                r.push(key(TOOLS[fingers - 1], true));
            } else {
                r.push(key(Key::BTN_TOUCH, false));
            }
            self.tool = fingers;
        }
        // The legacy pointer follows the lowest active slot.
        if let Some(t) = self.slots.iter().flatten().next() {
            r.push(abs(Abs::ABS_X, t.x));
            r.push(abs(Abs::ABS_Y, t.y));
        }
    }
}

/// The pad's two physical buttons, as the phone's Left click / Right click buttons hold them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Buttons {
    left: bool,
    right: bool,
}

impl Buttons {
    fn slot(&mut self, b: Button) -> &mut bool {
        match b {
            Button::Left => &mut self.left,
            Button::Right => &mut self.right,
        }
    }

    pub fn any_down(&self) -> bool {
        self.left || self.right
    }

    /// Press or release `b`. A report only when it changes (a repeated "down" is nothing).
    pub fn set(&mut self, b: Button, down: bool) -> Option<Report> {
        let held = self.slot(b);
        if *held == down {
            return None;
        }
        *held = down;
        let code = match b {
            Button::Left => Key::BTN_LEFT,
            Button::Right => Key::BTN_RIGHT,
        };
        Some(vec![key(code, down)])
    }

    /// Let go of both (connection lost, phone silent, pad closed).
    pub fn release_all(&mut self) -> Vec<Report> {
        [Button::Left, Button::Right].into_iter().filter_map(|b| self.set(b, false)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geo() -> Geometry {
        Geometry::from_pad(PadInfo { width_dmm: 700, height_dmm: 1400 })
    }

    fn c(slot: u8, id: u16, x: u16, y: u16) -> Contact {
        Contact { slot, id, x, y }
    }

    /// Render a report as readable (type, code, value) names for assertions.
    fn names(r: &Report) -> Vec<String> {
        r.iter()
            .map(|e| match e.event_type() {
                EventType::ABSOLUTE => format!("{:?}={}", Abs(e.code()), e.value()),
                EventType::KEY => format!("{:?}={}", Key(e.code()), e.value()),
                _ => format!("{e:?}"),
            })
            .collect()
    }

    #[test]
    fn geometry_matches_the_phone_size() {
        let g = geo();
        assert_eq!((g.max_x, g.max_y), (2800, 5600)); // 70 mm × 140 mm at 40 units/mm
        let tiny = Geometry::from_pad(PadInfo { width_dmm: 1, height_dmm: 60_000 });
        assert_eq!((tiny.max_x, tiny.max_y), (800, 16_000), "clamped to 2–40 cm");
    }

    #[test]
    fn one_finger_down_move_up() {
        let mut p = PadState::new(geo());
        let down = p.apply(&[c(0, 7, 5000, 5000)]);
        assert_eq!(down.len(), 1);
        assert_eq!(
            names(&down[0]),
            [
                "ABS_MT_SLOT=0",
                "ABS_MT_TRACKING_ID=0",
                "ABS_MT_POSITION_X=1400",
                "ABS_MT_POSITION_Y=2800",
                "BTN_TOUCH=1",
                "BTN_TOOL_FINGER=1",
                "ABS_X=1400",
                "ABS_Y=2800",
            ]
        );

        let mv = p.apply(&[c(0, 7, 10_000, 5000)]);
        assert_eq!(names(&mv[0]), ["ABS_MT_POSITION_X=2800", "ABS_X=2800", "ABS_Y=2800"]);

        assert!(p.apply(&[c(0, 7, 10_000, 5000)]).is_empty(), "no change, no report");

        let up = p.apply(&[]);
        assert_eq!(
            names(&up[0]),
            ["ABS_MT_TRACKING_ID=-1", "BTN_TOOL_FINGER=0", "BTN_TOUCH=0"]
        );
        assert_eq!(p.fingers_down(), 0);
    }

    #[test]
    fn three_fingers_switch_the_tool_keys() {
        let mut p = PadState::new(geo());
        p.apply(&[c(0, 1, 100, 100)]);
        let r = p.apply(&[c(0, 1, 100, 100), c(1, 2, 200, 100), c(2, 3, 300, 100)]);
        let n = names(&r[0]);
        assert!(n.contains(&"BTN_TOOL_FINGER=0".to_string()));
        assert!(n.contains(&"BTN_TOOL_TRIPLETAP=1".to_string()));
        assert!(!n.iter().any(|s| s.starts_with("BTN_TOUCH")), "touch stays down");
        assert_eq!(p.fingers_down(), 3);

        let up = p.release_all();
        let n = names(&up[0]);
        assert_eq!(n.iter().filter(|s| *s == "ABS_MT_TRACKING_ID=-1").count(), 3);
        assert!(n.contains(&"BTN_TOOL_TRIPLETAP=0".to_string()));
        assert!(n.contains(&"BTN_TOUCH=0".to_string()));
    }

    #[test]
    fn a_new_touch_in_the_same_slot_lifts_the_old_one_first() {
        let mut p = PadState::new(geo());
        p.apply(&[c(0, 1, 100, 100)]);
        // Finger lifted and put down again between two frames (the "up" frame was lost).
        let r = p.apply(&[c(0, 2, 9000, 9000)]);
        assert_eq!(r.len(), 2, "lift and new touch are separate reports");
        assert_eq!(
            names(&r[0]),
            ["ABS_MT_TRACKING_ID=-1", "BTN_TOOL_FINGER=0", "BTN_TOUCH=0"]
        );
        assert!(names(&r[1]).contains(&"ABS_MT_TRACKING_ID=1".to_string()));
        assert!(names(&r[1]).contains(&"BTN_TOUCH=1".to_string()));
    }

    #[test]
    fn slot_selection_is_only_sent_when_it_changes() {
        let mut p = PadState::new(geo());
        p.apply(&[c(0, 1, 0, 0), c(1, 2, 0, 0)]);
        let r = p.apply(&[c(0, 1, 0, 0), c(1, 2, 50, 0)]);
        assert_eq!(names(&r[0])[0], "ABS_MT_POSITION_X=14", "slot 1 is still selected");
        let r = p.apply(&[c(0, 1, 50, 0), c(1, 2, 50, 0)]);
        assert_eq!(names(&r[0])[0], "ABS_MT_SLOT=0");
    }

    #[test]
    fn buttons_press_once_and_release_together() {
        let mut b = Buttons::default();
        assert_eq!(names(&b.set(Button::Right, true).unwrap()), ["BTN_RIGHT=1"]);
        assert!(b.set(Button::Right, true).is_none(), "a repeated press changes nothing");
        assert_eq!(names(&b.set(Button::Left, true).unwrap()), ["BTN_LEFT=1"]);
        assert!(b.any_down());
        let up: Vec<_> = b.release_all().iter().map(names).collect();
        assert_eq!(up, [vec!["BTN_LEFT=0"], vec!["BTN_RIGHT=0"]]);
        assert!(!b.any_down());
        assert!(b.release_all().is_empty(), "nothing left to release");
        assert!(b.set(Button::Left, false).is_none(), "a stray release changes nothing");
    }
}
