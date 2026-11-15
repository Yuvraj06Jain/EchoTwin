//! Approving sudo with the phone: the laptop service only relays (the root-owned helper,
//! tested in packaging/sudo-approval/test.sh, checks signatures). Simulated phone.

use std::{
    io::{BufRead, BufReader},
    net::SocketAddr,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};

use phonelink_core::{AppMessage, Event, Fingerprint, Node, NodeConfig, PairingOffer, CAP_APPROVE};
use phonelink_desktop::{
    client::Client,
    config::Config,
    daemon::{run, DaemonOptions},
    effects::Recorded,
    ipc::{Reply, Request},
    lock::Buses,
    notifications::Popups,
};
use tokio::{sync::mpsc::UnboundedReceiver, time::timeout};

const WAIT: Duration = Duration::from_secs(15);

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


struct Rig {
    socket: std::path::PathBuf,
    _dir: tempfile::TempDir,
    phone: Node,
    phone_rx: UnboundedReceiver<Event>,
    laptop: Fingerprint,
    _effects: Arc<Recorded>,
    _stop: tokio::sync::oneshot::Sender<()>,
}

async fn setup_caps(bus: &str, caps: &[&str]) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("pl.sock");
    let effects = Recorded::new();
    let opts = DaemonOptions {
        data_dir: dir.path().join("data"),
        socket: socket.clone(),
        config: Config { port: 0, discovery: false, ..Config::default() },
        config_path: Some(dir.path().join("config.toml")),
        popups: Some(Popups::recording().0),
        buses: Buses { session: Some(bus.to_string()), system: Buses::unreachable().system },
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
    let Some(Reply::PairStarted { uri, .. }) = c.next().await.unwrap() else { panic!() };
    let mut offer = PairingOffer::from_uri(&uri).unwrap();
    offer.addrs = vec![SocketAddr::from(([127, 0, 0, 1], offer.addrs[0].port()))];
    let mut cfg = NodeConfig::new("Galaxy S24 FE", None, "127.0.0.1:0".parse().unwrap());
    cfg.capabilities.extend(caps.iter().map(|c| c.to_string()));
    let (phone, mut phone_rx) = Node::start(cfg).await.unwrap();
    phone.pair_with(&offer).await.unwrap();
    timeout(WAIT, async {
        while !matches!(phone_rx.recv().await, Some(Event::PeerConnected { .. })) {}
    })
    .await
    .unwrap();
    Rig { socket, _dir: dir, phone, phone_rx, laptop: offer.fingerprint, _effects: effects, _stop: stop }
}

impl Rig {
    fn send(&self, m: AppMessage) {
        self.phone.send(&self.laptop, m.encode().unwrap()).unwrap();
    }

    async fn request(&self, r: Request) -> Reply {
        Client::connect(&self.socket).await.unwrap().request(&r).await.unwrap()
    }

