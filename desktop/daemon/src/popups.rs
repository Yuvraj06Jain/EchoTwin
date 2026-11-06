//! Desktop popups through the freedesktop notification D-Bus API. Clicking one opens the
//! PhoneLink window.
//!
//! All popups go through one long-lived D-Bus connection on one thread, so:
//! * an update replaces the earlier popup and a removal on the phone closes it;
//! * the click comes back to us. KDE Plasma sends `ActionInvoked` only to the connection
//!   that showed the popup, so a fresh connection per popup would never hear the click.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
};

use tracing::{debug, info, warn};
use zbus::{
    blocking::{Connection, MessageIterator},
    message::Type,
    zvariant::Value,
    MatchRule,
};

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const IFACE: &str = "org.freedesktop.Notifications";
/// Popups this program showed, so a click on an older one (in the notification history)
/// still works, and clicks on other apps' popups are ignored.
const REMEMBERED_IDS: usize = 500;
/// Popups whose handle we keep for updates/closing.
const MAX_OPEN: usize = 200;
/// The freedesktop action invoked by clicking the popup itself.
const DEFAULT_ACTION: &str = "default";

pub enum PopupCmd {
    Show {
        /// Our id: an update with the same id replaces the popup.
        id: String,
        app: String,
        /// Already markup-escaped.
        title: String,
        body: String,
        icon: Option<PathBuf>,
    },
    Close {
        id: String,
    },
}

/// What a click does. Gets the activation token if the server sent one (needed on
/// Wayland for the window to be allowed to take focus).
pub type Opener = Arc<dyn Fn(Option<String>) + Send + Sync>;

#[derive(Clone)]
pub struct Popups {
    tx: mpsc::Sender<PopupCmd>,
}

impl Popups {
    /// Popups on the session bus; a click opens the PhoneLink window.
    pub fn start() -> Self {
        Self::start_with(None, Arc::new(open_app))
    }

    /// `bus`: a D-Bus address (tests), or `None` for the session bus.
    pub fn start_with(bus: Option<String>, opener: Opener) -> Self {
        let (tx, rx) = mpsc::channel::<PopupCmd>();
        std::thread::Builder::new()
            .name("phonelink-popups".into())
            .spawn(move || popup_thread(rx, bus, opener))
            .expect("spawn popup thread");
        Self { tx }
    }

    /// For tests: a sink that records commands instead of talking to D-Bus.
    pub fn recording() -> (Self, mpsc::Receiver<PopupCmd>) {
        let (tx, rx) = mpsc::channel();
        (Self { tx }, rx)
    }

    pub fn send(&self, cmd: PopupCmd) {
        let _ = self.tx.send(cmd);
    }
}

#[derive(Default)]
struct Ours {
    order: VecDeque<u32>,
    ids: HashSet<u32>,
    tokens: HashMap<u32, String>,
}

impl Ours {
    fn remember(&mut self, id: u32) {
        if self.ids.insert(id) {
            self.order.push_back(id);
        }
        while self.order.len() > REMEMBERED_IDS {
            if let Some(old) = self.order.pop_front() {
                self.ids.remove(&old);
                self.tokens.remove(&old);
            }
        }
    }
}

fn connect(bus: &Option<String>) -> zbus::Result<Connection> {
    match bus {
        Some(addr) => zbus::blocking::connection::Builder::address(addr.as_str())?.build(),
        None => Connection::session(),
    }
}

fn popup_thread(rx: mpsc::Receiver<PopupCmd>, bus: Option<String>, opener: Opener) {
    let ours = Arc::new(Mutex::new(Ours::default()));
    let mut conn: Option<Connection> = None;
    // Our id -> the server's id, for updates and closing.
    let mut open: HashMap<String, u32> = HashMap::new();
    let mut order: VecDeque<String> = VecDeque::new();
    while let Ok(cmd) = rx.recv() {
        // (Re)connect lazily: the session bus may not be up yet, or may have restarted.
        if conn.is_none() {
            match connect(&bus) {
                Ok(c) => {
                    if let Err(e) = start_listener(c.clone(), ours.clone(), opener.clone()) {
                        warn!("popup clicks will not open the window: {e}");
                    }
                    open.clear();
                    order.clear();
                    conn = Some(c);
                }
                Err(e) => {
                    debug!("no D-Bus session for popups: {e}");
                    continue;
                }
            }
        }
        let Some(c) = conn.as_ref() else { continue };
        let result = match cmd {
            PopupCmd::Show {
                id,
                app,
                title,
                body,
                icon,
            } => {
                let icon = icon
                    .filter(|p| p.exists())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "smartphone".into());
                let replaces = open.get(&id).copied().unwrap_or(0);
                notify(c, &app, replaces, &icon, &title, &body).map(|server_id| {
                    lock(&ours).remember(server_id);
                    if open.insert(id.clone(), server_id).is_none() {
                        order.push_back(id);
                    }
                    while order.len() > MAX_OPEN {
                        if let Some(old) = order.pop_front() {
                            open.remove(&old); // just forget it; the server expires it
                        }
                    }
                })
            }
            PopupCmd::Close { id } => match open.remove(&id) {
                Some(server_id) => {
                    order.retain(|x| x != &id);
                    c.call_method(Some(DEST), PATH, Some(IFACE), "CloseNotification", &(server_id,))
                        .map(|_| ())
                }
                None => Ok(()),
            },
        };
        if let Err(e) = result {
            debug!("popup failed: {e}");
            if matches!(e, zbus::Error::InputOutput(_)) {
                conn = None; // bus went away; reconnect on the next popup
            }
        }
    }
}

