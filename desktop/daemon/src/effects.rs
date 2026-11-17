//! Things the daemon does to the desktop on the phone's behalf (open a link, ring). Behind
//! a trait so tests can record them instead of opening a browser or making noise.

use std::sync::{Arc, Mutex};

pub trait Effects: Send + Sync {
    /// Open a web link (already checked with `is_web_link`) in the default browser.
    fn open_url(&self, url: &str) -> bool;
    /// Start or stop ringing this computer (stops by itself after `ring::MAX_RING`).
    fn ring(&self, on: bool) -> Result<(), String>;
    /// Run one of the user's own commands (`sh -c`), not waiting for it.
    fn run_command(&self, run: &str) -> Result<(), String>;
    /// Change the default output's volume, e.g. "5%+".
    fn volume(&self, change: &str) -> bool;
    /// Mute or unmute the default output.
    fn mute_toggle(&self) -> bool;
}

/// The real desktop.
pub struct System {
    ringer: crate::ring::Ringer,
}

impl System {
    /// `data_dir`: where the ring sound is kept.
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self { ringer: crate::ring::Ringer::new(data_dir) }
    }
}

impl Effects for System {
    fn open_url(&self, url: &str) -> bool {
        crate::open::open_url(url)
    }

    fn ring(&self, on: bool) -> Result<(), String> {
        if on {
            self.ringer.start()
        } else {
            self.ringer.stop();
            Ok(())
        }
    }

    fn run_command(&self, run: &str) -> Result<(), String> {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(run);
        crate::open::detach(cmd)
    }

    fn volume(&self, change: &str) -> bool {
        let wpctl = std::process::Command::new("wpctl")
            .args(["set-volume", "-l", "1.5", "@DEFAULT_AUDIO_SINK@", change])
            .status()
            .is_ok_and(|s| s.success());
        wpctl || {
            let pa = match change.strip_suffix('+') {
                Some(step) => format!("+{step}"),
                None => format!("-{}", change.trim_end_matches('-')),
            };
            std::process::Command::new("pactl")
                .args(["set-sink-volume", "@DEFAULT_SINK@", &pa])
                .status()
                .is_ok_and(|s| s.success())
        }
    }

    fn mute_toggle(&self) -> bool {
        let ok = |cmd: &str, args: &[&str]| std::process::Command::new(cmd).args(args).status().is_ok_and(|s| s.success());
        ok("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]) || ok("pactl", &["set-sink-mute", "@DEFAULT_SINK@", "toggle"])
    }
}

/// For tests: remembers what would have happened, e.g. `"open https://…"`.
#[derive(Default)]
pub struct Recorded {
    pub log: Mutex<Vec<String>>,
}

impl Recorded {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn note(&self, s: String) {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).push(s);
    }
}

impl Effects for Recorded {
    fn open_url(&self, url: &str) -> bool {
        self.note(format!("open {url}"));
        true
    }

    fn ring(&self, on: bool) -> Result<(), String> {
        self.note(format!("ring {}", if on { "on" } else { "off" }));
        Ok(())
    }

    fn run_command(&self, run: &str) -> Result<(), String> {
        self.note(format!("run {run}"));
        Ok(())
    }

    fn volume(&self, change: &str) -> bool {
        self.note(format!("volume {change}"));
        true
    }

    fn mute_toggle(&self) -> bool {
        self.note("mute".into());
        true
    }
}
