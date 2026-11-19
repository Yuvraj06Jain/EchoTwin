//! The virtual touchpad itself: a uinput device that looks like a laptop touchpad with two
//! physical buttons (the phone's Left click / Right click).
//!
//! What makes libinput (and udev's `ID_INPUT_TOUCHPAD`) treat it as a touchpad:
//! ABS_X/Y and ABS_MT_* axes with a resolution, BTN_TOOL_FINGER…QUINTTAP, BTN_TOUCH,
//! BTN_LEFT, and the POINTER property. It has BTN_RIGHT too and is *not* a BUTTONPAD
//! (clickpad), so libinput takes BTN_LEFT / BTN_RIGHT as real left and right buttons, and
//! holding Left while a finger moves is a drag. With `ID_INPUT_TOUCHPAD` set, the
//! InputActions installer's udev rule also lets InputActions read it (see
//! `packaging/touchpad/72-phonelink-pad.rules.in`).
//!
//! Ported from TouchTwin (`daemon/src/uinput.rs`).

use std::path::PathBuf;

use anyhow::Context;
use evdev::{
    uinput::VirtualDevice, AbsInfo, AbsoluteAxisCode as Abs, AttributeSet, BusType, InputId,
    KeyCode as Key, PropType, UinputAbsSetup,
};
use phonelink_core::touchpad::MAX_CONTACTS;

use crate::pad::{Geometry, Report, UNITS_PER_MM};

/// Stable name, so KDE remembers the device's settings between runs.
pub const DEVICE_NAME: &str = "EchoTwin Touchpad";
const VENDOR: u16 = 0x7470; // "tp"
const PRODUCT: u16 = 0x6574; // "et"

pub struct VirtualPad {
    dev: VirtualDevice,
    geometry: Geometry,
}

impl VirtualPad {
    pub fn create(geometry: Geometry) -> anyhow::Result<Self> {
        let mut keys = AttributeSet::<Key>::new();
        for k in [
            Key::BTN_LEFT,
            Key::BTN_RIGHT,
            Key::BTN_TOUCH,
            Key::BTN_TOOL_FINGER,
            Key::BTN_TOOL_DOUBLETAP,
            Key::BTN_TOOL_TRIPLETAP,
            Key::BTN_TOOL_QUADTAP,
            Key::BTN_TOOL_QUINTTAP,
        ] {
            keys.insert(k);
        }
        let mut props = AttributeSet::<PropType>::new();
        props.insert(PropType::POINTER);

        let axis = |code, max| UinputAbsSetup::new(code, AbsInfo::new(0, 0, max, 0, 0, UNITS_PER_MM));
        let (mx, my) = (geometry.max_x, geometry.max_y);

        let builder = VirtualDevice::builder().map_err(|e| {
            anyhow::anyhow!(
                "the laptop cannot create a touchpad: /dev/uinput: {e}. \
                 Run packaging/install-touchpad.sh on the laptop (it lets only the touchpad service open /dev/uinput)."
            )
        })?;
        let mut dev = builder
            .name(DEVICE_NAME)
            .input_id(InputId::new(BusType::BUS_VIRTUAL, VENDOR, PRODUCT, 1))
            .with_keys(&keys)?
            .with_properties(&props)?
            .with_absolute_axis(&axis(Abs::ABS_X, mx))?
            .with_absolute_axis(&axis(Abs::ABS_Y, my))?
            .with_absolute_axis(&UinputAbsSetup::new(
                Abs::ABS_MT_SLOT,
                AbsInfo::new(0, 0, MAX_CONTACTS as i32 - 1, 0, 0, 0),
            ))?
            .with_absolute_axis(&UinputAbsSetup::new(
                Abs::ABS_MT_TRACKING_ID,
                AbsInfo::new(0, 0, 0xffff, 0, 0, 0),
            ))?
            .with_absolute_axis(&axis(Abs::ABS_MT_POSITION_X, mx))?
            .with_absolute_axis(&axis(Abs::ABS_MT_POSITION_Y, my))?
            .build()
            .context("creating the virtual touchpad")?;

        let sys_name = dev.enumerate_dev_nodes_blocking().ok().and_then(|nodes| {
            nodes
                .flatten()
                .filter_map(|p: PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .find(|n| n.starts_with("event"))
        });
        tracing::info!(?sys_name, max_x = mx, max_y = my, "virtual touchpad created");
        Ok(Self { dev, geometry })
    }

    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// Write the reports, each followed by SYN_REPORT.
    pub fn send(&mut self, reports: &[Report]) -> std::io::Result<()> {
        for r in reports {
            self.dev.emit(r)?;
        }
        Ok(())
    }
}
