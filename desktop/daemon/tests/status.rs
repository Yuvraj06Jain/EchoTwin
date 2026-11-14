//! Battery and network status both ways, the panel widget's JSON, battery alerts, and the
//! small "do this on the other device" requests (open a link, ring), with a simulated phone.

use std::{net::SocketAddr, time::Duration};

use phonelink_core::{
    AppMessage, Event, Fingerprint, Node, NodeConfig, PairingOffer, PhoneStatus, CAP_STATUS,
};
use phonelink_desktop::{
    client::Client,
    config::Config,
    daemon::{run, DaemonOptions},
    ipc::{Reply, Request, WatchEvent},
    lock::Buses,
    effects::Recorded,
    notifications::{PopupCmd, Popups},
};
use tokio::{sync::mpsc::UnboundedReceiver, time::timeout};

const WAIT: Duration = Duration::from_secs(15);

struct Rig {
    socket: std::path::PathBuf,
    _dir: tempfile::TempDir,
    phone: Node,
    phone_rx: UnboundedReceiver<Event>,
    laptop: Fingerprint,
    watch: Client,
    popups: std::sync::mpsc::Receiver<PopupCmd>,
    effects: std::sync::Arc<Recorded>,
    _stop: tokio::sync::oneshot::Sender<()>,
}

async fn setup(phone_caps: &[&str]) -> Rig {
    setup_with(phone_caps, Config::default()).await
}

