//! End-to-end behaviour between a paired laptop and phone.

mod common;

use std::time::Duration;

use common::*;
use phonelink_core::{Event, PairingOffer};

#[tokio::test]
async fn pair_via_qr_then_exchange_messages_both_ways() {
    let (laptop, mut lrx) = node("Kubuntu Laptop").await;
    let (phone, mut prx) = node("Galaxy S24 FE").await;

    let offer = laptop.start_pairing(Duration::from_secs(120), vec![laptop.local_addr().unwrap()]);
    // Simulate the QR code: everything travels through the URI string.
    let scanned = PairingOffer::from_uri(&offer.to_uri()).unwrap();
    let rec = phone.pair_with(&scanned).await.unwrap();
    assert_eq!(rec.fingerprint, laptop.fingerprint());
    assert_eq!(rec.name, "Kubuntu Laptop");

    let paired = wait_for(&mut lrx, "laptop paired", |e| {
        matches!(e, Event::Paired { .. })
    })
    .await;
    let Event::Paired { peer, name } = paired else {
        unreachable!()
    };
    assert_eq!(
        (peer, name.as_str()),
        (phone.fingerprint(), "Galaxy S24 FE")
    );

    // Both sides derived the same shared key, without ever sending it.
    assert_eq!(
        laptop.peer(&phone.fingerprint()).unwrap().pair_key,
        rec.pair_key
    );

    wait_for(&mut lrx, "laptop session", |e| {
        matches!(e, Event::PeerConnected { .. })
    })
    .await;
    wait_for(&mut prx, "phone session", |e| {
        matches!(e, Event::PeerConnected { .. })
    })
    .await;

    phone
        .send(&laptop.fingerprint(), b"hello laptop".to_vec())
        .unwrap();
    wait_for(&mut lrx, "message at laptop", is_msg(b"hello laptop")).await;
    laptop
        .send(&phone.fingerprint(), b"hello phone".to_vec())
        .unwrap();
    wait_for(&mut prx, "message at phone", is_msg(b"hello phone")).await;

    eventually("acks clear both outboxes", || {
        phone.pending_count(&laptop.fingerprint()).unwrap() == 0
            && laptop.pending_count(&phone.fingerprint()).unwrap() == 0
    })
    .await;
}

#[tokio::test]
async fn messages_sent_while_offline_survive_restarts_and_arrive_once_in_order() {
    let ldir = tempfile::tempdir().unwrap();
    let pdir = tempfile::tempdir().unwrap();
    let (laptop, mut lrx) = node_at("Laptop", Some(ldir.path().into()), "127.0.0.1:0").await;
    let (phone, _prx) = node_at("Phone", Some(pdir.path().into()), "127.0.0.1:0").await;
    let offer = laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]);
    phone.pair_with(&offer).await.unwrap();
    wait_for(&mut lrx, "connected", |e| {
        matches!(e, Event::PeerConnected { .. })
    })
    .await;
    let (lfp, pfp) = (laptop.fingerprint(), phone.fingerprint());

    // Laptop goes away (lid closed); phone keeps producing notifications.
    laptop.shutdown().await;
    drop(lrx);
    for i in 0..5 {
        phone
            .send(&lfp, format!("notification {i}").into_bytes())
            .unwrap();
    }
    assert_eq!(phone.pending_count(&lfp).unwrap(), 5);

    // The phone app is killed and restarted, and the laptop reboots.
    phone.shutdown().await;
    let (phone2, _prx2) = node_at("Phone", Some(pdir.path().into()), "127.0.0.1:0").await;
    let (laptop2, mut lrx2) = node_at("Laptop", Some(ldir.path().into()), "127.0.0.1:0").await;
    assert_eq!(phone2.fingerprint(), pfp, "phone identity survives restart");
    assert_eq!(
        laptop2.fingerprint(),
        lfp,
        "laptop identity survives restart"
    );
    assert!(laptop2.peer(&pfp).is_some(), "pairing survives restart");
    assert_eq!(
        phone2.pending_count(&lfp).unwrap(),
        5,
        "outbox survives restart"
    );

    phone2
        .connect(lfp, laptop2.local_addr().unwrap())
        .await
        .unwrap();
    let got = drain_messages(&mut lrx2, Duration::from_secs(2)).await;
    let expected: Vec<Vec<u8>> = (0..5)
        .map(|i| format!("notification {i}").into_bytes())
        .collect();
    assert_eq!(got, expected, "all queued messages, once each, in order");
    eventually("outbox drained", || {
        phone2.pending_count(&lfp).unwrap() == 0
    })
    .await;
}

