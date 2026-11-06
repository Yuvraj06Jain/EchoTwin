//! PhoneLink for the Linux desktop.
//!
//! `phonelinkd` (systemd user service) owns the [`phonelink_core::Node`]. The `phonelink`
//! CLI, and later the tray UI, talk to it over a private Unix socket using line-delimited
//! JSON ([`ipc`]).

pub mod client;
pub mod clipboard;
pub mod config;
pub mod daemon;
pub mod ipc;
pub mod lock;
pub mod media;
pub mod netaddrs;
pub mod notifications;
pub mod paths;
pub mod popups;
