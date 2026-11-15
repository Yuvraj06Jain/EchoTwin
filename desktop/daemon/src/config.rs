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
    /// Where files from the phone are saved. Default: `~/Downloads/EchoTwin`.
    pub download_dir: Option<String>,
    /// Let the phone connect over Bluetooth when Wi-Fi is gone.
    pub bluetooth_backup: bool,
    /// The connection switch: off = no searching, no connecting, nothing in the background.
    pub connection: bool,
    /// Fewer background wake-ups: slower keep-alive, and a phone that is away is looked
    /// for less often.
    pub battery_saver: bool,
    /// The battery saver turns on by itself when this computer's battery is at or below this
    /// percentage and not charging (off again 5% above it). 0 = never. At most 50.
    pub auto_saver_below: u8,
    /// Popups when the phone's battery is low or full.
    pub battery_alerts: bool,
    /// Let the phone open web links here ("Open on computer").
    pub open_links: bool,
    /// Let the phone ring this computer (to find it).
    pub allow_ring: bool,
    /// Pause music and videos here while the phone has a call; resume afterwards.
    pub pause_media_on_call: bool,
    /// Let the phone press the buttons below (Remote screen).
    pub allow_commands: bool,
    /// Let the phone act as a presentation clicker (five keys only).
    pub presentation_remote: bool,
    /// Pass `sudo` approval requests to the phone (only a messenger: the root-owned checker
    /// decides; switching this off only disables it).
    pub phone_approval: bool,
    /// Your own buttons for the phone's Remote screen: a name, and a shell command run here.
    /// Only the names go to the phone.
    pub commands: Vec<CommandEntry>,
    /// Where batteries are read from (tests only). Default: `/sys/class/power_supply`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_supply_dir: Option<String>,
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
            connection: true,
            battery_saver: true,
            auto_saver_below: 20,
            battery_alerts: true,
            open_links: true,
            allow_ring: true,
            pause_media_on_call: true,
            allow_commands: true,
            presentation_remote: true,
            phone_approval: true,
            commands: vec![
                CommandEntry { name: "Suspend".into(), run: "systemctl suspend".into() },
                CommandEntry { name: "Mute / unmute".into(), run: "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle".into() },
            ],
            power_supply_dir: None,
        }
    }
}

/// One of your own buttons for the phone's Remote screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEntry {
    pub name: String,
    pub run: String,
}

impl CommandEntry {
    /// What the phone knows it by: 8 hex digits of BLAKE3(name). Stable across restarts.
    pub fn id(&self) -> String {
        blake3::hash(self.name.as_bytes()).to_hex()[..8].to_string()
    }
}

/// Names 1–40 characters, commands 1–500, no NUL bytes, at most 20, names unique.
pub fn check_commands(items: &[CommandEntry]) -> Result<(), String> {
    if items.len() > phonelink_core::message::MAX_COMMANDS {
        return Err(format!("at most {} buttons", phonelink_core::message::MAX_COMMANDS));
    }
    let mut names = std::collections::HashSet::new();
    for c in items {
        let n = c.name.trim();
        if n.is_empty() || n.chars().count() > 40 || n.len() > phonelink_core::message::MAX_COMMAND_NAME * 4 {
            return Err("a button name must be 1 to 40 characters".into());
        }
        if c.run.trim().is_empty() || c.run.chars().count() > 500 {
            return Err(format!("the command for \"{n}\" must be 1 to 500 characters"));
        }
        if c.name.contains('\0') || c.run.contains('\0') || c.name.chars().any(char::is_control) {
            return Err("names and commands can't contain control characters".into());
        }
        if !names.insert(n.to_lowercase()) {
            return Err(format!("two buttons are called \"{n}\""));
        }
    }
    Ok(())
}

/// Simple on/off settings, set with `Request::SetSwitch` and listed in `Reply::Status`.
pub const SWITCHES: &[&str] = &["battery_alerts", "open_links", "allow_ring", "pause_media_on_call", "allow_commands", "presentation_remote", "phone_approval"];

impl Config {
    pub fn switch(&self, name: &str) -> Option<bool> {
        Some(match name {
            "battery_alerts" => self.battery_alerts,
            "open_links" => self.open_links,
            "allow_ring" => self.allow_ring,
            "pause_media_on_call" => self.pause_media_on_call,
            "allow_commands" => self.allow_commands,
            "presentation_remote" => self.presentation_remote,
            "phone_approval" => self.phone_approval,
            _ => return None,
        })
    }

    /// False for an unknown name.
    pub fn set_switch(&mut self, name: &str, on: bool) -> bool {
        let slot = match name {
            "battery_alerts" => &mut self.battery_alerts,
            "open_links" => &mut self.open_links,
            "allow_ring" => &mut self.allow_ring,
            "pause_media_on_call" => &mut self.pause_media_on_call,
            "allow_commands" => &mut self.allow_commands,
            "presentation_remote" => &mut self.presentation_remote,
            "phone_approval" => &mut self.phone_approval,
            _ => return false,
        };
        *slot = on;
        true
    }

    pub fn switches(&self) -> std::collections::BTreeMap<String, bool> {
        SWITCHES
            .iter()
            .filter_map(|n| Some((n.to_string(), self.switch(n)?)))
            .collect()
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
