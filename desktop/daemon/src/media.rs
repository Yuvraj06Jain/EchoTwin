//! What is playing on each connected phone, kept in memory only.
//!
//! The phone sends its state on every change; while something plays, the position is
//! advanced here from the time it arrived, so no clock agreement between the devices is
//! needed.

use std::{collections::HashMap, time::Instant};

use base64::Engine;
use phonelink_core::{
    message::{is_art_image, truncate_text},
    Fingerprint, NowPlaying,
};
use serde::{Deserialize, Serialize};

/// One phone's player, as the UI and CLI see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaInfo {
    pub device: String,
    pub device_name: String,
    pub app: String,
    pub package: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    /// Where playback is now (advanced locally while playing).
    pub position_ms: u64,
    pub playing: bool,
    pub can_previous: bool,
    pub can_next: bool,
    pub volume: u32,
    pub max_volume: u32,
    /// Album art as a `data:` URL.
    pub art: Option<String>,
}

struct Entry {
    now: NowPlaying,
    received: Instant,
}

#[derive(Default)]
pub struct MediaStore {
    playing: HashMap<Fingerprint, Entry>,
    /// Latest art per phone (key, image). Can arrive just before the state that names it.
    art: HashMap<Fingerprint, (String, Vec<u8>)>,
}

fn clean(mut n: NowPlaying) -> NowPlaying {
    n.app = truncate_text(&n.app, 100);
    n.package = truncate_text(&n.package, 200);
    n.title = truncate_text(&n.title, 300);
    n.artist = truncate_text(&n.artist, 300);
    n.album = truncate_text(&n.album, 300);
    n.art_key = truncate_text(&n.art_key, 100);
    n.max_volume = n.max_volume.min(1000);
    n.volume = n.volume.min(n.max_volume);
    n
}

impl MediaStore {
    /// Returns whether anything changed.
    pub fn update(&mut self, device: Fingerprint, now: Option<NowPlaying>, at: Instant) -> bool {
        match now {
            Some(n) => {
                let n = clean(n);
                let same = self.playing.get(&device).is_some_and(|e| e.now == n);
                self.playing.insert(device, Entry { now: n, received: at });
                !same
            }
            None => self.playing.remove(&device).is_some(),
        }
    }

    /// Store album art; false if it isn't a small JPEG/PNG.
    pub fn set_art(&mut self, device: Fingerprint, key: String, image: Vec<u8>) -> bool {
        if key.is_empty() || key.len() > 100 || !is_art_image(&image) {
            return false;
        }
        self.art.insert(device, (key, image));
        true
    }

    pub fn forget(&mut self, device: &Fingerprint) -> bool {
        self.art.remove(device);
        self.playing.remove(device).is_some()
    }

    pub fn max_volume(&self, device: &Fingerprint) -> Option<u32> {
        self.playing.get(device).map(|e| e.now.max_volume)
    }

    pub fn view(&self, device: &Fingerprint, device_name: &str, at: Instant) -> Option<MediaInfo> {
        let e = self.playing.get(device)?;
        let n = &e.now;
        let mut position = n.position_ms;
        if n.playing {
            position = position.saturating_add(at.saturating_duration_since(e.received).as_millis() as u64);
        }
        if n.duration_ms > 0 {
            position = position.min(n.duration_ms);
        }
        let art = self
            .art
            .get(device)
            .filter(|(k, _)| !n.art_key.is_empty() && *k == n.art_key)
            .map(|(_, img)| {
                let mime = if img.starts_with(&[0xFF, 0xD8]) { "image/jpeg" } else { "image/png" };
                format!(
                    "data:{mime};base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(img)
                )
            });
        Some(MediaInfo {
            device: device.short(),
            device_name: device_name.to_string(),
            app: n.app.clone(),
            package: n.package.clone(),
            title: n.title.clone(),
            artist: n.artist.clone(),
            album: n.album.clone(),
            duration_ms: n.duration_ms,
            position_ms: position,
            playing: n.playing,
            can_previous: n.can_previous,
            can_next: n.can_next,
            volume: n.volume,
            max_volume: n.max_volume,
            art,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn song(playing: bool) -> NowPlaying {
        NowPlaying {
            app: "Spotify".into(),
            package: "com.spotify.music".into(),
            title: "Kesariya".into(),
            artist: "Arijit Singh".into(),
            album: "Brahmastra".into(),
            duration_ms: 100_000,
            position_ms: 10_000,
            playing,
            can_previous: true,
            can_next: true,
            volume: 30,
            max_volume: 15,
            art_key: "k1".into(),
        }
    }

    #[test]
    fn position_advances_only_while_playing_and_stops_at_the_end() {
        let d = Fingerprint([1; 32]);
        let mut s = MediaStore::default();
        let t0 = Instant::now();
        assert!(s.update(d, Some(song(true)), t0));
        assert!(!s.update(d, Some(song(true)), t0), "same state: no change");
        let v = s.view(&d, "S24", t0 + Duration::from_secs(5)).unwrap();
        assert_eq!(v.position_ms, 15_000);
        assert_eq!(v.volume, 15, "volume clamped to the maximum");
        assert_eq!(s.view(&d, "S24", t0 + Duration::from_secs(500)).unwrap().position_ms, 100_000);
        s.update(d, Some(song(false)), t0);
        assert_eq!(s.view(&d, "S24", t0 + Duration::from_secs(5)).unwrap().position_ms, 10_000);
        assert!(s.update(d, None, t0));
        assert!(s.view(&d, "S24", t0).is_none());
    }

    #[test]
    fn art_must_be_an_image_and_match_the_current_song() {
        let d = Fingerprint([2; 32]);
        let mut s = MediaStore::default();
        let t0 = Instant::now();
        assert!(!s.set_art(d, "k1".into(), b"<svg/>".to_vec()));
        assert!(s.set_art(d, "k1".into(), vec![0xFF, 0xD8, 0xFF, 0xE0]));
        s.update(d, Some(song(true)), t0);
        let art = s.view(&d, "S24", t0).unwrap().art.unwrap();
        assert!(art.starts_with("data:image/jpeg;base64,"), "{art}");
        let mut other = song(true);
        other.art_key = "k2".into(); // next song, its art not here yet
        s.update(d, Some(other), t0);
        assert!(s.view(&d, "S24", t0).unwrap().art.is_none(), "no stale art");
        assert!(s.forget(&d));
        assert!(s.view(&d, "S24", t0).is_none());
    }
}
