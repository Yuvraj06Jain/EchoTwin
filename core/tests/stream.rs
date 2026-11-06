//! The Bluetooth backup link, with an in-memory pipe standing in for the RFCOMM socket:
//! same pinned mutual TLS as QUIC, exactly-once delivery across the switch, Wi-Fi preferred.

mod common;

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use common::*;
use phonelink_core::{tls, Event, Fingerprint, Identity, Node, Transport};
use tokio::{
    io::{AsyncReadExt, DuplexStream},
    task::JoinHandle,
    time::timeout,
};

const BT: &str = "AA:BB:CC:DD:EE:FF";

/// A "radio link" between two stream ends. Aborting the relay = walking out of range.
/// The relay also finishes on its own once both sides have closed their ends.
fn radio(buf: usize) -> (DuplexStream, DuplexStream, JoinHandle<()>) {
    let (phone_end, mut a) = tokio::io::duplex(buf);
    let (laptop_end, mut b) = tokio::io::duplex(buf);
    let relay = tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
    });
    (phone_end, laptop_end, relay)
}

async fn link_over_radio(
    laptop: &Node,
    phone: &Node,
    buf: usize,
) -> (JoinHandle<()>, phonelink_core::Result<Fingerprint>, phonelink_core::Result<()>) {
    let (p, l, relay) = radio(buf);
    let laptop2 = laptop.clone();
    let accept = tokio::spawn(async move { laptop2.accept_stream(l).await });
    let dial = phone.connect_stream(laptop.fingerprint(), p).await;
    let accepted = timeout(WAIT, accept).await.expect("accept finished").unwrap();
    (relay, accepted, dial)
}

fn is_connected_via(t: Transport) -> impl Fn(&Event) -> bool {
    move |e| matches!(e, Event::PeerConnected { transport, .. } if *transport == t)
}

fn bodies(range: std::ops::RangeInclusive<u32>) -> Vec<Vec<u8>> {
    range.map(|i| format!("n{i}").into_bytes()).collect()
}

#[tokio::test]
async fn bluetooth_carries_the_session_while_wifi_is_down_then_wifi_takes_over() {
    let (laptop, mut lrx, phone, mut prx) = paired_pair().await;
    let (lfp, pfp) = (laptop.fingerprint(), phone.fingerprint());
    let laptop_addr = laptop.local_addr().unwrap();

    // Over Wi-Fi the laptop tells the phone where to find it over Bluetooth.
    laptop.set_bluetooth_address(Some(BT.to_lowercase()));
    eventually("phone learned the laptop's Bluetooth address", || {
        phone.peer(&lfp).unwrap().bluetooth.as_deref() == Some(BT)
    })
    .await;

    // Wi-Fi goes away.
    assert!(phone.disconnect(&lfp));
    wait_for(&mut lrx, "laptop sees the drop", |e| {
        matches!(e, Event::PeerDisconnected { .. })
    })
    .await;
    wait_for(&mut prx, "phone sees the drop", |e| {
        matches!(e, Event::PeerDisconnected { .. })
    })
    .await;
    for b in bodies(1..=5) {
        phone.send(&lfp, b).unwrap(); // queued while offline
    }

    // Bluetooth: a small buffer so frames really are split across many reads.
    let (relay, accepted, dial) = link_over_radio(&laptop, &phone, 512).await;
    assert_eq!(accepted.unwrap(), pfp);
    dial.unwrap();
    wait_for(&mut lrx, "laptop on Bluetooth", is_connected_via(Transport::Stream)).await;
    wait_for(&mut prx, "phone on Bluetooth", is_connected_via(Transport::Stream)).await;
    assert_eq!(laptop.transport(&pfp), Some(Transport::Stream));
    assert_eq!(phone.transport(&lfp), Some(Transport::Stream));

    for b in bodies(6..=10) {
        phone.send(&lfp, b).unwrap();
    }
    let big = vec![7u8; 200_000]; // a large notification icon, say
    laptop.send(&pfp, big.clone()).unwrap();
    wait_for(&mut prx, "large message over Bluetooth", move |e| {
        matches!(e, Event::Message { body, .. } if *body == big)
    })
    .await;

    // Wi-Fi comes back (here the phone's reconnect loop would dial).
    for b in bodies(11..=15) {
        phone.send(&lfp, b).unwrap();
    }
    phone.connect(lfp, laptop_addr).await.unwrap();
    wait_for(&mut prx, "phone back on Wi-Fi", is_connected_via(Transport::Quic)).await;
    eventually("both on Wi-Fi", || {
        laptop.transport(&pfp) == Some(Transport::Quic)
            && phone.transport(&lfp) == Some(Transport::Quic)
    })
    .await;
    for b in bodies(16..=20) {
        phone.send(&lfp, b).unwrap();
    }
    // The Bluetooth link was closed by both sides (saves battery).
    timeout(WAIT, relay).await.expect("Bluetooth link closed").unwrap();

    let got: Vec<Vec<u8>> = {
        let mut got = vec![];
        timeout(WAIT, async {
            while got.len() < 20 {
                if let Some(Event::Message { body, .. }) = lrx.recv().await {
                    got.push(body);
                }
            }
        })
        .await
        .expect("all 20 messages arrive");
        got.extend(drain_messages(&mut lrx, Duration::from_millis(500)).await);
        got
    };
    assert_eq!(got, bodies(1..=20), "every message exactly once, in order, across both switches");
    // No spurious "disconnected" when moving from Bluetooth back to Wi-Fi.
    assert!(!std::iter::from_fn(|| lrx.try_recv().ok())
        .any(|e| matches!(e, Event::PeerDisconnected { .. })));
    eventually("outbox empty", || phone.pending_count(&lfp).unwrap() == 0).await;
}

