//! Battery and network status both ways, and the panel widget's JSON, with a simulated phone.

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
    notifications::Popups,
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
    _stop: tokio::sync::oneshot::Sender<()>,
}

async fn setup(phone_caps: &[&str]) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let bat = dir.path().join("power/BAT0");
    std::fs::create_dir_all(&bat).unwrap();
    for (f, v) in [("type", "Battery"), ("capacity", "50"), ("status", "Charging")] {
        std::fs::write(bat.join(f), v).unwrap();
    }
    let socket = dir.path().join("pl.sock");
    let opts = DaemonOptions {
        data_dir: dir.path().join("data"),
        socket: socket.clone(),
        config: Config {
            port: 0,
            discovery: false,
            power_supply_dir: Some(dir.path().join("power").to_string_lossy().into()),
            ..Config::default()
        },
        config_path: None,
        popups: Some(Popups::recording().0),
        buses: Buses::unreachable(),
        download_dir: dir.path().join("downloads"),
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
