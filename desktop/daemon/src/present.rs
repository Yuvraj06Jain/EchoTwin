//! The phone as a presentation clicker: next, previous, start (per app), end and black
//! screen, pressed on this computer.
//!
//! Wayland doesn't let a program type into other windows, so this asks the desktop through
//! the xdg-desktop-portal RemoteDesktop interface, the same permission KDE Connect uses. KDE
//! shows "allow remote control?" once; the portal's restore token (kept in the data
//! directory, mode 0600) lets later sessions start without asking again. The phone can only
//! send these keys (and the chords below): never text, never other keys.
//!
//! Start differs per app: F5 in PowerPoint and Impress, but in a browser F5 reloads the page
//! (which ended a Canva presentation). So Google Slides gets Ctrl+F5, Canva Ctrl+Alt+P, and
//! anything else F11 (full screen).
//!
//! On X11 without a portal, `xdotool` (if installed) does the same.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc,
    time::Duration,
};

use phonelink_core::PresentKeyKind;
use tracing::{debug, info, warn};
use zbus::{
    blocking::{connection, Connection, MessageIterator},
    zvariant::{OwnedObjectPath, OwnedValue, Value},
    MatchRule,
};

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.RemoteDesktop";
/// Keyboard only.
const DEVICE_KEYBOARD: u32 = 1;
/// Remember the permission until it is revoked.
const PERSIST_UNTIL_REVOKED: u32 = 2;

/// Linux evdev key codes: what presentation clickers send. A chord is pressed in order and
/// released in reverse order.
pub fn keys(key: PresentKeyKind) -> &'static [i32] {
    match key {
        PresentKeyKind::Next => &[109],                // Page Down
        PresentKeyKind::Previous => &[104],            // Page Up
        PresentKeyKind::Start => &[63],                // F5 (PowerPoint, Impress)
        PresentKeyKind::End => &[1],                   // Esc
        PresentKeyKind::Blank => &[48],                // B
        PresentKeyKind::StartSlides => &[29, 63],      // Left Ctrl + F5 (Google Slides)
        PresentKeyKind::StartCanva => &[29, 56, 25],   // Left Ctrl + Left Alt + P (Canva)
        PresentKeyKind::StartFullscreen => &[87],      // F11
    }
}

fn xdotool_key(key: PresentKeyKind) -> &'static str {
    match key {
        PresentKeyKind::Next => "Next",
        PresentKeyKind::Previous => "Prior",
        PresentKeyKind::Start => "F5",
        PresentKeyKind::End => "Escape",
        PresentKeyKind::Blank => "b",
        PresentKeyKind::StartSlides => "ctrl+F5",
        PresentKeyKind::StartCanva => "ctrl+alt+p",
        PresentKeyKind::StartFullscreen => "F11",
    }
}

type Job = (PresentKeyKind, mpsc::Sender<Result<(), String>>);

/// Presses keys on one thread that owns the portal session.
#[derive(Clone)]
pub struct Presenter {
    tx: mpsc::Sender<Job>,
}

impl Presenter {
    /// `bus`: the session bus (tests: a private one). `token`: where the restore token lives.
    pub fn start(bus: Option<String>, token: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("phonelink-present".into())
            .spawn(move || {
                let mut session: Option<(Connection, OwnedObjectPath)> = None;
                while let Ok((key, reply)) = rx.recv() {
                    let _ = reply.send(press(&bus, &token, &mut session, key));
                }
            })
            .expect("spawn presenter thread");
        Self { tx }
    }

    /// Queue `key` now (keys are pressed in the order they were queued) and get its result
    /// later from the returned receiver: see [`Presenter::wait`].
    pub fn submit(&self, key: PresentKeyKind) -> mpsc::Receiver<Result<(), String>> {
        let (tx, rx) = mpsc::channel();
        if self.tx.send((key, tx.clone())).is_err() {
            let _ = tx.send(Err("the presenter stopped".into()));
        }
        rx
    }