#[tokio::test]
async fn wifi_session_is_kept_when_bluetooth_also_connects() {
    let (laptop, _lrx, phone, _prx) = paired_pair().await;
    let (lfp, pfp) = (laptop.fingerprint(), phone.fingerprint());
    // The phone declines to open a Bluetooth session while Wi-Fi works.
    let (p, _l, _relay) = radio(4096);
    assert!(phone.connect_stream(lfp, p).await.is_err());
    assert_eq!(laptop.transport(&pfp), Some(Transport::Quic));
    assert_eq!(phone.transport(&lfp), Some(Transport::Quic));
}

#[tokio::test]
async fn losing_the_bluetooth_link_disconnects_and_messages_wait() {
    let (laptop, mut lrx, phone, mut prx) = paired_pair().await;
    let lfp = laptop.fingerprint();
    phone.disconnect(&lfp);
    wait_for(&mut lrx, "drop", |e| matches!(e, Event::PeerDisconnected { .. })).await;
    wait_for(&mut prx, "drop", |e| matches!(e, Event::PeerDisconnected { .. })).await;

    let (relay, accepted, dial) = link_over_radio(&laptop, &phone, 4096).await;
    accepted.unwrap();
    dial.unwrap();
    wait_for(&mut lrx, "on Bluetooth", is_connected_via(Transport::Stream)).await;

    relay.abort(); // out of range
    wait_for(&mut lrx, "laptop notices", |e| matches!(e, Event::PeerDisconnected { .. })).await;
    wait_for(&mut prx, "phone notices", |e| matches!(e, Event::PeerDisconnected { .. })).await;
    assert!(!phone.is_connected(&lfp));
    phone.send(&lfp, b"later".to_vec()).unwrap();
    assert_eq!(phone.pending_count(&lfp).unwrap(), 1, "kept for the next link");

    phone.connect(lfp, laptop.local_addr().unwrap()).await.unwrap();
    wait_for(&mut lrx, "delivered on reconnect", is_msg(b"later")).await;
}

