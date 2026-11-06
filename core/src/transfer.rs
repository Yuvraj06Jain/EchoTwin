//! Resumable file transfers between paired devices.
//!
//! * Each file goes over its own QUIC stream, so a big file never holds up notifications.
//! * Interrupted transfers resume where they stopped: after Wi-Fi drops, the phone sleeping,
//!   or either side restarting. The receiver keeps a `.part` file and tells the sender how
//!   much it already has.
//! * A file only appears under its real name once all of it arrived and its BLAKE3 hash
//!   matches the sender's; a damaged copy is thrown away and sent again.
//! * Cancelling on either side stops both, and the other side is told even if it is
//!   offline at that moment (through the durable message queue).
//! * Over the Bluetooth backup, transfers wait for Wi-Fi.
//!
//! The same code runs on the laptop and (through the Android bindings) on the phone.

use std::{
    collections::HashMap,
    io::{Read, Seek},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{mpsc, watch, Notify},
    time::timeout,
};
use tracing::{debug, info, warn};

use crate::{
    message::AppMessage,
    node::DataStream,
    wire::{self, Control, Frame},
    Error, Fingerprint, Node, Result, Transport,
};

/// Capability string: this device sends and receives files.
pub const CAP_FILES: &str = "files/1";

const CHUNK: usize = 256 * 1024;
/// The receiver makes its data durable this often, so a resume never re-sends much.
const SYNC_EVERY: u64 = 8 * 1024 * 1024;
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
const HEADER_TIMEOUT: Duration = Duration::from_secs(15);
const RESULT_TIMEOUT: Duration = Duration::from_secs(120);
/// A file whose hash keeps failing is given up after this many tries.
const MAX_ATTEMPTS: u32 = 3;
const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);
/// `Reject` reason for a transfer the receiver cancelled.
const CANCELLED: &str = "Cancelled";
/// Finished transfers kept in the list.
const KEEP_FINISHED: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Outgoing,
    Incoming,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    /// Will start as soon as possible.
    Queued,
    /// The other device is offline, only on Bluetooth, or the last attempt was cut off.
    Waiting,
    Active,
    Done,
    Failed,
    Cancelled,
}

impl TransferState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Waiting => "waiting",
            Self::Active => "active",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "queued" => Self::Queued,
            "active" => Self::Active,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => Self::Waiting,
        }
    }

    pub fn finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

/// One transfer, as shown to the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferInfo {
    pub id: String,
    pub peer: Fingerprint,
    pub direction: Direction,
    pub name: String,
    pub size: u64,
    /// Bytes transferred so far.
    pub done: u64,
    pub state: TransferState,
    /// Why it waits or failed; empty otherwise.
    pub message: String,
    pub mime: String,
    /// Incoming and done: where the file was saved.
    pub path: Option<String>,
    pub updated_ms: u64,
}

/// How the sending side reads a file. The laptop opens paths; the phone hands over a file
/// descriptor for a content URI.
pub trait SourceOpener: Send + Sync {
    /// Open `source` (as given to [`Transfers::send_file`]) for reading from the start.
    fn open(&self, id: &str, source: &str) -> std::io::Result<std::fs::File>;
}

/// Sources are file paths.
pub struct PathOpener;

impl SourceOpener for PathOpener {
    fn open(&self, _id: &str, source: &str) -> std::io::Result<std::fs::File> {
        std::fs::File::open(source)
    }
}

#[derive(Clone, Debug)]
pub struct TransferConfig {
    /// `None`: keep state in memory (tests).
    pub db_path: Option<PathBuf>,
    /// Received files land here (created when needed).
    pub incoming_dir: PathBuf,
    /// Refuse an incoming file if less than this would stay free on the disk.
    pub reserve_bytes: u64,
}

struct Row {
    info: TransferInfo,
    source: Option<String>,
    part: Option<String>,
    attempts: u32,
}

struct Inner {
    node: Node,
    cfg: TransferConfig,
    db: Mutex<Connection>,
    opener: Arc<dyn SourceOpener>,
    events: mpsc::UnboundedSender<TransferInfo>,
    /// Running transfers: id → stop signal.
    running: Mutex<HashMap<String, watch::Sender<bool>>>,
    /// Peers with an outgoing transfer running (one file at a time per peer).
    sending_to: Mutex<HashMap<Fingerprint, String>>,
    /// Earliest next attempt for outgoing transfers that were cut off.
    retry_at: Mutex<HashMap<String, (Instant, Duration)>>,
    /// One receiving task per transfer at a time.
    receiving: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    wake: Notify,
}

