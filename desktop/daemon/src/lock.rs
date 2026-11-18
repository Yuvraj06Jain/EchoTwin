//! Locking and unlocking this computer's screen when the paired phone asks.
//!
//! Locking tries the desktop's screen saver first (KDE Plasma, GNOME and most others
//! implement `org.freedesktop.ScreenSaver.Lock`), then systemd-logind, which asks the desktop
//! to lock every graphical session of this user. Unlocking goes through logind only
//! (`Session.Unlock`, what `loginctl unlock-session` does; KDE's lock screen obeys it, and a
//! user may unlock their own session without root). The daemon allows it only when the
//! `allow_unlock` switch is on, and the phone asks for its fingerprint or screen lock first.

use zbus::blocking::{connection, Connection};

/// D-Bus addresses to use instead of the real buses (tests).
#[derive(Clone, Debug, Default)]
pub struct Buses {
    pub session: Option<String>,
    pub system: Option<String>,
}

impl Buses {
    /// Buses that don't exist: nothing on this machine is touched (tests).
    pub fn unreachable() -> Self {
        let nowhere = "unix:path=/nonexistent/phonelink-test-bus".to_string();
        Self {
            session: Some(nowhere.clone()),
            system: Some(nowhere),
        }
    }
}

fn open(addr: &Option<String>, system: bool) -> zbus::Result<Connection> {
    match addr {
        Some(a) => connection::Builder::address(a.as_str())?.build(),
        None if system => Connection::system(),
        None => Connection::session(),
    }
}

/// (service, object path, interface) of screen savers that have a `Lock()` method.
const SCREEN_SAVERS: &[(&str, &str, &str)] = &[
    ("org.freedesktop.ScreenSaver", "/ScreenSaver", "org.freedesktop.ScreenSaver"),
    ("org.freedesktop.ScreenSaver", "/org/freedesktop/ScreenSaver", "org.freedesktop.ScreenSaver"),
    ("org.gnome.ScreenSaver", "/org/gnome/ScreenSaver", "org.gnome.ScreenSaver"),
];

/// Lock the screen. Returns which mechanism worked, or why none did. Blocking.
pub fn lock_screen(buses: &Buses) -> Result<&'static str, String> {
    let mut errors = vec![];
    match open(&buses.session, false) {
        Ok(c) => {
            for (dest, path, iface) in SCREEN_SAVERS {
                match c.call_method(Some(*dest), *path, Some(*iface), "Lock", &()) {
                    Ok(_) => return Ok("screen saver"),
                    Err(e) => errors.push(format!("{dest}{path}: {e}")),
                }
            }
        }
        Err(e) => errors.push(format!("session bus: {e}")),
    }
    match lock_with_logind(buses) {
        Ok(n) if n > 0 => Ok("logind"),
        Ok(_) => {
            errors.push("logind: no graphical session of this user".into());
            Err(errors.join("; "))
        }
        Err(e) => {
            errors.push(format!("logind: {e}"));
            Err(errors.join("; "))
        }
    }
}

fn my_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(u32::MAX)
}

/// Unlock this user's graphical sessions (through logind). Returns how many. Blocking.
pub fn unlock_screen(buses: &Buses) -> Result<usize, String> {
    match session_call(buses, "Unlock") {
        Ok(0) => Err("no graphical session of this user".into()),
        Ok(n) => Ok(n),
        Err(e) => Err(format!("logind: {e}")),
    }
}

/// Lock this user's graphical sessions through logind. Returns how many were locked.
fn lock_with_logind(buses: &Buses) -> zbus::Result<usize> {
    session_call(buses, "Lock")
}

/// Call `method` ("Lock" or "Unlock") on each graphical logind session of this user.
fn session_call(buses: &Buses, method: &str) -> zbus::Result<usize> {
    let c = open(&buses.system, true)?;
    let dest = "org.freedesktop.login1";
    let sessions: Vec<(String, u32, String, String, zbus::zvariant::OwnedObjectPath)> = c
        .call_method(
            Some(dest),
            "/org/freedesktop/login1",
            Some("org.freedesktop.login1.Manager"),
            "ListSessions",
            &(),
        )?
        .body()
        .deserialize()?;
    let uid = my_uid();
    let mut locked = 0;
    for (_, session_uid, _, _, path) in sessions {
        if session_uid != uid {
            continue;
        }
        let kind: zbus::zvariant::OwnedValue = c
            .call_method(
                Some(dest),
                path.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.freedesktop.login1.Session", "Type"),
            )?
            .body()
            .deserialize()?;
        let kind: String = kind.try_into().unwrap_or_default();
        if !["x11", "wayland", "mir"].contains(&kind.as_str()) {
            continue; // ssh/tty sessions have no screen to lock or unlock
        }
        c.call_method(
            Some(dest),
            path.as_str(),
            Some("org.freedesktop.login1.Session"),
            method,
            &(),
        )?;
        locked += 1;
    }
    Ok(locked)
}