/// Speaks TLS to the laptop's Bluetooth server with an arbitrary certificate.
async fn raw_tls_client(
    me: &Identity,
    laptop: &Node,
) -> (std::io::Result<()>, phonelink_core::Result<Fingerprint>) {
    let (p, l, _relay) = radio(4096);
    let laptop2 = laptop.clone();
    let accept = tokio::spawn(async move { laptop2.accept_stream(l).await });
    let cfg = tls::stream_client_config(me, laptop.fingerprint(), tls::crypto_provider()).unwrap();
    let name = rustls::pki_types::ServerName::try_from(tls::SERVER_NAME).unwrap();
    let client = async {
        let mut s = tokio_rustls::TlsConnector::from(cfg).connect(name, p).await?;
        // TLS 1.3: a refused client certificate only shows up on the first read.
        let mut b = [0u8; 1];
        match s.read(&mut b).await? {
            0 => Err(std::io::Error::other("closed")),
            _ => Ok(()),
        }
    };
    let res = timeout(WAIT, client).await.expect("client finished");
    let accepted = timeout(WAIT, accept).await.expect("accept finished").unwrap();
    (res, accepted)
}

#[tokio::test]
async fn strangers_cannot_use_the_bluetooth_link_even_while_pairing_is_open() {
    let (laptop, mut lrx, _phone, _prx) = paired_pair().await;
    let stranger = Identity::generate().unwrap();

    // Refused inside the TLS handshake itself (a certificate alert), not merely
    // dropped afterwards.
    fn refused_by_tls(res: &std::io::Result<()>) -> bool {
        matches!(
            res.as_ref().err().and_then(|e| e.get_ref()).and_then(|i| i.downcast_ref::<rustls::Error>()),
            Some(rustls::Error::AlertReceived(_))
        )
    }
    let (res, accepted) = raw_tls_client(&stranger, &laptop).await;
    assert!(refused_by_tls(&res), "handshake must fail for an unpaired key: {res:?}");
    assert!(accepted.is_err());

    // The QR pairing window admits unknown keys over Wi-Fi only; never over Bluetooth.
    let _offer = laptop.start_pairing(Duration::from_secs(60), vec![]);
    let (res, accepted) = raw_tls_client(&stranger, &laptop).await;
    assert!(refused_by_tls(&res), "no pairing over the Bluetooth link: {res:?}");
    assert!(accepted.is_err());

    assert!(!std::iter::from_fn(|| lrx.try_recv().ok())
        .any(|e| matches!(e, Event::Paired { .. } | Event::Message { .. })));
}

#[tokio::test]
async fn a_bluetooth_address_from_the_peer_must_be_well_formed() {
    let (laptop, _lrx, phone, _prx) = paired_pair().await;
    let lfp = laptop.fingerprint();
    laptop.set_bluetooth_address(Some("not an address".into()));
    laptop.set_bluetooth_address(Some(BT.into()));
    eventually("stored", || phone.peer(&lfp).unwrap().bluetooth.as_deref() == Some(BT)).await;
    laptop.set_bluetooth_address(None); // backup switched off on the laptop
    eventually("cleared", || phone.peer(&lfp).unwrap().bluetooth.is_none()).await;
}

