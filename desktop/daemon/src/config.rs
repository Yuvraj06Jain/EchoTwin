use std::path::Path;

use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 47100;

/// `~/.config/phonelink/config.toml`. Every field is optional.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// UDP port to listen on (0 = pick a free one).
    pub port: u16,
    /// Shown on the phone. Defaults to the hostname.
    pub device_name: Option<String>,
    /// Desktop notifications for connections and messages.
    pub notifications: bool,
    /// Advertise on the LAN (anonymously) so the phone finds the laptop after IP changes.
    pub discovery: bool,
    /// "Dismiss on both devices": clearing a notification on one side clears it on the other.
    pub sync_dismissals: bool,
    /// Phone apps (package names) whose notifications are not shown here.
    pub muted_apps: Vec<String>,
    /// Share the clipboard with the phone (text copied here can be pasted there, and the
    /// phone's "Send clipboard" lands here).
    pub clipboard_sync: bool,
    /// Let the paired phone lock this computer's screen.
    pub allow_lock: bool,
    /// Where files from the phone are saved. Default: `~/Downloads/PhoneLink`.
    pub download_dir: Option<String>,
    /// Let the phone connect over Bluetooth when Wi-Fi is gone.
    pub bluetooth_backup: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            device_name: None,
            notifications: true,
            discovery: true,
            sync_dismissals: false,
            muted_apps: Vec::new(),
            clipboard_sync: true,
            allow_lock: true,
            download_dir: None,
            bluetooth_backup: true,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(anyhow::anyhow!("{}: {e}", path.display())),
        }
    }

    pub fn device_name(&self) -> String {
        self.device_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .or_else(|| {
                std::fs::read_to_string("/proc/sys/kernel/hostname")
                    .ok()
                    .map(|h| h.trim().to_string())
                    .filter(|h| !h.is_empty())
            })
            .unwrap_or_else(|| "Linux computer".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults_and_partial_file_merges() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Config::load(&dir.path().join("nope.toml")).unwrap(),
            Config::default()
        );
        let p = dir.path().join("c.toml");
        std::fs::write(&p, "port = 50000\ndevice_name = \"ThinkPad\"\n").unwrap();
        let c = Config::load(&p).unwrap();
        assert_eq!(
            (c.port, c.device_name(), c.discovery),
            (50000, "ThinkPad".into(), true)
        );
        std::fs::write(&p, "prot = 1\n").unwrap();
        assert!(Config::load(&p).is_err(), "typos are reported, not ignored");
    }
}
