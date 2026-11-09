//! A EchoTwin node: one QUIC endpoint that both listens and dials.
//!
//! Connection life-cycle:
//! 1. QUIC + mutual TLS handshake. Unpaired certificates are refused here (see [`crate::tls`]).
//! 2. The dialer opens the control stream and sends `Hello` (or `PairRequest` when pairing).
//! 3. Both sides exchange capabilities, then flush their durable outbox to each other.
//! 4. `Deliver`/`Ack` until the connection drops; [`Node::maintain`] then reconnects with
//!    backoff and the outbox resends whatever was not acknowledged.
//!
//! A session can also run over a byte stream the app supplies (the Bluetooth backup link):
//! [`Node::connect_stream`] / [`Node::accept_stream`] do mutual TLS 1.3 with the same pinned
//! keys, then the same Hello/Deliver/Ack protocol. One session per peer: QUIC always wins,
//! and a QUIC connection that comes back replaces a running stream session.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::Duration,
};

use quinn::{Connection, Endpoint, SendStream, VarInt};
use rustls::{crypto::CryptoProvider, pki_types::ServerName};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, watch},
    task::JoinHandle,
    time::timeout,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing::{debug, info, warn};

use crate::{
    limiter::HandshakeLimiter,
    outbox::Outbox,
    pairing::{self, PairingOffer},
    tls,
    trust::{AccessGate, TrustStore},
    util,
    wire::{self, Control, Frame, MAX_BODY_LEN, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION},
    Error, Fingerprint, Identity, MsgId, PeerRecord, Result,
};

/// Capability for the generic durable message channel.
pub const CAP_MESSAGES: &str = "messages/1";

pub mod close_code {
    pub const NORMAL: u32 = 0;
    pub const REJECTED: u32 = 0x10;
    pub const PAIRED: u32 = 0x11;
    pub const REPLACED: u32 = 0x12;
    pub const PROTOCOL: u32 = 0x13;
    pub const SHUTDOWN: u32 = 0x14;
    pub const UNPAIRED: u32 = 0x15;
}

/// `Reject` reason meaning "my owner switched the connection off; don't keep trying".
pub const PAUSED_REASON: &str = "paused";
/// While the peer has switched off, look again this rarely (it normally dials us when it
/// switches back on).
const PEER_PAUSED_RECHECK: Duration = Duration::from_secs(600);

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(8);
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const BACKOFF_MIN: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const SEEN_RETENTION_SECS: u64 = 60 * 60 * 24 * 30;
/// Undelivered messages are given up after a week (well inside SEEN_RETENTION_SECS, so a
/// late delivery can still never be a duplicate).
const PENDING_RETENTION_SECS: u64 = 60 * 60 * 24 * 7;
/// Byte-stream links (Bluetooth) are slower to set up than QUIC on a LAN.
const STREAM_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const STREAM_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// A stream link has no transport keep-alive of its own: ping when idle, and give up on a
/// link that has been silent for a while (out of range, socket wedged).
pub const STREAM_PING_EVERY: Duration = Duration::from_secs(10);
pub const STREAM_SILENCE_LIMIT: Duration = Duration::from_secs(35);

#[derive(Clone, Debug)]
pub struct NodeConfig {
    /// Where identity, peers and the outbox live. `None` = ephemeral (tests).
    pub data_dir: Option<PathBuf>,
    pub device_name: String,
    pub bind: SocketAddr,
    pub capabilities: Vec<String>,
    /// Finding a peer whose address changed without help from discovery.
    pub probe: ProbeConfig,
    /// Keep-alive on stream (Bluetooth) links: ping this often when idle…
    pub stream_ping_every: Duration,
    /// …and drop a link that has been silent this long.
    pub stream_silence_limit: Duration,
}

/// How [`Node::maintain`] looks for a peer whose remembered addresses stopped working.
#[derive(Clone, Debug)]
pub struct ProbeConfig {
    pub enabled: bool,
    /// Subnets to search (own address, prefix). `None`: this device's LAN interfaces.
    pub subnets: Option<Vec<(std::net::Ipv4Addr, u8)>>,
    /// Gateways to try every round. `None`: this machine's default gateways.
    pub gateways: Option<Vec<std::net::Ipv4Addr>>,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            subnets: None,
            gateways: None,
        }
    }
}

/// Search the whole subnet after this many failed rounds…
const PROBE_AFTER_FAILURES: u32 = 2;
/// …and again this often while still disconnected.
const PROBE_INTERVAL: Duration = Duration::from_secs(180);
/// After a nudge (network change, phone unlocked, "Reconnect now") search at most this often.
const PROBE_AFTER_NUDGE_MIN_GAP: Duration = Duration::from_secs(20);
/// While a peer stays unreachable, say so in the log this often.
const OFFLINE_LOG_INTERVAL: Duration = Duration::from_secs(180);

impl NodeConfig {
    pub fn new(
        device_name: impl Into<String>,
        data_dir: Option<PathBuf>,
        bind: SocketAddr,
    ) -> Self {
        Self {
            data_dir,
            device_name: device_name.into(),
            bind,
            capabilities: vec![CAP_MESSAGES.to_string()],
            probe: ProbeConfig::default(),
            stream_ping_every: STREAM_PING_EVERY,
            stream_silence_limit: STREAM_SILENCE_LIMIT,
        }
    }
}

/// An extra QUIC stream the peer opened on an established session (file data).
pub struct DataStream {
    pub peer: Fingerprint,
    pub send: SendStream,
    pub recv: quinn::RecvStream,
}

/// Which kind of link carries a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Transport {
    /// QUIC over IP: Wi-Fi, a hotspot, Ethernet. Preferred whenever it works.
    Quic,
    /// TLS over a byte stream the app supplied: the Bluetooth backup link.
    Stream,
}

/// A live link to a peer. Closing it ends the session's tasks, which drops the stream.
#[derive(Clone)]
enum Link {
    Quic(Connection),
    Stream(Arc<watch::Sender<bool>>),
}

impl Link {
    fn new_stream() -> Self {
        Link::Stream(Arc::new(watch::channel(false).0))
    }

    fn transport(&self) -> Transport {
        match self {
            Link::Quic(_) => Transport::Quic,
            Link::Stream(_) => Transport::Stream,
        }
    }

    fn is_open(&self) -> bool {
        match self {
            Link::Quic(c) => c.close_reason().is_none(),
            Link::Stream(closed) => !*closed.borrow(),
        }
    }