/// A paired device sending a malformed address is ignored (the receiver validates; it
/// does not rely on the sender's own check). Speaks the protocol by hand, with the real
/// phone's key loaded from its data directory.
#[tokio::test]
async fn a_malformed_bluetooth_address_from_a_paired_peer_is_ignored() {
    use phonelink_core::wire::{self, Control, Frame};
    let pdir = tempfile::tempdir().unwrap();
    let (laptop, mut lrx) = node("Laptop").await;
    let (phone, _prx) = node_at("Phone", Some(pdir.path().into()), "127.0.0.1:0").await;
    let offer = laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]);
    phone.pair_with(&offer).await.unwrap();
    wait_for(&mut lrx, "connected", |e| matches!(e, Event::PeerConnected { .. })).await;
    let pfp = phone.fingerprint();
    phone.shutdown().await;
    wait_for(&mut lrx, "gone", |e| matches!(e, Event::PeerDisconnected { .. })).await;

    let me = Identity::load_or_create(pdir.path()).unwrap();
    assert_eq!(me.fingerprint(), pfp);
    let (p, l, _relay) = radio(4096);
    let laptop2 = laptop.clone();
    let accept = tokio::spawn(async move { laptop2.accept_stream(l).await });
    let cfg = tls::stream_client_config(&me, laptop.fingerprint(), tls::crypto_provider()).unwrap();
    let name = rustls::pki_types::ServerName::try_from(tls::SERVER_NAME).unwrap();
    let mut s = tokio_rustls::TlsConnector::from(cfg).connect(name, p).await.unwrap();
    let hello = Control::Hello {
        version: phonelink_core::PROTOCOL_VERSION,
        name: "Phone".into(),
        capabilities: vec![],
    };
    wire::write_frame(&mut s, &hello).await.unwrap();
    assert!(matches!(
        wire::read_frame(&mut s).await.unwrap(),
        Some(Frame::Known(Control::Hello { .. }))
    ));
    assert_eq!(timeout(WAIT, accept).await.unwrap().unwrap().unwrap(), pfp);

    for bad in ["aa:bb:cc:dd:ee:ff", "AA:BB:CC:DD:EE", "AA-BB-CC-DD-EE-FF", "AA:BB:CC:DD:EE:FF\n", ""] {
        let links = Control::Links { bluetooth: Some(bad.into()) };
        wire::write_frame(&mut s, &links).await.unwrap();
    }
    let good = Control::Links { bluetooth: Some(BT.into()) };
    wire::write_frame(&mut s, &good).await.unwrap();
    eventually("the good one is stored", || laptop.peer(&pfp).unwrap().bluetooth.as_deref() == Some(BT)).await;
    for bad in ["zz:zz:zz:zz:zz:zz", "AA:BB:CC:DD:EE:F"] {
        let links = Control::Links { bluetooth: Some(bad.into()) };
        wire::write_frame(&mut s, &links).await.unwrap();
    }
    wire::write_frame(&mut s, &Control::Ping).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(laptop.peer(&pfp).unwrap().bluetooth.as_deref(), Some(BT), "bad ones never replace it");
}

/// Like [`radio`], but the link can be "wedged": bytes are swallowed while both ends stay
/// open, the way a Bluetooth link can hang without ever reporting an error.
fn wedgeable_radio() -> (DuplexStream, DuplexStream, Arc<AtomicBool>) {
    let wedged = Arc::new(AtomicBool::new(false));
    let (phone_end, a) = tokio::io::duplex(4096);
    let (laptop_end, b) = tokio::io::duplex(4096);
    let (ar, aw) = tokio::io::split(a);
    let (br, bw) = tokio::io::split(b);
    tokio::spawn(pump(ar, bw, wedged.clone()));
    tokio::spawn(pump(br, aw, wedged.clone()));
    (phone_end, laptop_end, wedged)
}

async fn pump(
    mut from: impl tokio::io::AsyncRead + Unpin,
    mut to: impl tokio::io::AsyncWrite + Unpin,
    wedged: Arc<AtomicBool>,
) {
    use tokio::io::AsyncWriteExt;
    let mut buf = [0u8; 1024];
    while let Ok(n @ 1..) = from.read(&mut buf).await {
        if !wedged.load(Ordering::SeqCst) && to.write_all(&buf[..n]).await.is_err() {
            break;
        }
    }
}

async fn quick_keepalive_node(name: &str) -> (Node, tokio::sync::mpsc::UnboundedReceiver<Event>) {
    init_tracing();
    let mut cfg = phonelink_core::NodeConfig::new(name, None, "127.0.0.1:0".parse().unwrap());
    cfg.stream_ping_every = Duration::from_millis(100);
    cfg.stream_silence_limit = Duration::from_millis(500);
    Node::start(cfg).await.unwrap()
}

