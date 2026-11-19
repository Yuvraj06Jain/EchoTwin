//! Touchpad sessions: finger frames as QUIC datagrams, touchpad messages as fleeting frames.
//! (Ported from TouchTwin's `touchtwin-core/tests/sessions.rs`.)

mod common;

use std::time::Duration;

use common::*;
use phonelink_core::{
    touchpad::{Button, Contact, Msg, PadInfo, TouchFrame, CAP_TOUCHPAD},
    Event, Node, NodeConfig,
};
use tokio::sync::mpsc::UnboundedReceiver;

async fn pad_node(name: &str, datagrams: bool) -> (Node, UnboundedReceiver<Event>) {
    init_tracing();
    let mut cfg = NodeConfig::new(name, None, "127.0.0.1:0".parse().unwrap());
    cfg.capabilities = vec![CAP_TOUCHPAD.into()];
    cfg.datagrams = datagrams;
    Node::start(cfg).await.unwrap()
}

/// Laptop and phone, paired and connected (the phone dialled).
async fn paired(
    laptop_reads_datagrams: bool,
) -> (Node, UnboundedReceiver<Event>, Node, UnboundedReceiver<Event>) {
    let (laptop, mut lrx) = pad_node("Laptop", laptop_reads_datagrams).await;
    let (phone, mut prx) = pad_node("Phone", true).await;
    let offer = laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]);
    phone.pair_with(&offer).await.unwrap();
    wait_for(&mut lrx, "laptop connected", |e| matches!(e, Event::PeerConnected { .. })).await;
    wait_for(&mut prx, "phone connected", |e| matches!(e, Event::PeerConnected { .. })).await;
    (laptop, lrx, phone, prx)
}

#[tokio::test]
async fn touch_frames_arrive_as_datagrams() {
    let (laptop, mut lrx, phone, _prx) = paired(true).await;
    let lfp = laptop.fingerprint();
    assert!(phone.peer_supports(&lfp, CAP_TOUCHPAD));
    assert!(phone.rtt(&lfp).is_some());

    for seq in 0..20u32 {
        let f = TouchFrame {
            seq,
            contacts: vec![Contact { slot: 0, id: 1, x: seq as u16 * 100, y: 50 }],
        };
        assert!(phone.send_datagram(&lfp, f.encode()).unwrap());
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let mut seqs = vec![];
    tokio::time::timeout(WAIT, async {
        while seqs.len() < 20 {
            if let Some(Event::Datagram { peer, body }) = lrx.recv().await {
                assert_eq!(peer, phone.fingerprint());
                seqs.push(TouchFrame::decode(&body).expect("valid frame").seq);
            }
        }
    })
    .await
    .expect("frames arrive on loopback");
    seqs.sort();
    assert_eq!(seqs, (0..20).collect::<Vec<_>>());
}

#[tokio::test]
async fn a_node_without_datagrams_never_reports_them() {
    let (laptop, mut lrx, phone, _prx) = paired(false).await;
    let lfp = laptop.fingerprint();
    for _ in 0..10 {
        let _ = phone.send_datagram(&lfp, vec![1, 2, 3]);
    }
    // A fleeting message after them proves the session is alive and events flow.
    assert!(phone.send_fleeting(&lfp, Msg::Release.encode()).unwrap());
    let mut got_msg = false;
    let _ = tokio::time::timeout(Duration::from_millis(600), async {
        while let Some(e) = lrx.recv().await {
            match e {
                Event::Datagram { .. } => panic!("datagram reported by a node that doesn't read them"),
                Event::Message { .. } => got_msg = true,
                _ => {}
            }
        }
    })
    .await;
    assert!(got_msg);
}

#[tokio::test]
async fn touchpad_messages_arrive_in_order() {
    let (laptop, mut lrx, phone, mut prx) = paired(true).await;
    let (lfp, pfp) = (laptop.fingerprint(), phone.fingerprint());
    let sent = [
        Msg::PadInfo(PadInfo { width_dmm: 700, height_dmm: 1500 }),
        Msg::Button { button: Button::Right, down: true },
        Msg::Button { button: Button::Right, down: false },
        Msg::Release,
    ];
    for m in &sent {
        assert!(phone.send_fleeting(&lfp, m.encode()).unwrap());
    }
    let got = drain_messages(&mut lrx, Duration::from_millis(500)).await;
    let got: Vec<Msg> = got.iter().filter_map(|b| Msg::decode(b)).collect();
    assert_eq!(got, sent);

    let status = Msg::status(false, &"x".repeat(1000));
    assert!(laptop.send_fleeting(&pfp, status.encode()).unwrap(), "a long error still fits");
    let got = drain_messages(&mut prx, Duration::from_millis(500)).await;
    assert_eq!(got.iter().filter_map(|b| Msg::decode(b)).collect::<Vec<_>>(), vec![status]);
}

#[tokio::test]
async fn nothing_is_sent_without_a_session() {
    let (laptop, _l) = pad_node("Laptop", true).await;
    let (phone, _p) = pad_node("Phone", true).await;
    assert!(!phone.send_datagram(&laptop.fingerprint(), vec![1]).unwrap());
    assert!(phone.send_datagram(&laptop.fingerprint(), vec![0; 4096]).is_err());
    assert!(phone.rtt(&laptop.fingerprint()).is_none());
}
