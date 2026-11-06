//! Notification mirroring end to end: a simulated phone posts notifications to a real
//! phonelinkd over the encrypted link; popups are recorded instead of shown.

use std::{net::SocketAddr, sync::mpsc, time::Duration};

use phonelink_core::{
    AppMessage, Event, Fingerprint, Node, NodeConfig, PairingOffer, PhoneNotification,
};
use phonelink_desktop::{
    client::Client,
    config::Config,
    daemon::{run, DaemonOptions},
    ipc::{Reply, Request, WatchEvent},
    notifications::{PopupCmd, Popups},
};
use tokio::{sync::mpsc::UnboundedReceiver, time::timeout};

const WAIT: Duration = Duration::from_secs(15);

struct Rig {
    socket: std::path::PathBuf,
    dir: tempfile::TempDir,
    popups: mpsc::Receiver<PopupCmd>,
    phone: Node,
    phone_rx: UnboundedReceiver<Event>,
    laptop_fp: Fingerprint,
    watch: Client,
    _stop: tokio::sync::oneshot::Sender<()>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn notif(key: &str, app: &str, title: &str, text: &str) -> PhoneNotification {
    PhoneNotification {
        key: key.into(),
        package: format!("com.{}", app.to_lowercase()),
        app: app.into(),
        title: title.into(),
        text: text.into(),
        posted_ms: now_ms(),
        can_reply: app == "WhatsApp",
        actions: vec!["Mark as read".into()],
    }
}

async fn setup() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("pl.sock");
    let (popups, popup_rx) = Popups::recording();
    let config = Config {
        port: 0,
        notifications: true,
        discovery: false,
        ..Config::default()
    };
    let opts = DaemonOptions {
        data_dir: dir.path().join("data"),
        socket: socket.clone(),
        config,
        config_path: Some(dir.path().join("config.toml")),
        popups: Some(popups),
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
    let (phone, mut phone_rx) = Node::start(NodeConfig::new(
        "Galaxy S24 FE",
        None,
        "127.0.0.1:0".parse().unwrap(),
    ))
    .await
    .unwrap();
    phone.pair_with(&offer).await.unwrap();
    timeout(WAIT, async {
        while !matches!(phone_rx.recv().await, Some(Event::PeerConnected { .. })) {}
    })
    .await
    .unwrap();
    let mut watch = Client::connect(&socket).await.unwrap();
    watch.send(&Request::Watch).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    Rig {
        socket,
        dir,
        popups: popup_rx,
        phone,
        phone_rx,
        laptop_fp: offer.fingerprint,
        watch,
        _stop: stop,
    }
}

impl Rig {
    fn phone_sends(&self, m: AppMessage) {
        self.phone
            .send(&self.laptop_fp, m.encode().unwrap())
            .unwrap();
    }

