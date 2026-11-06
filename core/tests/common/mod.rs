#![allow(dead_code)]

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use phonelink_core::{Event, Node, NodeConfig};
use tokio::{sync::mpsc::UnboundedReceiver, time::timeout};

pub const WAIT: Duration = Duration::from_secs(15);

pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}

pub async fn node_at(
    name: &str,
    dir: Option<PathBuf>,
    bind: &str,
) -> (Node, UnboundedReceiver<Event>) {
    init_tracing();
    let bind: SocketAddr = bind.parse().unwrap();
    // A just-closed endpoint may hold its port for a moment; retry briefly.
    let mut last = None;
    for _ in 0..50 {
        let mut cfg = NodeConfig::new(name, dir.clone(), bind);
        cfg.capabilities.push(phonelink_core::transfer::CAP_FILES.into());
        match Node::start(cfg).await {
            Ok(x) => return x,
            Err(e) => last = Some(e),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("could not start node {name}: {last:?}");
}

pub async fn node(name: &str) -> (Node, UnboundedReceiver<Event>) {
    node_at(name, None, "127.0.0.1:0").await
}

pub async fn wait_for<F: Fn(&Event) -> bool>(
    rx: &mut UnboundedReceiver<Event>,
    what: &str,
    f: F,
) -> Event {
    timeout(WAIT, async {
        loop {
            let e = rx.recv().await.expect("event channel closed");
            if f(&e) {
                return e;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

pub async fn eventually<F: Fn() -> bool>(what: &str, f: F) {
    timeout(WAIT, async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting until {what}"))
}

/// Collect every Message body that arrives within `d`.
pub async fn drain_messages(rx: &mut UnboundedReceiver<Event>, d: Duration) -> Vec<Vec<u8>> {
    let mut out = vec![];
    let _ = timeout(d, async {
        while let Some(e) = rx.recv().await {
            if let Event::Message { body, .. } = e {
                out.push(body);
            }
        }
    })
    .await;
    out
}

pub fn is_msg(body: &'static [u8]) -> impl Fn(&Event) -> bool {
    move |e| matches!(e, Event::Message { body: b, .. } if b == body)
}

/// A laptop and phone that are paired and connected.
pub async fn paired_pair() -> (
    Node,
    UnboundedReceiver<Event>,
    Node,
    UnboundedReceiver<Event>,
) {
    let (laptop, mut lrx) = node("Laptop").await;
    let (phone, mut prx) = node("Phone").await;
    let offer = laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]);
    phone.pair_with(&offer).await.expect("pairing");
    wait_for(&mut lrx, "laptop connected", |e| {
        matches!(e, Event::PeerConnected { .. })
    })
    .await;
    wait_for(&mut prx, "phone connected", |e| {
        matches!(e, Event::PeerConnected { .. })
    })
    .await;
    (laptop, lrx, phone, prx)
}
