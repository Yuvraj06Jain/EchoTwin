//! Media players on this computer (MPRIS on the session bus): pausing what plays while
//! the phone has a call, and resuming exactly those players afterwards.
//!
//! All calls run on one thread, in order, so a quick "ringing, then hung up" can never
//! resume before the pause has finished.

use std::sync::mpsc;

use tracing::{debug, info};
use zbus::blocking::{connection, Connection};

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

fn connect(bus: &Option<String>) -> zbus::Result<Connection> {
    match bus {
        Some(a) => connection::Builder::address(a.as_str())?.build(),
        None => Connection::session(),
    }
}

/// Bus names of the running media players.
pub fn players(c: &Connection) -> Vec<String> {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(c) else {
        return vec![];
    };
    dbus.list_names()
        .map(|names| {
            names
                .into_iter()
                .map(|n| n.to_string())
                .filter(|n| n.starts_with(PREFIX))
                .collect()
        })
        .unwrap_or_default()
}

/// "Playing", "Paused" or "Stopped".
pub fn status(c: &Connection, player: &str) -> Option<String> {
    let proxy = zbus::blocking::fdo::PropertiesProxy::builder(c)
        .destination(player.to_string())
        .ok()?
        .path(PATH)
        .ok()?
        .build()
        .ok()?;
    let v = proxy.get(PLAYER.try_into().ok()?, "PlaybackStatus").ok()?;
    String::try_from(v).ok()
}

pub fn call(c: &Connection, player: &str, method: &str) -> bool {
    c.call_method(Some(player), PATH, Some(PLAYER), method, &())
        .map_err(|e| debug!(player, method, "media player call failed: {e}"))
        .is_ok()
}

/// Pauses media for calls. `set(true)` pauses everything that is playing (once);
/// `set(false)` resumes those players if they are still paused.
#[derive(Clone)]
pub struct CallPause {
    tx: mpsc::Sender<bool>,
}

impl CallPause {
    pub fn start(bus: Option<String>) -> Self {
        let (tx, rx) = mpsc::channel::<bool>();
        std::thread::Builder::new()
            .name("phonelink-mpris".into())
            .spawn(move || {
                // Some(list) while a call has paused players.
                let mut paused: Option<Vec<String>> = None;
                while let Ok(pause) = rx.recv() {
                    let c = match connect(&bus) {
                        Ok(c) => c,
                        Err(e) => {
                            debug!("no session bus for media players: {e}");
                            continue;
                        }
                    };
                    if pause {
                        if paused.is_some() {
                            continue;
                        }
                        let list: Vec<String> = players(&c)
                            .into_iter()
                            .filter(|p| status(&c, p).as_deref() == Some("Playing"))
                            .filter(|p| call(&c, p, "Pause"))
                            .collect();
                        if !list.is_empty() {
                            info!(count = list.len(), "paused media for a call");
                        }
                        paused = Some(list);
                    } else if let Some(list) = paused.take() {
                        for p in list {
                            // Not if the user started or stopped it during the call.
                            if status(&c, &p).as_deref() == Some("Paused") {
                                call(&c, &p, "Play");
                            }
                        }
                    }
                }
            })
            .expect("spawn mpris thread");
        Self { tx }
    }

    pub fn set(&self, pause: bool) {
        let _ = self.tx.send(pause);
    }
}