/// The transfer engine. Cheap to clone.
#[derive(Clone)]
pub struct Transfers {
    inner: Arc<Inner>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS transfers (
    id        TEXT PRIMARY KEY,
    peer      TEXT NOT NULL,
    direction INTEGER NOT NULL,
    name      TEXT NOT NULL,
    size      INTEGER NOT NULL,
    mime      TEXT NOT NULL,
    source    TEXT,
    path      TEXT,
    part      TEXT,
    done      INTEGER NOT NULL DEFAULT 0,
    state     TEXT NOT NULL,
    message   TEXT NOT NULL DEFAULT '',
    attempts  INTEGER NOT NULL DEFAULT 0,
    created   INTEGER NOT NULL,
    updated   INTEGER NOT NULL
);
";

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn new_id() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}

fn parse_id(id: &str) -> Option<[u8; 16]> {
    let v = hex::decode(id).ok()?;
    v.try_into().ok()
}

/// A file name that is safe to create in the download folder: no directories, no hidden
/// files, no control characters, not too long (keeping the extension).
pub fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base.chars().filter(|c| !c.is_control()).collect();
    let trimmed = cleaned.trim().trim_start_matches(['.', ' ']).trim_end_matches([' ', '.']);
    if trimmed.is_empty() {
        return "file".into();
    }
    const MAX: usize = 180;
    if trimmed.len() <= MAX {
        return trimmed.to_string();
    }
    let (stem, ext) = match trimmed.rfind('.') {
        Some(i) if trimmed.len() - i <= 16 => (&trimmed[..i], &trimmed[i..]),
        _ => (trimmed, ""),
    };
    let mut end = MAX - ext.len();
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &stem[..end], ext)
}

