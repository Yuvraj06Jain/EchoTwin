//! The phone's Touchpad tab: a [`TouchpadLink`], separate from [`crate::PhoneLink`].
//!
//! It has its own node, identity and trust store (the app passes `filesDir/touchpad`) and
//! pairs only with the laptop's `phonelink-touchpad` service, by its own QR code
//! (`phonelink touchpad pair`). The main EchoTwin pairing never gives touchpad power, and
//! this one gives nothing else.
//!
//! `pair` blocks: call it off the main thread. `touch` and `button` never block; they are
//! called on every touch event, straight from the UI thread. Events arrive on a background
//! thread through [`PadListener`].
//!
//! While a finger or a button is down, a frame goes out at least every [`HEARTBEAT`], so the
//! laptop can tell "holding still" from "connection lost". When the last finger lifts, the
//! empty frame is repeated, because a single lost "lift" datagram would leave it held.
//!
//! Ported from TouchTwin (`touchtwin-ffi/src/lib.rs`); `button` is new, and touchpad
//! messages now go out with `send_fleeting` (never queued for later).

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

use phonelink_core::{
    lan::Lan,
    touchpad::{
        offer_from_pair_uri, Button, Contact, Msg, PadInfo, TouchFrame, CAP_TOUCHPAD, COORD_MAX,
        MAX_CONTACTS,
    },
    Event, Fingerprint, Node, NodeConfig, PeerRecord,
};
use tokio::{runtime::Runtime, task::JoinHandle};

/// While fingers or buttons are down, send at least this often even without movement.
pub const HEARTBEAT: Duration = Duration::from_millis(100);
/// After the last finger lifts, send the empty frame this many more times…
const LIFT_REPEATS: u32 = 2;
/// …this far apart.
const LIFT_REPEAT_GAP: Duration = Duration::from_millis(15);

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum PadError {
    #[error("{0}")]
    Failed(String),
}

impl From<phonelink_core::Error> for PadError {
    fn from(e: phonelink_core::Error) -> Self {
        PadError::Failed(e.to_string())
    }
}

/// A laptop paired with the Touchpad tab.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PadLaptop {
    /// Short id shown on both screens, e.g. `3F2A-91C0-77DE-0B14`.
    pub id: String,
    pub name: String,
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum PadEvent {
    Paired { name: String },
    Connected { name: String },
    Disconnected,
    /// The laptop created its virtual touchpad: touches now move the pointer.
    PadReady,
    /// The laptop could not create its virtual touchpad (`message` says how to fix it).
    PadFailed { message: String },
}

/// One finger, as Android reports it. `x`, `y` are 0.0–1.0 across the pad.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct Finger {
    pub slot: u8,
    /// New for every touch (Android's pointer id plus a counter, any u16).
    pub id: u16,
    pub x: f32,
    pub y: f32,
}

/// Implemented in Kotlin. Called on a background thread, one event at a time, in order.
#[uniffi::export(with_foreign)]
pub trait PadListener: Send + Sync {
    fn on_event(&self, event: PadEvent);
}

/// What the touch side shares with the heartbeat task.
#[derive(Default)]
struct TouchState {
    /// Laptop the pad drives (the most recently connected one).
    target: Option<Fingerprint>,
    /// The pad is open on the phone, at this size.
    pad: Option<PadInfo>,
    contacts: Vec<Contact>,
    /// Left / Right click held on the phone.
    left: bool,
    right: bool,
    last_sent: Option<Instant>,
}

impl TouchState {
    fn holding(&self) -> bool {
        !self.contacts.is_empty() || self.left || self.right
    }
}

