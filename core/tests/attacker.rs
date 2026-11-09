//! A stranger on the same (college) network tries everything they can.

mod common;

use std::time::Duration;

use common::*;
use phonelink_core::{tls, wire, Event, Identity, PairingOffer};

/// Raw QUIC client that speaks our protocol but holds an unpaired certificate.
async fn raw_attempt(
    target: std::net::SocketAddr,
    target_fp: phonelink_core::Fingerprint,
    bind: &str,
) -> Result<(), String> {
    let me = Identity::generate().unwrap();
    let transport = tls::transport(tls::KEEP_ALIVE, tls::IDLE_TIMEOUT);
    let cfg = tls::client_config(&me, target_fp, tls::crypto_provider(), transport).unwrap();
    let mut ep = quinn::Endpoint::client(bind.parse().unwrap()).map_err(|e| e.to_string())?;
    ep.set_default_client_config(cfg);
    let res = async {
        let conn = ep
            .connect(target, tls::SERVER_NAME)
            .map_err(|e| e.to_string())?
            .await
            .map_err(|e| e.to_string())?;
        let (mut send, mut recv) = conn.open_bi().await.map_err(|e| e.to_string())?;
        let hello = wire::Control::Hello {
            version: 1,
            name: "evil".into(),
            capabilities: vec!["messages/1".into()],
        };
        wire::write_frame(&mut send, &hello)
            .await
            .map_err(|e| e.to_string())?;
        let deliver = wire::Control::Deliver {
            id: phonelink_core::MsgId::random(),
            body: b"injected".to_vec(),
        };
        wire::write_frame(&mut send, &deliver)
            .await
            .map_err(|e| e.to_string())?;
        match wire::read_frame(&mut recv).await {
            // Any answer at all means the stranger got through.
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err("stream closed".to_string()),
            Err(e) => Err(e.to_string()),
        }
    };
    let out = tokio::time::timeout(Duration::from_secs(10), res)
        .await
        .unwrap_or(Err("timeout".into()));
    ep.close(0u32.into(), b"");
    out
}

#[tokio::test]
async fn unpaired_device_cannot_connect_or_inject_data() {
    let (laptop, mut lrx, phone, _prx) = paired_pair().await;
    // The attacker even knows the laptop's exact fingerprint and address.
    let r = raw_attempt(
        laptop.local_addr().unwrap(),
        laptop.fingerprint(),
        "127.0.0.1:0",
    )
    .await;
    assert!(r.is_err(), "stranger must be refused");

    let got = drain_messages(&mut lrx, Duration::from_millis(500)).await;
    assert!(
        got.is_empty(),
        "no data from a stranger reaches the app: {got:?}"
    );
    assert_eq!(laptop.peers().len(), 1);
    assert_eq!(laptop.peers()[0].fingerprint, phone.fingerprint());
}

#[tokio::test]
async fn stranger_cannot_pair_without_the_qr_secret_and_cannot_block_real_pairing() {
    let (laptop, mut lrx) = node("Laptop").await;
    let (attacker, _arx) = node("Attacker").await;
    let real = laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]);

    // The attacker sees pairing is open and knows the fingerprint, but guesses the secret.
    for _ in 0..5 {
        let guess = PairingOffer {
            secret: rand_secret(),
            ..real.clone()
        };
        let err = attacker.pair_with(&guess).await.unwrap_err();
        assert!(err.to_string().contains("rejected"), "{err}");
    }
    assert!(laptop.peers().is_empty());

    // The real phone, holding the real QR code, still pairs fine.
    let (phone, _prx) = node("Phone").await;
    phone
        .pair_with(&real)
        .await
        .expect("real pairing still works");
    wait_for(&mut lrx, "paired", |e| matches!(e, Event::Paired { .. })).await;
    assert_eq!(laptop.peers().len(), 1);
    assert_eq!(laptop.peers()[0].fingerprint, phone.fingerprint());
}

