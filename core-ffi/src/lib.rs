//! Kotlin-facing API of the PhoneLink core, generated with UniFFI.
//!
//! The Android app creates one [`PhoneLink`] inside its foreground service. All networking,
//! crypto, storage and reconnect logic is the shared Rust core, so the phone and the laptop
//! run identical protocol code. Methods block and should be called off the main thread
//! (`Dispatchers.IO`). Events arrive on a dedicated background thread through
//! [`EventListener`].

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{mpsc as std_mpsc, Arc, Mutex},
    time::Duration,
};

use phonelink_core::{
    lan::Lan,
    message::{truncate_text, MAX_NOTIFICATION_TEXT, MAX_SNAPSHOT_ITEMS},
    AppMessage, Event, Fingerprint, Node, NodeConfig, PairingOffer, PeerRecord, PhoneNotification,
    CAP_NOTIFICATIONS,
};
use tokio::{runtime::Runtime, task::JoinHandle};

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum LinkError {
    #[error("{0}")]
    Failed(String),
}

impl From<phonelink_core::Error> for LinkError {
    fn from(e: phonelink_core::Error) -> Self {
        LinkError::Failed(e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Device {
    /// Short id shown on both screens, e.g. `3F2A-91C0-77DE-0B14`.
    pub id: String,
    pub name: String,
    pub connected: bool,
    /// Messages waiting for this device to come back.
    pub pending: u64,
}

/// What a scanned QR code says, shown before the user confirms pairing.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PairingCode {
    pub name: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum LinkEvent {
    Paired {
        id: String,
        name: String,
    },
    Connected {
        id: String,
        name: String,
    },
    Disconnected {
        id: String,
        name: String,
    },
    Text {
        id: String,
        name: String,
        text: String,
    },
    /// The computer asks to clear this notification on the phone ("dismiss on both").
    DismissRequested {
        id: String,
        key: String,
    },
    /// The computer replies to this notification.
    ReplyRequested {
        id: String,
        key: String,
        text: String,
    },
    /// The computer presses action button `index` (position in `MirroredNotification::actions`).
    ActionRequested {
        id: String,
        key: String,
        index: u32,
    },
}

/// A notification shown on this phone, to mirror to the computer.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MirroredNotification {
    pub key: String,
    pub package: String,
    pub app: String,
    pub title: String,
    pub text: String,
    pub posted_ms: u64,
    pub can_reply: bool,
    pub actions: Vec<String>,
}

impl MirroredNotification {
    fn into_wire(self) -> PhoneNotification {
        PhoneNotification {
            key: self.key,
            package: self.package,
            app: truncate_text(&self.app, 100),
            title: truncate_text(&self.title, 300),
            text: truncate_text(&self.text, MAX_NOTIFICATION_TEXT),
            posted_ms: self.posted_ms,
            can_reply: self.can_reply,
            actions: self
                .actions
                .into_iter()
                .take(3)
                .map(|a| truncate_text(&a, 60))
                .collect(),
        }
    }
}

/// Implemented in Kotlin. Called on a background thread, one event at a time, in order.
#[uniffi::export(with_foreign)]
pub trait EventListener: Send + Sync {
    fn on_event(&self, event: LinkEvent);
}

#[derive(uniffi::Object)]
pub struct PhoneLink {
    rt: Runtime,
    node: Node,
    lan: Mutex<Option<Lan>>,
    keepers: Arc<Mutex<HashMap<Fingerprint, JoinHandle<()>>>>,
}

fn keep_connected(
    node: &Node,
    keepers: &Mutex<HashMap<Fingerprint, JoinHandle<()>>>,
    fp: Fingerprint,
) {
    let mut k = keepers.lock().unwrap_or_else(|e| e.into_inner());
    if k.get(&fp).is_some_and(|h| !h.is_finished()) {
        return;
    }
    k.insert(fp, node.maintain(fp, vec![]));
}

fn device(node: &Node, p: &PeerRecord) -> Device {
    Device {
        id: p.fingerprint.short(),
        name: p.name.clone(),
        connected: node.is_connected(&p.fingerprint),
        pending: node.pending_count(&p.fingerprint).unwrap_or(0) as u64,
    }
}

fn name_of(node: &Node, fp: &Fingerprint) -> String {
    node.peer(fp)
        .map(|p| p.name)
        .unwrap_or_else(|| "Unknown device".into())
}

#[uniffi::export]
impl PhoneLink {
    /// Start the node. `data_dir` should be the app's private files directory.
    #[uniffi::constructor]
    pub fn start(
        data_dir: String,
        device_name: String,
        listener: Arc<dyn EventListener>,
    ) -> Result<Arc<Self>, LinkError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("phonelink")
            .enable_all()
            .build()
            .map_err(|e| LinkError::Failed(e.to_string()))?;
        let dir = PathBuf::from(data_dir);
        let (node, mut events) = rt.block_on(async {
            let mut last = None;
            for bind in [
                SocketAddr::from(([0u16; 8], 0)),
                SocketAddr::from(([0u8; 4], 0)),
            ] {
                let mut cfg = NodeConfig::new(device_name.clone(), Some(dir.clone()), bind);
                cfg.capabilities.push(CAP_NOTIFICATIONS.to_string());
                match Node::start(cfg).await {
                    Ok(x) => return Ok(x),
                    Err(e) => last = Some(e),
                }
            }
            Err(last.expect("at least one bind attempted"))
        })?;

        let keepers = Arc::new(Mutex::new(HashMap::new()));
        for p in node.peers() {
            let _guard = rt.enter();
            keep_connected(&node, &keepers, p.fingerprint);
        }

        // Callbacks into Kotlin happen on one plain thread, never on a runtime worker.
        let (tx, rx) = std_mpsc::channel::<LinkEvent>();
        std::thread::Builder::new()
            .name("phonelink-events".into())
            .spawn(move || {
                while let Ok(ev) = rx.recv() {
                    listener.on_event(ev);
                }
            })
            .map_err(|e| LinkError::Failed(e.to_string()))?;
        let (pump_node, pump_keepers) = (node.clone(), keepers.clone());
        rt.spawn(async move {
            while let Some(ev) = events.recv().await {
                let out = match ev {
                    Event::Paired { peer, name } => {
                        keep_connected(&pump_node, &pump_keepers, peer);
                        LinkEvent::Paired {
                            id: peer.short(),
                            name,
                        }
                    }
                    Event::PeerConnected { peer, name, .. } => LinkEvent::Connected {
                        id: peer.short(),
                        name,
                    },
                    Event::PeerDisconnected { peer } => LinkEvent::Disconnected {
                        id: peer.short(),
                        name: name_of(&pump_node, &peer),
                    },
                    Event::Message { peer, body, .. } => match AppMessage::decode(&body) {
                        AppMessage::Text { text } => LinkEvent::Text {
                            id: peer.short(),
                            name: name_of(&pump_node, &peer),
                            text,
                        },
                        AppMessage::DismissNotification { key } => LinkEvent::DismissRequested {
                            id: peer.short(),
                            key,
                        },
                        AppMessage::ReplyNotification { key, text } => LinkEvent::ReplyRequested {
                            id: peer.short(),
                            key,
                            text,
                        },
                        AppMessage::NotificationAction { key, index } => {
                            LinkEvent::ActionRequested {
                                id: peer.short(),
                                key,
                                index,
                            }
                        }
                        // Phone → computer messages never arrive here; unknown ones are skipped.
                        _ => continue,
                    },
                };
                if tx.send(out).is_err() {
                    return;
                }
            }
        });

        Ok(Arc::new(Self {
            rt,
            node,
            lan: Mutex::new(None),
            keepers,
        }))
    }

    /// This phone's short id (to compare with what the laptop shows).
    pub fn id(&self) -> String {
        self.node.fingerprint().short()
    }

    /// Start browsing the LAN for paired laptops. On Android, hold a
    /// `WifiManager.MulticastLock` while this runs, or multicast packets are dropped.
    pub fn start_discovery(&self) -> Result<(), LinkError> {
        let mut lan = self.lan.lock().unwrap_or_else(|e| e.into_inner());
        if lan.is_some() {
            return Ok(());
        }
        let _guard = self.rt.enter();
        let l = Lan::new()?;
        l.browse(self.node.clone())?;
        *lan = Some(l);
        Ok(())
    }

    pub fn stop_discovery(&self) {
        if let Some(l) = self.lan.lock().unwrap_or_else(|e| e.into_inner()).take() {
            l.shutdown();
        }
    }

    pub fn devices(&self) -> Vec<Device> {
        self.node
            .peers()
            .iter()
            .map(|p| device(&self.node, p))
            .collect()
    }

    /// Parse a scanned QR code without contacting anything.
    pub fn read_pairing_code(&self, uri: String) -> Result<PairingCode, LinkError> {
        let offer = PairingOffer::from_uri(&uri)?;
        Ok(PairingCode {
            name: offer.name,
            id: offer.fingerprint.short(),
        })
    }

    /// Pair with the laptop whose QR code was scanned. Blocks for up to ~10 s.
    pub fn pair(&self, uri: String) -> Result<Device, LinkError> {
        let offer = PairingOffer::from_uri(&uri)?;
        let rec = self.rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(20), self.node.pair_with(&offer))
                .await
                .map_err(|_| {
                    LinkError::Failed(
                        "the laptop did not answer; are you on the same network?".into(),
                    )
                })?
                .map_err(LinkError::from)
        })?;
        {
            let _guard = self.rt.enter();
            keep_connected(&self.node, &self.keepers, rec.fingerprint);
        }
        Ok(device(&self.node, &rec))
    }

    /// Queue a text for `device_id` (or the only paired device). Returns true if the
    /// device is connected right now; otherwise it is delivered when it reconnects.
    pub fn send_text(&self, device_id: Option<String>, text: String) -> Result<bool, LinkError> {
        let fp = self.resolve(device_id)?;
        let body = AppMessage::Text { text }.encode()?;
        self.node.send(&fp, body)?;
        Ok(self.node.is_connected(&fp))
    }

    pub fn unpair(&self, device_id: String) -> Result<(), LinkError> {
        let fp = self.resolve(Some(device_id))?;
        if let Some(h) = self
            .keepers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&fp)
        {
            h.abort();
        }
        self.node.unpair(&fp)?;
        Ok(())
    }

    /// Mirror a new or updated notification to every paired computer.
    pub fn post_notification(&self, notification: MirroredNotification) -> Result<(), LinkError> {
        self.send_all(AppMessage::NotificationPosted(notification.into_wire()))
    }

    /// The notification was cleared on the phone.
    pub fn remove_notification(&self, key: String) -> Result<(), LinkError> {
        self.send_all(AppMessage::NotificationRemoved { key })
    }

    /// Everything currently shown; call when a computer (re)connects.
    pub fn send_snapshot(&self, notifications: Vec<MirroredNotification>) -> Result<(), LinkError> {
        let items = notifications
            .into_iter()
            .take(MAX_SNAPSHOT_ITEMS)
            .map(|n| n.into_wire())
            .collect();
        self.send_all(AppMessage::NotificationSnapshot { items })
    }

    /// An app's icon as a small PNG (sent once per app).
    pub fn send_app_icon(&self, package: String, png: Vec<u8>) -> Result<(), LinkError> {
        self.send_all(AppMessage::AppIcon { package, png })
    }

    /// Tell the computer `device_id` how its request about `key` went.
    pub fn send_action_result(
        &self,
        device_id: String,
        key: String,
        ok: bool,
        message: String,
    ) -> Result<(), LinkError> {
        let fp = self.resolve(Some(device_id))?;
        let body = AppMessage::ActionResult { key, ok, message }.encode()?;
        self.node.send(&fp, body)?;
        Ok(())
    }

    /// Call from `ConnectivityManager.NetworkCallback`: retry immediately instead of
    /// waiting out the reconnect backoff.
    pub fn on_network_changed(&self) {
        self.node.nudge();
    }

    pub fn shutdown(&self) {
        self.stop_discovery();
        self.rt.block_on(self.node.shutdown());
    }
}

