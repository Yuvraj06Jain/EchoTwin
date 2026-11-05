//! Notifications mirrored from the phone: an in-memory store, the KDE popups, and app icons.
//!
//! Content is kept in memory only. The phone remains the source of truth and resends a
//! snapshot on every reconnect.

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc,
};

use phonelink_core::{Fingerprint, PhoneNotification};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Newest notifications kept per computer (all phones together).
pub const MAX_STORED: usize = 200;
/// Only notifications at most this old (by the phone's clock) get a popup; older ones (a
/// backlog delivered after the laptop wakes up) go to the list silently.
pub const POPUP_MAX_AGE_MS: u64 = 2 * 60 * 1000;
const MAX_ICON_BYTES: usize = 64 * 1024;

/// One notification as the UI and CLI see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationInfo {
    /// Unique on this computer: `<device id>/<phone key>`.
    pub id: String,
    pub device: String,
    pub device_name: String,
    pub package: String,
    pub app: String,
    pub title: String,
    pub text: String,
    pub posted_ms: u64,
    pub can_reply: bool,
    pub actions: Vec<String>,
    /// Cleared on the phone, kept here because "dismiss on both devices" is off.
    pub removed_on_phone: bool,
}

/// An app that has sent notifications, for the "Show notifications from" settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub package: String,
    pub app: String,
    pub muted: bool,
}

pub fn make_id(device: &Fingerprint, key: &str) -> String {
    format!("{}/{}", device.short(), key)
}

#[derive(Default)]
pub struct Store {
    /// Oldest first.
    items: VecDeque<(Fingerprint, NotificationInfo)>,
    /// package → app name, for everything ever seen this session.
    apps: HashMap<String, String>,
}

/// What a change means for the popups.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// New or updated: show / update the popup if it is fresh and not muted.
    Posted(NotificationInfo),
    /// Close the popup; `deleted` tells whether it also left the list.
    Removed {
        id: String,
        deleted: bool,
    },
    Nothing,
}

impl Store {
    pub fn get(&self, id: &str) -> Option<&(Fingerprint, NotificationInfo)> {
        self.items.iter().find(|(_, n)| n.id == id)
    }

    /// Newest first.
    pub fn list(&self) -> Vec<NotificationInfo> {
        let mut v: Vec<NotificationInfo> = self.items.iter().map(|(_, n)| n.clone()).collect();
        v.sort_by_key(|n| std::cmp::Reverse(n.posted_ms));
        v
    }

    pub fn apps(&self, muted: &dyn Fn(&str) -> bool) -> Vec<AppInfo> {
        let mut v: Vec<AppInfo> = self
            .apps
            .iter()
            .map(|(p, a)| AppInfo {
                package: p.clone(),
                app: a.clone(),
                muted: muted(p),
            })
            .collect();
        v.sort_by_key(|a| a.app.to_lowercase());
        v
    }

    fn info(device: &Fingerprint, device_name: &str, n: &PhoneNotification) -> NotificationInfo {
        NotificationInfo {
            id: make_id(device, &n.key),
            device: device.short(),
            device_name: device_name.to_string(),
            package: n.package.clone(),
            app: n.app.clone(),
            title: n.title.clone(),
            text: n.text.clone(),
            posted_ms: n.posted_ms,
            can_reply: n.can_reply,
            actions: n.actions.clone(),
            removed_on_phone: false,
        }
    }

    pub fn post(
        &mut self,
        device: Fingerprint,
        device_name: &str,
        n: &PhoneNotification,
    ) -> Effect {
        let info = Self::info(&device, device_name, n);
        self.apps.insert(n.package.clone(), n.app.clone());
        if let Some(slot) = self.items.iter_mut().find(|(_, x)| x.id == info.id) {
            slot.1 = info.clone();
        } else {
            self.items.push_back((device, info.clone()));
            while self.items.len() > MAX_STORED {
                self.items.pop_front();
            }
        }
        Effect::Posted(info)
    }

    /// The phone cleared it. With `sync` the entry goes; otherwise it is only marked.
    pub fn removed_on_phone(&mut self, device: Fingerprint, key: &str, sync: bool) -> Effect {
        let id = make_id(&device, key);
        let Some(pos) = self.items.iter().position(|(_, n)| n.id == id) else {
            return Effect::Nothing;
        };
        if sync {
            self.items.remove(pos);
            Effect::Removed { id, deleted: true }
        } else {
            self.items[pos].1.removed_on_phone = true;
            Effect::Removed { id, deleted: false }
        }
    }

