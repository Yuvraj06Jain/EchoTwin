//! Drive a real phonelinkd over its control socket, with a simulated phone.

use std::{
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use phonelink_core::{AppMessage, Event, Node, NodeConfig, PairingOffer};
use phonelink_desktop::{
    client::Client,
    config::Config,
    daemon::{run, DaemonOptions},
    ipc::{Reply, Request, WatchEvent},
};
use tokio::{sync::oneshot, time::timeout};

const WAIT: Duration = Duration::from_secs(15);

struct TestDaemon {
    socket: PathBuf,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    _dir: tempfile::TempDir,
}

async fn start_daemon() -> TestDaemon {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("run/phonelink.sock");
    let config = Config {
        port: 0,
        device_name: Some("Test Laptop".into()),
        notifications: false,
        discovery: false,
        ..Config::default()
    };
    let opts = DaemonOptions {
        data_dir: dir.path().join("data"),
        socket: socket.clone(),
        config,
        config_path: Some(dir.path().join("config.toml")),
        popups: Some(phonelink_desktop::notifications::Popups::recording().0),
    };
    let (tx, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(run(opts, async move {
        let _ = rx.await;
    }));
    timeout(WAIT, async {
        while Client::connect(&socket).await.is_err() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("daemon socket never appeared");
    TestDaemon {
        socket,
        stop: Some(tx),
        task,
        _dir: dir,
    }
}

async fn client(sock: &Path) -> Client {
    Client::connect(sock).await.unwrap()
}

fn cli(sock: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_phonelink"))
        .args(args)
        .env("PHONELINK_SOCKET", sock)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

async fn phone() -> (Node, tokio::sync::mpsc::UnboundedReceiver<Event>) {
    Node::start(NodeConfig::new(
        "Galaxy S24 FE",
        None,
        "127.0.0.1:0".parse().unwrap(),
    ))
    .await
    .unwrap()
}

async fn wait_event<F: Fn(&Event) -> bool>(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    f: F,
) -> Event {
    timeout(WAIT, async {
        loop {
            let e = rx.recv().await.unwrap();
            if f(&e) {
                return e;
            }
        }
    })
    .await
    .expect("event")
}

#[tokio::test(flavor = "multi_thread")]
async fn full_pairing_and_messaging_through_the_daemon() {
    let d = start_daemon().await;
    let mode = std::fs::metadata(&d.socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "control socket must be private");

    let (ok, text) = cli(&d.socket, &["devices"]);
    assert!(ok && text.contains("No paired devices"), "{text}");

    // --- pairing, exactly as the CLI does it
    let mut pairing = client(&d.socket).await;
    pairing.send(&Request::Pair { ttl_secs: 60 }).await.unwrap();
    let Some(Reply::PairStarted { uri, id, addrs, .. }) = pairing.next().await.unwrap() else {
        panic!("expected PairStarted");
    };
    assert!(!addrs.is_empty());
    let mut offer = PairingOffer::from_uri(&uri).unwrap();
    assert_eq!(
        offer.fingerprint.short(),
        id,
        "QR fingerprint matches what the CLI prints"
    );
    let port = offer.addrs[0].port();
    offer.addrs = vec![SocketAddr::from(([127, 0, 0, 1], port))]; // the test phone lives on loopback

    // A stranger who sees the pairing window open but not the QR code gets nowhere.
    let (stranger, _s) = Node::start(NodeConfig::new(
        "Stranger",
        None,
        "127.0.0.1:0".parse().unwrap(),
    ))
    .await
    .unwrap();
    let fake = PairingOffer {
        secret: [0x42; 32],
        ..offer.clone()
    };
    assert!(stranger.pair_with(&fake).await.is_err());

    let (phone, mut prx) = phone().await;
    phone.pair_with(&offer).await.unwrap();
    let Some(Reply::Paired { device }) = timeout(WAIT, pairing.next()).await.unwrap().unwrap()
    else {
        panic!("expected Paired");
    };
    assert_eq!(device.name, "Galaxy S24 FE");
    wait_event(&mut prx, |e| matches!(e, Event::PeerConnected { .. })).await;

    // --- the CLI sees one connected device (and not the stranger)
    let (ok, text) = cli(&d.socket, &["devices"]);
    assert!(
        ok && text.contains("Galaxy S24 FE") && text.contains("connected"),
        "{text}"
    );
    assert!(!text.contains("Stranger"), "{text}");

    // --- laptop -> phone via the CLI
    let (ok, text) = cli(&d.socket, &["send-text", "hello", "phone"]);
    assert!(ok && text.contains("Sent"), "{text}");
    let Event::Message { body, .. } =
        wait_event(&mut prx, |e| matches!(e, Event::Message { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(
        AppMessage::decode(&body),
        AppMessage::Text {
            text: "hello phone".into()
        }
    );

    // --- phone -> laptop shows up for watchers
    let mut watcher = client(&d.socket).await;
    watcher.send(&Request::Watch).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    phone
        .send(
            &offer.fingerprint,
            AppMessage::Text {
                text: "hi laptop".into(),
            }
            .encode()
            .unwrap(),
        )
        .unwrap();
    let got = timeout(WAIT, watcher.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        got,
        Reply::Event {
            event: WatchEvent::Text {
                id: phone.fingerprint().short(),
                name: "Galaxy S24 FE".into(),
                text: "hi laptop".into()
            }
        }
    );

    // --- device lookup by name; unpair via the CLI
    let (ok, text) = cli(&d.socket, &["send-text", "--to", "nobody", "x"]);
    assert!(!ok && text.contains("no paired device matches"), "{text}");
    let (ok, text) = cli(&d.socket, &["unpair", "galaxy s24 fe"]);
    assert!(ok, "{text}");
    let (_, text) = cli(&d.socket, &["devices"]);
    assert!(text.contains("No paired devices"), "{text}");

    // --- desktop notifications can be toggled at runtime and are saved
    let mut c = client(&d.socket).await;
    assert_eq!(
        c.request(&Request::SetNotifications { enabled: true })
            .await
            .unwrap(),
        Reply::Done
    );
    let Reply::Status { notifications, .. } = c.request(&Request::Status).await.unwrap() else {
        panic!()
    };
    assert!(notifications, "status reflects the new setting");
    let saved = std::fs::read_to_string(d._dir.path().join("config.toml")).unwrap();
    assert!(saved.contains("notifications = true"), "{saved}");

    // --- a second daemon on the same socket refuses to start
    let dup = DaemonOptions {
        data_dir: d._dir.path().join("data2"),
        socket: d.socket.clone(),
        config: Config {
            port: 0,
            notifications: false,
            discovery: false,
            ..Config::default()
        },
        config_path: None,
        popups: Some(phonelink_desktop::notifications::Popups::recording().0),
    };
    let err = run(dup, async {}).await.unwrap_err();
    assert!(err.to_string().contains("already running"), "{err}");

    // --- clean shutdown removes the socket
    let TestDaemon {
        socket,
        mut stop,
        task,
        _dir,
    } = d;
    stop.take().unwrap().send(()).unwrap();
    timeout(WAIT, task).await.unwrap().unwrap().unwrap();
    assert!(!socket.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_pair_closes_the_window() {
    let d = start_daemon().await;
    let mut pairing = client(&d.socket).await;
    pairing.send(&Request::Pair { ttl_secs: 60 }).await.unwrap();
    let Some(Reply::PairStarted { uri, .. }) = pairing.next().await.unwrap() else {
        panic!()
    };
    drop(pairing); // user pressed Ctrl-C
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut offer = PairingOffer::from_uri(&uri).unwrap();
    offer.addrs = vec![SocketAddr::from(([127, 0, 0, 1], offer.addrs[0].port()))];
    let (phone, _p) = phone().await;
    assert!(
        phone.pair_with(&offer).await.is_err(),
        "window must close when the CLI goes away"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn daemon_with_no_cli_reports_helpful_error() {
    let dir = tempfile::tempdir().unwrap();
    let (ok, text) = cli(&dir.path().join("missing.sock"), &["status"]);
    assert!(
        !ok && text.contains("systemctl --user start phonelinkd"),
        "{text}"
    );
}