#[tokio::test]
async fn qr_code_is_single_use() {
    let (laptop, mut lrx) = node("Laptop").await;
    let offer = laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]);
    let (phone, _p) = node("Phone").await;
    phone.pair_with(&offer).await.unwrap();
    wait_for(&mut lrx, "paired", |e| matches!(e, Event::Paired { .. })).await;

    // Someone photographed the QR code and tries it afterwards.
    let (thief, _t) = node("Thief").await;
    assert!(thief.pair_with(&offer).await.is_err());
    assert_eq!(laptop.peers().len(), 1);
}

#[tokio::test]
async fn expired_pairing_window_refuses_everyone() {
    let (laptop, _lrx) = node("Laptop").await;
    let offer = laptop.start_pairing(
        Duration::from_millis(200),
        vec![laptop.local_addr().unwrap()],
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (phone, _p) = node("Phone").await;
    assert!(phone.pair_with(&offer).await.is_err());
    assert!(laptop.peers().is_empty());
}

#[tokio::test]
async fn impostor_laptop_cannot_capture_the_phone() {
    // An attacker redirects the phone to their own machine (e.g. ARP spoofing) while it pairs.
    let (laptop, _l) = node("Laptop").await;
    let (evil, _e) = node("Evil").await;
    let evil_offer_window = evil.start_pairing(Duration::from_secs(60), vec![]);
    let mut offer =
        laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]);
    offer.addrs = vec![evil.local_addr().unwrap()]; // traffic goes to the impostor
    let _ = evil_offer_window;

    let (phone, _p) = node("Phone").await;
    assert!(
        phone.pair_with(&offer).await.is_err(),
        "fingerprint from the QR code doesn't match"
    );
    assert!(phone.peers().is_empty());
    assert!(evil.peers().is_empty());
}

#[tokio::test]
async fn handshake_flood_does_not_disturb_paired_devices() {
    let (laptop, mut lrx) = node_at("Laptop", None, "127.0.0.1:0").await;
    let laddr = laptop.local_addr().unwrap();
    let lfp = laptop.fingerprint();

    let (phone, _p) = node_at("Phone", None, "127.0.0.1:0").await;
    phone
        .pair_with(&laptop.start_pairing(Duration::from_secs(60), vec![laddr]))
        .await
        .unwrap();
    // A second paired device that will (re)connect *during* the flood.
    let tdir = tempfile::tempdir().unwrap();
    let (tablet, _t) = node_at("Tablet", Some(tdir.path().into()), "127.0.0.3:0").await;
    tablet
        .pair_with(&laptop.start_pairing(Duration::from_secs(60), vec![laddr]))
        .await
        .unwrap();
    tablet.shutdown().await;
    eventually("phone connected", || phone.is_connected(&lfp)).await;

    // 300 hostile handshakes from another machine on the LAN.
    let flood: Vec<_> = (0..300)
        .map(|_| tokio::spawn(raw_attempt(laddr, lfp, "127.0.0.2:0")))
        .collect();

    // A paired device can still establish a brand-new connection mid-flood.
    let (tablet2, _t2) = node_at("Tablet", Some(tdir.path().into()), "127.0.0.3:0").await;
    tablet2
        .connect(lfp, laddr)
        .await
        .expect("paired device connects during flood");

    // Meanwhile the paired phone keeps talking.
    for i in 0..50u8 {
        phone.send(&lfp, vec![i]).unwrap();
    }
    let mut got = vec![];
    tokio::time::timeout(WAIT, async {
        while got.len() < 50 {
            if let Some(Event::Message { body, .. }) = lrx.recv().await {
                got.push(body[0]);
            }
        }
    })
    .await
    .expect("paired traffic keeps flowing under attack");
    assert_eq!(got, (0..50).collect::<Vec<u8>>());

    let mut refused = 0;
    for f in flood {
        if f.await.unwrap().is_err() {
            refused += 1;
        }
    }
    assert_eq!(refused, 300, "every hostile handshake failed");
    assert_eq!(laptop.peers().len(), 2);
}

fn rand_secret() -> [u8; 32] {
    rand::random()
}