    /// Everything the phone currently shows. Adds what's missing (silently) and treats
    /// what's gone like a removal on the phone.
    pub fn snapshot(
        &mut self,
        device: Fingerprint,
        device_name: &str,
        items: &[PhoneNotification],
        sync: bool,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();
        let present: std::collections::HashSet<String> =
            items.iter().map(|n| make_id(&device, &n.key)).collect();
        let gone: Vec<String> = self
            .items
            .iter()
            .filter(|(d, n)| *d == device && !n.removed_on_phone && !present.contains(&n.id))
            .map(|(_, n)| n.id.clone())
            .collect();
        for id in gone {
            let key = id
                .split_once('/')
                .map(|(_, k)| k.to_string())
                .unwrap_or_default();
            effects.push(self.removed_on_phone(device, &key, sync));
        }
        for n in items {
            let id = make_id(&device, &n.key);
            let existing = self
                .items
                .iter()
                .find(|(_, x)| x.id == id)
                .map(|(_, x)| x.clone());
            let fresh = Self::info(&device, device_name, n);
            if existing.as_ref().map(|e| NotificationInfo {
                removed_on_phone: false,
                ..e.clone()
            }) != Some(fresh.clone())
            {
                // New or changed while we were apart: store it, but no popup for old news.
                self.post(device, device_name, n);
                effects.push(Effect::Nothing);
            }
        }
        effects
    }

    /// Dismissed on this computer.
    pub fn dismiss(&mut self, id: &str) -> Option<(Fingerprint, NotificationInfo)> {
        let pos = self.items.iter().position(|(_, n)| n.id == id)?;
        self.items.remove(pos)
    }

    pub fn clear(&mut self) -> Vec<(Fingerprint, NotificationInfo)> {
        self.items.drain(..).collect()
    }

    pub fn forget_device(&mut self, device: &Fingerprint) {
        self.items.retain(|(d, _)| d != device);
    }
}

// ------------------------------------------------------------------ app icons

/// Package names are untrusted input from the phone: only allow what Android allows.
pub fn safe_package(package: &str) -> bool {
    !package.is_empty()
        && package.len() <= 200
        && package
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
        && !package.starts_with('.')
}

pub fn is_png(data: &[u8]) -> bool {
    data.len() <= MAX_ICON_BYTES && data.starts_with(b"\x89PNG\r\n\x1a\n")
}

pub fn icon_path(icons_dir: &Path, package: &str) -> Option<PathBuf> {
    safe_package(package).then(|| icons_dir.join(format!("{package}.png")))
}

pub fn save_icon(icons_dir: &Path, package: &str, png: &[u8]) -> std::io::Result<bool> {
    let Some(path) = icon_path(icons_dir, package) else {
        return Ok(false);
    };
    if !is_png(png) {
        return Ok(false);
    }
    std::fs::create_dir_all(icons_dir)?;
    let tmp = path.with_extension("png.tmp");
    std::fs::write(&tmp, png)?;
    std::fs::rename(&tmp, &path)?;
    Ok(true)
}

// ------------------------------------------------------------------ popups

/// Escape text for the notification body (servers may interpret simple HTML markup).
pub fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub enum PopupCmd {
    Show {
        id: String,
        app: String,
        title: String,
        body: String,
        icon: Option<PathBuf>,
    },
    Close {
        id: String,
    },
}

/// Owns the popup handles on one thread, so an update replaces the earlier popup and a
/// removal on the phone closes it.
#[derive(Clone)]
pub struct Popups {
    tx: mpsc::Sender<PopupCmd>,
}

impl Popups {
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel::<PopupCmd>();
        std::thread::Builder::new()
            .name("phonelink-popups".into())
            .spawn(move || popup_thread(rx))
            .expect("spawn popup thread");
        Self { tx }
    }

    /// For tests: a sink that records commands instead of talking to D-Bus.
    pub fn recording() -> (Self, mpsc::Receiver<PopupCmd>) {
        let (tx, rx) = mpsc::channel();
        (Self { tx }, rx)
    }

    pub fn send(&self, cmd: PopupCmd) {
        let _ = self.tx.send(cmd);
    }
}

