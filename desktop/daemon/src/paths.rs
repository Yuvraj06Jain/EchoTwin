//! XDG locations. Every one can be overridden by an environment variable (used by tests).

use std::{env, path::PathBuf};

fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(fallback))
}

/// Keys, paired devices and the message outbox. Private to the user.
pub fn data_dir() -> PathBuf {
    env::var_os("PHONELINK_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| xdg("XDG_DATA_HOME", ".local/share").join("phonelink"))
}

pub fn config_file() -> PathBuf {
    env::var_os("PHONELINK_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| xdg("XDG_CONFIG_HOME", ".config").join("phonelink/config.toml"))
}

/// Where received files go: `PhoneLink` inside the desktop's Downloads folder.
pub fn download_dir() -> PathBuf {
    if let Some(p) = env::var_os("PHONELINK_DOWNLOAD_DIR") {
        return PathBuf::from(p);
    }
    user_download_dir().join("PhoneLink")
}

/// The Downloads folder from `~/.config/user-dirs.dirs` (localised on many desktops).
fn user_download_dir() -> PathBuf {
    let file = xdg("XDG_CONFIG_HOME", ".config").join("user-dirs.dirs");
    let home = home();
    if let Ok(text) = std::fs::read_to_string(file) {
        for line in text.lines() {
            if let Some(v) = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=") {
                let v = v.trim().trim_matches('"').replace("$HOME", &home.to_string_lossy());
                let p = PathBuf::from(v);
                if p.is_absolute() && p != home {
                    return p;
                }
            }
        }
    }
    home.join("Downloads")
}

/// Control socket. `$XDG_RUNTIME_DIR` is per-user and mode 0700 on systemd systems.
pub fn socket_path() -> PathBuf {
    if let Some(p) = env::var_os("PHONELINK_SOCKET") {
        return PathBuf::from(p);
    }
    match env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        Some(dir) => dir.join("phonelink.sock"),
        None => data_dir().join("phonelink.sock"),
    }
}
