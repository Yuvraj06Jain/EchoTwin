//! Handshake flood protection: per-IP rate limit plus a global cap on concurrent
//! handshakes. Established sessions are never affected.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub struct HandshakeLimiter {
    per_ip: Mutex<HashMap<IpAddr, (Instant, u32)>>,
    window: Duration,
    max_per_window: u32,
    inflight: Arc<Semaphore>,
}

impl HandshakeLimiter {
    pub fn new(max_per_window: u32, window: Duration, max_inflight: usize) -> Self {
        Self {
            per_ip: Mutex::new(HashMap::new()),
            window,
            max_per_window,
            inflight: Arc::new(Semaphore::new(max_inflight)),
        }
    }

    /// Record an attempt from `ip`; false if that address is over its budget.
    pub fn allow(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut map = self.per_ip.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() > 4096 {
            let window = self.window;
            map.retain(|_, (start, _)| now.duration_since(*start) < window);
        }
        let entry = map.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= self.max_per_window
    }

    pub fn try_permit(&self) -> Option<OwnedSemaphorePermit> {
        self.inflight.clone().try_acquire_owned().ok()
    }
}

impl Default for HandshakeLimiter {
    fn default() -> Self {
        Self::new(20, Duration::from_secs(10), 64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_per_ip_independently() {
        let l = HandshakeLimiter::new(3, Duration::from_secs(60), 8);
        let (a, b): (IpAddr, IpAddr) = ("10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap());
        assert!((0..3).all(|_| l.allow(a)));
        assert!(!l.allow(a));
        assert!(l.allow(b), "one noisy address must not lock out others");
    }

    #[test]
    fn caps_concurrent_handshakes() {
        let l = HandshakeLimiter::new(100, Duration::from_secs(1), 2);
        let p1 = l.try_permit().unwrap();
        let _p2 = l.try_permit().unwrap();
        assert!(l.try_permit().is_none());
        drop(p1);
        assert!(l.try_permit().is_some());
    }
}