impl PhoneLink {
    /// Queue a message for every paired computer (durably: it arrives after a reconnect).
    fn send_all(&self, msg: AppMessage) -> Result<(), LinkError> {
        let body = msg.encode()?;
        for p in self.node.peers() {
            self.node.send(&p.fingerprint, body.clone())?;
        }
        Ok(())
    }

    fn resolve(&self, device_id: Option<String>) -> Result<Fingerprint, LinkError> {
        let peers = self.node.peers();
        match device_id {
            None => match peers.as_slice() {
                [only] => Ok(only.fingerprint),
                [] => Err(LinkError::Failed("no paired devices".into())),
                _ => Err(LinkError::Failed(
                    "several devices paired; choose one".into(),
                )),
            },
            Some(id) => peers
                .iter()
                .find(|p| p.fingerprint.short().eq_ignore_ascii_case(&id))
                .map(|p| p.fingerprint)
                .ok_or_else(|| LinkError::Failed(format!("no paired device {id}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    struct Collect(Mutex<mpsc::Sender<LinkEvent>>);
    impl EventListener for Collect {
        fn on_event(&self, event: LinkEvent) {
            let _ = self.0.lock().unwrap().send(event);
        }
    }

    fn wait(rx: &mpsc::Receiver<LinkEvent>, f: impl Fn(&LinkEvent) -> bool) -> LinkEvent {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let ev = rx.recv_timeout(left).expect("timed out waiting for event");
            if f(&ev) {
                return ev;
            }
        }
    }

    /// The exact call sequence the Android app makes, against a real laptop node.
    #[test]
    fn android_call_sequence_against_a_laptop() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (laptop, mut lrx) = rt
            .block_on(Node::start(NodeConfig::new(
                "Kubuntu",
                None,
                "127.0.0.1:0".parse().unwrap(),
            )))
            .unwrap();
        let uri = laptop
            .start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()])
            .to_uri();

        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let phone = PhoneLink::start(
            dir.path().to_string_lossy().into(),
            "Galaxy S24 FE".into(),
            Arc::new(Collect(Mutex::new(tx))),
        )
        .unwrap();

        let code = phone.read_pairing_code(uri.clone()).unwrap();
        assert_eq!(
            code,
            PairingCode {
                name: "Kubuntu".into(),
                id: laptop.fingerprint().short()
            }
        );
        assert!(phone
            .read_pairing_code("https://example.com".into())
            .is_err());

        let dev = phone.pair(uri).unwrap();
        assert_eq!(dev.name, "Kubuntu");
        wait(&rx, |e| matches!(e, LinkEvent::Connected { .. }));
        assert!(phone.devices()[0].connected);

        assert!(phone.send_text(None, "from phone".into()).unwrap());
        let got = rt.block_on(async {
            loop {
                if let Some(Event::Message { body, .. }) = lrx.recv().await {
                    return AppMessage::decode(&body);
                }
            }
        });
        assert_eq!(
            got,
            AppMessage::Text {
                text: "from phone".into()
            }
        );

        let phone_fp = laptop.peers()[0].fingerprint;
        laptop
            .send(
                &phone_fp,
                AppMessage::Text {
                    text: "from laptop".into(),
                }
                .encode()
                .unwrap(),
            )
            .unwrap();
        let ev = wait(&rx, |e| matches!(e, LinkEvent::Text { .. }));
        assert_eq!(
            ev,
            LinkEvent::Text {
                id: dev.id.clone(),
                name: "Kubuntu".into(),
                text: "from laptop".into()
            }
        );

        phone.on_network_changed(); // must be harmless while connected
        phone.unpair(dev.id).unwrap();
        assert!(phone.devices().is_empty());
        phone.shutdown();
        rt.block_on(laptop.shutdown());
    }

    /// The calls MirrorService makes, against a real laptop node.
    #[test]
    fn notification_mirroring_call_sequence() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (laptop, mut lrx) = rt
            .block_on(Node::start(NodeConfig::new(
                "Kubuntu",
                None,
                "127.0.0.1:0".parse().unwrap(),
            )))
            .unwrap();
        let uri = laptop
            .start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()])
            .to_uri();
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let phone = PhoneLink::start(
            dir.path().to_string_lossy().into(),
            "S24".into(),
            Arc::new(Collect(Mutex::new(tx))),
        )
        .unwrap();
        phone.pair(uri).unwrap();
        wait(&rx, |e| matches!(e, LinkEvent::Connected { .. }));

        let next_msg = |lrx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>| {
            rt.block_on(async {
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        if let Some(Event::Message { body, .. }) = lrx.recv().await {
                            return AppMessage::decode(&body);
                        }
                    }
                })
                .await
                .expect("laptop got a message")
            })
        };

        let long = "x".repeat(5000);
        phone
            .post_notification(MirroredNotification {
                key: "k1".into(),
                package: "com.whatsapp".into(),
                app: "WhatsApp".into(),
                title: "Rahul".into(),
                text: long,
                posted_ms: 1,
                can_reply: true,
                actions: vec!["a".into(), "b".into(), "c".into(), "d".into()],
            })
            .unwrap();
        let AppMessage::NotificationPosted(n) = next_msg(&mut lrx) else {
            panic!("expected a posted notification")
        };
        assert_eq!(
            n.text.len(),
            phonelink_core::message::MAX_NOTIFICATION_TEXT,
            "long text truncated"
        );
        assert_eq!(n.actions.len(), 3, "at most three buttons");

        phone.send_snapshot(vec![]).unwrap();
        assert_eq!(
            next_msg(&mut lrx),
            AppMessage::NotificationSnapshot { items: vec![] }
        );
        phone
            .send_app_icon("com.whatsapp".into(), vec![1, 2, 3])
            .unwrap();
        assert!(matches!(next_msg(&mut lrx), AppMessage::AppIcon { .. }));
        phone.remove_notification("k1".into()).unwrap();
        assert_eq!(
            next_msg(&mut lrx),
            AppMessage::NotificationRemoved { key: "k1".into() }
        );

        // The laptop asks the phone to reply; the phone reports back.
        let phone_fp = laptop.peers()[0].fingerprint;
        laptop
            .send(
                &phone_fp,
                AppMessage::ReplyNotification {
                    key: "k1".into(),
                    text: "On my way".into(),
                }
                .encode()
                .unwrap(),
            )
            .unwrap();
        let ev = wait(&rx, |e| matches!(e, LinkEvent::ReplyRequested { .. }));
        let LinkEvent::ReplyRequested { id, key, text } = ev else {
            unreachable!()
        };
        assert_eq!((key.as_str(), text.as_str()), ("k1", "On my way"));
        phone
            .send_action_result(id, key, true, "Reply sent".into())
            .unwrap();
        assert_eq!(
            next_msg(&mut lrx),
            AppMessage::ActionResult {
                key: "k1".into(),
                ok: true,
                message: "Reply sent".into()
            }
        );

        laptop
            .send(
                &phone_fp,
                AppMessage::DismissNotification { key: "k9".into() }
                    .encode()
                    .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            wait(&rx, |e| matches!(e, LinkEvent::DismissRequested { .. })),
            LinkEvent::DismissRequested { .. }
        ));
        laptop
            .send(
                &phone_fp,
                AppMessage::NotificationAction {
                    key: "k9".into(),
                    index: 1,
                }
                .encode()
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            wait(&rx, |e| matches!(e, LinkEvent::ActionRequested { .. })),
            LinkEvent::ActionRequested { index: 1, .. }
        ));

        phone.shutdown();
        rt.block_on(laptop.shutdown());
    }
}