#[tokio::test]
async fn maintain_reconnects_after_the_link_drops() {
    let ldir = tempfile::tempdir().unwrap();
    let (laptop, mut lrx) = node_at("Laptop", Some(ldir.path().into()), "127.0.0.1:0").await;
    let laddr = laptop.local_addr().unwrap();
    let (phone, mut prx) = node("Phone").await;
    let offer = laptop.start_pairing(Duration::from_secs(60), vec![laddr]);
    phone.pair_with(&offer).await.unwrap();
    let lfp = laptop.fingerprint();
    let _keeper = phone.maintain(lfp, vec![]);
    wait_for(&mut lrx, "connected", |e| {
        matches!(e, Event::PeerConnected { .. })
    })
    .await;

    // Hard drop: the laptop process disappears. Dropping the last handle must free the
    // UDP port (proves no internal task leaks the endpoint).
    laptop.shutdown().await;
    drop(laptop);
    drop(lrx);
    wait_for(&mut prx, "phone notices drop", |e| {
        matches!(e, Event::PeerDisconnected { .. })
    })
    .await;
    phone.send(&lfp, b"sent during outage".to_vec()).unwrap();

    // Laptop comes back on the same address; the phone finds it again by itself.
    let (_laptop2, mut lrx2) =
        node_at("Laptop", Some(ldir.path().into()), &laddr.to_string()).await;
    wait_for(
        &mut lrx2,
        "message after auto-reconnect",
        is_msg(b"sent during outage"),
    )
    .await;
    eventually("phone sees connection", || phone.is_connected(&lfp)).await;
}

