//! Ringing this computer from the phone ("where did I leave my laptop?"): a two-tone
//! sound looped for at most [`MAX_RING`], loud and unmuted, then the volume is put back.
//!
//! No new dependency: the sound is a small WAV written once, played with whichever of
//! `pw-play`, `paplay` or `aplay` exists. The volume is set with `wpctl` when available.

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tracing::{debug, info, warn};

pub const MAX_RING: Duration = Duration::from_secs(60);
const PLAYERS: &[&str] = &["pw-play", "paplay", "aplay"];
const RATE: u32 = 22_050;
const SINK: &str = "@DEFAULT_AUDIO_SINK@";

/// One second of sound: two alternating tones, then a short pause.
pub fn ring_wav() -> Vec<u8> {
    let mut samples: Vec<i16> = Vec::with_capacity(RATE as usize);
    for i in 0..RATE {
        let t = i as f32 / RATE as f32;
        let beep = (t * 8.0) as u32; // eighths of a second
        let v = if beep < 6 {
            let f = if beep.is_multiple_of(2) { 880.0 } else { 1320.0 };
            // A soft start/end on each beep avoids clicks.
            let pos = t * 8.0 - beep as f32;
            let env = (pos * 20.0).min(1.0).min((1.0 - pos) * 20.0);
            (t * f * std::f32::consts::TAU).sin() * env * 0.8
        } else {
            0.0
        };
        samples.push((v * i16::MAX as f32) as i16);
    }
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    for (k, v) in crate::popups::session_env() {
        if std::env::var_os(&k).is_none() {
            cmd.env(k, v);
        }
    }
    cmd
}

/// The default output's volume and mute state, as `wpctl get-volume` prints them.
fn read_volume(wpctl: &Path) -> Option<(String, bool)> {
    let out = command(wpctl).args(["get-volume", SINK]).stdout(Stdio::piped()).output().ok()?;
    parse_volume(&String::from_utf8_lossy(&out.stdout))
}

/// "Volume: 0.40 [MUTED]" → ("0.40", true).
pub fn parse_volume(s: &str) -> Option<(String, bool)> {
    let v = s.trim().strip_prefix("Volume:")?.split_whitespace().next()?;
    v.parse::<f32>().ok()?;
    Some((v.to_string(), s.contains("[MUTED]")))
}

fn wpctl(wpctl: &Path, args: &[&str]) {
    if let Err(e) = command(wpctl).args(args).status() {
        debug!("wpctl {args:?}: {e}");
    }
}

#[derive(Default)]
struct State {
    /// Bumped by every start/stop; a loop that sees a different value ends.
    generation: u64,
    child: Option<Child>,
    /// What to put back when ringing ends.
    restore: Option<(String, bool)>,
}

#[derive(Clone)]
pub struct Ringer {
    sound: PathBuf,
    state: Arc<Mutex<State>>,
}

impl Ringer {
    /// `dir`: where the sound file is kept (the data directory).
    pub fn new(dir: &Path) -> Self {
        Self {
            sound: dir.join("ring.wav"),
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub fn is_ringing(&self) -> bool {
        lock(&self.state).child.is_some()
    }

    pub fn start(&self) -> Result<(), String> {
        let player = PLAYERS
            .iter()
            .find_map(|p| find_program(p))
            .ok_or("no sound player found (pw-play, paplay or aplay)")?;
        if !self.sound.exists() {
            std::fs::write(&self.sound, ring_wav()).map_err(|e| e.to_string())?;
        }
        let generation = {
            let mut s = lock(&self.state);
            if s.child.is_some() {
                return Ok(()); // already ringing
            }
            s.generation += 1;
            if let Some(w) = find_program("wpctl") {
                if s.restore.is_none() {
                    s.restore = read_volume(&w);
                }
                wpctl(&w, &["set-mute", SINK, "0"]);
                wpctl(&w, &["set-volume", SINK, "0.9"]);
            }
            s.generation
        };
        info!("ringing this computer");
        let (state, sound) = (self.state.clone(), self.sound.clone());
        std::thread::Builder::new()
            .name("phonelink-ring".into())
            .spawn(move || ring_loop(state, generation, &player, &sound))
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn stop(&self) {
        let mut s = lock(&self.state);
        s.generation += 1;
        stop_locked(&mut s);
    }
}

fn stop_locked(s: &mut State) {
    if let Some(mut c) = s.child.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    if let Some((volume, muted)) = s.restore.take() {
        if let Some(w) = find_program("wpctl") {
            wpctl(&w, &["set-volume", SINK, &volume]);
            if muted {
                wpctl(&w, &["set-mute", SINK, "1"]);
            }
        }
    }
}

fn ring_loop(state: Arc<Mutex<State>>, generation: u64, player: &Path, sound: &Path) {
    let started = Instant::now();
    loop {
        {
            let mut s = lock(&state);
            if s.generation != generation {
                return;
            }
            if started.elapsed() >= MAX_RING {
                stop_locked(&mut s);
                return;
            }
            let reaped = s.child.as_mut().map(|c| matches!(c.try_wait(), Ok(Some(_)) | Err(_)));
            if reaped != Some(false) {
                // Not started yet, or the last play finished: play again.
                let mut cmd = command(player);
                cmd.arg(sound);
                std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
                match cmd.spawn() {
                    Ok(c) => s.child = Some(c),
                    Err(e) => {
                        warn!("could not play the ring sound: {e}");
                        stop_locked(&mut s);
                        return;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_sound_is_a_valid_wav() {
        let w = ring_wav();
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()) as usize, w.len() - 8);
        assert_eq!(&w[36..40], b"data");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize, w.len() - 44);
        assert_eq!(w.len(), 44 + RATE as usize * 2, "one second, 16-bit mono");
        let loudest = w[44..].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs()).max();
        assert!(loudest.unwrap() > 20_000, "loud enough");
    }

    #[test]
    fn reads_wpctl_volume() {
        assert_eq!(parse_volume("Volume: 0.40\n"), Some(("0.40".into(), false)));
        assert_eq!(parse_volume("Volume: 1.00 [MUTED]\n"), Some(("1.00".into(), true)));
        assert_eq!(parse_volume("garbage"), None);
        assert_eq!(parse_volume("Volume: ; rm -rf"), None);
    }
}