    fn close(&self, code: u32, reason: &'static [u8]) {
        match self {
            Link::Quic(c) => c.close(VarInt::from_u32(code), reason),
            Link::Stream(closed) => {
                closed.send_replace(true);
            }
        }
    }

    async fn closed(&self) {
        match self {
            Link::Quic(c) => {
                c.closed().await;
            }
            Link::Stream(closed) => {
                let mut rx = closed.subscribe();
                let _ = rx.wait_for(|v| *v).await;
            }
        }
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        match self {
            Link::Quic(c) => Some(c.remote_address()),
            Link::Stream(_) => None,
        }
    }
}

/// `AA:BB:CC:DD:EE:FF`, upper-case hex. Anything else from a peer is ignored.
pub fn valid_bluetooth_addr(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 17
        && b.iter().enumerate().all(|(i, c)| {
            if i % 3 == 2 {
                *c == b':'
            } else {
                c.is_ascii_digit() || (b'A'..=b'F').contains(c)
            }
        })
}

#[derive(Debug, Clone)]
pub enum Event {
    Paired {
        peer: Fingerprint,
        name: String,
    },
    /// Also sent when an existing session moves to another link (Bluetooth → Wi-Fi).
    PeerConnected {
        peer: Fingerprint,
        name: String,
        capabilities: Vec<String>,
        /// `None` for a stream link.
        addr: Option<SocketAddr>,
        transport: Transport,
    },
    PeerDisconnected {
        peer: Fingerprint,
    },
    /// The peer switched its connection off (it will dial us when it switches back on).
    PeerPaused {
        peer: Fingerprint,
    },
    /// Delivered exactly once per message ID, in arrival order.
    Message {
        peer: Fingerprint,
        id: MsgId,
        body: Vec<u8>,
    },
}

struct SessionHandle {
    id: u64,
    link: Link,
    tx: mpsc::UnboundedSender<Control>,
    we_dialed: bool,
    /// Features both sides support on this session.
    capabilities: Vec<String>,
}

struct Inner {
    identity: Identity,
    config: NodeConfig,
    endpoint: Endpoint,
    provider: Arc<CryptoProvider>,
    gate: Arc<AccessGate>,
    outbox: Outbox,
    limiter: HandshakeLimiter,
    sessions: Mutex<HashMap<Fingerprint, SessionHandle>>,
    events: mpsc::UnboundedSender<Event>,
    next_session: AtomicU64,
    shutdown: watch::Sender<bool>,
    /// Fresh addresses learned from LAN discovery, most recent first.
    addr_hints: Mutex<HashMap<Fingerprint, Vec<SocketAddr>>>,
    /// Bumped to make reconnect loops retry at once (network changed / peer discovered).
    /// A counter in a watch channel cannot lose a wake-up, unlike a bare Notify.
    nudge: watch::Sender<u64>,
    /// One outgoing dial per peer at a time. Without this, two callers (e.g. pairing and
    /// the reconnect loop) could both connect, and the second connection would replace
    /// the first: a needless disconnect/reconnect blip.
    dial_locks: Mutex<HashMap<Fingerprint, Arc<tokio::sync::Mutex<()>>>>,
    /// The `Links` frame sent to every peer after `Hello` (`None`: not set, send nothing).
    links: Mutex<Option<Control>>,
    /// Extra streams opened by peers (file data), for whoever called `data_streams`.
    data_tx: mpsc::UnboundedSender<DataStream>,
    data_rx: Mutex<Option<mpsc::UnboundedReceiver<DataStream>>>,
    /// Connection switched off on this device: no dialling, incoming sessions refused.
    paused: watch::Sender<bool>,
    /// Peers that switched off on their side; we stop dialling them.
    peers_paused: Mutex<HashSet<Fingerprint>>,
    /// Bumped when `peers_paused` loses an entry, so waiting reconnect loops look again.
    peers_paused_changed: watch::Sender<u64>,
}

#[derive(Clone)]
pub struct Node {
    inner: Arc<Inner>,
}

struct PeerHello {
    version: u16,
    name: String,
    capabilities: Vec<String>,
}

impl Node {
    /// Start listening. Must be called inside a Tokio runtime.
    pub async fn start(config: NodeConfig) -> Result<(Node, mpsc::UnboundedReceiver<Event>)> {
        let (identity, trust, outbox) = match &config.data_dir {
            Some(dir) => {
                std::fs::create_dir_all(dir)?;
                let outbox = Outbox::open(&dir.join("outbox.sqlite3"))?;
                let _ = outbox.prune_seen(SEEN_RETENTION_SECS);
                let _ = outbox.prune_pending(PENDING_RETENTION_SECS);
                (
                    Identity::load_or_create(dir)?,
                    TrustStore::load(dir.join("peers.json"))?,
                    outbox,
                )
            }
            None => (
                Identity::generate()?,
                TrustStore::in_memory(),
                Outbox::in_memory()?,
            ),
        };
        let provider = tls::crypto_provider();
        let gate = Arc::new(AccessGate::new(trust));
        let server_config = tls::server_config(&identity, gate.clone(), provider.clone())?;
        let endpoint = Endpoint::server(server_config, config.bind)?;
        let (events, rx) = mpsc::unbounded_channel();
        let (data_tx, data_rx) = mpsc::unbounded_channel();
        let node = Node {
            inner: Arc::new(Inner {
                identity,
                config,
                endpoint,
                provider,
                gate,
                outbox,
                limiter: HandshakeLimiter::default(),
                sessions: Mutex::new(HashMap::new()),
                events,
                next_session: AtomicU64::new(1),
                shutdown: watch::channel(false).0,
                addr_hints: Mutex::new(HashMap::new()),
                nudge: watch::channel(0).0,
                dial_locks: Mutex::new(HashMap::new()),
                links: Mutex::new(None),
                data_tx,
                data_rx: Mutex::new(Some(data_rx)),
                paused: watch::channel(false).0,
                peers_paused: Mutex::new(HashSet::new()),
                peers_paused_changed: watch::channel(0).0,
            }),
        };
        info!(fingerprint = %node.fingerprint().short(), addr = ?node.local_addr(), "node started");
        tokio::spawn(node.clone().accept_loop());
        Ok((node, rx))
    }

