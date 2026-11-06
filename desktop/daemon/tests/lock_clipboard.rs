//! Locking from the phone and sharing the clipboard, against stand-ins for KDE's screen
//! locker and clipboard service (Klipper) on a private D-Bus, with a simulated phone.

use std::{
    io::{BufRead, BufReader},
    net::SocketAddr,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use phonelink_core::{
    message::LOCK_RESULT_KEY, AppMessage, Event, Fingerprint, Node, NodeConfig, PairingOffer,
    CAP_CLIPBOARD, CAP_LOCK,
};
use phonelink_desktop::{
    client::Client,
    config::Config,
    daemon::{run, DaemonOptions},
    ipc::{Reply, Request, WatchEvent},
    lock::{lock_screen, Buses},
    notifications::Popups,
};
use tokio::{sync::mpsc::UnboundedReceiver, time::timeout};

const WAIT: Duration = Duration::from_secs(15);

// ------------------------------------------------------------------ a private D-Bus

struct Bus {
    child: Child,
    address: String,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn private_bus() -> Option<Bus> {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take()?).read_line(&mut line).ok()?;
    Some(Bus {
        child,
        address: line.trim().to_string(),
    })
}

fn serve<I: zbus::object_server::Interface>(
    bus: &str,
    name: &str,
    path: &str,
    iface: I,
) -> zbus::blocking::Connection {
    zbus::blocking::connection::Builder::address(bus)
        .unwrap()
        .name(name)
        .unwrap()
        .serve_at(path, iface)
        .unwrap()
        .build()
        .unwrap()
}

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<(String, u32)>>);

/// Klipper: the clipboard text, and a signal on every change (also our own sets, as the
/// real one does).
struct FakeKlipper(Shared);

#[zbus::interface(name = "org.kde.klipper.klipper")]
impl FakeKlipper {
    #[zbus(name = "getClipboardContents")]
    fn get_clipboard_contents(&self) -> String {
        self.0 .0.lock().unwrap().0.clone()
    }

    #[zbus(name = "setClipboardContents")]
    async fn set_clipboard_contents(&self, text: String, #[zbus(connection)] conn: &zbus::Connection) {
        self.0 .0.lock().unwrap().0 = text;
        let _ = conn
            .emit_signal(None::<&str>, "/klipper", "org.kde.klipper.klipper", "clipboardHistoryUpdated", &())
            .await;
    }
}

struct FakeScreenSaver(Shared);

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl FakeScreenSaver {
    fn lock(&self) {
        self.0 .0.lock().unwrap().1 += 1;
    }
}

// ------------------------------------------------------------------ rig