    /// Wait for a queued key (the first one waits for "allow remote control", up to a minute).
    pub fn wait(rx: mpsc::Receiver<Result<(), String>>) -> Result<(), String> {
        rx.recv_timeout(Duration::from_secs(75)).map_err(|_| "no answer from the desktop".to_string())?
    }

    /// Press `key` and wait for it.
    pub fn press(&self, key: PresentKeyKind) -> Result<(), String> {
        Self::wait(self.submit(key))
    }
}

fn connect(bus: &Option<String>) -> zbus::Result<Connection> {
    match bus {
        Some(a) => connection::Builder::address(a.as_str())?.build(),
        None => Connection::session(),
    }
}

fn press(
    bus: &Option<String>,
    token: &PathBuf,
    session: &mut Option<(Connection, OwnedObjectPath)>,
    key: PresentKeyKind,
) -> Result<(), String> {
    for attempt in 0..2 {
        if session.is_none() {
            match open_session(bus, token) {
                Ok(s) => *session = Some(s),
                Err(e) if e.no_portal => return xdotool(key).map_err(|x| format!("{}; {x}", e.message)),
                Err(e) => return Err(e.message),
            }
        }
        let (c, handle) = session.as_ref().expect("session");
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        let send = |code: i32, state: u32| {
            c.call_method(Some(DEST), PATH, Some(IFACE), "NotifyKeyboardKeycode", &(handle, &options, code, state)).map(|_| ())
        };
        let chord = keys(key);
        let mut result = Ok(());
        let mut pressed = 0;
        for &code in chord {
            result = send(code, 1);
            if result.is_err() {
                break;
            }
            pressed += 1;
        }
        // Release what was pressed, even after an error, so no key stays held down.
        for &code in chord[..pressed].iter().rev() {
            let up = send(code, 0);
            if result.is_ok() {
                result = up;
            }
        }
        match result {
            Ok(_) => return Ok(()),
            Err(e) => {
                // The session was closed (revoked, desktop restarted): start a new one once.
                debug!("remote control session failed: {e}");
                *session = None;
                if attempt == 1 {
                    return Err(format!("the desktop refused the key: {e}"));
                }
            }
        }
    }
    Err("no remote control session".into())
}

struct OpenError {
    message: String,
    /// No RemoteDesktop portal at all (try xdotool instead).
    no_portal: bool,
}

fn fail(message: impl Into<String>) -> OpenError {
    OpenError { message: message.into(), no_portal: false }
}

/// The portal's request object path for `token` on connection `c`.
fn request_path(c: &Connection, token: &str) -> Result<String, OpenError> {
    let me = c.unique_name().ok_or_else(|| fail("no D-Bus name"))?;
    Ok(format!(
        "/org/freedesktop/portal/desktop/request/{}/{token}",
        me.as_str().trim_start_matches(':').replace('.', "_")
    ))
}

/// Call a portal method that answers through a Request object, and wait for its Response.
fn request(
    c: &Connection,
    method: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
    token: &str,
    wait: Duration,
) -> Result<HashMap<String, OwnedValue>, OpenError> {
    let path = request_path(c, token)?;
    // Subscribe first, so the answer can't be missed.
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.Request")
        .and_then(|b| b.member("Response"))
        .and_then(|b| b.path(path.as_str()))
        .map_err(|e| fail(e.to_string()))?
        .build();
    let it = MessageIterator::for_match_rule(rule, c, Some(4)).map_err(|e| fail(e.to_string()))?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for m in it {
            let Ok(m) = m else { break };
            if let Ok(r) = m.body().deserialize::<(u32, HashMap<String, OwnedValue>)>() {
                let _ = tx.send(r);
                break;
            }
        }
    });
    if let Err(e) = c.call_method(Some(DEST), PATH, Some(IFACE), method, body) {
        let no_portal = matches!(&e, zbus::Error::MethodError(name, _, _)
            if name.as_str().ends_with("ServiceUnknown") || name.as_str().ends_with("UnknownInterface") || name.as_str().ends_with("UnknownMethod") || name.as_str().ends_with("UnknownObject"));
        return Err(OpenError { message: format!("the desktop has no remote control portal ({e})"), no_portal });
    }
    match rx.recv_timeout(wait) {
        Ok((0, results)) => Ok(results),
        Ok((1, _)) => Err(fail("remote control was not allowed on the computer")),
        Ok((_, _)) => Err(fail("the desktop cancelled remote control")),
        Err(_) => Err(fail("no answer from the computer's desktop (was the permission dialog left open?)")),
    }
}