#[derive(uniffi::Object)]
pub struct TouchpadLink {
    rt: Runtime,
    node: Node,
    lan: Mutex<Option<Lan>>,
    keepers: Arc<Mutex<HashMap<Fingerprint, JoinHandle<()>>>>,
    touch: Arc<Mutex<TouchState>>,
    seq: Arc<AtomicU32>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn keep_connected(node: &Node, keepers: &Mutex<HashMap<Fingerprint, JoinHandle<()>>>, fp: Fingerprint) {
    let mut k = lock(keepers);
    if k.get(&fp).is_some_and(|h| !h.is_finished()) {
        return;
    }
    k.insert(fp, node.maintain(fp, vec![]));
}

fn laptop(node: &Node, p: &PeerRecord) -> PadLaptop {
    PadLaptop {
        id: p.fingerprint.short(),
        name: p.name.clone(),
        connected: node.is_connected(&p.fingerprint),
    }
}

fn send_frame(node: &Node, seq: &AtomicU32, st: &mut TouchState) -> bool {
    let Some(target) = st.target else { return false };
    if st.pad.is_none() {
        return false;
    }
    let frame = TouchFrame {
        seq: seq.fetch_add(1, Ordering::Relaxed),
        contacts: st.contacts.clone(),
    };
    st.last_sent = Some(Instant::now());
    node.send_datagram(&target, frame.encode()).unwrap_or(false)
}

fn send_msg(node: &Node, st: &TouchState, m: Msg) -> bool {
    match st.target {
        Some(target) => node.send_fleeting(&target, m.encode()).unwrap_or(false),
        None => false,
    }
}

fn send_pad_info(node: &Node, st: &TouchState) {
    if let Some(pad) = st.pad {
        send_msg(node, st, Msg::PadInfo(pad));
    }
}

fn to_contact(f: &Finger) -> Option<Contact> {
    if f.slot as usize >= MAX_CONTACTS || !f.x.is_finite() || !f.y.is_finite() {
        return None;
    }
    let scale = |v: f32| (v.clamp(0.0, 1.0) * COORD_MAX as f32).round() as u16;
    Some(Contact { slot: f.slot, id: f.id, x: scale(f.x), y: scale(f.y) })
}

#[uniffi::export]
impl TouchpadLink {
    /// Start the touchpad's own node. `data_dir`: a directory of its own (the app uses
    /// `filesDir/touchpad`), never the main link's.
    #[uniffi::constructor]
    pub fn start(
        data_dir: String,
        device_name: String,
        listener: Arc<dyn PadListener>,
    ) -> Result<Arc<Self>, PadError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("touchpad")
            .enable_all()
            .build()
            .map_err(|e| PadError::Failed(e.to_string()))?;
        let dir = PathBuf::from(data_dir);
        std::fs::create_dir_all(&dir).map_err(|e| PadError::Failed(e.to_string()))?;
        // The laptop never dials the phone, so any port will do.
        let (node, mut events) = rt.block_on(async {
            let mut last = None;
            for bind in [SocketAddr::from(([0u16; 8], 0)), SocketAddr::from(([0u8; 4], 0))] {
                let mut cfg = NodeConfig::new(device_name.clone(), Some(dir.clone()), bind);
                cfg.capabilities = vec![CAP_TOUCHPAD.to_string()];
                match Node::start(cfg).await {
                    Ok(x) => return Ok(x),
                    Err(e) => last = Some(e),
                }
            }
            Err(PadError::from(last.expect("tried at least once")))
        })?;

        let keepers = Arc::new(Mutex::new(HashMap::new()));
        let touch = Arc::new(Mutex::new(TouchState::default()));
        let seq = Arc::new(AtomicU32::new(0));
        let lan = {
            let _guard = rt.enter();
            for p in node.peers() {
                keep_connected(&node, &keepers, p.fingerprint);
            }
            let lan = Lan::new().ok();
            if let Some(l) = &lan {
                if let Err(e) = l.browse(node.clone()) {
                    tracing::warn!("mDNS browse failed: {e}");
                }
            }
            lan
        };