struct Rig {
    _bus: Bus,
    _services: Vec<zbus::blocking::Connection>,
    klipper: zbus::blocking::Connection,
    clip: Shared,
    screen: Shared,
    socket: std::path::PathBuf,
    _dir: tempfile::TempDir,
    phone: Node,
    phone_rx: UnboundedReceiver<Event>,
    laptop: Fingerprint,
    watch: Client,
    _stop: tokio::sync::oneshot::Sender<()>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

async fn setup() -> Option<Rig> {
    let bus = private_bus()?;
    let clip = Shared::default();
    clip.0.lock().unwrap().0 = "copied before the service started".into();
    let screen = Shared::default();
    let klipper = serve(&bus.address, "org.kde.klipper", "/klipper", FakeKlipper(clip.clone()));
    let saver = serve(
        &bus.address,
        "org.freedesktop.ScreenSaver",
        "/ScreenSaver",
        FakeScreenSaver(screen.clone()),
    );

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("pl.sock");
    let opts = DaemonOptions {
        data_dir: dir.path().join("data"),
        socket: socket.clone(),
        config: Config {
            port: 0,
            discovery: false,
            ..Config::default()
        },
        config_path: Some(dir.path().join("config.toml")),
        popups: Some(Popups::recording().0),
        buses: Buses {
            session: Some(bus.address.clone()),
            system: Buses::unreachable().system,
        },
    };
    let (stop, stop_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(run(opts, async move {
        let _ = stop_rx.await;
    }));
    let mut c = timeout(WAIT, async {
        loop {
            if let Ok(c) = Client::connect(&socket).await {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    c.send(&Request::Pair { ttl_secs: 60 }).await.unwrap();
    let Some(Reply::PairStarted { uri, .. }) = c.next().await.unwrap() else {
        panic!()
    };
    let mut offer = PairingOffer::from_uri(&uri).unwrap();
    offer.addrs = vec![SocketAddr::from(([127, 0, 0, 1], offer.addrs[0].port()))];
    let mut cfg = NodeConfig::new("Galaxy S24 FE", None, "127.0.0.1:0".parse().unwrap());
    cfg.capabilities.extend([CAP_LOCK, CAP_CLIPBOARD].map(String::from));
    let (phone, mut phone_rx) = Node::start(cfg).await.unwrap();
    phone.pair_with(&offer).await.unwrap();
    timeout(WAIT, async {
        while !matches!(phone_rx.recv().await, Some(Event::PeerConnected { .. })) {}
    })
    .await
    .unwrap();
    let mut watch = Client::connect(&socket).await.unwrap();
    watch.send(&Request::Watch).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    Some(Rig {
        _bus: bus,
        _services: vec![saver],
        klipper,
        clip,
        screen,
        socket,
        _dir: dir,
        phone,
        phone_rx,
        laptop: offer.fingerprint,
        watch,
        _stop: stop,
    })
}

impl Rig {
    fn phone_sends(&self, m: AppMessage) {
        self.phone.send(&self.laptop, m.encode().unwrap()).unwrap();
    }

    async fn request(&self, r: Request) -> Reply {
        Client::connect(&self.socket)
            .await
            .unwrap()
            .request(&r)
            .await
            .unwrap()
    }

    /// The user copies `text` on the laptop.
    fn laptop_copies(&self, text: &str) {
        self.clip.0.lock().unwrap().0 = text.into();
        self.klipper
            .emit_signal(None::<&str>, "/klipper", "org.kde.klipper.klipper", "clipboardHistoryUpdated", &())
            .unwrap();
    }

    fn laptop_clipboard(&self) -> String {
        self.clip.0.lock().unwrap().0.clone()
    }

    fn locks(&self) -> u32 {
        self.screen.0.lock().unwrap().1
    }

    /// Messages the phone receives within `d`.
    async fn phone_gets(&mut self, d: Duration) -> Vec<AppMessage> {
        let mut out = vec![];
        let _ = timeout(d, async {
            loop {
                if let Some(Event::Message { body, .. }) = self.phone_rx.recv().await {
                    out.push(AppMessage::decode(&body));
                }
            }
        })
        .await;
        out
    }

    async fn phone_waits_for<F: Fn(&AppMessage) -> bool>(&mut self, what: &str, f: F) -> AppMessage {
        timeout(WAIT, async {
            loop {
                if let Some(Event::Message { body, .. }) = self.phone_rx.recv().await {
                    let m = AppMessage::decode(&body);
                    if f(&m) {
                        return m;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
    }

    async fn event<F: Fn(&WatchEvent) -> bool>(&mut self, what: &str, f: F) -> WatchEvent {
        timeout(WAIT, async {
            loop {
                if let Some(Reply::Event { event }) = self.watch.next().await.unwrap() {
                    if f(&event) {
                        return event;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
    }
}

async fn eventually(what: &str, f: impl Fn() -> bool) {
    timeout(WAIT, async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting until {what}"))
}

fn is_clip(m: &AppMessage) -> bool {
    matches!(m, AppMessage::Clipboard { .. })
}

// ------------------------------------------------------------------ tests

#[tokio::test]
async fn clipboard_goes_both_ways_without_echoes() {
    let Some(mut r) = setup().await else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    // What was on the clipboard before is not a new copy: nothing is sent at start-up, nor
    // when Klipper announces a history change that left the top entry as it was.
    r.klipper
        .emit_signal(None::<&str>, "/klipper", "org.kde.klipper.klipper", "clipboardHistoryUpdated", &())
        .unwrap();
    assert!(r.phone_gets(Duration::from_millis(500)).await.iter().all(|m| !is_clip(m)));

    // Laptop → phone, automatically.
    r.laptop_copies("ssh yuvraj@lab-pc");
    let m = r.phone_waits_for("laptop copy on the phone", is_clip).await;
    let AppMessage::Clipboard { text, copied_ms } = m else { unreachable!() };
    assert_eq!(text, "ssh yuvraj@lab-pc");
    assert!(copied_ms.abs_diff(now_ms()) < 10_000);

    // Phone → laptop ("Send clipboard" on the phone).
    r.phone_sends(AppMessage::Clipboard {
        text: "OTP-free text from the phone".into(),
        copied_ms: now_ms(),
    });
    let ev = r.event("clipboard received", |e| matches!(e, WatchEvent::ClipboardReceived { .. })).await;
    assert!(matches!(ev, WatchEvent::ClipboardReceived { name, .. } if name == "Galaxy S24 FE"));
    eventually("on the laptop clipboard", || r.laptop_clipboard() == "OTP-free text from the phone").await;
    // Setting it made Klipper announce a change; that must not bounce back to the phone.
    assert!(
        r.phone_gets(Duration::from_millis(800)).await.iter().all(|m| !is_clip(m)),
        "no echo"
    );

    // A copy that waited in a queue for ages is not pasted over something newer.
    r.phone_sends(AppMessage::Clipboard {
        text: "stale".into(),
        copied_ms: now_ms() - 10 * 60_000,
    });
    r.phone_sends(AppMessage::Clipboard {
        text: String::new(),
        copied_ms: now_ms(),
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(r.laptop_clipboard(), "OTP-free text from the phone");

    // Too long to be useful as a paste (and not cut off half-way): not sent.
    r.laptop_copies(&"x".repeat(phonelink_core::message::MAX_CLIPBOARD_TEXT + 1));
    r.laptop_copies("   ");
    assert!(r.phone_gets(Duration::from_millis(500)).await.iter().all(|m| !is_clip(m)));

    // Switched off on the laptop: nothing in either direction.
    assert_eq!(r.request(Request::SetClipboardSync { enabled: false }).await, Reply::Done);
    r.laptop_copies("private");
    r.phone_sends(AppMessage::Clipboard {
        text: "from phone while off".into(),
        copied_ms: now_ms(),
    });
    assert!(r.phone_gets(Duration::from_millis(600)).await.iter().all(|m| !is_clip(m)));
    assert_eq!(r.laptop_clipboard(), "private");
    let Reply::Status { clipboard_sync, devices, .. } = r.request(Request::Status).await else {
        panic!()
    };
    assert!(!clipboard_sync);
    assert!(devices[0].capabilities.iter().any(|c| c == CAP_CLIPBOARD));

    // Back on: works again.
    r.request(Request::SetClipboardSync { enabled: true }).await;
    r.laptop_copies("back on");
    r.phone_waits_for("copy after re-enabling", |m| {
        matches!(m, AppMessage::Clipboard { text, .. } if text == "back on")
    })
    .await;
}

#[tokio::test]
async fn the_phone_can_lock_the_computer_but_only_now_and_only_if_allowed() {
    let Some(mut r) = setup().await else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let is_lock_result = |m: &AppMessage| matches!(m, AppMessage::ActionResult { key, .. } if key == LOCK_RESULT_KEY);

    r.phone_sends(AppMessage::LockComputer { sent_ms: now_ms() });
    let res = r.phone_waits_for("lock result", is_lock_result).await;
    assert!(matches!(res, AppMessage::ActionResult { ok: true, .. }), "{res:?}");
    assert_eq!(r.locks(), 1);
    r.event("lock event", |e| matches!(e, WatchEvent::LockedByPhone { .. })).await;

    // A request that sat in a queue (phone was offline) does not lock hours later.
    r.phone_sends(AppMessage::LockComputer {
        sent_ms: now_ms() - 30 * 60_000,
    });
    let res = r.phone_waits_for("stale lock result", is_lock_result).await;
    assert!(matches!(res, AppMessage::ActionResult { ok: false, .. }));
    assert_eq!(r.locks(), 1);

    // Switched off on the laptop.
    assert_eq!(r.request(Request::SetAllowLock { enabled: false }).await, Reply::Done);
    r.phone_sends(AppMessage::LockComputer { sent_ms: now_ms() });
    let res = r.phone_waits_for("refused lock", is_lock_result).await;
    assert!(
        matches!(&res, AppMessage::ActionResult { ok: false, message, .. } if message.contains("switched off")),
        "{res:?}"
    );
    assert_eq!(r.locks(), 1);
    let Reply::Status { allow_lock, .. } = r.request(Request::Status).await else {
        panic!()
    };
    assert!(!allow_lock);
    let saved = std::fs::read_to_string(r._dir.path().join("config.toml")).unwrap();
    assert!(saved.contains("allow_lock = false"), "{saved}");
}

// ------------------------------------------------------------------ logind fallback

/// logind with three sessions: this user's graphical one, this user's ssh one, and
/// someone else's graphical one. Only the first may be locked.
struct FakeLogind {
    uid: u32,
}

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl FakeLogind {
    fn list_sessions(&self) -> Vec<(String, u32, String, String, zbus::zvariant::OwnedObjectPath)> {
        let p = |s: &str| zbus::zvariant::OwnedObjectPath::try_from(format!("/org/freedesktop/login1/session/{s}")).unwrap();
        vec![
            ("2".into(), self.uid, "me".into(), "seat0".into(), p("_32")),
            ("5".into(), self.uid, "me".into(), String::new(), p("_35")),
            ("7".into(), self.uid + 1, "other".into(), "seat0".into(), p("_37")),
        ]
    }
}

struct FakeSession {
    kind: &'static str,
    locked: Arc<Mutex<Vec<&'static str>>>,
    id: &'static str,
}

#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl FakeSession {
    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        self.kind.into()
    }

    fn lock(&self) {
        self.locked.lock().unwrap().push(self.id);
    }
}

#[test]
fn without_a_screen_saver_logind_locks_only_this_users_graphical_session() {
    let (Some(session), Some(system)) = (private_bus(), private_bus()) else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata("/proc/self").unwrap().uid();
    let locked = Arc::new(Mutex::new(vec![]));
    let s = |id, kind| FakeSession {
        kind,
        locked: locked.clone(),
        id,
    };
    let _logind = zbus::blocking::connection::Builder::address(system.address.as_str())
        .unwrap()
        .name("org.freedesktop.login1")
        .unwrap()
        .serve_at("/org/freedesktop/login1", FakeLogind { uid })
        .unwrap()
        .serve_at("/org/freedesktop/login1/session/_32", s("mine-wayland", "wayland"))
        .unwrap()
        .serve_at("/org/freedesktop/login1/session/_35", s("mine-ssh", "tty"))
        .unwrap()
        .serve_at("/org/freedesktop/login1/session/_37", s("theirs", "x11"))
        .unwrap()
        .build()
        .unwrap();
    let buses = Buses {
        session: Some(session.address.clone()), // no screen saver on it
        system: Some(system.address.clone()),
    };
    assert_eq!(lock_screen(&buses), Ok("logind"));
    assert_eq!(*locked.lock().unwrap(), vec!["mine-wayland"]);

    // Nothing at all to lock with: an error, not a pretend success.
    assert!(lock_screen(&Buses::unreachable()).is_err());
}
