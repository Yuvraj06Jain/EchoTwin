//! A PhoneLink node: one QUIC endpoint that both listens and dials.
//!
//! Connection life-cycle:
//! 1. QUIC + mutual TLS handshake. Unpaired certificates are refused here (see [`crate::tls`]).
//! 2. The dialer opens the control stream and sends `Hello` (or `PairRequest` when pairing).
//! 3. Both sides exchange capabilities, then flush their durable outbox to each other.
//! 4. `Deliver`/`Ack` until the connection drops; [`Node::maintain`] then reconnects with
//!    backoff and the outbox resends whatever was not acknowledged.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::Duration,
};

use quinn::{Connection, Endpoint, RecvStream, SendStream, VarInt};
use rustls::crypto::CryptoProvider;
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
    time::timeout,
};
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

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(8);
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const BACKOFF_MIN: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const SEEN_RETENTION_SECS: u64 = 60 * 60 * 24 * 30;

#[derive(Clone, Debug)]
pub struct NodeConfig {
    /// Where identity, peers and the outbox live. `None` = ephemeral (tests).
    pub data_dir: Option<PathBuf>,
    pub device_name: String,
    pub bind: SocketAddr,
    pub capabilities: Vec<String>,
}

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
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Paired {
        peer: Fingerprint,
        name: String,
    },
    PeerConnected {
        peer: Fingerprint,
        name: String,
        capabilities: Vec<String>,
        addr: SocketAddr,
    },
    PeerDisconnected {
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
    conn: Connection,
    tx: mpsc::UnboundedSender<Control>,
    we_dialed: bool,
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
            }),
        };
        info!(fingerprint = %node.fingerprint().short(), addr = ?node.local_addr(), "node started");
        tokio::spawn(node.clone().accept_loop());
        Ok((node, rx))
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
        self.sessions()
            .get(fp)
            .is_some_and(|s| s.conn.close_reason().is_none())
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
            Ok(_) | Err(Error::Read(_)) | Err(Error::Connection(_)) => Err(Error::PairingRejected(
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

    pub fn unpair(&self, fp: &Fingerprint) -> Result<bool> {
        let removed = self.inner.gate.trust().remove(fp)?;
        if let Some(s) = self.sessions().remove(fp) {
            s.conn
                .close(VarInt::from_u32(close_code::UNPAIRED), b"unpaired");
        }
        self.inner.outbox.forget_peer(fp)?;
        Ok(removed)
    }

    // ---------------------------------------------------------------- connections

    async fn dial(&self, addr: SocketAddr, expected: Fingerprint) -> Result<Connection> {
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
    pub async fn connect_any(&self, peer: Fingerprint, addrs: Vec<SocketAddr>) -> Result<()> {
        if !self.inner.gate.is_trusted(&peer) {
            return Err(Error::NotPaired);
        }
        if self.is_connected(&peer) {
            return Ok(());
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
                    conn,
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
            Some(Frame::Known(Control::Reject { reason })) => Err(Error::Protocol(reason)),
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
                if wire::write_frame(&mut send, &self.hello()).await.is_err() {
                    return;
                }
                self.start_session(
                    conn,
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

    fn start_session(
        &self,
        conn: Connection,
        peer: Fingerprint,
        mut send: SendStream,
        mut recv: RecvStream,
        we_dialed: bool,
        hello: PeerHello,
    ) {
        let id = self.inner.next_session.fetch_add(1, Ordering::Relaxed);
        let (tx, mut rx) = mpsc::unbounded_channel::<Control>();
        {
            let mut sessions = self.sessions();
            if let Some(existing) = sessions.get(&peer) {
                if existing.conn.close_reason().is_none() {
                    // Both sides may dial each other at the same moment. Both apply the same
                    // rule, so they keep the same connection: the one dialed by the lower
                    // fingerprint. A newer connection from the same dialer replaces the old one
                    // (e.g. the phone changed networks).
                    let keep_new =
                        existing.we_dialed == we_dialed || we_dialed == (self.fingerprint() < peer);
                    if !keep_new {
                        debug!(peer = %peer.short(), "dropping duplicate connection");
                        return conn.close(VarInt::from_u32(close_code::REPLACED), b"duplicate");
                    }
                    existing
                        .conn
                        .close(VarInt::from_u32(close_code::REPLACED), b"replaced");
                }
            }
            sessions.insert(
                peer,
                SessionHandle {
                    id,
                    conn: conn.clone(),
                    tx: tx.clone(),
                    we_dialed,
                },
            );
        }

        let addr = conn.remote_address();
        if let Err(e) = self.inner.gate.trust().remember_addr(&peer, addr) {
            debug!("could not persist peer address: {e}");
        }
        let capabilities: Vec<String> = hello
            .capabilities
            .into_iter()
            .filter(|c| self.inner.config.capabilities.contains(c))
            .collect();
        info!(peer = %peer.short(), %addr, version = hello.version.min(PROTOCOL_VERSION), "connected");
        self.emit(Event::PeerConnected {
            peer,
            name: util::sanitize_name(&hello.name),
            capabilities,
            addr,
        });

        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if wire::write_frame(&mut send, &msg).await.is_err() {
                    break;
                }
            }
            let _ = send.finish();
        });

        match self.inner.outbox.pending(&peer) {
            Ok(pending) => {
                for (id, body) in pending {
                    let _ = tx.send(Control::Deliver { id, body });
                }
            }
            Err(e) => warn!("could not read outbox: {e}"),
        }

        let node = self.clone();
        tokio::spawn(async move {
            loop {
                match wire::read_frame(&mut recv).await {
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
                    Ok(Some(Frame::Known(other))) => debug!("ignoring unexpected {other:?}"),
                    Ok(Some(Frame::Unknown)) => {
                        debug!("skipping frame from a newer protocol version")
                    }
                    Ok(None) => break,
                    Err(e) => {
                        debug!(peer = %peer.short(), "session ended: {e}");
                        break;
                    }
                }
            }
            drop(tx);
            node.end_session(peer, id, &conn);
        });
    }

    fn end_session(&self, peer: Fingerprint, id: u64, conn: &Connection) {
        conn.close(VarInt::from_u32(close_code::NORMAL), b"");
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
            loop {
                if *shutdown.borrow() {
                    return;
                }
                let Some(rec) = node.peer(&peer) else { return }; // unpaired
                let existing = node.sessions().get(&peer).map(|s| s.conn.clone());
                if let Some(conn) = existing.filter(|c| c.close_reason().is_none()) {
                    backoff = BACKOFF_MIN;
                    tokio::select! {
                        _ = conn.closed() => continue,
                        _ = shutdown.changed() => return,
                    }
                }
                nudges.borrow_and_update();
                let addrs = node.candidate_addrs(&rec, &extra_addrs);
                tokio::select! {
                    res = node.connect_any(peer, addrs) => match res {
                        Ok(()) => {
                            backoff = BACKOFF_MIN;
                            continue;
                        }
                        Err(e) => debug!(peer = %peer.short(), "reconnect attempt failed: {e}"),
                    },
                    // New information (address hint, network change): abandon this attempt
                    // and start over with the fresh candidate list.
                    _ = nudges.changed() => {
                        backoff = BACKOFF_MIN;
                        continue;
                    }
                    _ = shutdown.changed() => return,
                }
                let jitter = 0.8 + rand::random::<f64>() * 0.4;
                tokio::select! {
                    _ = tokio::time::sleep(backoff.mul_f64(jitter)) => {
                        backoff = (backoff * 2).min(BACKOFF_MAX);
                    }
                    _ = nudges.changed() => backoff = BACKOFF_MIN,
                    _ = shutdown.changed() => return,
                }
            }
        })
    }

    /// Close all connections and stop listening.
    pub async fn shutdown(&self) {
        let _ = self.inner.shutdown.send(true);
        let sessions: Vec<_> = self.sessions().drain().map(|(_, s)| s.conn).collect();
        for c in sessions {
            c.close(VarInt::from_u32(close_code::SHUTDOWN), b"shutdown");
        }
        self.inner
            .endpoint
            .close(VarInt::from_u32(close_code::SHUTDOWN), b"shutdown");
        let _ = timeout(Duration::from_secs(2), self.inner.endpoint.wait_idle()).await;
    }
}