    async fn request(&self, r: Request) -> Reply {
        Client::connect(&self.socket)
            .await
            .unwrap()
            .request(&r)
            .await
            .unwrap()
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

    /// Next message the phone receives from the laptop, if any within `d`.
    async fn phone_gets(&mut self, d: Duration) -> Option<AppMessage> {
        timeout(d, async {
            loop {
                if let Some(Event::Message { body, .. }) = self.phone_rx.recv().await {
                    return AppMessage::decode(&body);
                }
            }
        })
        .await
        .ok()
    }

    fn popups(&self) -> Vec<PopupCmd> {
        std::thread::sleep(Duration::from_millis(50));
        self.popups.try_iter().collect()
    }

    async fn list(&self) -> Vec<phonelink_desktop::ipc::NotificationInfo> {
        match self.request(Request::Notifications).await {
            Reply::Notifications { items, .. } => items,
            r => panic!("{r:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn notifications_flow_from_phone_to_laptop() {
    let mut r = setup().await;

    // --- a fresh WhatsApp message: listed, broadcast, and a popup (markup escaped)
    r.phone_sends(AppMessage::NotificationPosted(notif(
        "k1",
        "WhatsApp",
        "Rahul",
        "Lab at <3pm> & viva",
    )));
    let ev = r
        .event("posted", |e| {
            matches!(e, WatchEvent::NotificationPosted { .. })
        })
        .await;
    let WatchEvent::NotificationPosted { item } = ev else {
        unreachable!()
    };
    assert_eq!(
        (item.app.as_str(), item.title.as_str()),
        ("WhatsApp", "Rahul")
    );
    let id = item.id.clone();
    let shown = r.popups();
    assert!(
        matches!(shown.as_slice(), [PopupCmd::Show { id: i, app, body, .. }] if *i == id && app == "WhatsApp" && body == "Lab at &lt;3pm&gt; &amp; viva"),
        "one escaped popup"
    );

    // --- an update to the same notification replaces it (list and popup)
    r.phone_sends(AppMessage::NotificationPosted(notif(
        "k1",
        "WhatsApp",
        "Rahul (2 messages)",
        "Also bring the report",
    )));
    r.event("update", |e| {
        matches!(e, WatchEvent::NotificationPosted { .. })
    })
    .await;
    let list = r.list().await;
    assert_eq!(list.len(), 1, "same key updates in place");
    assert_eq!(list[0].title, "Rahul (2 messages)");
    assert!(
        matches!(r.popups().as_slice(), [PopupCmd::Show { id: i, .. }] if *i == id),
        "popup updated, not duplicated"
    );

    // --- an old notification (laptop was asleep): listed, but no popup
    let mut old = notif(
        "k-old",
        "Gmail",
        "Placement Cell",
        "Registration closes tomorrow",
    );
    old.posted_ms = now_ms() - 10 * 60 * 1000;
    r.phone_sends(AppMessage::NotificationPosted(old));
    r.event("old posted", |e| {
        matches!(e, WatchEvent::NotificationPosted { .. })
    })
    .await;
    assert!(r.popups().is_empty(), "no popup for old news");
    assert_eq!(r.list().await.len(), 2);

    // --- "dismiss on both devices" OFF (default)
    let Reply::Status {
        sync_dismissals, ..
    } = r.request(Request::Status).await
    else {
        panic!()
    };
    assert!(!sync_dismissals, "off by default");
    // cleared on the phone: popup closes, entry stays (marked)
    r.phone_sends(AppMessage::NotificationRemoved { key: "k1".into() });
    let ev = r
        .event("removed", |e| {
            matches!(e, WatchEvent::NotificationRemoved { .. })
        })
        .await;
    assert_eq!(
        ev,
        WatchEvent::NotificationRemoved {
            id: id.clone(),
            deleted: false
        }
    );
    assert!(matches!(r.popups().as_slice(), [PopupCmd::Close { id: i }] if *i == id));
    assert!(
        r.list()
            .await
            .iter()
            .any(|n| n.id == id && n.removed_on_phone),
        "kept, marked as cleared on phone"
    );
    // dismissed here: the phone is not touched
    let gmail_id = r
        .list()
        .await
        .iter()
        .find(|n| n.app == "Gmail")
        .unwrap()
        .id
        .clone();
    assert_eq!(
        r.request(Request::DismissNotification { id: gmail_id })
            .await,
        Reply::Done
    );
    assert!(
        r.phone_gets(Duration::from_millis(700)).await.is_none(),
        "sync off: nothing sent to the phone"
    );

    // --- turn sync ON: both directions now remove
    assert_eq!(
        r.request(Request::SetSyncDismissals { enabled: true })
            .await,
        Reply::Done
    );
    let saved = std::fs::read_to_string(r.dir.path().join("config.toml")).unwrap();
    assert!(saved.contains("sync_dismissals = true"), "{saved}");
    r.phone_sends(AppMessage::NotificationPosted(notif(
        "k2", "WhatsApp", "Mom", "Call me",
    )));
    r.event("k2", |e| matches!(e, WatchEvent::NotificationPosted { .. }))
        .await;
    let mom = r
        .list()
        .await
        .iter()
        .find(|n| n.title == "Mom")
        .unwrap()
        .id
        .clone();
    assert_eq!(
        r.request(Request::DismissNotification { id: mom }).await,
        Reply::Done
    );
    assert_eq!(
        r.phone_gets(WAIT).await,
        Some(AppMessage::DismissNotification { key: "k2".into() }),
        "sync on: phone told to dismiss"
    );
    r.phone_sends(AppMessage::NotificationPosted(notif(
        "k3",
        "WhatsApp",
        "Ananya",
        "Notes uploaded",
    )));
    r.event("k3", |e| matches!(e, WatchEvent::NotificationPosted { .. }))
        .await;
    r.phone_sends(AppMessage::NotificationRemoved { key: "k3".into() });
    let ev = r
        .event("k3 removed", |e| {
            matches!(e, WatchEvent::NotificationRemoved { .. })
        })
        .await;
    assert!(
        matches!(ev, WatchEvent::NotificationRemoved { deleted: true, .. }),
        "sync on: removed from the list"
    );

    // --- reply and action buttons go to the phone; results come back
    r.phone_sends(AppMessage::NotificationPosted(notif(
        "k4", "WhatsApp", "Rahul", "Coming?",
    )));
    r.event("k4", |e| matches!(e, WatchEvent::NotificationPosted { .. }))
        .await;
    r.phone_sends(AppMessage::NotificationPosted(notif(
        "k5", "Calendar", "OS lab", "2 pm",
    )));
    r.event("k5", |e| matches!(e, WatchEvent::NotificationPosted { .. }))
        .await;
    let items = r.list().await;
    let wa = items
        .iter()
        .find(|n| n.title == "Rahul" && !n.removed_on_phone)
        .unwrap()
        .id
        .clone();
    let cal = items
        .iter()
        .find(|n| n.app == "Calendar")
        .unwrap()
        .id
        .clone();
    let reply = r
        .request(Request::ReplyNotification {
            id: wa.clone(),
            text: "  On my way ".into(),
        })
        .await;
    assert!(
        matches!(
            reply,
            Reply::Sent {
                delivered_now: true,
                ..
            }
        ),
        "{reply:?}"
    );
    assert_eq!(
        r.phone_gets(WAIT).await,
        Some(AppMessage::ReplyNotification {
            key: "k4".into(),
            text: "On my way".into()
        })
    );
    r.phone_sends(AppMessage::ActionResult {
        key: "k4".into(),
        ok: true,
        message: "Reply sent".into(),
    });
    let ev = r
        .event("result", |e| matches!(e, WatchEvent::ActionResult { .. }))
        .await;
    assert_eq!(
        ev,
        WatchEvent::ActionResult {
            id: wa.clone(),
            ok: true,
            message: "Reply sent".into()
        }
    );
    assert!(
        matches!(
            r.request(Request::ReplyNotification {
                id: cal.clone(),
                text: "x".into()
            })
            .await,
            Reply::Error { .. }
        ),
        "no reply where the app offers none"
    );
    assert!(
        matches!(
            r.request(Request::ReplyNotification {
                id: wa.clone(),
                text: "   ".into()
            })
            .await,
            Reply::Error { .. }
        ),
        "empty reply refused"
    );
    assert!(
        matches!(
            r.request(Request::NotificationAction {
                id: cal.clone(),
                index: 5
            })
            .await,
            Reply::Error { .. }
        ),
        "bad button index refused"
    );
    assert!(matches!(
        r.request(Request::NotificationAction {
            id: cal.clone(),
            index: 0
        })
        .await,
        Reply::Sent { .. }
    ));
    assert_eq!(
        r.phone_gets(WAIT).await,
        Some(AppMessage::NotificationAction {
            key: "k5".into(),
            index: 0
        })
    );

    // --- muting an app hides it and stops its popups; unmuting brings it back
    assert_eq!(
        r.request(Request::SetAppMuted {
            package: "com.calendar".into(),
            muted: true
        })
        .await,
        Reply::Done
    );
    assert!(
        !r.list().await.iter().any(|n| n.app == "Calendar"),
        "muted app hidden"
    );
    let Reply::Notifications { apps, .. } = r.request(Request::Notifications).await else {
        panic!()
    };
    assert!(
        apps.iter().any(|a| a.package == "com.calendar" && a.muted),
        "listed as muted in settings"
    );
    r.popups();
    r.phone_sends(AppMessage::NotificationPosted(notif(
        "k6", "Calendar", "Another", "event",
    )));
    r.event("k6", |e| matches!(e, WatchEvent::NotificationPosted { .. }))
        .await;
    assert!(r.popups().is_empty(), "no popup from a muted app");
    assert!(
        r.request(Request::SetAppMuted {
            package: "../../etc".into(),
            muted: true
        })
        .await
            != Reply::Done,
        "invalid package refused"
    );
    r.request(Request::SetAppMuted {
        package: "com.calendar".into(),
        muted: false,
    })
    .await;
    assert!(r.list().await.iter().any(|n| n.app == "Calendar"));

    // --- snapshot after a reconnect replaces this phone's entries (sync on)
    r.phone_sends(AppMessage::NotificationSnapshot {
        items: vec![notif("s1", "WhatsApp", "Only this one", "left")],
    });
    // (mute/unmute above also broadcast "changed", so poll for the outcome instead)
    let titles = timeout(WAIT, async {
        loop {
            let t: Vec<String> = r
                .list()
                .await
                .into_iter()
                .filter(|n| !n.removed_on_phone)
                .map(|n| n.title)
                .collect();
            if t == vec!["Only this one".to_string()] {
                return t;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("snapshot replaced this phone's entries");
    assert_eq!(titles.len(), 1);
    // The one cleared on the phone back when sync was off stays: turning sync on later
    // doesn't retroactively delete what you chose to keep here.
    let kept: Vec<String> = r
        .list()
        .await
        .into_iter()
        .filter(|n| n.removed_on_phone)
        .map(|n| n.id)
        .collect();
    assert_eq!(kept, vec![id.clone()]);

    // --- app icons: valid PNG stored and served; a path-traversal name is ignored
    let png = b"\x89PNG\r\n\x1a\nfake-icon-bytes".to_vec();
    r.phone_sends(AppMessage::AppIcon {
        package: "com.whatsapp".into(),
        png: png.clone(),
    });
    r.event("icon", |e| matches!(e, WatchEvent::AppIcon { .. }))
        .await;
    r.phone_sends(AppMessage::AppIcon {
        package: "../../evil".into(),
        png,
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    let Reply::AppIcons { icons } = r.request(Request::AppIcons).await else {
        panic!()
    };
    assert_eq!(
        icons.keys().cloned().collect::<Vec<_>>(),
        vec!["com.whatsapp".to_string()]
    );
    assert!(icons["com.whatsapp"].starts_with("data:image/png;base64,"));
    assert!(
        !r.dir.path().join("evil.png").exists() && !r.dir.path().join("data/evil.png").exists()
    );

    // --- "Reconnect now" is accepted (it nudges the reconnect loops)
    assert_eq!(r.request(Request::Reconnect).await, Reply::Done);

    // --- clear all (sync on): the phone is asked to dismiss what it still shows
    assert_eq!(r.request(Request::ClearNotifications).await, Reply::Done);
    assert!(r.list().await.is_empty());
    assert_eq!(
        r.phone_gets(WAIT).await,
        Some(AppMessage::DismissNotification { key: "s1".into() })
    );
}