    async fn phone_waits_for<T>(&mut self, what: &str, f: impl Fn(AppMessage) -> Option<T>) -> T {
        timeout(WAIT, async {
            loop {
                if let Some(Event::Message { body, .. }) = self.phone_rx.recv().await {
                    if let Some(t) = f(AppMessage::decode(&body)) {
                        return t;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
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


fn relay(nonce: &str, secs: u64) -> Request {
    Request::ApproveRelay { what: "sudo".into(), host: "MARK-I".into(), nonce_hex: nonce.into(), timeout_secs: Some(secs) }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_laptop_relays_approvals_and_keeps_the_phone_key_for_root() {
    let Some(bus) = private_bus() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let mut rig = setup_caps(&bus.address, &[CAP_APPROVE]).await;

    // Enrolment: the phone's public key is kept as a PEM for the root-owned setup script.
    let der: Vec<u8> = [vec![0x30, 0x59, 0x30, 0x13], vec![7u8; 87]].concat();
    rig.send(AppMessage::ApproveEnroll { public_key: der.clone() });
    let pem = rig._dir.path().join("data/approve-pending.pem");
    eventually("pending key written", || pem.exists()).await;
    let text = std::fs::read_to_string(&pem).unwrap();
    assert!(text.starts_with("-----BEGIN PUBLIC KEY-----\n") && text.ends_with("-----END PUBLIC KEY-----\n"), "{text}");
    use base64::Engine;
    let body: String = text.lines().filter(|l| !l.starts_with("-----")).collect();
    assert_eq!(base64::engine::general_purpose::STANDARD.decode(body).unwrap(), der);
    // Not a DER sequence, or too big: ignored.
    std::fs::remove_file(&pem).unwrap();
    rig.send(AppMessage::ApproveEnroll { public_key: vec![1, 2, 3] });
    rig.send(AppMessage::ApproveEnroll { public_key: vec![0x30; 500] });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!pem.exists());

    // A relay: the phone gets the request; its signature comes back as hex.
    let nonce = "0123456789abcdef".repeat(4);
    let socket = rig.socket.clone();
    let n = nonce.clone();
    let ask = tokio::spawn(async move { Client::connect(&socket).await.unwrap().request(&relay(&n, 10)).await.unwrap() });
    let (id, what, host, got_nonce, sent_ms) = rig
        .phone_waits_for("request", |m| match m {
            AppMessage::ApproveRequest { id, what, host, nonce_hex, sent_ms } => Some((id, what, host, nonce_hex, sent_ms)),
            _ => None,
        })
        .await;
    assert_eq!((what.as_str(), host.as_str(), got_nonce.as_str()), ("sudo", "MARK-I", nonce.as_str()));
    assert!(sent_ms.abs_diff(now_ms()) < 10_000);
    rig.send(AppMessage::ApproveResponse { id, approved: true, signature: vec![0x30, 0x45, 0xab] });
    assert_eq!(ask.await.unwrap(), Reply::ApproveSignature { hex: "3045ab".into() });

    // Two at once are matched by id; a denial is an error.
    let sock = rig.socket.clone();
    let start = |secs| {
        let socket = sock.clone();
        let n = nonce.clone();
        tokio::spawn(async move { Client::connect(&socket).await.unwrap().request(&relay(&n, secs)).await.unwrap() })
    };
    let a = start(10);
    let id_a = rig.phone_waits_for("a", |m| match m { AppMessage::ApproveRequest { id, .. } => Some(id), _ => None }).await;
    let b = start(10);
    let id_b = rig.phone_waits_for("b", |m| match m { AppMessage::ApproveRequest { id, .. } => Some(id), _ => None }).await;
    rig.send(AppMessage::ApproveResponse { id: id_b, approved: true, signature: vec![0xbb] });
    rig.send(AppMessage::ApproveResponse { id: id_a, approved: false, signature: vec![] });
    assert_eq!(b.await.unwrap(), Reply::ApproveSignature { hex: "bb".into() });
    assert!(matches!(a.await.unwrap(), Reply::Error { message } if message.contains("denied")));

    // No answer: an error after the timeout.
    let c = start(1);
    rig.phone_waits_for("c", |m| matches!(m, AppMessage::ApproveRequest { .. }).then_some(())).await;
    assert!(matches!(c.await.unwrap(), Reply::Error { message } if message.contains("no answer")));

    // Refused at once: a bad nonce, bad text, switched off.
    assert!(matches!(rig.request(relay("abc", 5)).await, Reply::Error { .. }));
    let bad_host = Request::ApproveRelay { what: "sudo".into(), host: "a\nb".into(), nonce_hex: nonce.clone(), timeout_secs: None };
    assert!(matches!(rig.request(bad_host).await, Reply::Error { .. }));
    assert_eq!(rig.request(Request::SetSwitch { name: "phone_approval".into(), on: false }).await, Reply::Done);
    let t = std::time::Instant::now();
    assert!(matches!(rig.request(relay(&nonce, 10)).await, Reply::Error { message } if message.contains("switched off")));
    assert!(t.elapsed() < Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn without_an_approving_phone_the_relay_fails_at_once() {
    let Some(bus) = private_bus() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let rig = setup_caps(&bus.address, &[]).await;
    let t = std::time::Instant::now();
    let r = rig.request(relay(&"ab".repeat(32), 10)).await;
    assert!(matches!(r, Reply::Error { message } if message.contains("no phone")));
    assert!(t.elapsed() < Duration::from_secs(2));
}