fn popup_thread(rx: mpsc::Receiver<PopupCmd>) {
    let mut open: HashMap<String, notify_rust::NotificationHandle> = HashMap::new();
    let mut order: VecDeque<String> = VecDeque::new();
    while let Ok(cmd) = rx.recv() {
        match cmd {
            PopupCmd::Show {
                id,
                app,
                title,
                body,
                icon,
            } => {
                let icon = icon
                    .filter(|p| p.exists())
                    .map(|p| p.to_string_lossy().into_owned());
                let icon = icon.as_deref().unwrap_or("smartphone");
                if let Some(handle) = open.get_mut(&id) {
                    handle.appname(&app).summary(&title).body(&body).icon(icon);
                    if let Err(e) = handle.update() {
                        debug!("popup update failed: {e}");
                    }
                    continue;
                }
                match notify_rust::Notification::new()
                    .appname(&app)
                    .summary(&title)
                    .body(&body)
                    .icon(icon)
                    .show()
                {
                    Ok(handle) => {
                        open.insert(id.clone(), handle);
                        order.push_back(id);
                        while order.len() > MAX_STORED {
                            if let Some(old) = order.pop_front() {
                                open.remove(&old); // just forget it; the server expires it
                            }
                        }
                    }
                    Err(e) => debug!("popup failed: {e}"),
                }
            }
            PopupCmd::Close { id } => {
                if let Some(handle) = open.remove(&id) {
                    order.retain(|x| x != &id);
                    handle.close();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(key: &str, title: &str, t: u64) -> PhoneNotification {
        PhoneNotification {
            key: key.into(),
            package: "com.whatsapp".into(),
            app: "WhatsApp".into(),
            title: title.into(),
            text: "hi".into(),
            posted_ms: t,
            can_reply: true,
            actions: vec![],
        }
    }

    #[test]
    fn post_update_and_cap() {
        let phone = Fingerprint([1; 32]);
        let mut s = Store::default();
        s.post(phone, "Phone", &n("a", "Rahul", 1));
        s.post(phone, "Phone", &n("a", "Rahul (2 messages)", 2));
        assert_eq!(s.list().len(), 1, "same key updates in place");
        assert_eq!(s.list()[0].title, "Rahul (2 messages)");
        for i in 0..(MAX_STORED + 50) {
            s.post(phone, "Phone", &n(&format!("k{i}"), "x", 10 + i as u64));
        }
        assert_eq!(s.list().len(), MAX_STORED, "oldest dropped beyond the cap");
        assert_eq!(
            s.list()[0].posted_ms,
            10 + (MAX_STORED + 49) as u64,
            "newest first"
        );
    }

    #[test]
    fn removal_respects_sync_setting() {
        let phone = Fingerprint([1; 32]);
        let mut s = Store::default();
        s.post(phone, "Phone", &n("a", "A", 1));
        s.post(phone, "Phone", &n("b", "B", 2));
        assert_eq!(
            s.removed_on_phone(phone, "a", false),
            Effect::Removed {
                id: make_id(&phone, "a"),
                deleted: false
            }
        );
        assert!(
            s.get(&make_id(&phone, "a")).unwrap().1.removed_on_phone,
            "kept, marked"
        );
        assert_eq!(
            s.removed_on_phone(phone, "b", true),
            Effect::Removed {
                id: make_id(&phone, "b"),
                deleted: true
            }
        );
        assert!(s.get(&make_id(&phone, "b")).is_none(), "gone with sync on");
        assert_eq!(s.removed_on_phone(phone, "zzz", true), Effect::Nothing);
    }

    #[test]
    fn snapshot_adds_missing_and_handles_gone() {
        let phone = Fingerprint([1; 32]);
        let other = Fingerprint([2; 32]);
        let mut s = Store::default();
        s.post(phone, "Phone", &n("old", "Old", 1));
        s.post(other, "Tablet", &n("t", "Tablet's", 1));
        s.snapshot(phone, "Phone", &[n("new", "New", 5)], false);
        let ids: Vec<String> = s.list().iter().map(|x| x.id.clone()).collect();
        assert!(ids.contains(&make_id(&phone, "new")));
        assert!(
            s.get(&make_id(&phone, "old")).unwrap().1.removed_on_phone,
            "sync off: kept, marked"
        );
        assert!(
            s.get(&make_id(&other, "t")).is_some(),
            "other devices untouched"
        );
        s.snapshot(phone, "Phone", &[n("new", "New", 5)], true);
        assert!(
            s.get(&make_id(&phone, "old")).is_some(),
            "already marked entries are not re-processed"
        );
        let mut s2 = Store::default();
        s2.post(phone, "Phone", &n("old", "Old", 1));
        s2.snapshot(phone, "Phone", &[], true);
        assert!(s2.list().is_empty(), "sync on: gone");
    }

    #[test]
    fn icon_inputs_are_validated() {
        assert!(safe_package("com.whatsapp"));
        assert!(!safe_package("../../etc/passwd"));
        assert!(!safe_package("a/b"));
        assert!(!safe_package(""));
        assert!(is_png(b"\x89PNG\r\n\x1a\nrest"));
        assert!(!is_png(b"GIF89a"));
        assert!(!is_png(&[0u8; MAX_ICON_BYTES + 1]));
        let dir = tempfile::tempdir().unwrap();
        assert!(!save_icon(dir.path(), "../evil", b"\x89PNG\r\n\x1a\n").unwrap());
        assert!(save_icon(dir.path(), "com.whatsapp", b"\x89PNG\r\n\x1a\nx").unwrap());
        assert!(dir.path().join("com.whatsapp.png").exists());
    }

    #[test]
    fn markup_is_escaped() {
        assert_eq!(
            escape_markup("<b>hi</b> & bye"),
            "&lt;b&gt;hi&lt;/b&gt; &amp; bye"
        );
    }
}
