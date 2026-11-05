//! Privacy-preserving discovery tokens.
//!
//! Instead of broadcasting a device name or a stable ID on the LAN, each side advertises
//! `HMAC(pair_key, 10-minute window)`, truncated to 16 bytes. The paired partner can
//! recompute and recognise it. To everyone else it is random and changes every 10
//! minutes, so you cannot be identified or tracked across sessions on a shared network.

use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const TOKEN_WINDOW_SECS: u64 = 600;
const LABEL: &[u8] = b"phonelink-disco-v1";

pub fn token_at(pair_key: &[u8; 32], unix_secs: u64) -> [u8; 16] {
    token_for_window(pair_key, unix_secs / TOKEN_WINDOW_SECS)
}

fn token_for_window(pair_key: &[u8; 32], window: u64) -> [u8; 16] {
    let mut m = Hmac::<Sha256>::new_from_slice(pair_key).expect("any key length");
    m.update(LABEL);
    m.update(&window.to_be_bytes());
    let full = m.finalize().into_bytes();
    full[..16].try_into().expect("16 <= 32")
}

pub fn token_hex_now(pair_key: &[u8; 32]) -> String {
    hex::encode(token_at(pair_key, crate::util::now_unix()))
}

/// Accepts the previous, current and next window to tolerate clock skew between devices.
pub fn matches(pair_key: &[u8; 32], token: &[u8; 16], unix_secs: u64) -> bool {
    let w = unix_secs / TOKEN_WINDOW_SECS;
    [w.saturating_sub(1), w, w + 1]
        .iter()
        .fold(false, |acc, win| {
            // Non-short-circuiting, constant-time-ish comparison.
            let t = token_for_window(pair_key, *win);
            acc | (t.iter().zip(token).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_and_matches_only_with_key() {
        let key = [5u8; 32];
        let t0 = token_at(&key, 1_000_000);
        assert!(matches(&key, &t0, 1_000_000));
        assert!(
            matches(&key, &t0, 1_000_000 + TOKEN_WINDOW_SECS),
            "tolerates one window of skew"
        );
        assert!(
            !matches(&key, &t0, 1_000_000 + 3 * TOKEN_WINDOW_SECS),
            "old tokens die"
        );
        assert!(
            !matches(&[6u8; 32], &t0, 1_000_000),
            "other keys cannot recognise it"
        );
        assert_ne!(
            t0,
            token_at(&key, 1_000_000 + TOKEN_WINDOW_SECS),
            "token rotates"
        );
    }
}