    /// Switch the connection off (`true`) or back on. Off: every session is told
    /// "paused" and closed, reconnect loops stop dialling, and paired devices that connect
    /// are told "paused" too (so they stop searching). On: reconnect at once.
    pub fn set_paused(&self, paused: bool) {
        if self.inner.paused.send_replace(paused) == paused {
            return;
        }
        if paused {
            let links: Vec<(Link, mpsc::UnboundedSender<Control>)> = self
                .sessions()
                .values()
                .map(|s| (s.link.clone(), s.tx.clone()))
                .collect();
            for (link, tx) in links {
                let _ = tx.send(Control::Reject {
                    reason: PAUSED_REASON.into(),
                });
                tokio::spawn(async move {
                    // Let the writer send the frame, then hang up.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    link.close(close_code::NORMAL, b"paused");
                });
            }
            info!("connection switched off");
        } else {
            info!("connection switched on");
            self.nudge();
        }
    }

    pub fn is_paused(&self) -> bool {
        *self.inner.paused.borrow()
    }

    /// Did `fp` switch its connection off (and has not connected since)?
    pub fn peer_paused(&self, fp: &Fingerprint) -> bool {
        self.inner
            .peers_paused
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(fp)
    }

    fn mark_peer_paused(&self, peer: Fingerprint) {
        let new = self
            .inner
            .peers_paused
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(peer);
        if new {
            info!(peer = %peer.short(), "peer switched its connection off");
            self.emit(Event::PeerPaused { peer });
        }
    }