        // Events → Kotlin (and session bookkeeping).
        {
            let (node, touch, keepers) = (node.clone(), touch.clone(), keepers.clone());
            rt.spawn(async move {
                while let Some(ev) = events.recv().await {
                    let out = match ev {
                        Event::Paired { peer, name } => {
                            keep_connected(&node, &keepers, peer);
                            Some(PadEvent::Paired { name })
                        }
                        Event::PeerConnected { peer, name, capabilities, .. } => {
                            if !capabilities.iter().any(|c| c == CAP_TOUCHPAD) {
                                continue;
                            }
                            let mut st = lock(&touch);
                            st.target = Some(peer);
                            send_pad_info(&node, &st);
                            Some(PadEvent::Connected { name })
                        }
                        Event::PeerDisconnected { peer } => {
                            let mut st = lock(&touch);
                            if st.target == Some(peer) {
                                // Fall back to another connected laptop, if any.
                                st.target = node
                                    .peers()
                                    .into_iter()
                                    .map(|p| p.fingerprint)
                                    .find(|fp| *fp != peer && node.is_connected(fp));
                                send_pad_info(&node, &st);
                                Some(PadEvent::Disconnected)
                            } else {
                                None
                            }
                        }
                        Event::Message { body, .. } => match Msg::decode(&body) {
                            Some(Msg::PadStatus { ok: true, .. }) => Some(PadEvent::PadReady),
                            Some(Msg::PadStatus { ok: false, message }) => {
                                Some(PadEvent::PadFailed { message })
                            }
                            _ => None,
                        },
                        Event::PeerPaused { .. } | Event::Datagram { .. } => None,
                    };
                    if let Some(e) = out {
                        listener.on_event(e);
                    }
                }
            });
        }

        // Heartbeat: keep held fingers and buttons alive on the laptop.
        {
            let (node, touch, seq) = (node.clone(), touch.clone(), seq.clone());
            rt.spawn(async move {
                let mut tick = tokio::time::interval(HEARTBEAT / 2);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tick.tick().await;
                    let mut st = lock(&touch);
                    if st.holding() && st.last_sent.is_none_or(|t| t.elapsed() >= HEARTBEAT) {
                        send_frame(&node, &seq, &mut st);
                    }
                }
            });
        }

