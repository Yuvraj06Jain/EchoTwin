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

// ---------------------------------------------------------------- the laptop as a remote

/// What plays on this computer, for the phone's Remote screen.
fn identity(c: &Connection, player: &str) -> String {
    let get = || -> Option<String> {
        let proxy = zbus::blocking::fdo::PropertiesProxy::builder(c)
            .destination(player.to_string())
            .ok()?
            .path(PATH)
            .ok()?
            .build()
            .ok()?;
        String::try_from(proxy.get("org.mpris.MediaPlayer2".try_into().ok()?, "Identity").ok()?).ok()
    };
    get().unwrap_or_else(|| player.trim_start_matches(PREFIX).split('.').next().unwrap_or("").to_string())
}

fn player_prop(c: &Connection, player: &str, name: &str) -> Option<zbus::zvariant::OwnedValue> {
    let proxy = zbus::blocking::fdo::PropertiesProxy::builder(c)
        .destination(player.to_string())
        .ok()?
        .path(PATH)
        .ok()?
        .build()
        .ok()?;
    proxy.get(PLAYER.try_into().ok()?, name).ok()
}

/// What plays, and its `mpris:artUrl` (empty if none). `art_key` is left empty here.
fn now_playing(c: &Connection, player: &str) -> Option<(phonelink_core::NowPlaying, String)> {
    use zbus::zvariant::OwnedValue;
    let status = status(c, player)?;
    if status == "Stopped" {
        return None;
    }
    let meta: std::collections::HashMap<String, OwnedValue> =
        player_prop(c, player, "Metadata").and_then(|v| v.try_into().ok()).unwrap_or_default();
    let text = |k: &str| meta.get(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
    let artist = meta
        .get("xesam:artist")
        .and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok())
        .map(|a| a.join(", "))
        .unwrap_or_default();
    let flag = |k: &str| player_prop(c, player, k).and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
    let cut = |s: String, n: usize| phonelink_core::message::truncate_text(&s, n);
    let art_url = text("mpris:artUrl");
    Some((phonelink_core::NowPlaying {
        app: cut(identity(c, player), 60),
        package: String::new(),
        title: cut(text("xesam:title"), 200),
        artist: cut(artist, 200),
        album: cut(text("xesam:album"), 200),
        duration_ms: 0,
        position_ms: 0,
        playing: status == "Playing",
        can_previous: flag("CanGoPrevious"),
        can_next: flag("CanGoNext"),
        volume: 0,
        max_volume: 0,
        art_key: String::new(),
    }, art_url))
}

/// Album art for the phone: (key, small JPEG).
pub type Art = (String, Vec<u8>);

/// Makes art once per picture file (by path, size and change time), not on every signal.
#[derive(Default)]
struct ArtCache {
    seen: Option<(String, Option<(u64, std::time::SystemTime)>)>,
    art: Option<Art>,
}

impl ArtCache {
    fn get(&mut self, url: &str) -> Option<Art> {
        let path = crate::art::art_path(url);
        let stamp = path.as_ref().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| Some((m.len(), m.modified().ok()?)));
        let id = (url.to_string(), stamp);
        if self.seen.as_ref() != Some(&id) {
            self.art = path.and_then(|p| crate::art::thumbnail(&p)).map(|jpeg| (crate::art::key(&jpeg), jpeg));
            self.seen = Some(id);
        }
        self.art.clone()
    }
}

/// The player that counts: one that is playing, else `prefer` (the last one shown) if it is
/// still around and paused, else the first paused one.
pub fn active_player(c: &Connection, prefer: Option<&str>) -> Option<String> {
    let list = players(c);
    let state: Vec<(String, Option<String>)> = list.iter().map(|p| (p.clone(), status(c, p))).collect();
    if let Some((p, _)) = state.iter().find(|(_, s)| s.as_deref() == Some("Playing")) {
        return Some(p.clone());
    }
    if let Some(p) = prefer.filter(|p| state.iter().any(|(q, s)| q == p && s.as_deref() == Some("Paused"))) {
        return Some(p.to_string());
    }
    state.into_iter().find(|(_, s)| s.as_deref() == Some("Paused")).map(|(p, _)| p)
}

