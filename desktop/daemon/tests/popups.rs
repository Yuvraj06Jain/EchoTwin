//! Popups against a stand-in notification server on a private D-Bus: updates replace,
//! removals close, and a click on our popup (and only ours) opens the window, with the
//! activation token. The server sends its signals only to the connection that showed the
//! popup, like KDE Plasma does.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

use phonelink_desktop::popups::{PopupCmd, Popups, Tab};
use zbus::zvariant::OwnedValue;

#[derive(Debug, Clone)]
struct Call {
    sender: String,
    replaces: u32,
    summary: String,
    actions: Vec<String>,
}

#[derive(Default)]
struct Seen {
    calls: Vec<Call>,
    closed: Vec<u32>,
}

struct Fake {
    seen: Arc<Mutex<Seen>>,
    next: u32,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Fake {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &mut self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        _app_name: &str,
        replaces_id: u32,
        _app_icon: &str,
        summary: &str,
        _body: &str,
        actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            self.next += 1;
            self.next
        };
        self.seen.lock().unwrap().calls.push(Call {
            sender: hdr.sender().map(|s| s.to_string()).unwrap_or_default(),
            replaces: replaces_id,
            summary: summary.into(),
            actions,
        });
        id
    }

    fn close_notification(&self, id: u32) {
        self.seen.lock().unwrap().closed.push(id);
    }
}

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

fn eventually(what: &str, f: impl Fn() -> bool) {
    for _ in 0..250 {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting until {what}");
}

fn show(id: &str, title: &str) -> PopupCmd {
    PopupCmd::Show {
        id: id.into(),
        app: "WhatsApp".into(),
        title: title.into(),
        body: "hi".into(),
        icon: None,
        tab: if id == "dev/b" { Tab::Files } else { Tab::Notifications },
    }
}

#[test]
fn clicking_our_popup_opens_the_window_with_its_activation_token() {
    let Some(bus) = private_bus() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let seen = Arc::new(Mutex::new(Seen::default()));
    let server = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at(
            "/org/freedesktop/Notifications",
            Fake {
                seen: seen.clone(),
                next: 0,
            },
        )
        .unwrap()
        .build()
        .unwrap();
    // Another app on the same bus, with its own popup id 77.
    let other_app = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .unwrap();

    let (open_tx, opened) = mpsc::channel::<(Tab, Option<String>)>();
    let open_tx = Mutex::new(open_tx);
    let popups = Popups::start_with(
        Some(bus.address.clone()),
        Arc::new(move |tab, token| {
            let _ = open_tx.lock().unwrap().send((tab, token));
        }),
    );

    popups.send(show("dev/a", "Rahul"));
    eventually("popup shown", || seen.lock().unwrap().calls.len() == 1);
    let first = seen.lock().unwrap().calls[0].clone();
    assert_eq!(first.replaces, 0);
    assert_eq!(first.actions[0], "default", "clicking the popup is an action");

    // An update replaces it instead of stacking a second popup.
    popups.send(show("dev/a", "Rahul (2 messages)"));
    eventually("popup updated", || seen.lock().unwrap().calls.len() == 2);
    let second = seen.lock().unwrap().calls[1].clone();
    assert_eq!((second.replaces, second.summary.as_str()), (1, "Rahul (2 messages)"));

    let path = "/org/freedesktop/Notifications";
    let iface = "org.freedesktop.Notifications";
    let to_us = Some(first.sender.as_str());
    // Clicks that must not open anything: another app's popup, a non-default button.
    server
        .emit_signal(None::<&str>, path, iface, "ActionInvoked", &(77u32, "default"))
        .unwrap();
    server
        .emit_signal(to_us, path, iface, "ActionInvoked", &(1u32, "archive"))
        .unwrap();
    // Some other client pretending to be a click on someone else's id.
    other_app
        .emit_signal(None::<&str>, path, iface, "ActionInvoked", &(5u32, "default"))
        .unwrap();
    assert!(
        opened.recv_timeout(Duration::from_millis(500)).is_err(),
        "nothing opened yet"
    );

    // The real click: activation token first (as KDE does), then the action.
    server
        .emit_signal(to_us, path, iface, "ActivationToken", &(1u32, "kwin-token-1"))
        .unwrap();
    server
        .emit_signal(to_us, path, iface, "ActionInvoked", &(1u32, "default"))
        .unwrap();
    assert_eq!(
        opened.recv_timeout(Duration::from_secs(5)).unwrap(),
        (Tab::Notifications, Some("kwin-token-1".into()))
    );

    // A server without activation tokens: still opens, just without one.
    popups.send(show("dev/b", "Placement Cell"));
    eventually("second popup", || seen.lock().unwrap().calls.len() == 3);
    server
        .emit_signal(to_us, path, iface, "ActionInvoked", &(2u32, "default"))
        .unwrap();
    assert_eq!(
        opened.recv_timeout(Duration::from_secs(5)).unwrap(),
        (Tab::Files, None),
        "a received-file popup opens the Files tab"
    );

    // Removed on the phone: the popup is closed.
    popups.send(PopupCmd::Close { id: "dev/a".into() });
    eventually("closed", || seen.lock().unwrap().closed == vec![1]);
    // All popups came from one connection (that's what makes the click reach us).
    let calls = seen.lock().unwrap().calls.clone();
    assert!(calls.iter().all(|c| c.sender == first.sender));
}