/// Reserve a free name in `dir` (`photo.jpg`, `photo (2).jpg`, …) by creating it empty.
fn reserve_unique(dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1..10_000 {
        let candidate = if n == 1 {
            dir.join(name)
        } else {
            dir.join(format!("{stem} ({n}){ext}"))
        };
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many files with this name"))
}

/// Free bytes for an unprivileged user on the disk holding `dir`.
fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` a properly sized out-parameter.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

enum Outcome {
    Done,
    /// Try again later (connection lost, damaged copy).
    Retry(String),
    /// Give up (refused, source gone, cancelled).
    Fatal(String),
    /// The other device cancelled it.
    CancelledThere,
    Stopped,
}

impl Transfers {
    /// Start the engine: takes over `node`'s incoming data streams. Events: every change
    /// to a transfer (progress at most 4 times a second per transfer).
    pub fn start(
        node: Node,
        cfg: TransferConfig,
        opener: Arc<dyn SourceOpener>,
    ) -> Result<(Transfers, mpsc::UnboundedReceiver<TransferInfo>)> {
        let db = match &cfg.db_path {
            Some(path) => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let db = Connection::open(path)?;
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
                }
                db.pragma_update(None, "journal_mode", "WAL")?;
                db
            }
            None => Connection::open_in_memory()?,
        };
        db.execute_batch(SCHEMA)?;
        // Whatever was running when we stopped resumes.
        db.execute(
            "UPDATE transfers SET state = 'queued' WHERE state = 'active' AND direction = 0",
            [],
        )?;
        db.execute(
            "UPDATE transfers SET state = 'waiting', message = '' WHERE state = 'active' AND direction = 1",
            [],
        )?;
        let data = node.data_streams();
        let (events, rx) = mpsc::unbounded_channel();
        let t = Transfers {
            inner: Arc::new(Inner {
                node,
                cfg,
                db: Mutex::new(db),
                opener,
                events,
                running: Mutex::new(HashMap::new()),
                sending_to: Mutex::new(HashMap::new()),
                retry_at: Mutex::new(HashMap::new()),
                receiving: Mutex::new(HashMap::new()),
                wake: Notify::new(),
            }),
        };
        tokio::spawn(t.clone().sender_loop());
        match data {
            Some(mut data) => {
                let me = t.clone();
                tokio::spawn(async move {
                    while let Some(stream) = data.recv().await {
                        tokio::spawn(me.clone().receive(stream));
                    }
                });
            }
            None => warn!("file data streams were already taken; receiving files is off"),
        }
        Ok((t, rx))
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.inner.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn row(&self, id: &str) -> Option<Row> {
        let db = self.db();
        db.query_row(
            "SELECT id, peer, direction, name, size, mime, source, path, part, done, state, message, attempts, updated
             FROM transfers WHERE id = ?1",
            params![id],
            Self::map_row,
        )
        .optional()
        .ok()
        .flatten()
    }

    fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
        let peer: String = r.get(1)?;
        let peer = Fingerprint::from_hex(&peer).unwrap_or(Fingerprint([0; 32]));
        let direction: i64 = r.get(2)?;
        let state: String = r.get(10)?;
        Ok(Row {
            info: TransferInfo {
                id: r.get(0)?,
                peer,
                direction: if direction == 0 {
                    Direction::Outgoing
                } else {
                    Direction::Incoming
                },
                name: r.get(3)?,
                size: r.get::<_, i64>(4)? as u64,
                mime: r.get(5)?,
                path: r.get(7)?,
                done: r.get::<_, i64>(9)? as u64,
                state: TransferState::parse(&state),
                message: r.get(11)?,
                updated_ms: r.get::<_, i64>(13)? as u64,
            },
            source: r.get(6)?,
            part: r.get(8)?,
            attempts: r.get::<_, i64>(12)? as u32,
        })
    }

    /// Change a transfer's state (and progress); tells the app.
    fn set(&self, id: &str, state: TransferState, done: Option<u64>, message: &str) {
        {
            let db = self.db();
            let r = match done {
                Some(d) => db.execute(
                    "UPDATE transfers SET state = ?2, done = ?3, message = ?4, updated = ?5 WHERE id = ?1",
                    params![id, state.as_str(), d as i64, message, now_ms() as i64],
                ),
                None => db.execute(
                    "UPDATE transfers SET state = ?2, message = ?3, updated = ?4 WHERE id = ?1",
                    params![id, state.as_str(), message, now_ms() as i64],
                ),
            };
            if let Err(e) = r {
                warn!("could not save transfer state: {e}");
            }
        }
        self.emit(id);
    }

    fn emit(&self, id: &str) {
        if let Some(r) = self.row(id) {
            let _ = self.inner.events.send(r.info);
        }
    }

    /// Progress without touching the database (it is saved at the end / on interruption).
    fn emit_progress(&self, row: &TransferInfo, done: u64) {
        let mut info = row.clone();
        info.done = done;
        info.state = TransferState::Active;
        info.message.clear();
        info.updated_ms = now_ms();
        let _ = self.inner.events.send(info);
    }

    // ------------------------------------------------------------------ public API

    /// Queue a file for `peer`. `source` is what the [`SourceOpener`] understands (a path
    /// on the laptop). Returns the transfer id.
    pub fn send_file(
        &self,
        peer: Fingerprint,
        source: String,
        name: &str,
        size: u64,
        mime: &str,
    ) -> Result<String> {
        if self.inner.node.peer(&peer).is_none() {
            return Err(Error::NotPaired);
        }
        let id = new_id();
        let now = now_ms() as i64;
        self.db().execute(
            "INSERT INTO transfers (id, peer, direction, name, size, mime, source, state, created, updated)
             VALUES (?1, ?2, 0, ?3, ?4, ?5, ?6, 'queued', ?7, ?7)",
            params![id, peer.to_hex(), safe_name(name), size as i64, mime, source, now],
        )?;
        self.emit(&id);
        self.inner.wake.notify_one();
        Ok(id)
    }

    /// Stop a transfer on both devices. False if it had already finished.
    pub fn cancel(&self, id: &str) -> Result<bool> {
        let Some(row) = self.row(id) else {
            return Err(Error::Protocol("no such transfer".into()));
        };
        if row.info.state.finished() {
            return Ok(false);
        }
        self.stop_local(&row, "Cancelled");
        let msg = AppMessage::FileCancel { id: id.to_string() }.encode()?;
        if let Err(e) = self.inner.node.send(&row.info.peer, msg) {
            debug!("could not tell the other device about the cancel: {e}");
        }
        Ok(true)
    }

    fn stop_local(&self, row: &Row, message: &str) {
        if let Some(stop) = self
            .inner
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&row.info.id)
        {
            let _ = stop.send(true);
        }
        if row.info.direction == Direction::Incoming {
            if let Some(part) = &row.part {
                let _ = std::fs::remove_file(part);
            }
        }
        self.set(&row.info.id, TransferState::Cancelled, None, message);
    }

    /// Handle a message from `peer` that belongs to file transfers. Returns true if it was
    /// one (the app ignores it then).
    pub fn on_message(&self, peer: &Fingerprint, msg: &AppMessage) -> bool {
        let AppMessage::FileCancel { id } = msg else {
            return false;
        };
        if let Some(row) = self.row(id) {
            if row.info.peer == *peer && !row.info.state.finished() {
                info!(transfer = %id, "cancelled on the other device");
                self.stop_local(&row, "Cancelled on the other device");
            }
        }
        true
    }

    /// All transfers, newest first.
    pub fn list(&self) -> Vec<TransferInfo> {
        let db = self.db();
        let Ok(mut stmt) = db.prepare(
            "SELECT id, peer, direction, name, size, mime, source, path, part, done, state, message, attempts, updated
             FROM transfers ORDER BY created DESC LIMIT 300",
        ) else {
            return vec![];
        };
        stmt.query_map([], Self::map_row)
            .map(|rows| rows.filter_map(|r| r.ok()).map(|r| r.info).collect())
            .unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<TransferInfo> {
        self.row(id).map(|r| r.info)
    }

    /// Remove finished transfers from the list (the files stay).
    pub fn clear_finished(&self) -> Result<usize> {
        Ok(self.db().execute(
            "DELETE FROM transfers WHERE state IN ('done', 'failed', 'cancelled')",
            [],
        )?)
    }

    /// Forget everything about a device that was unpaired.
    pub fn forget_peer(&self, peer: &Fingerprint) {
        let rows: Vec<Row> = self
            .list()
            .into_iter()
            .filter(|t| t.peer == *peer)
            .filter_map(|t| self.row(&t.id))
            .collect();
        for r in rows {
            if !r.info.state.finished() {
                self.stop_local(&r, "Device unpaired");
            }
        }
        let _ = self
            .db()
            .execute("DELETE FROM transfers WHERE peer = ?1", params![peer.to_hex()]);
    }

    /// Try waiting transfers now (call when a device connects).
    pub fn nudge(&self) {
        self.inner.retry_at.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.inner.wake.notify_one();
    }

    // ------------------------------------------------------------------ sending

    async fn sender_loop(self) {
        loop {
            tokio::select! {
                _ = self.inner.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
            self.start_due();
            self.prune_finished();
        }
    }

    fn prune_finished(&self) {
        let _ = self.db().execute(
            "DELETE FROM transfers WHERE state IN ('done', 'failed', 'cancelled') AND id NOT IN
             (SELECT id FROM transfers WHERE state IN ('done', 'failed', 'cancelled') ORDER BY updated DESC LIMIT ?1)",
            params![KEEP_FINISHED as i64],
        );
    }

    fn start_due(&self) {
        let pending: Vec<Row> = {
            let db = self.db();
            let Ok(mut stmt) = db.prepare(
                "SELECT id, peer, direction, name, size, mime, source, path, part, done, state, message, attempts, updated
                 FROM transfers WHERE direction = 0 AND state IN ('queued', 'waiting') ORDER BY created",
            ) else {
                return;
            };
            stmt.query_map([], Self::map_row)
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        };
        let now = Instant::now();
        for row in pending {
            let peer = row.info.peer;
            if self
                .inner
                .sending_to
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&peer)
            {
                continue; // one file at a time per device
            }
            let waiting = |msg: String| {
                if row.info.state != TransferState::Waiting || row.info.message != msg {
                    self.set(&row.info.id, TransferState::Waiting, None, &msg);
                }
            };
            match self.inner.node.transport(&peer) {
                Some(_) if !self.inner.node.peer_supports(&peer, CAP_FILES) => {
                    let name = self.inner.node.peer(&peer).map(|p| p.name).unwrap_or_default();
                    waiting(format!("Update PhoneLink on {name} to send files"));
                    continue;
                }
                Some(Transport::Quic) => {}
                Some(Transport::Stream) => {
                    waiting("Waiting for Wi-Fi (connected over Bluetooth)".into());
                    continue;
                }
                None => {
                    let name = self.inner.node.peer(&peer).map(|p| p.name).unwrap_or_default();
                    waiting(format!("Waiting for {name} to connect"));
                    continue;
                }
            }
            if let Some((at, _)) = self
                .inner
                .retry_at
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&row.info.id)
            {
                if *at > now {
                    continue;
                }
            }
            self.inner
                .sending_to
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(peer, row.info.id.clone());
            let me = self.clone();
            tokio::spawn(async move {
                let id = row.info.id.clone();
                me.run_send(row).await;
                me.inner
                    .sending_to
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&peer);
                me.inner.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                me.inner.wake.notify_one();
            });
        }
    }

    fn register(&self, id: &str) -> watch::Receiver<bool> {
        let (tx, rx) = watch::channel(false);
        self.inner
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), tx);
        rx
    }

    async fn run_send(&self, row: Row) {
        let id = row.info.id.clone();
        let mut stop = self.register(&id);
        self.set(&id, TransferState::Active, None, "");
        let progress = Arc::new(std::sync::atomic::AtomicU64::new(row.info.done));
        let outcome = tokio::select! {
            o = self.send_once(&row, progress.clone()) => o,
            _ = stop.wait_for(|v| *v) => Outcome::Stopped,
        };
        let done = progress.load(std::sync::atomic::Ordering::Relaxed);
        match outcome {
            Outcome::Done => {
                self.inner.retry_at.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                info!(transfer = %id, name = %row.info.name, "sent");
                self.set(&id, TransferState::Done, Some(row.info.size), "");
            }
            Outcome::Stopped => {} // cancelled: state already set
            Outcome::CancelledThere => {
                self.set(&id, TransferState::Cancelled, Some(done), "Cancelled on the other device")
            }
            Outcome::Fatal(msg) => {
                warn!(transfer = %id, "sending failed: {msg}");
                self.set(&id, TransferState::Failed, Some(done), &msg);
            }
            Outcome::Retry(msg) => {
                let attempts = row.attempts + 1;
                let _ = self.db().execute(
                    "UPDATE transfers SET attempts = ?2 WHERE id = ?1",
                    params![id, attempts as i64],
                );
                if attempts >= MAX_ATTEMPTS * 10 {
                    self.set(&id, TransferState::Failed, Some(done), &msg);
                    return;
                }
                let mut retry = self.inner.retry_at.lock().unwrap_or_else(|e| e.into_inner());
                let delay = retry
                    .get(&id)
                    .map(|(_, d)| (*d * 2).min(RETRY_MAX))
                    .unwrap_or(RETRY_MIN);
                retry.insert(id.clone(), (Instant::now() + delay, delay));
                drop(retry);
                debug!(transfer = %id, "send interrupted ({msg}); retrying in {delay:?}");
                self.set(&id, TransferState::Waiting, Some(done), &msg);
            }
        }
    }

    async fn send_once(&self, row: &Row, progress: Arc<std::sync::atomic::AtomicU64>) -> Outcome {
        let info = &row.info;
        let Some(id) = parse_id(&info.id) else {
            return Outcome::Fatal("bad transfer id".into());
        };
        let (mut send, mut recv) = match self.inner.node.open_data(&info.peer).await {
            Ok(s) => s,
            Err(Error::NoNetworkLink) => return Outcome::Retry("Waiting for Wi-Fi".into()),
            Err(e) => return Outcome::Retry(format!("Interrupted: {e}")),
        };
        let start = Control::FileStart {
            id,
            name: info.name.clone(),
            size: info.size,
            mime: info.mime.clone(),
        };
        if let Err(e) = wire::write_frame(&mut send, &start).await {
            return Outcome::Retry(format!("Interrupted: {e}"));
        }
        let offset = match timeout(HEADER_TIMEOUT, wire::read_frame(&mut recv)).await {
            Ok(Ok(Some(Frame::Known(Control::FileResume { offset })))) => offset.min(info.size),
            Ok(Ok(Some(Frame::Known(Control::Reject { reason })))) if reason == CANCELLED => {
                return Outcome::CancelledThere
            }
            Ok(Ok(Some(Frame::Known(Control::Reject { reason })))) => return Outcome::Fatal(reason),
            Ok(Ok(_)) => return Outcome::Retry("The other device did not answer".into()),
            Ok(Err(e)) => return Outcome::Retry(format!("Interrupted: {e}")),
            Err(_) => return Outcome::Retry("The other device did not answer in time".into()),
        };

        // Open the source (on the phone this asks the app for a file descriptor).
        let opener = self.inner.opener.clone();
        let (tid, source) = (info.id.clone(), row.source.clone().unwrap_or_default());
        let file = match tokio::task::spawn_blocking(move || {
            let mut f = opener.open(&tid, &source)?;
            let _ = f.rewind(); // a reopened descriptor may not be at the start
            Ok::<_, std::io::Error>(f)
        })
        .await
        {
            Ok(Ok(f)) => f,
            Ok(Err(e)) => {
                return Outcome::Fatal(format!("Can't read the file any more ({e}). Share it again."))
            }
            Err(e) => return Outcome::Fatal(e.to_string()),
        };
        let mut file = tokio::fs::File::from_std(file);
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; CHUNK];

        // The receiver already has [0, offset): read it only to hash it.
        let mut pos = 0u64;
        while pos < offset {
            let want = ((offset - pos) as usize).min(CHUNK);
            match file.read(&mut buf[..want]).await {
                Ok(0) => return Outcome::Fatal("The file changed while it was being sent".into()),
                Ok(n) => {
                    hasher.update(&buf[..n]);
                    pos += n as u64;
                }
                Err(e) => return Outcome::Fatal(format!("Can't read the file: {e}")),
            }
        }
        progress.store(offset, std::sync::atomic::Ordering::Relaxed);
        self.emit_progress(info, offset);
        let mut last_emit = Instant::now();
        while pos < info.size {
            let want = ((info.size - pos) as usize).min(CHUNK);
            let n = match file.read(&mut buf[..want]).await {
                Ok(0) => return Outcome::Fatal("The file changed while it was being sent".into()),
                Ok(n) => n,
                Err(e) => return Outcome::Fatal(format!("Can't read the file: {e}")),
            };
            hasher.update(&buf[..n]);
            if let Err(e) = send.write_all(&buf[..n]).await {
                return Outcome::Retry(format!("Interrupted: {e}"));
            }
            pos += n as u64;
            progress.store(pos, std::sync::atomic::Ordering::Relaxed);
            if last_emit.elapsed() >= PROGRESS_EVERY {
                last_emit = Instant::now();
                self.emit_progress(info, pos);
            }
        }
        // Anything beyond `size` means the file grew: what we sent is a stale copy.
        let mut extra = [0u8; 1];
        if matches!(file.read(&mut extra).await, Ok(n) if n > 0) {
            return Outcome::Fatal("The file changed while it was being sent".into());
        }
        if let Err(e) = send.write_all(hasher.finalize().as_bytes()).await {
            return Outcome::Retry(format!("Interrupted: {e}"));
        }
        let _ = send.finish();
        match timeout(RESULT_TIMEOUT, wire::read_frame(&mut recv)).await {
            Ok(Ok(Some(Frame::Known(Control::FileResult { ok: true, .. })))) => Outcome::Done,
            Ok(Ok(Some(Frame::Known(Control::FileResult { ok: false, message })))) => {
                if row.attempts + 1 >= MAX_ATTEMPTS {
                    Outcome::Fatal(message)
                } else {
                    Outcome::Retry(message)
                }
            }
            Ok(Ok(Some(Frame::Known(Control::Reject { reason })))) if reason == CANCELLED => {
                Outcome::CancelledThere
            }
            Ok(Ok(Some(Frame::Known(Control::Reject { reason })))) => Outcome::Fatal(reason),
            Ok(_) => Outcome::Retry("Interrupted before the other device confirmed".into()),
            Err(_) => Outcome::Retry("The other device did not confirm in time".into()),
        }
    }

    // ------------------------------------------------------------------ receiving

    async fn receive(self, stream: DataStream) {
        let DataStream {
            peer,
            mut send,
            mut recv,
        } = stream;
        let header = match timeout(HEADER_TIMEOUT, wire::read_frame(&mut recv)).await {
            Ok(Ok(Some(Frame::Known(Control::FileStart {
                id,
                name,
                size,
                mime,
            })))) => (hex::encode(id), name, size, mime),
            _ => return debug!("data stream without a file header; closed"),
        };
        let (id, name, size, mime) = header;
        let reject = |reason: &str| Control::Reject {
            reason: reason.to_string(),
        };

        // One task per transfer: a retry waits for the previous attempt to wind down.
        let lock = self
            .inner
            .receiving
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.clone())
            .or_default()
            .clone();
        let Ok(_guard) = timeout(Duration::from_secs(10), lock.lock()).await else {
            let _ = wire::write_frame(&mut send, &reject("busy")).await;
            return;
        };

        let existing = self.row(&id);
        let row = match existing {
            Some(r) if r.info.peer != peer || r.info.direction != Direction::Incoming => {
                let _ = wire::write_frame(&mut send, &reject("transfer id in use")).await;
                return;
            }
            Some(r) if r.info.state == TransferState::Cancelled => {
                let _ = wire::write_frame(&mut send, &reject(CANCELLED)).await;
                return;
            }
            Some(r) if r.info.state == TransferState::Done => {
                // We have it already; the confirmation was lost. Confirm again.
                let _ = wire::write_frame(&mut send, &Control::FileResume { offset: size }).await;
                let mut hash = [0u8; 32];
                if recv.read_exact(&mut hash).await.is_ok() {
                    let _ = wire::write_frame(
                        &mut send,
                        &Control::FileResult {
                            ok: true,
                            message: "Saved".into(),
                        },
                    )
                    .await;
                    let _ = send.finish();
                }
                return;
            }
            Some(r) => r,
            None => {
                let dir = &self.inner.cfg.incoming_dir;
                if let Err(e) = std::fs::create_dir_all(dir) {
                    warn!("cannot create {}: {e}", dir.display());
                    let _ = wire::write_frame(&mut send, &reject("Can't save files on the other device")).await;
                    return;
                }
                let free = free_space(dir).unwrap_or(u64::MAX);
                if size.saturating_add(self.inner.cfg.reserve_bytes) > free {
                    let device = self.inner.node.device_name().to_string();
                    let _ = wire::write_frame(&mut send, &reject(&format!("Not enough free space on {device}"))).await;
                    return;
                }
                let safe = safe_name(&name);
                let part = dir.join(format!(".phonelink-{}.part", &id[..16]));
                let now = now_ms() as i64;
                if let Err(e) = self.db().execute(
                    "INSERT INTO transfers (id, peer, direction, name, size, mime, part, state, created, updated)
                     VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, 'active', ?7, ?7)",
                    params![id, peer.to_hex(), safe, size as i64, mime, part.to_string_lossy(), now],
                ) {
                    warn!("could not record incoming file: {e}");
                    return;
                }
                match self.row(&id) {
                    Some(r) => r,
                    None => return,
                }
            }
        };
        let mut stop = self.register(&id);
        self.set(&id, TransferState::Active, None, "");
        let progress = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let outcome = tokio::select! {
            o = self.receive_into(&row, &mut send, &mut recv, progress.clone()) => o,
            _ = stop.wait_for(|v| *v) => Outcome::Stopped,
        };
        self.inner.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        let done = progress.load(std::sync::atomic::Ordering::Relaxed);
        match outcome {
            Outcome::Done => {}
            Outcome::Stopped | Outcome::CancelledThere => {
                let _ = recv.stop(0u32.into());
            }
            Outcome::Retry(msg) => self.set(&id, TransferState::Waiting, Some(done), &msg),
            Outcome::Fatal(msg) => {
                if let Some(part) = &row.part {
                    let _ = std::fs::remove_file(part);
                }
                self.set(&id, TransferState::Failed, Some(done), &msg);
            }
        }
    }

    async fn receive_into(
        &self,
        row: &Row,
        send: &mut quinn::SendStream,
        recv: &mut quinn::RecvStream,
        progress: Arc<std::sync::atomic::AtomicU64>,
    ) -> Outcome {
        let info = &row.info;
        let Some(part) = row.part.clone().map(PathBuf::from) else {
            return Outcome::Fatal("no place for the file".into());
        };
        let size = info.size;
        // What we already have (and its hash so far).
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0).min(size);
        let part2 = part.clone();
        let hashed = tokio::task::spawn_blocking(move || -> std::io::Result<blake3::Hasher> {
            let mut h = blake3::Hasher::new();
            if have == 0 {
                return Ok(h);
            }
            let mut f = std::fs::File::open(&part2)?;
            let mut left = have;
            let mut buf = vec![0u8; CHUNK];
            while left > 0 {
                let n = f.read(&mut buf[..(left as usize).min(CHUNK)])?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
                left -= n as u64;
            }
            Ok(h)
        })
        .await;
        let mut hasher = match hashed {
            Ok(Ok(h)) => h,
            _ => blake3::Hasher::new(),
        };
        let mut file = match tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&part)
            .await
        {
            Ok(f) => f,
            Err(e) => return Outcome::Fatal(format!("Can't save the file: {e}")),
        };
        if file.set_len(have).await.is_err() {
            return Outcome::Fatal("Can't save the file".into());
        }
        let _ = file.seek(std::io::SeekFrom::Start(have)).await;
        if wire::write_frame(send, &Control::FileResume { offset: have }).await.is_err() {
            return Outcome::Retry("Interrupted".into());
        }
        if have > 0 {
            info!(transfer = %info.id, "resuming at {have} of {size} bytes");
        }

        let mut pos = have;
        progress.store(pos, std::sync::atomic::Ordering::Relaxed);
        self.emit_progress(info, pos); // where it (re)starts, shown at once
        let mut buf = vec![0u8; CHUNK];
        let mut since_sync = 0u64;
        let mut last_emit = Instant::now();
        let interrupted = |pos: u64| {
            let name = self.inner.node.peer(&info.peer).map(|p| p.name).unwrap_or_default();
            let _ = pos;
            Outcome::Retry(format!("Interrupted; continues when {name} is back"))
        };
        while pos < size {
            let want = ((size - pos) as usize).min(CHUNK);
            let n = match recv.read(&mut buf[..want]).await {
                Ok(Some(n)) if n > 0 => n,
                _ => {
                    let _ = file.sync_all().await;
                    return interrupted(pos);
                }
            };
            if let Err(e) = file.write_all(&buf[..n]).await {
                return Outcome::Fatal(format!("Can't save the file: {e}"));
            }
            hasher.update(&buf[..n]);
            pos += n as u64;
            since_sync += n as u64;
            progress.store(pos, std::sync::atomic::Ordering::Relaxed);
            if since_sync >= SYNC_EVERY {
                since_sync = 0;
                let _ = file.sync_data().await;
            }
            if last_emit.elapsed() >= PROGRESS_EVERY {
                last_emit = Instant::now();
                self.emit_progress(info, pos);
            }
        }
        let mut theirs = [0u8; 32];
        if recv.read_exact(&mut theirs).await.is_err() {
            let _ = file.sync_all().await;
            return interrupted(pos);
        }
        if file.sync_all().await.is_err() {
            return Outcome::Fatal("Can't save the file".into());
        }
        drop(file);
        if *hasher.finalize().as_bytes() != theirs {
            warn!(transfer = %info.id, "file arrived damaged; asking for it again");
            let _ = std::fs::remove_file(&part);
            let _ = wire::write_frame(
                send,
                &Control::FileResult {
                    ok: false,
                    message: "The file arrived damaged; sending it again".into(),
                },
            )
            .await;
            let _ = send.finish();
            return Outcome::Retry("Arrived damaged; sending it again".into());
        }
        let dir = self.inner.cfg.incoming_dir.clone();
        let final_path = match reserve_unique(&dir, &info.name).and_then(|p| {
            std::fs::rename(&part, &p)?;
            Ok(p)
        }) {
            Ok(p) => p,
            Err(e) => return Outcome::Fatal(format!("Can't save the file: {e}")),
        };
        let path = final_path.to_string_lossy().into_owned();
        let _ = self.db().execute(
            "UPDATE transfers SET path = ?2, part = NULL, done = size, state = 'done', message = '', updated = ?3 WHERE id = ?1",
            params![info.id, path, now_ms() as i64],
        );
        info!(transfer = %info.id, path = %path, "received");
        self.emit(&info.id);
        let _ = wire::write_frame(
            send,
            &Control::FileResult {
                ok: true,
                message: "Saved".into(),
            },
        )
        .await;
        let _ = send.finish();
        Outcome::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(safe_name("photo.jpg"), "photo.jpg");
        assert_eq!(safe_name("../../.bashrc"), "bashrc");
        assert_eq!(safe_name("C:\\Users\\x\\notes.txt"), "notes.txt");
        assert_eq!(safe_name("/etc/passwd"), "passwd");
        assert_eq!(safe_name("..."), "file");
        assert_eq!(safe_name(""), "file");
        assert_eq!(safe_name("a\nb\u{0}c.pdf"), "abc.pdf");
        assert_eq!(safe_name(" spaced name .txt "), "spaced name .txt");
        let long = format!("{}.mp4", "x".repeat(400));
        let s = safe_name(&long);
        assert!(s.len() <= 180 && s.ends_with(".mp4"), "{s}");
        let uni = format!("{}.jpg", "é".repeat(200));
        assert!(safe_name(&uni).ends_with(".jpg"));
    }

    #[test]
    fn unique_names_do_not_overwrite() {
        let d = tempfile::tempdir().unwrap();
        let a = reserve_unique(d.path(), "IMG.jpg").unwrap();
        let b = reserve_unique(d.path(), "IMG.jpg").unwrap();
        let c = reserve_unique(d.path(), "IMG.jpg").unwrap();
        let n = |p: &PathBuf| p.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!((n(&a), n(&b), n(&c)), ("IMG.jpg".into(), "IMG (2).jpg".into(), "IMG (3).jpg".into()));
        let r = reserve_unique(d.path(), "README").unwrap();
        let r2 = reserve_unique(d.path(), "README").unwrap();
        assert_eq!((n(&r), n(&r2)), ("README".into(), "README (2)".into()));
    }

    #[test]
    fn free_space_is_reported() {
        let d = tempfile::tempdir().unwrap();
        assert!(free_space(d.path()).unwrap() > 0);
    }
}
