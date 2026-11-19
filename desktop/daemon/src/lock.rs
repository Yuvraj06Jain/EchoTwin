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

/// What an unlock did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unlocked {
    /// Graphical sessions of this user that were unlocked.
    pub sessions: usize,
    /// The session wasn't the one on screen (Switch user: the login screen shows): `Some(true)`
    /// if it was brought back, `Some(false)` if logind refused.
    pub reactivated: Option<bool>,
}

/// Unlock this user's graphical sessions (through logind), and bring one back on screen if
/// another session or the login screen is showing ("Switch user"). Blocking.
pub fn unlock_screen(buses: &Buses) -> Result<Unlocked, String> {
    let c = open(&buses.system, true).map_err(|e| format!("logind: {e}"))?;
    let sessions = my_graphical_sessions(&c).map_err(|e| format!("logind: {e}"))?;
    if sessions.is_empty() {
        return Err("no graphical session of this user".into());
    }
    let mut reactivated = None;
    for path in &sessions {
        session_method(&c, path, "Unlock").map_err(|e| format!("logind: {e}"))?;
        if session_bool(&c, path, "Active") == Some(false) {
            reactivated = Some(session_method(&c, path, "Activate").is_ok());
        }
    }
    Ok(Unlocked { sessions: sessions.len(), reactivated })
}

/// Lock this user's graphical sessions through logind. Returns how many were locked.
fn lock_with_logind(buses: &Buses) -> zbus::Result<usize> {
    let c = open(&buses.system, true)?;
    let sessions = my_graphical_sessions(&c)?;
    for path in &sessions {
        session_method(&c, path, "Lock")?;
    }
    Ok(sessions.len())
}

const LOGIND: &str = "org.freedesktop.login1";
const SESSION: &str = "org.freedesktop.login1.Session";

/// This user's graphical logind sessions (not ssh/tty ones: no screen to lock or unlock).
fn my_graphical_sessions(c: &Connection) -> zbus::Result<Vec<zbus::zvariant::OwnedObjectPath>> {
    let sessions: Vec<(String, u32, String, String, zbus::zvariant::OwnedObjectPath)> = c
        .call_method(Some(LOGIND), "/org/freedesktop/login1", Some("org.freedesktop.login1.Manager"), "ListSessions", &())?
        .body()
        .deserialize()?;
    let uid = my_uid();
    let mut out = vec![];
    for (_, session_uid, _, _, path) in sessions {
        if session_uid != uid {
            continue;
        }
        let kind: zbus::zvariant::OwnedValue = c
            .call_method(Some(LOGIND), path.as_str(), Some("org.freedesktop.DBus.Properties"), "Get", &(SESSION, "Type"))?
            .body()
            .deserialize()?;
        let kind: String = kind.try_into().unwrap_or_default();
        if ["x11", "wayland", "mir"].contains(&kind.as_str()) {
            out.push(path);
        }
    }
    Ok(out)
}

fn session_method(c: &Connection, path: &zbus::zvariant::OwnedObjectPath, method: &str) -> zbus::Result<()> {
    c.call_method(Some(LOGIND), path.as_str(), Some(SESSION), method, &()).map(|_| ())
}

fn session_bool(c: &Connection, path: &zbus::zvariant::OwnedObjectPath, prop: &str) -> Option<bool> {
    let v: zbus::zvariant::OwnedValue = c
        .call_method(Some(LOGIND), path.as_str(), Some("org.freedesktop.DBus.Properties"), "Get", &(SESSION, prop))
        .ok()?
        .body()
        .deserialize()
        .ok()?;
    bool::try_from(v).ok()
}

// ---------------------------------------------------------------- is the screen locked?

/// Locked or not, or `None` if this desktop doesn't say: the screen saver's `GetActive`
/// (KDE: active == locked), else logind's `LockedHint` of this user's graphical session.
/// Blocking.
pub fn screen_locked(buses: &Buses) -> Option<bool> {
    if let Ok(c) = open(&buses.session, false) {
        for (dest, path, iface) in &SCREEN_SAVERS[..2] {
            if let Ok(m) = c.call_method(Some(*dest), *path, Some(*iface), "GetActive", &()) {
                if let Ok(active) = m.body().deserialize::<bool>() {
                    return Some(active);
                }
            }
        }
    }
    let c = open(&buses.system, true).ok()?;
    let sessions = my_graphical_sessions(&c).ok()?;
    // Locked if any of this user's graphical sessions says so.
    let hints: Vec<bool> = sessions.iter().filter_map(|p| session_bool(&c, p, "LockedHint")).collect();
    if hints.is_empty() {
        None
    } else {
        Some(hints.contains(&true))
    }
}

/// Follow the lock state: sends the current state at once (if known), then every change.
/// Signals only (the screen saver's `ActiveChanged`, logind's `PropertiesChanged`), no polling.
pub fn watch_lock_state(buses: Buses, tx: tokio::sync::mpsc::UnboundedSender<bool>) {
    std::thread::Builder::new()
        .name("phonelink-lock-state".into())
        .spawn(move || {
            let (tick_tx, tick_rx) = std::sync::mpsc::channel::<()>();
            let mut conns = vec![];
            if let Ok(c) = open(&buses.session, false) {
                let rule = zbus::MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .interface("org.freedesktop.ScreenSaver")
                    .and_then(|b| b.member("ActiveChanged"))
                    .map(|b| b.build());
                if let Ok(rule) = rule {
                    if let Ok(it) = zbus::blocking::MessageIterator::for_match_rule(rule, &c, Some(16)) {
                        let t = tick_tx.clone();
                        std::thread::spawn(move || {
                            for m in it {
                                if m.is_err() || t.send(()).is_err() {
                                    break;
                                }
                            }
                        });
                    }
                }
                conns.push(c);
            }
            if let Ok(c) = open(&buses.system, true) {
                let rule = zbus::MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .sender(LOGIND)
                    .and_then(|b| b.interface("org.freedesktop.DBus.Properties"))
                    .and_then(|b| b.member("PropertiesChanged"))
                    .map(|b| b.build());
                if let Ok(rule) = rule {
                    if let Ok(it) = zbus::blocking::MessageIterator::for_match_rule(rule, &c, Some(16)) {
                        let t = tick_tx.clone();
                        std::thread::spawn(move || {
                            for m in it {
                                if m.is_err() || t.send(()).is_err() {
                                    break;
                                }
                            }
                        });
                    }
                }
                conns.push(c);
            }
            drop(tick_tx);
            let mut last = None;
            loop {
                let now = screen_locked(&buses);
                if now.is_some() && now != last {
                    if tx.send(now.unwrap_or(false)).is_err() {
                        return;
                    }
                    last = now;
                }
                if tick_rx.recv().is_err() {
                    return; // nothing to listen to on this machine
                }
                // A burst of signals (both object paths, several properties): read once.
                std::thread::sleep(std::time::Duration::from_millis(150));
                while tick_rx.try_recv().is_ok() {}
            }
        })
        .expect("spawn lock-state thread");
}