#[tokio::test]
async fn an_idle_bluetooth_link_stays_up_and_a_wedged_one_is_dropped() {
    let (laptop, mut lrx) = quick_keepalive_node("Laptop").await;
    let (phone, mut prx) = quick_keepalive_node("Phone").await;
    let offer = laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]);
    phone.pair_with(&offer).await.unwrap();
    let (lfp, pfp) = (laptop.fingerprint(), phone.fingerprint());
    wait_for(&mut lrx, "wifi", |e| matches!(e, Event::PeerConnected { .. })).await;
    phone.disconnect(&lfp);
    wait_for(&mut lrx, "drop", |e| matches!(e, Event::PeerDisconnected { .. })).await;
    wait_for(&mut prx, "drop", |e| matches!(e, Event::PeerDisconnected { .. })).await;

    let (p, l, wedged) = wedgeable_radio();
    let laptop2 = laptop.clone();
    let accept = tokio::spawn(async move { laptop2.accept_stream(l).await });
    phone.connect_stream(lfp, p).await.unwrap();
    accept.await.unwrap().unwrap();

    // Ten silence limits with no traffic at all: pings keep it alive.
    tokio::time::sleep(Duration::from_millis(5000)).await;
    assert_eq!(laptop.transport(&pfp), Some(Transport::Stream));
    assert_eq!(phone.transport(&lfp), Some(Transport::Stream));

    // The link hangs without an error: both sides give up on their own.
    wedged.store(true, Ordering::SeqCst);
    let t = std::time::Instant::now();
    wait_for(&mut lrx, "laptop drops it", |e| matches!(e, Event::PeerDisconnected { .. })).await;
    wait_for(&mut prx, "phone drops it", |e| matches!(e, Event::PeerDisconnected { .. })).await;
    assert!(t.elapsed() < Duration::from_secs(3), "took {:?}", t.elapsed());
}

/// The reconnect loop keeps looking for Wi-Fi while on the Bluetooth backup, and moves
/// back as soon as the laptop is reachable again (here: rediscovered at a new address).
#[tokio::test]
async fn the_reconnect_loop_leaves_the_backup_when_wifi_returns() {
    init_tracing();
    let ldir = tempfile::tempdir().unwrap();
    let (laptop, mut lrx) = node_at("Laptop", Some(ldir.path().into()), "127.0.0.1:0").await;
    let mut cfg = phonelink_core::NodeConfig::new("Phone", None, "127.0.0.1:0".parse().unwrap());
    cfg.probe.enabled = false; // only the laptop's old address and hints
    let (phone, mut prx) = Node::start(cfg).await.unwrap();
    let offer = laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]);
    phone.pair_with(&offer).await.unwrap();
    let lfp = laptop.fingerprint();
    wait_for(&mut lrx, "wifi", |e| matches!(e, Event::PeerConnected { .. })).await;
    let _keep = phone.maintain(lfp, vec![]);

    // The laptop's Wi-Fi drops (it comes back at an address the phone doesn't know yet).
    laptop.shutdown().await;
    drop(laptop);
    wait_for(&mut prx, "phone sees the drop", |e| matches!(e, Event::PeerDisconnected { .. })).await;
    let (laptop, mut lrx) = node_at("Laptop", Some(ldir.path().into()), "127.0.0.1:0").await;
    let pfp = phone.fingerprint();

    let (relay, accepted, dial) = link_over_radio(&laptop, &phone, 4096).await;
    accepted.unwrap();
    dial.unwrap();
    wait_for(&mut prx, "phone on Bluetooth", is_connected_via(Transport::Stream)).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(phone.transport(&lfp), Some(Transport::Stream), "no Wi-Fi to go back to yet");
    phone.send(&lfp, b"via bluetooth".to_vec()).unwrap();
    wait_for(&mut lrx, "message over Bluetooth", is_msg(b"via bluetooth")).await;

    // Discovery finds the laptop again: the loop dials it and Wi-Fi takes over.
    phone.add_addr_hints(lfp, vec![laptop.local_addr().unwrap()]);
    wait_for(&mut prx, "phone back on Wi-Fi", is_connected_via(Transport::Quic)).await;
    eventually("laptop on Wi-Fi", || laptop.transport(&pfp) == Some(Transport::Quic)).await;
    timeout(WAIT, relay).await.expect("Bluetooth link closed").unwrap();
    phone.send(&lfp, b"via wifi".to_vec()).unwrap();
    wait_for(&mut lrx, "message over Wi-Fi", is_msg(b"via wifi")).await;
}