    fn clear_peer_paused(&self, peer: &Fingerprint) {
        let removed = self
            .inner
            .peers_paused
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer);
        if removed {
            self.inner.peers_paused_changed.send_modify(|v| *v += 1);
        }
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.inner.identity.fingerprint()
    }

    pub fn device_name(&self) -> &str {
        &self.inner.config.device_name
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.inner.endpoint.local_addr()?)
    }

    pub fn peers(&self) -> Vec<PeerRecord> {
        self.inner.gate.trust().list()
    }

    pub fn peer(&self, fp: &Fingerprint) -> Option<PeerRecord> {
        self.inner.gate.trust().get(fp).cloned()
    }

    pub fn is_connected(&self, fp: &Fingerprint) -> bool {
        self.transport(fp).is_some()
    }

    /// Streams peers open for file data. Can be taken once; without a taker, such streams
    /// are refused.
    pub fn data_streams(&self) -> Option<mpsc::UnboundedReceiver<DataStream>> {
        self.inner
            .data_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Open a stream for file data to a peer connected over QUIC. Over the Bluetooth backup
    /// (or when offline) this fails: files wait for Wi-Fi.
    pub async fn open_data(&self, peer: &Fingerprint) -> Result<(SendStream, quinn::RecvStream)> {
        let link = self
            .sessions()
            .get(peer)
            .filter(|s| s.link.is_open())
            .map(|s| s.link.clone());
        match link {
            Some(Link::Quic(conn)) => Ok(conn.open_bi().await?),
            Some(Link::Stream(_)) => Err(Error::NoNetworkLink),
            None => Err(Error::Protocol("not connected".into())),
        }
    }

    /// Whether the connected peer `fp` supports `capability` (false while offline).
    pub fn peer_supports(&self, fp: &Fingerprint, capability: &str) -> bool {
        self.sessions()
            .get(fp)
            .filter(|s| s.link.is_open())
            .is_some_and(|s| s.capabilities.iter().any(|c| c == capability))
    }

    /// The link the session with `fp` currently runs on, if connected.
    pub fn transport(&self, fp: &Fingerprint) -> Option<Transport> {
        self.sessions()
            .get(fp)
            .filter(|s| s.link.is_open())
            .map(|s| s.link.transport())
    }

    pub fn pending_count(&self, fp: &Fingerprint) -> Result<usize> {
        self.inner.outbox.pending_count(fp)
    }

    fn sessions(&self) -> MutexGuard<'_, HashMap<Fingerprint, SessionHandle>> {
        self.inner
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, e: Event) {
        let _ = self.inner.events.send(e);
    }

    fn hello(&self) -> Control {
        Control::Hello {
            version: PROTOCOL_VERSION,
            name: self.inner.config.device_name.clone(),
            capabilities: self.inner.config.capabilities.clone(),
        }
    }

    // ---------------------------------------------------------------- pairing

    /// Open the pairing window (the laptop side). Show `offer.to_uri()` as a QR code.
    /// `addrs` are the addresses the phone should dial (the LAN IPs of this machine).
    pub fn start_pairing(&self, ttl: Duration, addrs: Vec<SocketAddr>) -> PairingOffer {
        let secret = self.inner.gate.open_pairing(ttl);
        info!(?ttl, "pairing window opened");
        PairingOffer {
            fingerprint: self.fingerprint(),
            secret,
            addrs,
            name: self.inner.config.device_name.clone(),
        }
    }

    pub fn cancel_pairing(&self) {
        self.inner.gate.close_pairing();
    }

    /// Pair with a device whose QR code was scanned (the phone side).
    pub async fn pair_with(&self, offer: &PairingOffer) -> Result<PeerRecord> {
        let mut last_err = Error::BadOffer("no addresses".into());
        for addr in &offer.addrs {
            match self.try_pair(offer, *addr).await {
                Ok(rec) => {
                    // Best effort: go straight into a normal session.
                    if let Err(e) = self.connect(rec.fingerprint, *addr).await {
                        debug!("post-pairing connect failed: {e}");
                    }
                    return Ok(rec);
                }
                Err(e @ Error::PairingRejected(_)) => return Err(e),
                Err(e) => {
                    debug!(%addr, "pairing attempt failed: {e}");
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    async fn try_pair(&self, offer: &PairingOffer, addr: SocketAddr) -> Result<PeerRecord> {
        let me = self.fingerprint();
        let conn = self.dial(addr, offer.fingerprint).await?;
        let exchange = async {
            let (mut send, mut recv) = conn.open_bi().await?;
            let proof = pairing::client_proof(&offer.secret, &me, &offer.fingerprint);
            wire::write_frame(
                &mut send,
                &Control::PairRequest {
                    name: self.inner.config.device_name.clone(),
                    proof,
                },
            )
            .await?;
            let _ = send.finish();
            wire::read_frame(&mut recv).await
        };
        let reply = timeout(FIRST_FRAME_TIMEOUT, exchange)
            .await
            .map_err(|_| Error::Timeout)?;
        conn.close(VarInt::from_u32(close_code::PAIRED), b"paired");
        match reply {
            Ok(Some(Frame::Known(Control::PairResponse { name, proof }))) => {
                if !pairing::verify_server_proof(&offer.secret, &me, &offer.fingerprint, &proof) {
                    return Err(Error::PairingRejected(
                        "the other device could not prove it showed this code".into(),
                    ));
                }
                let rec = PeerRecord {
                    fingerprint: offer.fingerprint,
                    name: util::sanitize_name(&name),
                    pair_key: pairing::derive_pair_key(&offer.secret, &me, &offer.fingerprint),
                    paired_at: util::now_unix(),
                    last_addrs: vec![addr],
                    bluetooth: None,
                };
                self.inner.gate.trust().upsert(rec.clone())?;
                info!(peer = %rec.fingerprint.short(), name = %rec.name, "paired");
                self.emit(Event::Paired {
                    peer: rec.fingerprint,
                    name: rec.name.clone(),
                });
                Ok(rec)
            }
            Ok(Some(Frame::Known(Control::Reject { reason }))) => {
                Err(Error::PairingRejected(reason))
            }
            // Window closed/expired: the laptop refused our certificate during the handshake.
            Ok(_) | Err(Error::Io(_)) | Err(Error::Connection(_)) => Err(Error::PairingRejected(
                "the code expired or was already used".into(),
            )),
            Err(e) => Err(e),
        }
    }

    async fn serve_pairing(
        &self,
        conn: Connection,
        client: Fingerprint,
        mut send: SendStream,
        name: String,
        proof: [u8; 32],
    ) {
        let reject = |conn: &Connection, why: &str| {
            warn!(addr = %conn.remote_address(), "pairing attempt rejected: {why}");
            conn.close(VarInt::from_u32(close_code::REJECTED), b"rejected");
        };
        let Some(secret) = self.inner.gate.pairing_secret() else {
            return reject(&conn, "pairing window closed");
        };
        let me = self.fingerprint();
        if !pairing::verify_client_proof(&secret, &client, &me, &proof) {
            let _ = wire::write_frame(
                &mut send,
                &Control::Reject {
                    reason: "wrong pairing code".into(),
                },
            )
            .await;
            let _ = send.finish();
            let _ = timeout(Duration::from_secs(1), conn.closed()).await;
            return reject(&conn, "wrong secret");
        }
        if !self.inner.gate.consume_pairing(&secret) {
            return reject(&conn, "code already used");
        }
        let rec = PeerRecord {
            fingerprint: client,
            name: util::sanitize_name(&name),
            pair_key: pairing::derive_pair_key(&secret, &client, &me),
            paired_at: util::now_unix(),
            last_addrs: vec![conn.remote_address()],
            bluetooth: None,
        };
        if let Err(e) = self.inner.gate.trust().upsert(rec.clone()) {
            warn!("could not save paired device: {e}");
            return reject(&conn, "storage error");
        }
        let resp = Control::PairResponse {
            name: self.inner.config.device_name.clone(),
            proof: pairing::server_proof(&secret, &client, &me),
        };
        let _ = wire::write_frame(&mut send, &resp).await;
        let _ = send.finish();
        info!(peer = %client.short(), name = %rec.name, "paired");
        self.emit(Event::Paired {
            peer: client,
            name: rec.name,
        });
        // Let the phone read the response and close; closing first could discard it.
        let _ = timeout(Duration::from_secs(3), conn.closed()).await;
    }

    /// Drop the current session with `fp` (if any). A [`Node::maintain`] loop reconnects.
    pub fn disconnect(&self, fp: &Fingerprint) -> bool {
        match self.sessions().get(fp) {
            Some(s) if s.link.is_open() => {
                s.link.close(close_code::NORMAL, b"disconnect");
                true
            }
            _ => false,
        }
    }

    pub fn unpair(&self, fp: &Fingerprint) -> Result<bool> {
        let removed = self.inner.gate.trust().remove(fp)?;
        if let Some(s) = self.sessions().remove(fp) {
            s.link.close(close_code::UNPAIRED, b"unpaired");
        }
        self.inner.outbox.forget_peer(fp)?;
        Ok(removed)
    }

    // ---------------------------------------------------------------- connections

    async fn dial(&self, addr: SocketAddr, expected: Fingerprint) -> Result<Connection> {
        if self.is_paused() {
            return Err(Error::Protocol(PAUSED_REASON.into()));
        }
        let cfg = tls::client_config(&self.inner.identity, expected, self.inner.provider.clone())?;
        let connecting = self
            .inner
            .endpoint
            .connect_with(cfg, addr, tls::SERVER_NAME)?;
        Ok(timeout(HANDSHAKE_TIMEOUT, connecting)
            .await
            .map_err(|_| Error::Timeout)??)
    }

    /// Connect to a paired peer at `addr`. No-op if already connected.
    pub async fn connect(&self, peer: Fingerprint, addr: SocketAddr) -> Result<()> {
        self.connect_any(peer, vec![addr]).await
    }

    /// Dial all `addrs` in parallel and keep the first that completes the handshake.
    /// A dead address (laptop moved, old Wi-Fi) therefore never delays a live one.
    /// No-op if already connected over QUIC; a Bluetooth session is replaced on success.
    pub async fn connect_any(&self, peer: Fingerprint, addrs: Vec<SocketAddr>) -> Result<()> {
        if !self.inner.gate.is_trusted(&peer) {
            return Err(Error::NotPaired);
        }
        if self.is_paused() {
            return Err(Error::Protocol(PAUSED_REASON.into()));
        }
        if self.transport(&peer) == Some(Transport::Quic) {
            return Ok(());
        }
        let lock = self
            .inner
            .dial_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(peer)
            .or_default()
            .clone();
        let _dialing = lock.lock().await;
        if self.transport(&peer) == Some(Transport::Quic) {
            return Ok(()); // another caller connected while we waited
        }
        if addrs.is_empty() {
            return Err(Error::Protocol("no known address for peer".into()));
        }
        let mut dials = tokio::task::JoinSet::new();
        for addr in addrs {
            let node = self.clone();
            dials.spawn(async move { node.dial(addr, peer).await });
        }
        let mut last_err = Error::Timeout;
        while let Some(res) = dials.join_next().await {
            match res {
                Ok(Ok(conn)) => {
                    dials.abort_all(); // losers are dropped, which closes them
                    return self.establish(conn, peer).await;
                }
                Ok(Err(e)) => last_err = e,
                Err(_) => {}
            }
        }
        Err(last_err)
    }

    async fn establish(&self, conn: Connection, peer: Fingerprint) -> Result<()> {
        let handshake = async {
            let (mut send, mut recv) = conn.open_bi().await?;
            wire::write_frame(&mut send, &self.hello()).await?;
            let reply = wire::read_frame(&mut recv).await?;
            Ok::<_, Error>((send, recv, reply))
        };
        let (send, recv, reply) = match timeout(FIRST_FRAME_TIMEOUT, handshake).await {
            Ok(Ok(x)) => x,
            Ok(Err(e)) => {
                conn.close(VarInt::from_u32(close_code::PROTOCOL), b"");
                return Err(e);
            }
            Err(_) => {
                conn.close(VarInt::from_u32(close_code::PROTOCOL), b"");
                return Err(Error::Timeout);
            }
        };
        match reply {
            Some(Frame::Known(Control::Hello {
                version,
                name,
                capabilities,
            })) => {
                if version < MIN_PROTOCOL_VERSION {
                    conn.close(VarInt::from_u32(close_code::PROTOCOL), b"version");
                    return Err(Error::Protocol(format!(
                        "peer speaks protocol {version}, too old"
                    )));
                }
                self.start_session(
                    Link::Quic(conn),
                    peer,
                    send,
                    recv,
                    true,
                    PeerHello {
                        version,
                        name,
                        capabilities,
                    },
                );
                Ok(())
            }
            Some(Frame::Known(Control::Reject { reason })) => {
                if reason == PAUSED_REASON {
                    self.mark_peer_paused(peer);
                }
                Err(Error::Protocol(reason))
            }
            _ => Err(Error::Protocol("peer did not answer hello".into())),
        }
    }

    async fn accept_loop(self) {
        let endpoint = self.inner.endpoint.clone();
        while let Some(incoming) = endpoint.accept().await {
            let addr = incoming.remote_address();
            // QUIC Retry: proves the source address is real before we spend any crypto work,
            // so spoofed-address floods cost us nothing.
            if !incoming.remote_address_validated() {
                let _ = incoming.retry();
                continue;
            }
            if !self.inner.limiter.allow(addr.ip()) {
                debug!(%addr, "handshake rate limit hit; dropping");
                incoming.ignore();
                continue;
            }
            let Some(permit) = self.inner.limiter.try_permit() else {
                debug!(%addr, "too many concurrent handshakes; dropping");
                incoming.ignore();
                continue;
            };
            let node = self.clone();
            tokio::spawn(async move {
                let conn = match timeout(HANDSHAKE_TIMEOUT, incoming).await {
                    Ok(Ok(c)) => c,
                    Ok(Err(e)) => return debug!(%addr, "handshake refused: {e}"),
                    Err(_) => return debug!(%addr, "handshake timed out"),
                };
                drop(permit);
                node.handle_incoming(conn).await;
            });
        }
    }

    async fn handle_incoming(&self, conn: Connection) {
        let Some(peer) = tls::peer_fingerprint(&conn) else {
            return conn.close(VarInt::from_u32(close_code::REJECTED), b"");
        };
        let first = async {
            let (send, mut recv) = conn.accept_bi().await?;
            let frame = wire::read_frame(&mut recv).await?;
            Ok::<_, Error>((send, recv, frame))
        };
        let (mut send, recv, frame) = match timeout(FIRST_FRAME_TIMEOUT, first).await {
            Ok(Ok((s, r, Some(Frame::Known(c))))) => (s, r, c),
            _ => return conn.close(VarInt::from_u32(close_code::PROTOCOL), b""),
        };
        match frame {
            Control::Hello {
                version,
                name,
                capabilities,
            } if self.inner.gate.is_trusted(&peer) => {
                if version < MIN_PROTOCOL_VERSION {
                    let _ = wire::write_frame(
                        &mut send,
                        &Control::Reject {
                            reason: "protocol too old".into(),
                        },
                    )
                    .await;
                    return conn.close(VarInt::from_u32(close_code::PROTOCOL), b"version");
                }
                if self.is_paused() {
                    let reject = Control::Reject {
                        reason: PAUSED_REASON.into(),
                    };
                    let _ = wire::write_frame(&mut send, &reject).await;
                    let _ = send.finish();
                    let _ = timeout(Duration::from_secs(2), conn.closed()).await;
                    return conn.close(VarInt::from_u32(close_code::NORMAL), b"paused");
                }
                if wire::write_frame(&mut send, &self.hello()).await.is_err() {
                    return;
                }
                self.start_session(
                    Link::Quic(conn),
                    peer,
                    send,
                    recv,
                    false,
                    PeerHello {
                        version,
                        name,
                        capabilities,
                    },
                );
            }
            Control::PairRequest { name, proof } => {
                self.serve_pairing(conn, peer, send, name, proof).await
            }
            _ => conn.close(VarInt::from_u32(close_code::REJECTED), b""),
        }
    }

    /// Install a session on `link`. Returns false if an existing session was kept instead
    /// (then `link` is closed and the halves are dropped).
    fn start_session<R, W>(
        &self,
        link: Link,
        peer: Fingerprint,
        mut send: W,
        mut recv: R,
        we_dialed: bool,
        hello: PeerHello,
    ) -> bool
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let id = self.inner.next_session.fetch_add(1, Ordering::Relaxed);
        let capabilities: Vec<String> = hello
            .capabilities
            .iter()
            .filter(|c| self.inner.config.capabilities.contains(c))
            .cloned()
            .collect();
        let (tx, mut rx) = mpsc::unbounded_channel::<Control>();
        let transport = link.transport();
        {
            let mut sessions = self.sessions();
            if let Some(existing) = sessions.get(&peer) {
                if existing.link.is_open() {
                    let keep_new = match (existing.link.transport(), transport) {
                        // Wi-Fi is faster: it always wins over the Bluetooth backup.
                        (Transport::Quic, Transport::Stream) => false,
                        (Transport::Stream, Transport::Quic) => true,
                        // Both sides may dial each other at the same moment. Both apply the
                        // same rule, so they keep the same connection: the one dialed by the
                        // lower fingerprint. A newer connection from the same dialer replaces
                        // the old one (e.g. the phone changed networks).
                        _ => {
                            existing.we_dialed == we_dialed
                                || we_dialed == (self.fingerprint() < peer)
                        }
                    };
                    if !keep_new {
                        debug!(peer = %peer.short(), ?transport, "dropping duplicate connection");
                        link.close(close_code::REPLACED, b"duplicate");
                        return false;
                    }
                    existing.link.close(close_code::REPLACED, b"replaced");
                }
            }
            sessions.insert(
                peer,
                SessionHandle {
                    id,
                    link: link.clone(),
                    tx: tx.clone(),
                    we_dialed,
                    capabilities: capabilities.clone(),
                },
            );
        }

        let addr = link.remote_addr();
        if let Some(addr) = addr {
            if let Err(e) = self.inner.gate.trust().remember_addr(&peer, addr) {
                debug!("could not persist peer address: {e}");
            }
        }
        self.clear_peer_paused(&peer);
        info!(peer = %peer.short(), ?addr, ?transport, version = hello.version.min(PROTOCOL_VERSION), "connected");
        self.emit(Event::PeerConnected {
            peer,
            name: util::sanitize_name(&hello.name),
            capabilities,
            addr,
            transport,
        });

        // Further streams on a QUIC session carry file data.
        if let Link::Quic(conn) = &link {
            let (conn, data_tx) = (conn.clone(), self.inner.data_tx.clone());
            tokio::spawn(async move {
                while let Ok((send, recv)) = conn.accept_bi().await {
                    if data_tx.send(DataStream { peer, send, recv }).is_err() {
                        break; // nobody takes file data on this device
                    }
                }
            });
        }

        let keepalive = transport == Transport::Stream;
        let (ping_every, silence_limit) = (
            self.inner.config.stream_ping_every,
            self.inner.config.stream_silence_limit,
        );
        let writer_link = link.clone();
        tokio::spawn(async move {
            let mut ping = tokio::time::interval(ping_every);
            ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ping.reset();
            loop {
                let msg = tokio::select! {
                    m = rx.recv() => match m {
                        Some(m) => m,
                        None => break,
                    },
                    _ = ping.tick(), if keepalive => Control::Ping,
                    _ = writer_link.closed() => break,
                };
                // A stalled link must not wedge this task: closing the link frees it.
                let written = tokio::select! {
                    r = wire::write_frame(&mut send, &msg) => r.is_ok(),
                    _ = writer_link.closed() => false,
                };
                if !written {
                    break;
                }
            }
            let _ = timeout(Duration::from_secs(2), send.shutdown()).await;
        });

        match self.inner.outbox.pending(&peer) {
            Ok(pending) => {
                for (id, body) in pending {
                    let _ = tx.send(Control::Deliver { id, body });
                }
            }
            Err(e) => warn!("could not read outbox: {e}"),
        }
        if let Some(links) = self.links_frame() {
            let _ = tx.send(links);
        }

        let node = self.clone();
        tokio::spawn(async move {
            loop {
                let next = async {
                    if keepalive {
                        timeout(silence_limit, wire::read_frame(&mut recv))
                            .await
                            .unwrap_or(Err(Error::Timeout))
                    } else {
                        wire::read_frame(&mut recv).await
                    }
                };
                let frame = tokio::select! {
                    f = next => f,
                    _ = link.closed() => break,
                };
                match frame {
                    Ok(Some(Frame::Known(Control::Deliver { id: msg, body }))) => {
                        match node.inner.outbox.first_seen(&peer, &msg) {
                            Ok(true) => node.emit(Event::Message {
                                peer,
                                id: msg,
                                body,
                            }),
                            Ok(false) => debug!(?msg, "duplicate suppressed"),
                            Err(e) => {
                                // Not recorded => don't ack; the sender will retry.
                                warn!("could not record message: {e}");
                                continue;
                            }
                        }
                        let _ = tx.send(Control::Ack { id: msg });
                    }
                    Ok(Some(Frame::Known(Control::Ack { id: msg }))) => {
                        if let Err(e) = node.inner.outbox.ack(&peer, &msg) {
                            warn!("could not clear acked message: {e}");
                        }
                    }
                    Ok(Some(Frame::Known(Control::Ping))) => {}
                    Ok(Some(Frame::Known(Control::Reject { reason }))) => {
                        if reason == PAUSED_REASON {
                            node.mark_peer_paused(peer);
                        }
                        break;
                    }
                    Ok(Some(Frame::Known(Control::Links { bluetooth }))) => {
                        node.peer_links(&peer, bluetooth)
                    }
                    Ok(Some(Frame::Known(other))) => debug!("ignoring unexpected {other:?}"),
                    Ok(Some(Frame::Unknown)) => {
                        debug!("skipping frame from a newer protocol version")
                    }
                    Ok(None) => break,
                    Err(e) => {
                        debug!(peer = %peer.short(), ?transport, "session ended: {e}");
                        break;
                    }
                }
            }
            drop(tx);
            node.end_session(peer, id, &link);
        });
        true
    }

    fn end_session(&self, peer: Fingerprint, id: u64, link: &Link) {
        link.close(close_code::NORMAL, b"");
        let removed = {
            let mut sessions = self.sessions();
            if sessions.get(&peer).is_some_and(|s| s.id == id) {
                sessions.remove(&peer);
                true
            } else {
                false
            }
        };
        if removed {
            info!(peer = %peer.short(), "disconnected");
            self.emit(Event::PeerDisconnected { peer });
        }
    }

    // ---------------------------------------------------------------- stream links

    /// Run a session with the paired `peer` over a byte stream this app opened itself (the
    /// phone's Bluetooth socket to the laptop). Mutual TLS 1.3 with the same pinned keys as
    /// QUIC, then the same protocol. Returns once the session is up; it ends when the stream
    /// ends, or when a QUIC connection to the same peer takes over.
    pub async fn connect_stream<S>(&self, peer: Fingerprint, io: S) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        if !self.inner.gate.is_trusted(&peer) {
            return Err(Error::NotPaired);
        }
        if self.is_paused() {
            return Err(Error::Protocol(PAUSED_REASON.into()));
        }
        if self.transport(&peer) == Some(Transport::Quic) {
            return Err(Error::Protocol("already connected over the network".into()));
        }
        let cfg = tls::stream_client_config(&self.inner.identity, peer, self.inner.provider.clone())?;
        let server = ServerName::try_from(tls::SERVER_NAME)
            .map_err(|e| Error::Protocol(e.to_string()))?;
        let tls = timeout(
            STREAM_HANDSHAKE_TIMEOUT,
            TlsConnector::from(cfg).connect(server, io),
        )
        .await
        .map_err(|_| Error::Timeout)??;
        let (mut recv, mut send) = tokio::io::split(tls);
        let reply = timeout(STREAM_FIRST_FRAME_TIMEOUT, async {
            wire::write_frame(&mut send, &self.hello()).await?;
            wire::read_frame(&mut recv).await
        })
        .await
        .map_err(|_| Error::Timeout)??;
        let hello = match reply {
            Some(Frame::Known(Control::Hello {
                version,
                name,
                capabilities,
            })) if version >= MIN_PROTOCOL_VERSION => PeerHello {
                version,
                name,
                capabilities,
            },
            Some(Frame::Known(Control::Reject { reason })) => {
                if reason == PAUSED_REASON {
                    self.mark_peer_paused(peer);
                }
                return Err(Error::Protocol(reason));
            }
            _ => return Err(Error::Protocol("peer did not answer hello".into())),
        };
        if self.start_session(Link::new_stream(), peer, send, recv, true, hello) {
            Ok(())
        } else {
            Err(Error::Protocol("already connected over the network".into()))
        }
    }

    /// Serve one incoming byte-stream link (the laptop's Bluetooth server). Only paired
    /// devices get through the TLS handshake, and pairing is never possible over this link.
    /// Returns the peer once its session is up.
    pub async fn accept_stream<S>(&self, io: S) -> Result<Fingerprint>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let Some(permit) = self.inner.limiter.try_permit() else {
            return Err(Error::Protocol("too many handshakes in progress".into()));
        };
        let cfg = tls::stream_server_config(
            &self.inner.identity,
            self.inner.gate.clone(),
            self.inner.provider.clone(),
        )?;
        let tls = timeout(STREAM_HANDSHAKE_TIMEOUT, TlsAcceptor::from(cfg).accept(io))
            .await
            .map_err(|_| Error::Timeout)??;
        let peer = tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|c| c.first())
            .map(|c| Fingerprint::of(c))
            .ok_or_else(|| Error::Protocol("no client certificate".into()))?;
        if !self.inner.gate.is_trusted(&peer) {
            return Err(Error::NotPaired);
        }
        let (mut recv, mut send) = tokio::io::split(tls);
        let first = timeout(STREAM_FIRST_FRAME_TIMEOUT, wire::read_frame(&mut recv))
            .await
            .map_err(|_| Error::Timeout)??;
        let hello = match first {
            Some(Frame::Known(Control::Hello {
                version,
                name,
                capabilities,
            })) => {
                if version < MIN_PROTOCOL_VERSION {
                    let reject = Control::Reject {
                        reason: "protocol too old".into(),
                    };
                    let _ = wire::write_frame(&mut send, &reject).await;
                    return Err(Error::Protocol(format!("peer speaks protocol {version}")));
                }
                PeerHello {
                    version,
                    name,
                    capabilities,
                }
            }
            _ => return Err(Error::Protocol("expected hello".into())),
        };
        if self.is_paused() {
            let reject = Control::Reject {
                reason: PAUSED_REASON.into(),
            };
            let _ = wire::write_frame(&mut send, &reject).await;
            return Err(Error::Protocol(PAUSED_REASON.into()));
        }
        wire::write_frame(&mut send, &self.hello()).await?;
        drop(permit);
        if self.start_session(Link::new_stream(), peer, send, recv, false, hello) {
            Ok(peer)
        } else {
            Err(Error::Protocol("already connected over the network".into()))
        }
    }

    /// Tell every peer (now and on each new session) how to reach this device over
    /// Bluetooth. `None` tells them the backup is off.
    pub fn set_bluetooth_address(&self, addr: Option<String>) {
        let addr = addr
            .map(|a| a.to_ascii_uppercase())
            .filter(|a| valid_bluetooth_addr(a));
        let frame = Control::Links { bluetooth: addr };
        {
            let mut cur = self.inner.links.lock().unwrap_or_else(|e| e.into_inner());
            if cur.as_ref() == Some(&frame) {
                return;
            }
            *cur = Some(frame.clone());
        }
        for s in self.sessions().values() {
            let _ = s.tx.send(frame.clone());
        }
    }

    fn links_frame(&self) -> Option<Control> {
        self.inner
            .links
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn peer_links(&self, peer: &Fingerprint, bluetooth: Option<String>) {
        let bluetooth = match bluetooth {
            Some(a) if valid_bluetooth_addr(&a) => Some(a),
            Some(_) => return debug!("ignoring malformed Bluetooth address"),
            None => None,
        };
        if let Err(e) = self.inner.gate.trust().set_bluetooth(peer, bluetooth) {
            warn!("could not save peer's Bluetooth address: {e}");
        }
    }

    // ---------------------------------------------------------------- messaging

    /// Queue `body` for `peer` durably and send it now if connected. It is delivered
    /// exactly once, even across disconnects and restarts of either side.
    pub fn send(&self, peer: &Fingerprint, body: Vec<u8>) -> Result<MsgId> {
        if !self.inner.gate.is_trusted(peer) {
            return Err(Error::NotPaired);
        }
        if body.len() > MAX_BODY_LEN {
            return Err(Error::Protocol(format!(
                "message of {} bytes is too large",
                body.len()
            )));
        }
        let id = self.inner.outbox.enqueue(peer, &body)?;
        if let Some(s) = self.sessions().get(peer) {
            let _ = s.tx.send(Control::Deliver { id, body });
        }
        Ok(id)
    }

    /// Tell reconnect loops to retry right now instead of waiting out their backoff.
    /// Call when the OS reports a network change.
    pub fn nudge(&self) {
        self.inner.nudge.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Addresses where `peer` was just seen (e.g. via mDNS). Tried first on reconnect.
    pub fn add_addr_hints(&self, peer: Fingerprint, addrs: Vec<SocketAddr>) {
        if addrs.is_empty() || !self.inner.gate.is_trusted(&peer) {
            return;
        }
        {
            let mut hints = self
                .inner
                .addr_hints
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let list = hints.entry(peer).or_default();
            for a in addrs.into_iter().rev() {
                list.retain(|x| *x != a);
                list.insert(0, a);
            }
            list.truncate(8);
        }
        if !self.is_connected(&peer) {
            self.nudge();
        }
    }

    fn candidate_addrs(&self, rec: &PeerRecord, extra: &[SocketAddr]) -> Vec<SocketAddr> {
        let hints = self
            .inner
            .addr_hints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&rec.fingerprint)
            .cloned()
            .unwrap_or_default();
        let mut out: Vec<SocketAddr> = Vec::new();
        for a in hints.iter().chain(&rec.last_addrs).chain(extra) {
            if !out.contains(a) {
                out.push(*a);
            }
        }
        out
    }

    /// Resolves once [`Node::shutdown`] has been called.
    pub async fn closed(&self) {
        let mut rx = self.inner.shutdown.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }

    /// Keep a connection to `peer` alive: reconnect with exponential backoff (with jitter)
    /// whenever it drops, trying discovered addresses, remembered addresses and `extra_addrs`.
    pub fn maintain(&self, peer: Fingerprint, extra_addrs: Vec<SocketAddr>) -> JoinHandle<()> {
        let node = self.clone();
        let mut shutdown = self.inner.shutdown.subscribe();
        let mut nudges = self.inner.nudge.subscribe();
        tokio::spawn(async move {
            let mut backoff = BACKOFF_MIN;
            let mut failures: u32 = 0;
            let mut nudged = false;
            let mut last_probe: Option<std::time::Instant> = None;
            let mut offline_since: Option<std::time::Instant> = None;
            let mut last_log: Option<std::time::Instant> = None;
            let mut paused_rx = node.inner.paused.subscribe();
            let mut peer_resumed = node.inner.peers_paused_changed.subscribe();
            loop {
                if *shutdown.borrow() {
                    return;
                }
                // Switched off here: wait until switched back on (no dialling at all).
                if *paused_rx.borrow_and_update() {
                    tokio::select! {
                        _ = paused_rx.wait_for(|p| !*p) => {}
                        _ = shutdown.changed() => return,
                    }
                    backoff = BACKOFF_MIN;
                    failures = 0;
                    nudged = true;
                    continue;
                }
                // Switched off over there: it dials us when it's back. Look rarely, or
                // when we switch back on ourselves.
                if node.peer_paused(&peer) && node.sessions().get(&peer).is_none() {
                    peer_resumed.borrow_and_update();
                    tokio::select! {
                        _ = peer_resumed.changed() => {}
                        _ = paused_rx.changed() => {}
                        _ = tokio::time::sleep(PEER_PAUSED_RECHECK) => {
                            node.clear_peer_paused(&peer);
                        }
                        _ = shutdown.changed() => return,
                    }
                    continue;
                }
                let Some(rec) = node.peer(&peer) else { return }; // unpaired
                let existing = node.sessions().get(&peer).map(|s| s.link.clone());
                // On the Bluetooth backup we stay connected but keep trying to get back
                // onto the network; the QUIC session then replaces the backup.
                let on_backup = match existing.filter(|l| l.is_open()) {
                    Some(link) if link.transport() == Transport::Quic => {
                        backoff = BACKOFF_MIN;
                        failures = 0;
                        last_probe = None;
                        offline_since = None;
                        tokio::select! {
                            _ = link.closed() => continue,
                            _ = shutdown.changed() => return,
                        }
                    }
                    Some(_) => true,
                    None => false,
                };
                if on_backup {
                    offline_since = None;
                } else {
                    offline_since.get_or_insert_with(std::time::Instant::now);
                }
                nudges.borrow_and_update();
                let addrs = node.reconnect_candidates(
                    &rec,
                    &extra_addrs,
                    failures,
                    nudged,
                    &mut last_probe,
                );
                nudged = false;
                let tried = addrs.len();
                tokio::select! {
                    res = node.connect_any(peer, addrs) => match res {
                        Ok(()) => {
                            if on_backup {
                                info!(peer = %peer.short(), "back on the network; leaving the Bluetooth backup");
                            } else if failures > 0 {
                                info!(peer = %peer.short(), "reconnected");
                            }
                            backoff = BACKOFF_MIN;
                            failures = 0;
                            continue;
                        }
                        Err(e) => {
                            failures += 1;
                            debug!(peer = %peer.short(), "reconnect attempt failed: {e}");
                            if !on_backup && last_log.is_none_or(|t| t.elapsed() >= OFFLINE_LOG_INTERVAL) {
                                last_log = Some(std::time::Instant::now());
                                let mins = offline_since.map(|t| t.elapsed().as_secs() / 60).unwrap_or(0);
                                info!(
                                    peer = %peer.short(),
                                    name = %rec.name,
                                    "still trying to reach it ({mins} min, {tried} addresses this round, last error: {e})"
                                );
                            }
                        }
                    },
                    // New information (address hint, network change, unlock): abandon this
                    // attempt and start over, searching the local network too.
                    _ = nudges.changed() => {
                        backoff = BACKOFF_MIN;
                        nudged = true;
                        continue;
                    }
                    _ = shutdown.changed() => return,
                }
                let jitter = 0.8 + rand::random::<f64>() * 0.4;
                tokio::select! {
                    _ = tokio::time::sleep(backoff.mul_f64(jitter)) => {
                        backoff = (backoff * 2).min(BACKOFF_MAX);
                    }
                    _ = nudges.changed() => {
                        backoff = BACKOFF_MIN;
                        nudged = true;
                    }
                    _ = shutdown.changed() => return,
                }
            }
        })
    }

    /// Addresses to dial this round: discovery hints and remembered addresses, plus the
    /// default gateway on the peer's port, plus (sometimes) the whole local /24.
    fn reconnect_candidates(
        &self,
        rec: &PeerRecord,
        extra: &[SocketAddr],
        failures: u32,
        nudged: bool,
        last_probe: &mut Option<std::time::Instant>,
    ) -> Vec<SocketAddr> {
        let mut addrs = self.candidate_addrs(rec, extra);
        let cfg = &self.inner.config.probe;
        // The peer listens on the port we last reached it on.
        let Some(port) = rec.last_addrs.first().map(|a| a.port()).filter(|p| *p != 0) else {
            return addrs;
        };
        if !cfg.enabled {
            return addrs;
        }
        let gateways = cfg
            .gateways
            .clone()
            .unwrap_or_else(crate::probe::default_gateways);
        for gw in gateways {
            let sa = SocketAddr::new(std::net::IpAddr::V4(gw), port);
            if !addrs.contains(&sa) {
                addrs.push(sa);
            }
        }
        let since = last_probe.map(|t| t.elapsed());
        let due = (failures >= PROBE_AFTER_FAILURES && since.is_none_or(|d| d >= PROBE_INTERVAL))
            || (nudged && since.is_none_or(|d| d >= PROBE_AFTER_NUDGE_MIN_GAP));
        if due {
            let subnets = cfg
                .subnets
                .clone()
                .unwrap_or_else(crate::probe::local_subnets);
            let found = crate::probe::subnet_candidates(&subnets, port);
            if !found.is_empty() {
                debug!(peer = %rec.fingerprint.short(), count = found.len(), "searching the local network");
                *last_probe = Some(std::time::Instant::now());
                for sa in found {
                    if !addrs.contains(&sa) {
                        addrs.push(sa);
                    }
                }
            }
        }
        addrs
    }

    /// Close all connections and stop listening.
    pub async fn shutdown(&self) {
        let _ = self.inner.shutdown.send(true);
        let sessions: Vec<_> = self.sessions().drain().map(|(_, s)| s.link).collect();
        for l in sessions {
            l.close(close_code::SHUTDOWN, b"shutdown");
        }
        self.inner
            .endpoint
            .close(VarInt::from_u32(close_code::SHUTDOWN), b"shutdown");
        let _ = timeout(Duration::from_secs(2), self.inner.endpoint.wait_idle()).await;
    }
}
