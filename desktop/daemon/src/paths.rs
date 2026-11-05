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