        Ok(Arc::new(Self {
            rt,
            node,
            lan: Mutex::new(lan),
            keepers,
            touch,
            seq,
        }))
    }

    /// This phone's touchpad id, as shown by `phonelink touchpad devices` on the laptop.
    pub fn my_id(&self) -> String {
        self.node.fingerprint().short()
    }

    pub fn laptops(&self) -> Vec<PadLaptop> {
        self.node.peers().iter().map(|p| laptop(&self.node, p)).collect()
    }

    /// Pair with the laptop whose touchpad QR code (`phonelink touchpad pair`) was scanned.
    /// Blocks for up to ~20 s.
    pub fn pair(&self, uri: String) -> Result<PadLaptop, PadError> {
        let offer = offer_from_pair_uri(&uri)?;
        let rec = self.rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(20), self.node.pair_with(&offer))
                .await
                .map_err(|_| {
                    PadError::Failed("the laptop did not answer; are you on the same network?".into())
                })?
                .map_err(PadError::from)
        })?;
        {
            let _guard = self.rt.enter();
            keep_connected(&self.node, &self.keepers, rec.fingerprint);
        }
        Ok(laptop(&self.node, &rec))
    }

    pub fn unpair(&self, laptop_id: String) -> Result<(), PadError> {
        let Some(p) = self.node.peers().into_iter().find(|p| p.fingerprint.short() == laptop_id) else {
            return Err(PadError::Failed("no such laptop".into()));
        };
        if let Some(h) = lock(&self.keepers).remove(&p.fingerprint) {
            h.abort();
        }
        self.node.unpair(&p.fingerprint)?;
        Ok(())
    }

    /// Retry connecting at once (tab opened, network changed).
    pub fn reconnect_now(&self) {
        self.node.nudge();
    }

    /// The pad is on screen (or changed size). Physical size in millimetres.
    pub fn open_pad(&self, width_mm: f32, height_mm: f32) {
        let dmm = |mm: f32| (mm * 10.0).clamp(0.0, u16::MAX as f32) as u16;
        let mut st = lock(&self.touch);
        st.pad = Some(PadInfo { width_dmm: dmm(width_mm), height_dmm: dmm(height_mm) });
        st.contacts.clear();
        st.left = false;
        st.right = false;
        send_pad_info(&self.node, &st);
    }

    /// The pad left the screen: lift every finger and let go of both buttons on the laptop.
    pub fn close_pad(&self) {
        let mut st = lock(&self.touch);
        st.contacts.clear();
        st.left = false;
        st.right = false;
        send_msg(&self.node, &st, Msg::Release);
        st.pad = None;
    }

    /// Every touch event: the fingers that are down now (empty when the last one lifts).
    /// Returns false if nothing could be sent (not connected).
    pub fn touch(&self, fingers: Vec<Finger>) -> bool {
        let contacts: Vec<Contact> = fingers.iter().filter_map(to_contact).take(MAX_CONTACTS).collect();
        let mut st = lock(&self.touch);
        let lifted = contacts.is_empty() && !st.contacts.is_empty();
        st.contacts = contacts;
        let sent = send_frame(&self.node, &self.seq, &mut st);
        drop(st);
        if lifted {
            let (node, touch, seq) = (self.node.clone(), self.touch.clone(), self.seq.clone());
            self.rt.spawn(async move {
                for _ in 0..LIFT_REPEATS {
                    tokio::time::sleep(LIFT_REPEAT_GAP).await;
                    let mut st = lock(&touch);
                    if !st.contacts.is_empty() {
                        return; // a new touch started; its frames say everything
                    }
                    send_frame(&node, &seq, &mut st);
                }
            });
        }
        sent
    }

    /// The Left click (`left`) or Right click button went down or up. Holding Left while a
    /// finger moves is a drag. Returns false if it could not be sent (not connected).
    pub fn button(&self, left: bool, down: bool) -> bool {
        let mut st = lock(&self.touch);
        if st.pad.is_none() {
            return false;
        }
        let held = if left { &mut st.left } else { &mut st.right };
        *held = down;
        let button = if left { Button::Left } else { Button::Right };
        let sent = send_msg(&self.node, &st, Msg::Button { button, down });
        // Start the heartbeat clock now, so a held button is kept alive from here.
        st.last_sent = Some(Instant::now());
        sent
    }

    /// Round-trip time to the laptop the pad drives, for a latency indicator.
    pub fn latency_ms(&self) -> Option<u32> {
        let target = lock(&self.touch).target?;
        self.node.rtt(&target).map(|d| d.as_millis() as u32)
    }

    pub fn shutdown(&self) {
        if let Some(l) = lock(&self.lan).take() {
            l.shutdown();
        }
        for (_, h) in lock(&self.keepers).drain() {
            h.abort();
        }
        self.rt.block_on(self.node.shutdown());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    struct Collect(Mutex<mpsc::Sender<PadEvent>>);
    impl PadListener for Collect {
        fn on_event(&self, event: PadEvent) {
            let _ = lock(&self.0).send(event);
        }
    }

    fn wait(rx: &mpsc::Receiver<PadEvent>, want: impl Fn(&PadEvent) -> bool) -> PadEvent {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let e = rx.recv_timeout(left).expect("event in time");
            if want(&e) {
                return e;
            }
        }
    }

    enum Got {
        Msg(Msg),
        Frame(TouchFrame),
    }

    /// Everything the stand-in laptop receives within `d`.
    fn collect(rt: &Runtime, lrx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>, d: Duration) -> Vec<Got> {
        rt.block_on(async {
            let mut out = vec![];
            let _ = tokio::time::timeout(d, async {
                while let Some(e) = lrx.recv().await {
                    match e {
                        Event::Datagram { body, .. } => {
                            out.push(Got::Frame(TouchFrame::decode(&body).expect("valid frame")))
                        }
                        Event::Message { body, .. } => {
                            out.push(Got::Msg(Msg::decode(&body).expect("valid message")))
                        }
                        _ => {}
                    }
                }
            })
            .await;
            out
        })
    }

    /// The whole phone side against a stand-in laptop: pair, open the pad, touch, lift,
    /// buttons, close.
    #[test]
    fn phone_pairs_opens_the_pad_and_streams_fingers_and_buttons() {
        let rt = Runtime::new().unwrap();
        let (laptop, mut lrx) = rt
            .block_on(async {
                let mut cfg = NodeConfig::new("Laptop", None, "127.0.0.1:0".parse().unwrap());
                cfg.capabilities = vec![CAP_TOUCHPAD.into()];
                cfg.datagrams = true;
                Node::start(cfg).await
            })
            .unwrap();
        let offer = laptop.start_pairing(Duration::from_secs(60), vec![laptop.local_addr().unwrap()]);

        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let phone = TouchpadLink::start(
            dir.path().join("touchpad").to_string_lossy().into(),
            "Phone".into(),
            Arc::new(Collect(Mutex::new(tx))),
        )
        .unwrap();
        // The main EchoTwin code is refused here.
        assert!(phone.pair(offer.to_uri()).is_err());
        assert!(!phone.button(true, true), "no pad open, nothing sent");
        let l = phone.pair(phonelink_core::touchpad::to_pair_uri(&offer)).unwrap();
        assert_eq!(l.name, "Laptop");
        wait(&rx, |e| matches!(e, PadEvent::Connected { .. }));

        phone.open_pad(70.0, 150.0);
        // The laptop sees the pad size and answers that its pad is ready.
        let (pfp, info) = rt.block_on(async {
            loop {
                if let Some(Event::Message { peer, body, .. }) = lrx.recv().await {
                    if let Some(Msg::PadInfo(i)) = Msg::decode(&body) {
                        return (peer, i);
                    }
                }
            }
        });
        assert_eq!(info, PadInfo { width_dmm: 700, height_dmm: 1500 });
        laptop.send_fleeting(&pfp, Msg::status(true, "").encode()).unwrap();
        wait(&rx, |e| matches!(e, PadEvent::PadReady));
        assert!(phone.latency_ms().is_some());

        // Touch, hold still (heartbeat), lift (repeated).
        assert!(phone.touch(vec![Finger { slot: 0, id: 9, x: 0.5, y: 1.2 }]));
        std::thread::sleep(Duration::from_millis(350));
        assert!(phone.touch(vec![]));
        let frames: Vec<TouchFrame> = collect(&rt, &mut lrx, Duration::from_millis(800))
            .into_iter()
            .filter_map(|g| match g {
                Got::Frame(f) => Some(f),
                Got::Msg(_) => None,
            })
            .collect();
        let down: Vec<_> = frames.iter().filter(|f| !f.contacts.is_empty()).collect();
        let up = frames.iter().filter(|f| f.contacts.is_empty()).count();
        assert!(down.len() >= 3, "touch plus heartbeats while holding still: {}", down.len());
        assert_eq!(down[0].contacts[0], Contact { slot: 0, id: 9, x: 5000, y: COORD_MAX });
        assert_eq!(up, 1 + LIFT_REPEATS as usize, "the lift is repeated");
        let seqs: Vec<u32> = frames.iter().map(|f| f.seq).collect();
        let mut sorted = seqs.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), seqs.len(), "every frame has its own sequence number");

        // Right click held with no finger: the button, then empty heartbeat frames, then up.
        assert!(phone.button(false, true));
        std::thread::sleep(Duration::from_millis(450));
        assert!(phone.button(false, false));
        let got = collect(&rt, &mut lrx, Duration::from_millis(500));
        let msgs: Vec<&Msg> = got.iter().filter_map(|g| match g { Got::Msg(m) => Some(m), _ => None }).collect();
        assert_eq!(
            msgs,
            [
                &Msg::Button { button: Button::Right, down: true },
                &Msg::Button { button: Button::Right, down: false }
            ]
        );
        let beats = got.iter().filter(|g| matches!(g, Got::Frame(f) if f.contacts.is_empty())).count();
        assert!(beats >= 3, "a held button keeps the heartbeat going: {beats}");
        let after = collect(&rt, &mut lrx, Duration::from_millis(300));
        assert!(after.is_empty(), "nothing once everything is let go");

        // Leaving the tab lifts everything on the laptop.
        assert!(phone.button(true, true));
        phone.close_pad();
        let got = collect(&rt, &mut lrx, Duration::from_millis(300));
        assert!(got.iter().any(|g| matches!(g, Got::Msg(Msg::Release))));
        assert!(!phone.button(true, true), "closed pad: buttons do nothing");

        assert_eq!(phone.laptops().len(), 1);
        phone.unpair(l.id).unwrap();
        assert!(phone.laptops().is_empty());
        phone.shutdown();
        rt.block_on(laptop.shutdown());
    }
}
