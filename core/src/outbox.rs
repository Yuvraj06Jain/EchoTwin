//! Durable outbox + duplicate filter (SQLite, WAL).
//!
//! Every application message is written to disk *before* it is sent and deleted only when
//! the peer acknowledges it. After any disconnect, crash or reboot, unacknowledged messages
//! are re-sent on the next connection. Each message ID is processed at most once on the
//! receiving side, so a re-send never produces duplicates.

use std::{path::Path, sync::Mutex};

use rusqlite::{params, Connection, OptionalExtension};

use crate::{util, Fingerprint, MsgId, Result};

pub struct Outbox {
    db: Mutex<Connection>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS outbox (
    seq     INTEGER PRIMARY KEY AUTOINCREMENT,
    id      BLOB NOT NULL UNIQUE,
    peer    TEXT NOT NULL,
    created INTEGER NOT NULL,
    body    BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS outbox_peer ON outbox(peer, seq);
CREATE TABLE IF NOT EXISTS seen (
    peer TEXT NOT NULL,
    id   BLOB NOT NULL,
    at   INTEGER NOT NULL,
    PRIMARY KEY (peer, id)
) WITHOUT ROWID;
";

impl Outbox {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let db = Connection::open(path)?;
        #[cfg(unix)]
        {
            // Message bodies (notifications, SMS later) are private; SQLite gives its
            // -wal/-shm side files the same mode as the main file.
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        db.pragma_update(None, "journal_mode", "WAL")?;
        // NORMAL, not FULL: with WAL this skips an fsync per commit (every message and
        // ack) yet the database stays consistent even if the app crashes; only a power
        // cut can lose the last few commits.
        db.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(db)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(db: Connection) -> Result<Self> {
        db.execute_batch(SCHEMA)?;
        Ok(Self { db: Mutex::new(db) })
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn enqueue(&self, peer: &Fingerprint, body: &[u8]) -> Result<MsgId> {
        let id = MsgId::random();
        self.db().execute(
            "INSERT INTO outbox (id, peer, created, body) VALUES (?1, ?2, ?3, ?4)",
            params![&id.0[..], peer.to_hex(), util::now_unix() as i64, body],
        )?;
        Ok(id)
    }

    /// Unacknowledged messages for `peer`, oldest first.
    pub fn pending(&self, peer: &Fingerprint) -> Result<Vec<(MsgId, Vec<u8>)>> {
        let db = self.db();
        let mut stmt = db.prepare("SELECT id, body FROM outbox WHERE peer = ?1 ORDER BY seq")?;
        let rows = stmt.query_map(params![peer.to_hex()], |r| {
            let id: Vec<u8> = r.get(0)?;
            let body: Vec<u8> = r.get(1)?;
            Ok((id, body))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, body) = row?;
            if let Ok(arr) = <[u8; 16]>::try_from(id.as_slice()) {
                out.push((MsgId(arr), body));
            }
        }
        Ok(out)
    }

    pub fn pending_count(&self, peer: &Fingerprint) -> Result<usize> {
        let n: i64 = self.db().query_row(
            "SELECT COUNT(*) FROM outbox WHERE peer = ?1",
            params![peer.to_hex()],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    pub fn ack(&self, peer: &Fingerprint, id: &MsgId) -> Result<()> {
        self.db().execute(
            "DELETE FROM outbox WHERE peer = ?1 AND id = ?2",
            params![peer.to_hex(), &id.0[..]],
        )?;
        Ok(())
    }

    /// Records `id` as received. Returns true only the first time.
    pub fn first_seen(&self, peer: &Fingerprint, id: &MsgId) -> Result<bool> {
        let changed = self.db().execute(
            "INSERT OR IGNORE INTO seen (peer, id, at) VALUES (?1, ?2, ?3)",
            params![peer.to_hex(), &id.0[..], util::now_unix() as i64],
        )?;
        Ok(changed == 1)
    }

    /// Drop queued messages older than `max_age_secs` that never got through (the other
    /// device was away for a very long time). Keeps the outbox from growing forever.
    pub fn prune_pending(&self, max_age_secs: u64) -> Result<usize> {
        let cutoff = util::now_unix().saturating_sub(max_age_secs) as i64;
        Ok(self
            .db()
            .execute("DELETE FROM outbox WHERE created < ?1", params![cutoff])?)
    }

    /// Forget dedup entries older than `max_age_secs` (senders give up long before that).
    pub fn prune_seen(&self, max_age_secs: u64) -> Result<usize> {
        let cutoff = util::now_unix().saturating_sub(max_age_secs) as i64;
        Ok(self
            .db()
            .execute("DELETE FROM seen WHERE at < ?1", params![cutoff])?)
    }

    pub fn forget_peer(&self, peer: &Fingerprint) -> Result<()> {
        let db = self.db();
        db.execute("DELETE FROM outbox WHERE peer = ?1", params![peer.to_hex()])?;
        db.execute("DELETE FROM seen WHERE peer = ?1", params![peer.to_hex()])?;
        Ok(())
    }

    pub fn contains(&self, id: &MsgId) -> Result<bool> {
        Ok(self
            .db()
            .query_row(
                "SELECT 1 FROM outbox WHERE id = ?1",
                params![&id.0[..]],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_ack_dedup_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outbox.sqlite3");
        let peer = Fingerprint([1; 32]);
        let (a, b);
        {
            let o = Outbox::open(&path).unwrap();
            a = o.enqueue(&peer, b"first").unwrap();
            b = o.enqueue(&peer, b"second").unwrap();
            o.ack(&peer, &a).unwrap();
        }
        let o = Outbox::open(&path).unwrap(); // "reboot"
        assert_eq!(o.pending(&peer).unwrap(), vec![(b, b"second".to_vec())]);
        assert!(!o.contains(&a).unwrap());

        assert!(o.first_seen(&peer, &a).unwrap());
        assert!(!o.first_seen(&peer, &a).unwrap(), "duplicates are filtered");
        assert!(
            o.first_seen(&Fingerprint([2; 32]), &a).unwrap(),
            "dedup is per peer"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "message store must be owner-only");
        }
    }

    #[test]
    fn very_old_undelivered_messages_expire() {
        let o = Outbox::in_memory().unwrap();
        let peer = Fingerprint([3; 32]);
        let old = o.enqueue(&peer, b"from last month").unwrap();
        let fresh = o.enqueue(&peer, b"from today").unwrap();
        o.db()
            .execute(
                "UPDATE outbox SET created = 0 WHERE id = ?1",
                params![&old.0[..]],
            )
            .unwrap();
        assert_eq!(o.prune_pending(7 * 24 * 3600).unwrap(), 1);
        assert_eq!(
            o.pending(&peer).unwrap(),
            vec![(fresh, b"from today".to_vec())]
        );
    }
}