#[tokio::test]
async fn simultaneous_dialing_converges_on_one_working_connection() {
    let (laptop, mut lrx, phone, mut prx) = paired_pair().await;
    let (lfp, pfp) = (laptop.fingerprint(), phone.fingerprint());
    let paddr = phone.local_addr().unwrap();
    let laddr = laptop.local_addr().unwrap();
    // Both sides aggressively keep the link and dial each other.
    let _a = laptop.maintain(pfp, vec![paddr]);
    let _b = phone.maintain(lfp, vec![laddr]);
    for _ in 0..5 {
        let (l, p) = (laptop.clone(), phone.clone());
        let (r1, r2) = tokio::join!(l.connect(pfp, paddr), p.connect(lfp, laddr));
        let _ = (r1, r2);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(laptop.is_connected(&pfp) && phone.is_connected(&lfp));

    for i in 0..20u8 {
        phone.send(&lfp, vec![i]).unwrap();
        laptop.send(&pfp, vec![100 + i]).unwrap();
    }
    let mut at_laptop = vec![];
    let mut at_phone = vec![];
    tokio::time::timeout(WAIT, async {
        while at_laptop.len() < 20 || at_phone.len() < 20 {
            tokio::select! {
                Some(Event::Message { body, .. }) = lrx.recv() => at_laptop.push(body[0]),
                Some(Event::Message { body, .. }) = prx.recv() => at_phone.push(body[0]),
                else => {}
            }
        }
    })
    .await
    .expect("all messages delivered");
    at_laptop.sort();
    at_phone.sort();
    assert_eq!(
        at_laptop,
        (0..20).collect::<Vec<u8>>(),
        "no loss or duplicates"
    );
    assert_eq!(
        at_phone,
        (100..120).collect::<Vec<u8>>(),
        "no loss or duplicates"
    );
}

#[tokio::test]
async fn unpair_revokes_access_immediately() {
    let (laptop, _lrx, phone, mut prx) = paired_pair().await;
    let (lfp, pfp) = (laptop.fingerprint(), phone.fingerprint());
    assert!(laptop.unpair(&pfp).unwrap());
    wait_for(&mut prx, "phone dropped", |e| {
        matches!(e, Event::PeerDisconnected { .. })
    })
    .await;
    assert!(laptop.send(&pfp, b"x".to_vec()).is_err());
    // The phone still has the laptop pinned, but the laptop no longer accepts it.
    let err = phone.connect(lfp, laptop.local_addr().unwrap()).await;
    assert!(err.is_err(), "revoked phone must not reconnect");
    assert!(!laptop.is_connected(&pfp));
}

#[tokio::test]
async fn concurrent_dials_to_the_same_peer_make_one_connection() {
    let ldir = tempfile::tempdir().unwrap();
    let (laptop, mut lrx) = node_at("Laptop", Some(ldir.path().into()), "127.0.0.1:0").await;
    let (laddr, lfp) = (laptop.local_addr().unwrap(), laptop.fingerprint());
    let (phone, mut prx) = node("Phone").await;
    phone
        .pair_with(&laptop.start_pairing(Duration::from_secs(60), vec![laddr]))
        .await
        .unwrap();
    wait_for(&mut lrx, "connected", |e| {
        matches!(e, Event::PeerConnected { .. })
    })
    .await;

    // The laptop restarts; afterwards several callers on the phone (pairing, the reconnect
    // loop, a network-change nudge) try to reconnect at the same moment.
    laptop.shutdown().await;
    drop(laptop);
    drop(lrx);
    wait_for(&mut prx, "drop noticed", |e| {
        matches!(e, Event::PeerDisconnected { .. })
    })
    .await;
    let (_laptop2, mut l2rx) =
        node_at("Laptop", Some(ldir.path().into()), &laddr.to_string()).await;
    let (a, b, c) = tokio::join!(
        phone.connect_any(lfp, vec![laddr]),
        phone.connect_any(lfp, vec![laddr]),
        phone.connect_any(lfp, vec![laddr]),
    );
    assert!(a.is_ok() && b.is_ok() && c.is_ok(), "{a:?} {b:?} {c:?}");
    tokio::time::sleep(Duration::from_millis(800)).await;
    let (mut connects, mut disconnects) = (0, 0);
    while let Ok(e) = l2rx.try_recv() {
        match e {
            Event::PeerConnected { .. } => connects += 1,
            Event::PeerDisconnected { .. } => disconnects += 1,
            _ => {}
        }
    }
    assert!(phone.is_connected(&lfp));
    assert_eq!(
        (connects, disconnects),
        (1, 0),
        "exactly one connection, none replaced"
    );
}

/// Switching the connection off on one device stops both from searching; switching it back
/// on reconnects at once and delivers what was queued meanwhile.
#[tokio::test]
async fn switching_the_connection_off_and_on() {
    let (laptop, mut lrx, phone, mut prx) = paired_pair().await;
    let (lfp, pfp) = (laptop.fingerprint(), phone.fingerprint());
    let _lk = laptop.maintain(pfp, vec![]);
    let _pk = phone.maintain(lfp, vec![]);

    // Phone switches off: the laptop is told and stops dialling.
    phone.set_paused(true);
    wait_for(&mut lrx, "laptop told the phone paused", |e| {
        matches!(e, Event::PeerPaused { .. })
    })
    .await;
    eventually("both disconnected", || {
        !laptop.is_connected(&pfp) && !phone.is_connected(&lfp)
    })
    .await;
    assert!(laptop.peer_paused(&pfp));
    laptop.nudge(); // even a network change must not make it dial a paused phone…
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!laptop.is_connected(&pfp), "no reconnect while the phone is off");
    while let Ok(e) = lrx.try_recv() {
        assert!(!matches!(e, Event::PeerConnected { .. }), "laptop reconnected while paused");
    }
    // …and the phone itself refuses to dial.
    assert!(phone.connect_any(lfp, vec![laptop.local_addr().unwrap()]).await.is_err());
    laptop.send(&pfp, b"while you were off".to_vec()).unwrap();

    // Phone switches back on: reconnects at once, gets the queued message.
    phone.set_paused(false);
    wait_for(&mut prx, "queued message arrives", is_msg(b"while you were off")).await;
    eventually("laptop no longer thinks it's paused", || !laptop.peer_paused(&pfp)).await;

    // Laptop switches off: the phone is told; a phone that dials anyway is refused.
    laptop.set_paused(true);
    wait_for(&mut prx, "phone told the laptop paused", |e| {
        matches!(e, Event::PeerPaused { .. })
    })
    .await;
    eventually("disconnected", || !phone.is_connected(&lfp)).await;
    let r = phone.connect_any(lfp, vec![laptop.local_addr().unwrap()]).await;
    assert!(r.is_err(), "a paused laptop refuses the session");
    assert!(phone.peer_paused(&lfp));

    // Laptop back on: it dials the phone itself.
    laptop.set_paused(false);
    eventually("reconnected from the laptop side", || {
        laptop.is_connected(&pfp) && phone.is_connected(&lfp)
    })
    .await;
    assert!(!phone.peer_paused(&lfp));
}
