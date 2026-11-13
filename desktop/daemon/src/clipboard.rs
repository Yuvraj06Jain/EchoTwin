//! This computer's clipboard, through KDE Plasma's clipboard service (Klipper) on D-Bus.
//!
//! * Works on Wayland without window focus (Klipper is part of the desktop shell).
//! * Copies from password managers (KeePassXC, …) carry KDE's password hint; Klipper keeps
//!   them out of its history, so they are never seen here and never sent to the phone.
//! * Text we put on the clipboard ourselves (from the phone) is not reported back as a new
//!   copy, so nothing bounces between the devices.
//!
//! * Images: when a copy is a picture (a screenshot, "Copy image") it is read losslessly
//!   from the clipboard itself ([`ImageSource`]) and saved as PNG; a copied image *file*
//!   (Dolphin) is passed on as that file, untouched.
//!
//! When Klipper is not running (another desktop, or the clipboard widget removed), this
//! keeps checking every few seconds and starts working as soon as it appears.

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use tracing::{debug, info};
use zbus::{
    blocking::{connection, Connection, MessageIterator},
    message::Type,
    MatchRule,
};

const DEST: &str = "org.kde.klipper";
const PATH: &str = "/klipper";
const IFACE: &str = "org.kde.klipper.klipper";
/// How often to look for Klipper while it is missing…
const RETRY: Duration = Duration::from_secs(5);
/// …and how long to sleep while connected, where its signals wake us anyway.
const CONNECTED_WAIT: Duration = Duration::from_secs(60);

/// Something copied on this computer, for the phone.
#[derive(Debug, Clone, PartialEq)]
pub enum Copied {
    Text(String),
    /// An image file to send as it is (a copied file, or a PNG we saved from the clipboard).
    Image(PathBuf),
}

/// Reads a picture from the clipboard as PNG bytes (`None`: the copy isn't a picture).
pub trait ImageSource: Send {
    fn image_png(&mut self) -> Option<Vec<u8>>;
}

/// The real clipboard (Wayland data-control on KDE, X11 elsewhere), opened on first use.
pub struct SystemImages(Option<arboard::Clipboard>);

impl SystemImages {
    pub fn new() -> Self {
        Self(None)
    }
}

impl Default for SystemImages {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageSource for SystemImages {
    fn image_png(&mut self) -> Option<Vec<u8>> {
        if self.0.is_none() {
            self.0 = arboard::Clipboard::new()
                .map_err(|e| debug!("clipboard images unavailable: {e}"))
                .ok();
        }
        let img = self.0.as_mut()?.get_image().ok()?;
        encode_png(img.width as u32, img.height as u32, &img.bytes)
    }
}

/// RGBA pixels → PNG (lossless).
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().ok()?;
    w.write_image_data(rgba).ok()?;
    drop(w);
    Some(out)
}

/// A copied file that is an image (`file:///…/x.png`), as a local path.
pub fn image_file(text: &str) -> Option<PathBuf> {
    let line = text.trim();
    if line.contains('\n') {
        return None; // several files: not a clipboard image
    }
    let path = line.strip_prefix("file://")?;
    let path = PathBuf::from(percent_decode(path));
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let image = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "heic" | "avif");
    (image && path.is_file()).then_some(path)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Save a clipboard picture for sending; keeps only the newest few.
fn save_png(dir: &Path, png: &[u8]) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let mut old: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    old.sort();
    while old.len() >= 5 {
        let _ = std::fs::remove_file(old.remove(0));
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("Clipboard image {stamp}.png"));
    std::fs::write(&path, png).ok()?;
    Some(path)
}

enum Cmd {
    Set(String),
    Changed,
    Lost,
}

/// Handle to the clipboard thread.
#[derive(Clone)]
pub struct ClipboardSync {
    tx: mpsc::Sender<Cmd>,
}

impl ClipboardSync {
    /// `bus`: a D-Bus address (tests), or `None` for the session bus. Each new local copy
    /// goes to `copies`. `images`: where pictures are read from (`None`: text only);
    /// `image_dir`: where they are saved before sending.
    pub fn start(
        bus: Option<String>,
        copies: tokio::sync::mpsc::UnboundedSender<Copied>,
        images: Option<Box<dyn ImageSource>>,
        image_dir: PathBuf,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let me = tx.clone();
        std::thread::Builder::new()
            .name("phonelink-clipboard".into())
            .spawn(move || run(bus, rx, me, copies, images, image_dir))
            .expect("spawn clipboard thread");
        Self { tx }
    }

    /// Put `text` on this computer's clipboard.
    pub fn set(&self, text: String) {
        let _ = self.tx.send(Cmd::Set(text));
    }
}

fn connect(bus: &Option<String>) -> zbus::Result<Connection> {
    match bus {
        Some(a) => connection::Builder::address(a.as_str())?.build(),
        None => Connection::session(),
    }
}

fn klipper_running(c: &Connection) -> bool {
    let Ok(proxy) = zbus::blocking::fdo::DBusProxy::new(c) else {
        return false;
    };
    proxy
        .name_has_owner(DEST.try_into().expect("valid bus name"))
        .unwrap_or(false)
}

