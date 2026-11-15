//! The presentation remote against a stand-in xdg-desktop-portal (RemoteDesktop) on a
//! private D-Bus: the permission sequence, keyboard only, remembered permission, reuse of
//! the session, and a refusal.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use phonelink_core::PresentKeyKind;
use phonelink_desktop::present::Presenter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

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
    Some(Bus { child, address: line.trim().to_string() })
}

#[derive(Clone, Default)]
struct Portal {
    log: Arc<Mutex<Vec<String>>>,
    deny: Arc<AtomicBool>,
}

fn text(o: &HashMap<String, OwnedValue>, k: &str) -> String {
    o.get(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_else(|| "none".into())
}

fn num(o: &HashMap<String, OwnedValue>, k: &str) -> String {
    o.get(k).and_then(|v| u32::try_from(v.try_clone().ok()?).ok()).map(|n| n.to_string()).unwrap_or_else(|| "none".into())
}

impl Portal {
    /// Answer on the caller's request object, as the real portal does.
    async fn respond(
        conn: &zbus::Connection,
        hdr: &zbus::message::Header<'_>,
        options: &HashMap<String, OwnedValue>,
        code: u32,
        results: HashMap<&str, Value<'_>>,
    ) -> OwnedObjectPath {
        let sender = hdr.sender().unwrap().to_string();
        let path = format!(
            "/org/freedesktop/portal/desktop/request/{}/{}",
            sender.trim_start_matches(':').replace('.', "_"),
            text(options, "handle_token")
        );
        conn.emit_signal(Some(sender.as_str()), path.as_str(), "org.freedesktop.portal.Request", "Response", &(code, results))
            .await
            .unwrap();
        OwnedObjectPath::try_from(path).unwrap()
    }
}

#[zbus::interface(name = "org.freedesktop.portal.RemoteDesktop")]
impl Portal {
    async fn create_session(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        options: HashMap<String, OwnedValue>,
    ) -> OwnedObjectPath {
        self.log.lock().unwrap().push("create".into());
        let session = format!("/org/freedesktop/portal/desktop/session/x/{}", text(&options, "session_handle_token"));
        Self::respond(conn, &hdr, &options, 0, HashMap::from([("session_handle", Value::from(session))])).await
    }

    async fn select_devices(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        _session: OwnedObjectPath,
        options: HashMap<String, OwnedValue>,
    ) -> OwnedObjectPath {
        self.log.lock().unwrap().push(format!(
            "select types={} persist={} restore={}",
            num(&options, "types"),
            num(&options, "persist_mode"),
            text(&options, "restore_token")
        ));
        Self::respond(conn, &hdr, &options, 0, HashMap::new()).await
    }

    async fn start(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        _session: OwnedObjectPath,
        _parent: String,
        options: HashMap<String, OwnedValue>,
    ) -> OwnedObjectPath {
        self.log.lock().unwrap().push("start".into());
        if self.deny.load(Ordering::SeqCst) {
            return Self::respond(conn, &hdr, &options, 1, HashMap::new()).await;
        }
        let results = HashMap::from([("devices", Value::from(1u32)), ("restore_token", Value::from("tok-1"))]);
        Self::respond(conn, &hdr, &options, 0, results).await
    }

    fn notify_keyboard_keycode(&self, _session: OwnedObjectPath, _options: HashMap<String, OwnedValue>, keycode: i32, state: u32) {
        self.log.lock().unwrap().push(format!("key {keycode} {state}"));
    }
}

fn serve(bus: &str, portal: Portal) -> zbus::blocking::Connection {
    zbus::blocking::connection::Builder::address(bus)
        .unwrap()
        .name("org.freedesktop.portal.Desktop")
        .unwrap()
        .serve_at("/org/freedesktop/portal/desktop", portal)
        .unwrap()
        .build()
        .unwrap()
}

#[test]
fn the_first_key_asks_once_then_keys_flow_and_the_permission_is_remembered() {
    let Some(bus) = private_bus() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let portal = Portal::default();
    let _server = serve(&bus.address, portal.clone());
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("remote-control.token");
    let take = || std::mem::take(&mut *portal.log.lock().unwrap());

    let p = Presenter::start(Some(bus.address.clone()), token.clone());
    p.press(PresentKeyKind::Next).unwrap();
    assert_eq!(take(), ["create", "select types=1 persist=2 restore=none", "start", "key 109 1", "key 109 0"]);
    assert_eq!(std::fs::read_to_string(&token).unwrap(), "tok-1");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&token).unwrap().permissions().mode() & 0o777, 0o600);

    // Same session for the next keys, in order.
    p.press(PresentKeyKind::Blank).unwrap();
    p.press(PresentKeyKind::Previous).unwrap();
    assert_eq!(take(), ["key 48 1", "key 48 0", "key 104 1", "key 104 0"]);

    // After a restart the saved permission is offered, so KDE doesn't ask again.
    let p2 = Presenter::start(Some(bus.address.clone()), token.clone());
    p2.press(PresentKeyKind::Start).unwrap();
    assert_eq!(take(), ["create", "select types=1 persist=2 restore=tok-1", "start", "key 63 1", "key 63 0"]);
}

#[test]
fn a_refused_permission_presses_nothing() {
    let Some(bus) = private_bus() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let portal = Portal::default();
    portal.deny.store(true, Ordering::SeqCst);
    let _server = serve(&bus.address, portal.clone());
    let dir = tempfile::tempdir().unwrap();
    let p = Presenter::start(Some(bus.address.clone()), dir.path().join("t"));
    let err = p.press(PresentKeyKind::Next).unwrap_err();
    assert!(err.contains("not allowed"), "{err}");
    assert!(portal.log.lock().unwrap().iter().all(|l| !l.starts_with("key")));
    assert!(!dir.path().join("t").exists());
}