fn token() -> String {
    format!("echotwin{}", phonelink_core::MsgId::random().0[..6].iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn open_session(bus: &Option<String>, token_file: &PathBuf) -> Result<(Connection, OwnedObjectPath), OpenError> {
    let c = connect(bus).map_err(|e| OpenError { message: format!("no desktop session bus: {e}"), no_portal: true })?;

    let t = token();
    let opts: HashMap<&str, Value<'_>> = HashMap::from([
        ("handle_token", Value::from(t.as_str())),
        ("session_handle_token", Value::from(token())),
    ]);
    let r = request(&c, "CreateSession", &(opts,), &t, Duration::from_secs(10))?;
    let handle = r
        .get("session_handle")
        .and_then(|v| {
            String::try_from(v.try_clone().ok()?)
                .ok()
                .or_else(|| OwnedObjectPath::try_from(v.try_clone().ok()?).ok().map(|p| p.to_string()))
        })
        .and_then(|s| OwnedObjectPath::try_from(s).ok())
        .ok_or_else(|| fail("the desktop gave no remote control session"))?;

    let t = token();
    let saved = std::fs::read_to_string(token_file).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s.len() < 200);
    let mut opts: HashMap<&str, Value<'_>> = HashMap::from([
        ("handle_token", Value::from(t.as_str())),
        ("types", Value::from(DEVICE_KEYBOARD)),
        ("persist_mode", Value::from(PERSIST_UNTIL_REVOKED)),
    ]);
    if let Some(s) = &saved {
        opts.insert("restore_token", Value::from(s.as_str()));
    }
    request(&c, "SelectDevices", &(&handle, opts), &t, Duration::from_secs(10))?;

    let t = token();
    let opts: HashMap<&str, Value<'_>> = HashMap::from([("handle_token", Value::from(t.as_str()))]);
    // KDE asks "allow remote control?" here the first time: give the user a minute.
    let r = request(&c, "Start", &(&handle, "", opts), &t, Duration::from_secs(60))?;
    if let Some(new) = r.get("restore_token").and_then(|v| String::try_from(v.try_clone().ok()?).ok()) {
        save_token(token_file, &new);
    }
    info!("remote control session started for the presentation remote");
    Ok((c, handle))
}

fn save_token(path: &PathBuf, token: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(token.as_bytes()));
    if let Err(e) = written {
        warn!("could not save the remote control permission: {e}");
    }
}

fn xdotool(key: PresentKeyKind) -> Result<(), String> {
    if std::env::var_os("DISPLAY").is_none() {
        return Err("no way to press keys here".into());
    }
    let ok = std::process::Command::new("xdotool")
        .args(["key", xdotool_key(key)])
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        Ok(())
    } else {
        Err("install xdotool to use the presentation remote on X11".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicker_keys() {
        use PresentKeyKind::*;
        let all = [Next, Previous, Start, End, Blank, StartSlides, StartCanva, StartFullscreen];
        let want: [&[i32]; 8] = [&[109], &[104], &[63], &[1], &[48], &[29, 63], &[29, 56, 25], &[87]];
        assert_eq!(all.map(keys), want);
        assert_eq!(all.map(xdotool_key), ["Next", "Prior", "F5", "Escape", "b", "ctrl+F5", "ctrl+alt+p", "F11"]);
    }
}