fn notify(
    c: &Connection,
    app: &str,
    replaces: u32,
    icon: &str,
    summary: &str,
    body: &str,
) -> zbus::Result<u32> {
    // "default" = clicking the popup itself; servers don't show it as a button.
    let actions = vec![DEFAULT_ACTION, "Open PhoneLink"];
    let hints: HashMap<&str, Value<'_>> = HashMap::new();
    c.call_method(
        Some(DEST),
        PATH,
        Some(IFACE),
        "Notify",
        &(app, replaces, icon, summary, body, actions, hints, -1i32),
    )?
    .body()
    .deserialize::<u32>()
}

fn start_listener(conn: Connection, ours: Arc<Mutex<Ours>>, opener: Opener) -> zbus::Result<()> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(IFACE)?
        .path(PATH)?
        .build();
    // Subscribing before any popup is shown, so no click can be missed.
    let signals = MessageIterator::for_match_rule(rule, &conn, Some(64))?;
    std::thread::Builder::new()
        .name("phonelink-popup-clicks".into())
        .spawn(move || {
            for msg in signals {
                let Ok(msg) = msg else { break };
                let header = msg.header();
                match header.member().map(|m| m.as_str()) {
                    // Sent just before ActionInvoked by servers that support activation.
                    Some("ActivationToken") => {
                        if let Ok((id, token)) = msg.body().deserialize::<(u32, String)>() {
                            let mut o = lock(&ours);
                            if o.ids.contains(&id) {
                                o.tokens.insert(id, token);
                            }
                        }
                    }
                    Some("ActionInvoked") => {
                        let Ok((id, action)) = msg.body().deserialize::<(u32, String)>() else {
                            continue;
                        };
                        let token = {
                            let mut o = lock(&ours);
                            if action != DEFAULT_ACTION || !o.ids.contains(&id) {
                                continue;
                            }
                            o.tokens.remove(&id)
                        };
                        info!("popup clicked; opening the PhoneLink window");
                        opener(token);
                    }
                    _ => {}
                }
            }
            debug!("popup click listener ended");
        })
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;
    Ok(())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------------ opening the window

/// The desktop app next to this binary (both live in ~/.local/bin), else from PATH.
fn app_exe() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("phonelink-app")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("phonelink-app"))
}

/// Variables a GUI program needs that a systemd user service may have started without:
/// the service can start before the desktop session exists, while the session later
/// exports them to the user manager.
const SESSION_VARS: &[&str] = &[
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XAUTHORITY",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
];

/// `systemctl --user show-environment` output → the [`SESSION_VARS`] it sets.
pub fn parse_session_env(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| l.split_once('='))
        // `KEY=$'…'` is systemd's quoting for unusual values; none of ours need it.
        .filter(|(k, v)| SESSION_VARS.contains(k) && !v.starts_with("$'") && !v.is_empty())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn session_env() -> Vec<(String, String)> {
    Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_session_env(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// Arguments for the desktop app: show the notifications, with the activation token.
pub fn app_args(token: Option<&str>) -> Vec<String> {
    let mut args = vec!["--show=notifications".to_string()];
    if let Some(t) = token.filter(|t| valid_token(t)) {
        args.push(format!("--activation-token={t}"));
    }
    args
}

/// Tokens are opaque, but only ever printable ASCII; refuse anything else.
fn valid_token(t: &str) -> bool {
    !t.is_empty() && t.len() <= 512 && t.bytes().all(|b| b.is_ascii_graphic())
}

/// Launch the window, or focus it if it is already open (the app is single-instance).
pub fn open_app(token: Option<String>) {
    let exe = app_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(app_args(token.as_deref()));
    for (k, v) in session_env() {
        if std::env::var_os(&k).is_none() {
            cmd.env(k, v);
        }
    }
    if let Some(t) = token.as_deref().filter(|t| valid_token(t)) {
        cmd.env("XDG_ACTIVATION_TOKEN", t).env("DESKTOP_STARTUP_ID", t);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    match cmd.spawn() {
        Ok(mut child) => {
            // Reap it so it never lingers as a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => warn!("could not open the PhoneLink window ({}): {e}", exe.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_env_takes_only_display_variables() {
        let out = "HOME=/home/y\nWAYLAND_DISPLAY=wayland-0\nDISPLAY=:0\nXDG_CURRENT_DESKTOP=KDE\n\
                   LD_PRELOAD=/evil.so\nXAUTHORITY=$'/tmp/odd\\npath'\nPATH=/usr/bin\nXDG_SESSION_TYPE=\n";
        assert_eq!(
            parse_session_env(out),
            vec![
                ("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string()),
                ("DISPLAY".to_string(), ":0".to_string()),
                ("XDG_CURRENT_DESKTOP".to_string(), "KDE".to_string()),
            ]
        );
    }

    #[test]
    fn app_arguments_carry_only_sane_tokens() {
        assert_eq!(app_args(None), vec!["--show=notifications"]);
        assert_eq!(
            app_args(Some("kwin-42_abc")),
            vec!["--show=notifications", "--activation-token=kwin-42_abc"]
        );
        assert_eq!(app_args(Some("a b")), vec!["--show=notifications"]);
        assert_eq!(app_args(Some("")), vec!["--show=notifications"]);
        assert_eq!(app_args(Some("x\n--demo")), vec!["--show=notifications"]);
    }
}
