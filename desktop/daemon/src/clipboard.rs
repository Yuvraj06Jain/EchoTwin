//! This computer's clipboard, through KDE Plasma's clipboard service (Klipper) on D-Bus.
//!
//! * Works on Wayland without window focus (Klipper is part of the desktop shell).
//! * Copies from password managers (KeePassXC, …) carry KDE's password hint; Klipper keeps
//!   them out of its history, so they are never seen here and never sent to the phone.
//! * Text we put on the clipboard ourselves (from the phone) is not reported back as a new
//!   copy, so nothing bounces between the devices.
//!
//! When Klipper is not running (another desktop, or the clipboard widget removed), this
//! keeps checking every few seconds and starts working as soon as it appears.

use std::{sync::mpsc, time::Duration};

use tracing::{debug, info};
use zbus::{
    blocking::{connection, Connection, MessageIterator},
    message::Type,
    MatchRule,
};

const DEST: &str = "org.kde.klipper";
const PATH: &str = "/klipper";
const IFACE: &str = "org.kde.klipper.klipper";
const RETRY: Duration = Duration::from_secs(5);

enum Cmd {
    Set(String),
    Changed,
    Lost,
}

/// Handle to the clipboard thread.
#[derive(Clone)]
pub struct ClipboardSync {
    tx: mpsc::Sender<Cmd>,
}

impl ClipboardSync {
    /// `bus`: a D-Bus address (tests), or `None` for the session bus. Each new local copy
    /// (text) is sent to `copies`.
    pub fn start(bus: Option<String>, copies: tokio::sync::mpsc::UnboundedSender<String>) -> Self {
        let (tx, rx) = mpsc::channel();
        let me = tx.clone();
        std::thread::Builder::new()
            .name("phonelink-clipboard".into())
            .spawn(move || run(bus, rx, me, copies))
            .expect("spawn clipboard thread");
        Self { tx }
    }

    /// Put `text` on this computer's clipboard.
    pub fn set(&self, text: String) {
        let _ = self.tx.send(Cmd::Set(text));
    }
}

fn connect(bus: &Option<String>) -> zbus::Result<Connection> {
    match bus {
        Some(a) => connection::Builder::address(a.as_str())?.build(),
        None => Connection::session(),
    }
}

fn klipper_running(c: &Connection) -> bool {
    let Ok(proxy) = zbus::blocking::fdo::DBusProxy::new(c) else {
        return false;
    };
    proxy
        .name_has_owner(DEST.try_into().expect("valid bus name"))
        .unwrap_or(false)
}

fn read(c: &Connection) -> zbus::Result<String> {
    c.call_method(Some(DEST), PATH, Some(IFACE), "getClipboardContents", &())?
        .body()
        .deserialize::<String>()
}

fn write(c: &Connection, text: &str) -> zbus::Result<()> {
    c.call_method(Some(DEST), PATH, Some(IFACE), "setClipboardContents", &(text,))
        .map(|_| ())
}

/// Forward Klipper's "history changed" signal to the clipboard thread.
fn listen(c: &Connection, tx: mpsc::Sender<Cmd>) -> zbus::Result<()> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(IFACE)?
        .member("clipboardHistoryUpdated")?
        .path(PATH)?
        .build();
    let signals = MessageIterator::for_match_rule(rule, c, Some(64))?;
    std::thread::Builder::new()
        .name("phonelink-clipboard-signals".into())
        .spawn(move || {
            for msg in signals {
                if msg.is_err() || tx.send(Cmd::Changed).is_err() {
                    break;
                }
            }
            let _ = tx.send(Cmd::Lost);
        })
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;
    Ok(())
}

fn run(
    bus: Option<String>,
    rx: mpsc::Receiver<Cmd>,
    me: mpsc::Sender<Cmd>,
    copies: tokio::sync::mpsc::UnboundedSender<String>,
) {
    let mut conn: Option<Connection> = None;
    let mut logged_missing = false;
    // What the clipboard held last time we looked; a signal only counts if it changed.
    let mut current: Option<String> = None;
    loop {
        if conn.is_none() {
            match connect(&bus) {
                Ok(c) if klipper_running(&c) => match listen(&c, me.clone()) {
                    Ok(()) => {
                        info!("clipboard sharing ready (KDE clipboard)");
                        logged_missing = false;
                        current = read(&c).ok(); // what was there before we started: not a new copy
                        conn = Some(c);
                    }
                    Err(e) => debug!("clipboard signals unavailable: {e}"),
                },
                Ok(_) => {
                    if !logged_missing {
                        info!("KDE clipboard service not running; clipboard sharing waits for it");
                        logged_missing = true;
                    }
                }
                Err(e) => debug!("no session bus for the clipboard: {e}"),
            }
        }
        let cmd = match rx.recv_timeout(RETRY) {
            Ok(cmd) => cmd,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let Some(c) = conn.as_ref() else {
            continue; // a Set while Klipper is away is dropped: there is nowhere to put it
        };
        match cmd {
            Cmd::Set(text) => {
                // Mark it as ours first: the signal it triggers must not echo it back.
                current = Some(text.clone());
                if let Err(e) = write(c, &text) {
                    debug!("could not set the clipboard: {e}");
                    conn = None;
                }
            }
            Cmd::Changed => match read(c) {
                Ok(text) => {
                    if current.as_deref() != Some(text.as_str()) {
                        current = Some(text.clone());
                        if !text.is_empty() && copies.send(text).is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    debug!("could not read the clipboard: {e}");
                    conn = None;
                }
            },
            Cmd::Lost => conn = None,
        }
    }
}
