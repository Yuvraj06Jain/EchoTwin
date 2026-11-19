//! The laptop's own touchpad comes first: while a finger is on it (and for a moment after),
//! the phone's fingers, buttons and scrolling are ignored.
//!
//! Every few seconds the service looks for touchpads in /dev/input (multitouch position
//! axes and BTN_TOOL_FINGER, not EchoTwin's own devices) and reads each one on a thread of
//! its own. Reading never grabs the device, so libinput and KWin see every event as before.
//! Only BTN_TOUCH is used: a finger down or up. The service may read touchpads and nothing
//! else: `packaging/touchpad/73-phonelink-realpad.rules` gives its group read access to
//! touchpad device nodes only (never keyboards), and its unit allows `char-input` read-only.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use evdev::{AbsoluteAxisCode as Abs, Device, EventType, KeyCode as Key};
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::uinput::{DEVICE_NAME, WHEEL_NAME};

/// After the last finger leaves the laptop's touchpad, the phone waits this long.
pub const GRACE: Duration = Duration::from_millis(500);
/// How often new touchpads are looked for.
const RESCAN: Duration = Duration::from_secs(5);

/// A finger went down or up on one of the laptop's touchpads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Touch {
    pub device: PathBuf,
    pub down: bool,
}

/// Is the laptop's own touchpad in use? Pure: fed touches and the time.
#[derive(Debug, Default)]
pub struct Busy {
    touching: HashSet<PathBuf>,
    released: Option<Instant>,
}

impl Busy {
    pub fn touch(&mut self, t: &Touch, now: Instant) {
        if t.down {
            self.touching.insert(t.device.clone());
        } else if self.touching.remove(&t.device) && self.touching.is_empty() {
            self.released = Some(now);
        }
    }

    /// The device went away (unplugged): it can't hold a finger down any more.
    pub fn gone(&mut self, device: &PathBuf, now: Instant) {
        if self.touching.remove(device) && self.touching.is_empty() {
            self.released = Some(now);
        }
    }

    pub fn in_use(&self, now: Instant) -> bool {
        !self.touching.is_empty() || self.released.is_some_and(|t| now.duration_since(t) < GRACE)
    }
}

/// What reading a device reports back.
#[derive(Debug)]
pub enum Seen {
    Touch(Touch),
    Gone(PathBuf),
}

fn is_real_touchpad(dev: &Device) -> bool {
    let name = dev.name().unwrap_or_default();
    if name == DEVICE_NAME || name == WHEEL_NAME {
        return false;
    }
    let keys = dev.supported_keys().is_some_and(|k| k.contains(Key::BTN_TOOL_FINGER) && k.contains(Key::BTN_TOUCH));
    let axes = dev
        .supported_absolute_axes()
        .is_some_and(|a| a.contains(Abs::ABS_MT_POSITION_X) && a.contains(Abs::ABS_MT_POSITION_Y));
    keys && axes
}

/// Watch the laptop's touchpads for as long as the service runs; touches arrive on `tx`.
pub fn watch(tx: mpsc::UnboundedSender<Seen>) {
    let open: Arc<Mutex<HashSet<PathBuf>>> = Arc::default();
    std::thread::Builder::new()
        .name("realpad-scan".into())
        .spawn(move || {
            let mut said: HashMap<PathBuf, ()> = HashMap::new();
            loop {
                for (path, dev) in evdev::enumerate() {
                    if !is_real_touchpad(&dev) || open.lock().unwrap_or_else(|e| e.into_inner()).contains(&path) {
                        continue;
                    }
                    if said.insert(path.clone(), ()).is_none() {
                        info!(device = %path.display(), name = dev.name().unwrap_or_default(), "watching the laptop's touchpad");
                    }
                    open.lock().unwrap_or_else(|e| e.into_inner()).insert(path.clone());
                    let (tx, open) = (tx.clone(), open.clone());
                    let _ = std::thread::Builder::new().name("realpad".into()).spawn(move || {
                        read(dev, &path, &tx);
                        open.lock().unwrap_or_else(|e| e.into_inner()).remove(&path);
                        let _ = tx.send(Seen::Gone(path));
                    });
                }
                if tx.is_closed() {
                    return;
                }
                std::thread::sleep(RESCAN);
            }
        })
        .expect("spawning a thread");
}

fn read(mut dev: Device, path: &std::path::Path, tx: &mpsc::UnboundedSender<Seen>) {
    loop {
        let events = match dev.fetch_events() {
            Ok(e) => e,
            Err(e) => {
                debug!(device = %path.display(), "stopped reading: {e}");
                return;
            }
        };
        for e in events {
            if e.event_type() == EventType::KEY && e.code() == Key::BTN_TOUCH.0 && e.value() != 2 {
                let t = Touch { device: path.to_path_buf(), down: e.value() == 1 };
                if tx.send(Seen::Touch(t)).is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(dev: &str, down: bool) -> Touch {
        Touch { device: PathBuf::from(dev), down }
    }

    #[test]
    fn in_use_while_touched_and_for_a_moment_after() {
        let start = Instant::now();
        let mut b = Busy::default();
        assert!(!b.in_use(start));
        b.touch(&t("/dev/input/event5", true), start);
        assert!(b.in_use(start + Duration::from_secs(60)), "held for a minute: still in use");
        let up = start + Duration::from_secs(61);
        b.touch(&t("/dev/input/event5", false), up);
        assert!(b.in_use(up + GRACE / 2), "just let go");
        assert!(!b.in_use(up + GRACE), "the phone takes over again");
    }

    #[test]
    fn two_touchpads_and_unplugging() {
        let now = Instant::now();
        let mut b = Busy::default();
        b.touch(&t("a", true), now);
        b.touch(&t("b", true), now);
        b.touch(&t("a", false), now);
        assert!(b.in_use(now + GRACE * 4), "b is still touched");
        b.gone(&PathBuf::from("b"), now);
        assert!(!b.in_use(now + GRACE), "an unplugged touchpad holds nothing down");
        // A stray "up" for a device never touched changes nothing.
        let mut c = Busy::default();
        c.touch(&t("x", false), now);
        assert!(!c.in_use(now));
    }
}
