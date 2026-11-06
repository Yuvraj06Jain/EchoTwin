//! The laptop moves (new IP/port); the phone finds it again.

mod common;

use std::{sync::Arc, time::Duration};

use common::*;
use phonelink_core::{lan::Lan, Event};

#[tokio::test]
async fn address_hint_reconnects_immediately_despite_long_backoff() {
    let ldir = tempfile::tempdir().unwrap();
    let (laptop, mut lrx) = node_at("Laptop", Some(ldir.path().into()), "127.0.0.1:0").await;
    let (phone, mut prx) = node("Phone").await;
    phone
        .pair_with(
            &laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]),
        )
        .await
        .unwrap();
    let lfp = laptop.fingerprint();
    wait_for(&mut lrx, "connected", |e| {
        matches!(e, Event::PeerConnected { .. })
    })
    .await;
    let _keeper = phone.maintain(lfp, vec![]);

    laptop.shutdown().await;
    drop(laptop);
    drop(lrx);
    wait_for(&mut prx, "drop noticed", |e| {
        matches!(e, Event::PeerDisconnected { .. })
    })
    .await;
    // Let the reconnect loop fail a few times so its backoff grows.
    tokio::time::sleep(Duration::from_secs(4)).await;

    // Laptop is back, but on a different port (think: new DHCP lease).
    let (laptop2, mut lrx2) = node_at("Laptop", Some(ldir.path().into()), "127.0.0.1:0").await;
    let started = std::time::Instant::now();
    phone.add_addr_hints(lfp, vec![laptop2.local_addr().unwrap()]);
    wait_for(&mut lrx2, "reconnected via hint", |e| {
        matches!(e, Event::PeerConnected { .. })
    })
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "hint must cut the backoff short: {:?}",
        started.elapsed()
    );
}

/// Real multicast. Needs a network interface with multicast; containers often lack one.
/// Run on the laptop with: cargo test --test discovery -- --ignored
#[tokio::test]
#[ignore]
async fn live_mdns_discovery_finds_the_moved_laptop() {
    let ldir = tempfile::tempdir().unwrap();
    let (laptop, mut lrx) = node_at("Laptop", Some(ldir.path().into()), "0.0.0.0:0").await;
    let (phone, _prx) = node_at("Phone", None, "0.0.0.0:0").await;
    let port = laptop.local_addr().unwrap().port();
    let offer = laptop.start_pairing(Duration::from_secs(60), vec![([127, 0, 0, 1], port).into()]);
    phone.pair_with(&offer).await.unwrap();
    wait_for(&mut lrx, "connected", |e| {
        matches!(e, Event::PeerConnected { .. })
    })
    .await;
    let lfp = laptop.fingerprint();
    laptop.shutdown().await;
    drop(laptop);
    drop(lrx);

    // Laptop returns on an unknown port: the phone's remembered address is now useless.
    let (laptop2, mut lrx2) = node_at("Laptop", Some(ldir.path().into()), "0.0.0.0:0").await;
    let new_port = laptop2.local_addr().unwrap().port();
    let lan = Lan::new().unwrap();
    let _ad = lan.advertise(
        laptop2.clone(),
        new_port,
        Arc::new(tokio::sync::Notify::new()),
    );
    let _browse = lan.browse(phone.clone()).unwrap();
    let _keeper = phone.maintain(lfp, vec![]);
    wait_for(&mut lrx2, "phone found laptop via mDNS", |e| {
        matches!(e, Event::PeerConnected { .. })
    })
    .await;
    lan.shutdown();
}

/// The laptop comes back on a different address (what a phone hotspot does after every
/// restart) and discovery can't help. Returns whether the phone found it again.
async fn laptop_moves(search: bool) -> bool {
    use phonelink_core::{node::ProbeConfig, Node, NodeConfig};
    let ldir = tempfile::tempdir().unwrap();
    let (laptop, mut lrx) = node_at("Laptop", Some(ldir.path().into()), "127.0.0.1:0").await;
    let port = laptop.local_addr().unwrap().port();
    let lfp = laptop.fingerprint();

    let mut cfg = NodeConfig::new("Phone", None, "127.0.0.1:0".parse().unwrap());
    cfg.probe = ProbeConfig {
        enabled: search,
        subnets: Some(vec![("127.0.0.1".parse().unwrap(), 24)]),
        gateways: Some(vec![]),
    };
    let (phone, mut prx) = Node::start(cfg).await.unwrap();
    phone
        .pair_with(
            &laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]),
        )
        .await
        .unwrap();
    wait_for(&mut lrx, "connected", |e| {
        matches!(e, Event::PeerConnected { .. })
    })
    .await;
    let _keeper = phone.maintain(lfp, vec![]);

    laptop.shutdown().await;
    drop(laptop);
    drop(lrx);
    wait_for(&mut prx, "drop noticed", |e| {
        matches!(e, Event::PeerDisconnected { .. })
    })
    .await;

    // Same laptop (same keys), same port, new address. The phone only remembers the old one.
    let (_laptop2, mut l2rx) = node_at(
        "Laptop",
        Some(ldir.path().into()),
        &format!("127.0.0.77:{port}"),
    )
    .await;
    phone.nudge(); // e.g. the phone was unlocked or its network changed
    tokio::time::timeout(Duration::from_secs(6), async {
        while !matches!(l2rx.recv().await, Some(Event::PeerConnected { .. })) {}
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn subnet_search_finds_a_laptop_on_a_new_address() {
    assert!(
        laptop_moves(true).await,
        "the phone should find the laptop at 127.0.0.77 by searching the /24"
    );
}

#[tokio::test]
async fn without_the_search_a_moved_laptop_is_not_found() {
    assert!(
        !laptop_moves(false).await,
        "control: remembered addresses alone can't find it"
    );
}