async fn setup_with(phone_caps: &[&str], base: Config) -> Rig {
    let effects = Recorded::new();
    let dir = tempfile::tempdir().unwrap();
    let bat = dir.path().join("power/BAT0");
    std::fs::create_dir_all(&bat).unwrap();
    for (f, v) in [("type", "Battery"), ("capacity", "50"), ("status", "Charging")] {
        std::fs::write(bat.join(f), v).unwrap();
    }
    let socket = dir.path().join("pl.sock");
    let (popups, popup_rx) = Popups::recording();
    let opts = DaemonOptions {
        data_dir: dir.path().join("data"),
        socket: socket.clone(),
        config: Config {
            port: 0,
            discovery: false,
            power_supply_dir: Some(dir.path().join("power").to_string_lossy().into()),
            ..base
        },
        config_path: None,
        popups: Some(popups),
        buses: Buses::unreachable(),
        download_dir: dir.path().join("downloads"),
        effects: Some(effects.clone()),
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
    cfg.capabilities.extend(phone_caps.iter().map(|c| c.to_string()));
    let (phone, mut phone_rx) = Node::start(cfg).await.unwrap();
    phone.pair_with(&offer).await.unwrap();
    timeout(WAIT, async {
        while !matches!(phone_rx.recv().await, Some(Event::PeerConnected { .. })) {}
    })
    .await
    .unwrap();
    let mut watch = Client::connect(&socket).await.unwrap();
    watch.send(&Request::Watch).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    Rig {
        socket,
        _dir: dir,
        phone,
        phone_rx,
        laptop: offer.fingerprint,
        watch,
        popups: popup_rx,
        effects,
        _stop: stop,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn status_both_ways_and_widget_json() {
    let mut rig = setup(&[CAP_STATUS]).await;

    // The laptop tells the phone its battery as soon as it connects.
    let got = timeout(WAIT, async {
        loop {
            if let Some(Event::Message { body, .. }) = rig.phone_rx.recv().await {
                if let m @ AppMessage::ComputerStatus { .. } = AppMessage::decode(&body) {
                    return m;
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(got, AppMessage::ComputerStatus { battery: Some(50), charging: true });

    let status = PhoneStatus { battery: Some(82), charging: false, network: "5G".into(), signal: 3 };
    rig.phone.send(&rig.laptop, AppMessage::PhoneStatus(status.clone()).encode().unwrap()).unwrap();
    timeout(WAIT, async {
        loop {
            if let Some(Reply::Event { event: WatchEvent::PhoneStatusChanged { .. } }) =
                rig.watch.next().await.unwrap()
            {
                return;
            }
        }
    })
    .await
    .unwrap();
    let mut c = Client::connect(&rig.socket).await.unwrap();
    let Reply::Devices { devices } = c.request(&Request::Devices).await.unwrap() else { panic!() };
    assert_eq!(devices[0].status.as_ref(), Some(&status));

    // The widget's one-call JSON, through the real CLI.
    let sock = rig.socket.clone();
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_phonelink"))
            .arg("widget")
            .env("PHONELINK_SOCKET", sock)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["paired"], true);
    assert_eq!(v["phone"]["name"], "Galaxy S24 FE");
    assert_eq!(v["phone"]["connected"], true);
    assert_eq!(v["phone"]["can_ring"], false, "this phone did not say it can ring");
    assert_eq!(v["battery_saver"], true, "the widget's saver button shows the real state");
    assert_eq!(v["phone"]["status"]["battery"], 82);
    assert_eq!(v["phone"]["status"]["network"], "5G");
    assert!(v["notifications"].is_array() && v["sms"].is_array() && v["transfers"].is_array());
    assert!(v["media"].is_null());

    // Hostile values from the phone are bounded.
    let bad = PhoneStatus { battery: Some(255), charging: false, network: "x".repeat(500), signal: 99 };
    rig.phone.send(&rig.laptop, AppMessage::PhoneStatus(bad).encode().unwrap()).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let Reply::Devices { devices } = c.request(&Request::Devices).await.unwrap() else { panic!() };
    let s = devices[0].status.clone().unwrap();
    assert_eq!((s.battery, s.signal), (None, 4));
    assert!(s.network.len() <= 16);

    // Offline: the status is forgotten.
    rig.phone.shutdown().await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let Reply::Devices { devices } = c.request(&Request::Devices).await.unwrap() else { panic!() };
    assert!(devices[0].status.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_connection_switch() {
    let mut rig = setup(&[CAP_STATUS]).await;
    let mut c = Client::connect(&rig.socket).await.unwrap();
    let pfp = rig.phone.fingerprint();

    // Off on the laptop: the phone is told and dropped; Status says so.
    assert!(matches!(c.request(&Request::SetConnection { on: false }).await.unwrap(), Reply::Done));
    timeout(WAIT, async {
        while !matches!(rig.phone_rx.recv().await, Some(Event::PeerPaused { .. })) {}
    })
    .await
    .expect("phone told the laptop switched off");
    let Reply::Status { connection, .. } = c.request(&Request::Status).await.unwrap() else { panic!() };
    assert!(!connection);
    timeout(WAIT, async {
        loop {
            let Reply::Devices { devices } = c.request(&Request::Devices).await.unwrap() else { panic!() };
            if !devices[0].connected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("disconnected");
    assert!(rig.phone.connect_any(rig.laptop, vec![]).await.is_err());

    // On again: the laptop dials the phone by itself.
    c.request(&Request::SetConnection { on: true }).await.unwrap();
    timeout(WAIT, async {
        loop {
            if rig.phone.is_connected(&rig.laptop) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("reconnected after switching on");

    // Off on the phone: the laptop shows it and doesn't keep dialling.
    rig.phone.set_paused(true);
    timeout(WAIT, async {
        loop {
            if let Some(Reply::Event { event: WatchEvent::PeerPaused { .. } }) = rig.watch.next().await.unwrap() {
                return;
            }
        }
    })
    .await
    .expect("laptop sees the phone switched off");
    tokio::time::sleep(Duration::from_millis(600)).await;
    let Reply::Devices { devices } = c.request(&Request::Devices).await.unwrap() else { panic!() };
    assert!(devices[0].paused_by_peer && !devices[0].connected);
    let _ = pfp;
}

/// How many `PhoneStatusChanged` events arrive within `d`.
async fn status_events(watch: &mut Client, d: Duration) -> usize {
    let mut n = 0;
    let end = tokio::time::Instant::now() + d;
    while let Ok(r) = tokio::time::timeout_at(end, watch.next()).await {
        if let Some(Reply::Event { event: WatchEvent::PhoneStatusChanged { .. } }) = r.unwrap() {
            n += 1;
        }
    }
    n
}

#[tokio::test(flavor = "multi_thread")]
async fn wobbling_signal_bars_are_announced_rarely() {
    let mut rig = setup(&[CAP_STATUS]).await;
    let status = |battery: u8, signal: u8| PhoneStatus {
        battery: Some(battery),
        charging: false,
        network: "5G".into(),
        signal,
    };
    let send = |s: PhoneStatus| rig.phone.send(&rig.laptop, AppMessage::PhoneStatus(s).encode().unwrap()).unwrap();
    send(status(80, 1));
    assert_eq!(status_events(&mut rig.watch, Duration::from_secs(1)).await, 1, "first status at once");

    // Only the bars move: stored (the next status request shows them), not announced yet.
    for signal in [2, 3, 2] {
        send(status(80, signal));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(status_events(&mut rig.watch, Duration::from_millis(1500)).await, 0);
    let mut c = Client::connect(&rig.socket).await.unwrap();
    let Reply::Devices { devices } = c.request(&Request::Devices).await.unwrap() else { panic!() };
    assert_eq!(devices[0].status.as_ref().map(|s| s.signal), Some(2));

    // A battery change goes out immediately (and covers the held-back bars).
    send(status(79, 2));
    assert_eq!(status_events(&mut rig.watch, Duration::from_secs(1)).await, 1);

    // Bars alone again: announced once, when the 10 s quiet time ends.
    send(status(79, 4));
    assert_eq!(status_events(&mut rig.watch, Duration::from_secs(6)).await, 0, "held back");
    assert_eq!(status_events(&mut rig.watch, Duration::from_secs(6)).await, 1, "then announced");
}

fn widget(socket: &std::path::Path, args: &[&str]) -> std::process::Command {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_phonelink"));
    cmd.arg("widget").args(args).env("PHONELINK_SOCKET", socket);
    cmd
}

async fn widget_json(mut cmd: std::process::Command) -> serde_json::Value {
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap()).await.unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_widget_waits_for_changes() {
    let rig = setup(&[CAP_STATUS]).await;
    let first = widget_json(widget(&rig.socket, &[])).await;
    let hash = first["hash"].as_str().unwrap().to_string();
    assert_eq!(hash.len(), 16);

    // A different hash than the current state: printed at once.
    let started = std::time::Instant::now();
    let v = widget_json(widget(&rig.socket, &["--wait", "0000000000000000"])).await;
    assert_eq!(v["hash"], hash.as_str(), "nothing changed: same hash");
    assert!(started.elapsed() < Duration::from_secs(3));

    // The current hash: blocks until something changes…
    let mut child = widget(&rig.socket, &["--wait", &hash])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(child.try_wait().unwrap().is_none(), "still waiting");
    let status = PhoneStatus { battery: Some(64), charging: true, network: "Wi-Fi".into(), signal: 4 };
    rig.phone.send(&rig.laptop, AppMessage::PhoneStatus(status).encode().unwrap()).unwrap();
    let out = timeout(WAIT, tokio::task::spawn_blocking(move || child.wait_with_output().unwrap()))
        .await
        .expect("printed after the change")
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_ne!(v["hash"], hash.as_str());
    assert_eq!(v["phone"]["status"]["battery"], 64);

    // …or until the timeout, then prints the unchanged state anyway.
    let hash = v["hash"].as_str().unwrap().to_string();
    let started = std::time::Instant::now();
    let v = widget_json(widget(&rig.socket, &["--wait", &hash, "--timeout", "2"])).await;
    let waited = started.elapsed();
    assert_eq!(v["hash"], hash.as_str());
    assert!(waited >= Duration::from_secs(2) && waited < Duration::from_secs(6), "{waited:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_battery_saver_switch_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("pl.sock");
    let config_path = dir.path().join("config.toml");
    let opts = DaemonOptions {
        data_dir: dir.path().join("data"),
        socket: socket.clone(),
        config: Config { port: 0, discovery: false, ..Config::default() },
        config_path: Some(config_path.clone()),
        popups: Some(Popups::recording().0),
        buses: Buses::unreachable(),
        download_dir: dir.path().join("downloads"),
        effects: None,
    };
    let (_stop, stop_rx) = tokio::sync::oneshot::channel::<()>();
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
    let saver = |r: Reply| match r {
        Reply::Status { battery_saver, .. } => battery_saver,
        other => panic!("{other:?}"),
    };
    assert!(saver(c.request(&Request::Status).await.unwrap()), "on by default");

    // Through the real CLI.
    let sock = socket.clone();
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_phonelink"))
            .args(["saver", "off"])
            .env("PHONELINK_SOCKET", sock)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!saver(c.request(&Request::Status).await.unwrap()));
    assert!(!Config::load(&config_path).unwrap().battery_saver, "saved");

    assert!(matches!(c.request(&Request::SetBatterySaver { on: true }).await.unwrap(), Reply::Done));
    assert!(saver(c.request(&Request::Status).await.unwrap()));
    assert!(Config::load(&config_path).unwrap().battery_saver);

    // The simple switches: listed in the status, saved, unknown names refused.
    let switches = |r: Reply| match r {
        Reply::Status { switches, .. } => switches,
        other => panic!("{other:?}"),
    };
    assert_eq!(switches(c.request(&Request::Status).await.unwrap()).get("battery_alerts"), Some(&true));
    let off = Request::SetSwitch { name: "battery_alerts".into(), on: false };
    assert!(matches!(c.request(&off).await.unwrap(), Reply::Done));
    assert_eq!(switches(c.request(&Request::Status).await.unwrap()).get("battery_alerts"), Some(&false));
    assert!(!Config::load(&config_path).unwrap().battery_alerts, "saved");
    let bad = Request::SetSwitch { name: "port".into(), on: false };
    assert!(matches!(c.request(&bad).await.unwrap(), Reply::Error { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn low_and_full_battery_popups_come_once() {
    let rig = setup(&[CAP_STATUS]).await;
    let send = |b: u8, charging: bool| {
        let s = PhoneStatus { battery: Some(b), charging, network: "5G".into(), signal: 3 };
        rig.phone.send(&rig.laptop, AppMessage::PhoneStatus(s).encode().unwrap()).unwrap();
    };
    let battery_popups = |d: Duration| {
        std::thread::sleep(d);
        rig.popups
            .try_iter()
            .filter_map(|p| match p {
                PopupCmd::Show { id, title, .. } if id.starts_with("status/battery/") => Some(title),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    battery_popups(Duration::from_millis(300)); // the "connected" popup etc.
    for (b, c) in [(40, false), (15, false), (12, false), (14, false)] {
        send(b, c);
    }
    assert_eq!(battery_popups(Duration::from_millis(800)), ["Galaxy S24 FE battery low"]);
    for (b, c) in [(30, true), (99, true), (100, true), (100, true)] {
        send(b, c);
    }
    assert_eq!(battery_popups(Duration::from_millis(800)), ["Galaxy S24 FE is fully charged"]);
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

#[tokio::test(flavor = "multi_thread")]
async fn links_open_on_the_other_device_but_only_plain_fresh_web_links() {
    let mut rig = setup(&[phonelink_core::CAP_LINKS]).await;
    let send = |url: &str, sent_ms: u64| {
        let m = AppMessage::OpenUrl { url: url.into(), sent_ms };
        rig.phone.send(&rig.laptop, m.encode().unwrap()).unwrap();
    };
    send("https://example.org/notes?id=4", now());
    for bad in ["file:///etc/passwd", "javascript:alert(1)", "https://exa mple.org"] {
        send(bad, now());
    }
    send(&format!("https://example.org/{}", "a".repeat(3000)), now());
    send("https://example.org/old", now() - 10 * 60_000);
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(rig.effects.take(), ["open https://example.org/notes?id=4"], "only the good one, once");
    let titles: Vec<_> = rig
        .popups
        .try_iter()
        .filter_map(|p| match p {
            PopupCmd::Show { id, title, body, .. } if id.starts_with("status/link/") => Some((title, body)),
            _ => None,
        })
        .collect();
    assert_eq!(titles, [("Opened a link from Galaxy S24 FE".to_string(), "example.org".to_string())]);

    // Laptop → phone.
    let c = Client::connect(&rig.socket).await.unwrap();
    let mut c = c;
    let r = c
        .request(&Request::OpenUrl { device: None, url: "https://example.org/x".into() })
        .await
        .unwrap();
    assert!(matches!(r, Reply::Sent { delivered_now: true, .. }), "{r:?}");
    let got = timeout(WAIT, async {
        loop {
            if let Some(Event::Message { body, .. }) = rig.phone_rx.recv().await {
                if let AppMessage::OpenUrl { url, sent_ms } = AppMessage::decode(&body) {
                    return (url, sent_ms);
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(got.0, "https://example.org/x");
    assert!(got.1.abs_diff(now()) < 10_000);
    let r = c
        .request(&Request::OpenUrl { device: None, url: "file:///etc/passwd".into() })
        .await
        .unwrap();
    assert!(matches!(r, Reply::Error { .. }));

    // Switched off: nothing opens.
    let off = Request::SetSwitch { name: "open_links".into(), on: false };
    assert!(matches!(c.request(&off).await.unwrap(), Reply::Done));
    send("https://example.org/again", now());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(rig.effects.take().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_without_links_support_is_told_to_update() {
    let rig = setup(&[]).await;
    let mut c = Client::connect(&rig.socket).await.unwrap();
    let r = c
        .request(&Request::OpenUrl { device: None, url: "https://example.org".into() })
        .await
        .unwrap();
    assert!(matches!(&r, Reply::Error { message } if message.contains("update")), "{r:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn ringing_both_ways() {
    let mut rig = setup(&[phonelink_core::CAP_RING]).await;
    let ring = |on: bool, sent_ms: u64| {
        rig.phone.send(&rig.laptop, AppMessage::Ring { on, sent_ms }.encode().unwrap()).unwrap();
    };
    let ring_popups = |d: Duration| {
        std::thread::sleep(d);
        rig.popups
            .try_iter()
            .filter_map(|p| match p {
                PopupCmd::Show { id, actions, timeout_ms, .. } if id == "status/ring" => {
                    Some(format!("show {} {timeout_ms}", actions[0].0))
                }
                PopupCmd::Close { id } if id == "status/ring" => Some("close".into()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    ring_popups(Duration::from_millis(200));

    // Phone rings the laptop, then stops it.
    ring(true, now());
    assert_eq!(ring_popups(Duration::from_millis(600)), ["show ring-stop 60000"]);
    assert_eq!(rig.effects.take(), ["ring on"]);
    ring(false, now());
    assert_eq!(ring_popups(Duration::from_millis(600)), ["close"]);
    assert_eq!(rig.effects.take(), ["ring off"]);

    // A stale request (it waited in a queue) is ignored.
    ring(true, now() - 10 * 60_000);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(rig.effects.take().is_empty());

    // Switched off: the phone can't ring it.
    let mut c = Client::connect(&rig.socket).await.unwrap();
    let off = Request::SetSwitch { name: "allow_ring".into(), on: false };
    assert!(matches!(c.request(&off).await.unwrap(), Reply::Done));
    ring(true, now());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(rig.effects.take().is_empty());

    // Laptop rings the phone: the request reaches it with a fresh timestamp.
    let r = c.request(&Request::Ring { device: None, on: true }).await.unwrap();
    assert_eq!(r, Reply::Done);
    let got = timeout(WAIT, async {
        loop {
            if let Some(Event::Message { body, .. }) = rig.phone_rx.recv().await {
                if let AppMessage::Ring { on, sent_ms } = AppMessage::decode(&body) {
                    return (on, sent_ms);
                }
            }
        }
    })
    .await
    .unwrap();
    assert!(got.0 && got.1.abs_diff(now()) < 10_000);
}
