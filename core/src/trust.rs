//! Who is allowed in: the pinned-peer store plus the (normally closed) pairing window.

use std::{
    collections::BTreeMap,
    fmt,
    net::SocketAddr,
    path::PathBuf,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::{util, Fingerprint, Result};

const MAX_REMEMBERED_ADDRS: usize = 4;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerRecord {
    pub fingerprint: Fingerprint,
    pub name: String,
    /// Shared secret derived during pairing; keys the rotating discovery token.
    #[serde(with = "crate::util::hex32")]
    pub pair_key: [u8; 32],
    pub paired_at: u64,
    /// Most recent first.
    #[serde(default)]
    pub last_addrs: Vec<SocketAddr>,
}

impl fmt::Debug for PeerRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PeerRecord")
            .field("fingerprint", &self.fingerprint)
            .field("name", &self.name)
            .field("paired_at", &self.paired_at)
            .field("last_addrs", &self.last_addrs)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
struct TrustFile {
    version: u32,
    peers: Vec<PeerRecord>,
}

/// Pinned peers, persisted as `peers.json` (owner-only permissions).
#[derive(Default)]
pub struct TrustStore {
    path: Option<PathBuf>,
    peers: BTreeMap<Fingerprint, PeerRecord>,
}

impl TrustStore {
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn load(path: PathBuf) -> Result<Self> {
        let peers = match std::fs::read(&path) {
            Ok(bytes) => {
                let file: TrustFile = serde_json::from_slice(&bytes)?;
                file.peers.into_iter().map(|p| (p.fingerprint, p)).collect()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            path: Some(path),
            peers,
        })
    }

    pub fn contains(&self, fp: &Fingerprint) -> bool {
        self.peers.contains_key(fp)
    }

    pub fn get(&self, fp: &Fingerprint) -> Option<&PeerRecord> {
        self.peers.get(fp)
    }

    pub fn list(&self) -> Vec<PeerRecord> {
        self.peers.values().cloned().collect()
    }

    pub fn upsert(&mut self, rec: PeerRecord) -> Result<()> {
        self.peers.insert(rec.fingerprint, rec);
        self.save()
    }

    pub fn remove(&mut self, fp: &Fingerprint) -> Result<bool> {
        let removed = self.peers.remove(fp).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    pub fn remember_addr(&mut self, fp: &Fingerprint, addr: SocketAddr) -> Result<()> {
        let Some(rec) = self.peers.get_mut(fp) else {
            return Ok(());
        };
        if rec.last_addrs.first() == Some(&addr) {
            return Ok(());
        }
        rec.last_addrs.retain(|a| *a != addr);
        rec.last_addrs.insert(0, addr);
        rec.last_addrs.truncate(MAX_REMEMBERED_ADDRS);
        self.save()
    }

    fn save(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let file = TrustFile {
            version: 1,
            peers: self.peers.values().cloned().collect(),
        };
        util::write_atomic(path, &serde_json::to_vec_pretty(&file)?, true)?;
        Ok(())
    }
}

struct PairingWindow {
    secret: [u8; 32],
    expires: Instant,
}

/// What an authenticated-but-not-yet-trusted handshake is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// A pinned peer: full session.
    Trusted,
    /// Unknown cert, but the user has the pairing screen open: may *attempt* pairing.
    PairingOnly,
    /// Everyone else: dropped during the TLS handshake.
    Denied,
}

/// Shared between the TLS verifiers (sync, called inside the handshake) and the node.
pub struct AccessGate {
    trust: Mutex<TrustStore>,
    pairing: Mutex<Option<PairingWindow>>,
}

impl fmt::Debug for AccessGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessGate")
            .field("peers", &self.trust().peers.len())
            .field("pairing_open", &self.pairing_secret().is_some())
            .finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AccessGate {
    pub fn new(trust: TrustStore) -> Self {
        Self {
            trust: Mutex::new(trust),
            pairing: Mutex::new(None),
        }
    }

    pub fn trust(&self) -> MutexGuard<'_, TrustStore> {
        lock(&self.trust)
    }

    pub fn is_trusted(&self, fp: &Fingerprint) -> bool {
        self.trust().contains(fp)
    }

    pub fn admit(&self, fp: &Fingerprint) -> Admission {
        if self.is_trusted(fp) {
            Admission::Trusted
        } else if self.pairing_secret().is_some() {
            Admission::PairingOnly
        } else {
            Admission::Denied
        }
    }

    /// Open the pairing window with a fresh 256-bit secret. Replaces any open window.
    pub fn open_pairing(&self, ttl: Duration) -> [u8; 32] {
        let secret: [u8; 32] = rand::random();
        *lock(&self.pairing) = Some(PairingWindow {
            secret,
            expires: Instant::now() + ttl,
        });
        secret
    }

    pub fn close_pairing(&self) {
        *lock(&self.pairing) = None;
    }

    pub fn pairing_secret(&self) -> Option<[u8; 32]> {
        let mut guard = lock(&self.pairing);
        match &*guard {
            Some(w) if Instant::now() < w.expires => Some(w.secret),
            Some(_) => {
                *guard = None;
                None
            }
            None => None,
        }
    }

    /// Close the window if it is still the one holding `secret`. Returns false if it was
    /// already used, replaced or expired; this makes each QR code single-use.
    pub fn consume_pairing(&self, secret: &[u8; 32]) -> bool {
        let mut guard = lock(&self.pairing);
        match &*guard {
            Some(w) if Instant::now() < w.expires && w.secret == *secret => {
                *guard = None;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(n: u8) -> PeerRecord {
        PeerRecord {
            fingerprint: Fingerprint([n; 32]),
            name: format!("peer{n}"),
            pair_key: [n; 32],
            paired_at: 1,
            last_addrs: vec![],
        }
    }

    #[test]
    fn trust_store_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.json");
        let mut s = TrustStore::load(path.clone()).unwrap();
        s.upsert(rec(1)).unwrap();
        s.upsert(rec(2)).unwrap();
        s.remove(&Fingerprint([2; 32])).unwrap();
        s.remember_addr(&Fingerprint([1; 32]), "10.0.0.5:4000".parse().unwrap())
            .unwrap();
        let s2 = TrustStore::load(path).unwrap();
        assert_eq!(s2.list(), s.list());
        assert!(s2.contains(&Fingerprint([1; 32])) && !s2.contains(&Fingerprint([2; 32])));
    }

    #[test]
    fn pairing_window_is_single_use_and_expires() {
        let gate = AccessGate::new(TrustStore::in_memory());
        let stranger = Fingerprint([9; 32]);
        assert_eq!(gate.admit(&stranger), Admission::Denied);

        let secret = gate.open_pairing(Duration::from_secs(60));
        assert_eq!(gate.admit(&stranger), Admission::PairingOnly);
        assert!(gate.consume_pairing(&secret));
        assert!(!gate.consume_pairing(&secret), "QR code must be single-use");
        assert_eq!(gate.admit(&stranger), Admission::Denied);

        gate.open_pairing(Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(
            gate.admit(&stranger),
            Admission::Denied,
            "window must expire"
        );
    }
}