/// Watches the session bus for players appearing, going and changing (signals only, no
/// polling) and reports what plays, with its album art, whenever it changes.
pub fn watch(bus: Option<String>, tx: tokio::sync::mpsc::UnboundedSender<(Option<phonelink_core::NowPlaying>, Option<Art>)>) {
    std::thread::Builder::new()
        .name("phonelink-mpris-watch".into())
        .spawn(move || {
            let c = match connect(&bus) {
                Ok(c) => c,
                Err(e) => {
                    debug!("no session bus for the media remote: {e}");
                    return;
                }
            };
            let rules = [
                zbus::MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .interface("org.freedesktop.DBus.Properties")
                    .and_then(|b| b.member("PropertiesChanged"))
                    .and_then(|b| b.path(PATH))
                    .map(|b| b.build()),
                zbus::MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .interface("org.freedesktop.DBus")
                    .and_then(|b| b.member("NameOwnerChanged"))
                    .map(|b| b.build()),
            ];
            let (tick_tx, tick_rx) = mpsc::channel::<()>();
            for rule in rules.into_iter().flatten() {
                let Ok(it) = zbus::blocking::MessageIterator::for_match_rule(rule, &c, Some(64)) else { continue };
                let tick = tick_tx.clone();
                std::thread::spawn(move || {
                    for m in it {
                        let Ok(m) = m else { break };
                        // Only media players' names.
                        if m.header().member().map(|x| x.as_str()) == Some("NameOwnerChanged") {
                            let ok = m.body().deserialize::<(String, String, String)>().is_ok_and(|(n, _, _)| n.starts_with(PREFIX));
                            if !ok {
                                continue;
                            }
                        }
                        if tick.send(()).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(tick_tx);
            let mut shown: Option<String> = None;
            let mut last: Option<Option<phonelink_core::NowPlaying>> = None;
            let mut cache = ArtCache::default();
            loop {
                let player = active_player(&c, shown.as_deref());
                let (now, art) = match player.as_deref().and_then(|p| now_playing(&c, p)) {
                    Some((mut now, url)) => {
                        let art = cache.get(&url);
                        now.art_key = art.as_ref().map(|a| a.0.clone()).unwrap_or_default();
                        (Some(now), art)
                    }
                    None => (None, None),
                };
                shown = player;
                if last.as_ref() != Some(&now) {
                    if tx.send((now.clone(), art)).is_err() {
                        return;
                    }
                    last = Some(now);
                }
                // Wait for the next change, then let a burst of signals settle.
                if tick_rx.recv().is_err() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
                while tick_rx.try_recv().is_ok() {}
            }
        })
        .expect("spawn mpris watch thread");
}

/// Play/pause, next, previous on the active player; volume and mute of the default output.
pub fn control(bus: &Option<String>, action: phonelink_core::MediaAction, effects: &dyn crate::effects::Effects) -> Result<(), String> {
    use phonelink_core::MediaAction::*;
    let done = |ok: bool, what: &str| if ok { Ok(()) } else { Err(format!("couldn't {what}")) };
    match action {
        VolumeUp => return done(effects.volume("5%+"), "change the volume"),
        VolumeDown => return done(effects.volume("5%-"), "change the volume"),
        ToggleMute => return done(effects.mute_toggle(), "mute the computer"),
        SetVolume(_) => return Ok(()), // the phone shows − / + for the computer
        _ => {}
    }
    let c = connect(bus).map_err(|e| e.to_string())?;
    let player = active_player(&c, None).ok_or("nothing is playing on the computer")?;
    let method = match action {
        PlayPause => "PlayPause",
        Next => "Next",
        _ => "Previous",
    };
    if call(&c, &player, method) {
        Ok(())
    } else {
        Err("the player didn't respond".into())
    }
}