fn read(c: &Connection) -> zbus::Result<String> {
    c.call_method(Some(DEST), PATH, Some(IFACE), "getClipboardContents", &())?
        .body()
        .deserialize::<String>()
}

fn write(c: &Connection, text: &str) -> zbus::Result<()> {
    c.call_method(Some(DEST), PATH, Some(IFACE), "setClipboardContents", &(text,))
        .map(|_| ())
}

/// Forward Klipper's "history changed" signal to the clipboard thread.
fn listen(c: &Connection, tx: mpsc::Sender<Cmd>) -> zbus::Result<()> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(IFACE)?
        .member("clipboardHistoryUpdated")?
        .path(PATH)?
        .build();
    let signals = MessageIterator::for_match_rule(rule, c, Some(64))?;
    std::thread::Builder::new()
        .name("phonelink-clipboard-signals".into())
        .spawn(move || {
            for msg in signals {
                if msg.is_err() || tx.send(Cmd::Changed).is_err() {
                    break;
                }
            }
            let _ = tx.send(Cmd::Lost);
        })
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;
    Ok(())
}

fn run(
    bus: Option<String>,
    rx: mpsc::Receiver<Cmd>,
    me: mpsc::Sender<Cmd>,
    copies: tokio::sync::mpsc::UnboundedSender<Copied>,
    mut images: Option<Box<dyn ImageSource>>,
    image_dir: PathBuf,
) {
    // The picture on the clipboard last time we looked (hash), so it's sent once.
    let mut last_image: Option<u64> = None;
    let mut conn: Option<Connection> = None;
    let mut logged_missing = false;
    // What the clipboard held last time we looked; a signal only counts if it changed.
    let mut current: Option<String> = None;
    loop {
        if conn.is_none() {
            match connect(&bus) {
                Ok(c) if klipper_running(&c) => match listen(&c, me.clone()) {
                    Ok(()) => {
                        info!("clipboard sharing ready (KDE clipboard)");
                        logged_missing = false;
                        current = read(&c).ok(); // what was there before we started: not a new copy
                        conn = Some(c);
                    }
                    Err(e) => debug!("clipboard signals unavailable: {e}"),
                },
                Ok(_) => {
                    if !logged_missing {
                        info!("KDE clipboard service not running; clipboard sharing waits for it");
                        logged_missing = true;
                    }
                }
                Err(e) => debug!("no session bus for the clipboard: {e}"),
            }
        }
        let wait = if conn.is_some() { CONNECTED_WAIT } else { RETRY };
        let cmd = match rx.recv_timeout(wait) {
            Ok(cmd) => cmd,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let Some(c) = conn.as_ref() else {
            continue; // a Set while Klipper is away is dropped: there is nowhere to put it
        };
        match cmd {
            Cmd::Set(text) => {
                // Mark it as ours first: the signal it triggers must not echo it back.
                current = Some(text.clone());
                if let Err(e) = write(c, &text) {
                    debug!("could not set the clipboard: {e}");
                    conn = None;
                }
            }
            Cmd::Changed => match read(c) {
                Ok(text) => {
                    // A picture (screenshot, "Copy image"): Klipper's text is just a label.
                    if let Some(png) = images.as_mut().and_then(|s| s.image_png()) {
                        current = Some(text);
                        let h = {
                            use std::hash::{Hash, Hasher};
                            let mut h = std::collections::hash_map::DefaultHasher::new();
                            png.hash(&mut h);
                            h.finish()
                        };
                        if last_image != Some(h) {
                            last_image = Some(h);
                            if let Some(path) = save_png(&image_dir, &png) {
                                if copies.send(Copied::Image(path)).is_err() {
                                    return;
                                }
                            }
                        }
                        continue;
                    }
                    if current.as_deref() != Some(text.as_str()) {
                        current = Some(text.clone());
                        let copied = match image_file(&text) {
                            Some(path) => Copied::Image(path),
                            None => Copied::Text(text),
                        };
                        let empty = matches!(&copied, Copied::Text(t) if t.is_empty());
                        if !empty && copies.send(copied).is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    debug!("could not read the clipboard: {e}");
                    conn = None;
                }
            },
            Cmd::Lost => conn = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_image_files_and_pictures() {
        let dir = tempfile::tempdir().unwrap();
        let pic = dir.path().join("a b.JPG");
        std::fs::write(&pic, b"x").unwrap();
        let uri = format!("file://{}/a%20b.JPG", dir.path().display());
        assert_eq!(image_file(&uri), Some(pic.clone()));
        assert_eq!(image_file(&format!("{uri}\n{uri}")), None, "several files");
        assert_eq!(image_file("file:///nope/missing.png"), None);
        assert_eq!(image_file("just some text.png"), None);
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        assert_eq!(image_file(&format!("file://{}/notes.txt", dir.path().display())), None);
        // Lossless PNG from RGBA; nonsense sizes refused.
        let png = encode_png(2, 1, &[255, 0, 0, 255, 0, 255, 0, 128]).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert!(encode_png(2, 2, &[0; 4]).is_none());
        // Only the newest few pictures are kept.
        for _ in 0..7 {
            save_png(dir.path().join("clip").as_path(), &png).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(std::fs::read_dir(dir.path().join("clip")).unwrap().count(), 5);
    }
}
