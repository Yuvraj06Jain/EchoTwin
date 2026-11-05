//! QR-code pairing.
//!
//! The laptop opens a short pairing window and shows a QR code containing:
//! its certificate fingerprint, its LAN addresses, its name, and a fresh 256-bit secret.
//!
//! The phone connects, and the TLS layer checks the laptop's certificate against the QR
//! fingerprint, so a man in the middle cannot pose as the laptop. The phone proves it saw
//! the QR code with `HMAC(secret, client_fp ‖ server_fp)`. The laptop answers with its own
//! HMAC, and both derive a shared `pair_key` with HKDF. Binding both fingerprints into
//! every proof means a proof cannot be replayed or relayed by another device.

use std::{fmt, net::SocketAddr};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::{util, Error, Fingerprint, Result};

type HmacSha256 = Hmac<Sha256>;

const URI_PREFIX: &str = "phonelink:pair?";
const CLIENT_LABEL: &[u8] = b"phonelink-pair-client-v1";
const SERVER_LABEL: &[u8] = b"phonelink-pair-server-v1";
const KEY_LABEL: &[u8] = b"phonelink-pairkey-v1";

#[derive(Clone, PartialEq)]
pub struct PairingOffer {
    pub fingerprint: Fingerprint,
    pub secret: [u8; 32],
    pub addrs: Vec<SocketAddr>,
    pub name: String,
}

impl fmt::Debug for PairingOffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingOffer")
            .field("fingerprint", &self.fingerprint)
            .field("addrs", &self.addrs)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl PairingOffer {
    /// The string encoded into the QR code.
    pub fn to_uri(&self) -> String {
        let addrs: Vec<String> = self.addrs.iter().map(|a| a.to_string()).collect();
        format!(
            "{URI_PREFIX}v=1&fp={}&s={}&a={}&n={}",
            self.fingerprint.to_hex(),
            B64.encode(self.secret),
            addrs.join(","),
            B64.encode(self.name.as_bytes()),
        )
    }

    pub fn from_uri(uri: &str) -> Result<Self> {
        let bad = |m: &str| Error::BadOffer(m.to_string());
        let query = uri
            .trim()
            .strip_prefix(URI_PREFIX)
            .ok_or_else(|| bad("not a PhoneLink code"))?;
        let (mut v, mut fp, mut s, mut a, mut n) = (None, None, None, None, None);
        for kv in query.split('&') {
            let (k, val) = kv.split_once('=').ok_or_else(|| bad("malformed field"))?;
            match k {
                "v" => v = Some(val),
                "fp" => fp = Some(val),
                "s" => s = Some(val),
                "a" => a = Some(val),
                "n" => n = Some(val),
                _ => {} // ignore fields added by newer versions
            }
        }
        if v != Some("1") {
            return Err(bad("unsupported pairing code version"));
        }
        let fingerprint = Fingerprint::from_hex(fp.ok_or_else(|| bad("missing fingerprint"))?)
            .map_err(|_| bad("bad fingerprint"))?;
        let secret: [u8; 32] = B64
            .decode(s.ok_or_else(|| bad("missing secret"))?)
            .map_err(|_| bad("bad secret"))?
            .try_into()
            .map_err(|_| bad("secret must be 32 bytes"))?;
        let addrs = a
            .unwrap_or("")
            .split(',')
            .filter(|x| !x.is_empty())
            .map(|x| x.parse().map_err(|_| bad("bad address")))
            .collect::<Result<Vec<SocketAddr>>>()?;
        if addrs.is_empty() {
            return Err(bad("no addresses"));
        }
        let name = n
            .and_then(|x| B64.decode(x).ok())
            .and_then(|b| String::from_utf8(b).ok())
            .map(|s| util::sanitize_name(&s))
            .unwrap_or_else(|| "Unnamed device".into());
        Ok(Self {
            fingerprint,
            secret,
            addrs,
            name,
        })
    }
}

fn mac(secret: &[u8; 32], label: &[u8], client: &Fingerprint, server: &Fingerprint) -> HmacSha256 {
    let mut m = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    m.update(label);
    m.update(&client.0);
    m.update(&server.0);
    m
}

pub fn client_proof(secret: &[u8; 32], client: &Fingerprint, server: &Fingerprint) -> [u8; 32] {
    mac(secret, CLIENT_LABEL, client, server)
        .finalize()
        .into_bytes()
        .into()
}

pub fn server_proof(secret: &[u8; 32], client: &Fingerprint, server: &Fingerprint) -> [u8; 32] {
    mac(secret, SERVER_LABEL, client, server)
        .finalize()
        .into_bytes()
        .into()
}

/// Constant-time comparison.
pub fn verify_client_proof(
    secret: &[u8; 32],
    client: &Fingerprint,
    server: &Fingerprint,
    proof: &[u8; 32],
) -> bool {
    mac(secret, CLIENT_LABEL, client, server)
        .verify_slice(proof)
        .is_ok()
}

/// Constant-time comparison.
pub fn verify_server_proof(
    secret: &[u8; 32],
    client: &Fingerprint,
    server: &Fingerprint,
    proof: &[u8; 32],
) -> bool {
    mac(secret, SERVER_LABEL, client, server)
        .verify_slice(proof)
        .is_ok()
}

pub fn derive_pair_key(secret: &[u8; 32], client: &Fingerprint, server: &Fingerprint) -> [u8; 32] {
    let mut info = Vec::with_capacity(KEY_LABEL.len() + 64);
    info.extend_from_slice(KEY_LABEL);
    info.extend_from_slice(&client.0);
    info.extend_from_slice(&server.0);
    let mut out = [0u8; 32];
    Hkdf::<Sha256>::new(None, secret)
        .expand(&info, &mut out)
        .expect("32 bytes is a valid HKDF output length");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer() -> PairingOffer {
        PairingOffer {
            fingerprint: Fingerprint([3; 32]),
            secret: [9; 32],
            addrs: vec![
                "192.168.1.20:47100".parse().unwrap(),
                "[fe80::1]:47100".parse().unwrap(),
            ],
            name: "Kubuntu laptop & co".into(),
        }
    }

    #[test]
    fn uri_roundtrip() {
        let o = offer();
        assert_eq!(PairingOffer::from_uri(&o.to_uri()).unwrap(), o);
    }

    #[test]
    fn uri_rejects_garbage() {
        assert!(PairingOffer::from_uri("https://evil.example").is_err());
        assert!(PairingOffer::from_uri("phonelink:pair?v=2&fp=00").is_err());
        let truncated = offer().to_uri().replace("&s=", "&x=");
        assert!(PairingOffer::from_uri(&truncated).is_err());
    }

    #[test]
    fn proofs_bind_both_identities() {
        let (s, c, srv, other) = (
            [1; 32],
            Fingerprint([2; 32]),
            Fingerprint([3; 32]),
            Fingerprint([4; 32]),
        );
        let p = client_proof(&s, &c, &srv);
        assert!(verify_client_proof(&s, &c, &srv, &p));
        assert!(
            !verify_client_proof(&s, &other, &srv, &p),
            "proof is bound to the phone's cert"
        );
        assert!(
            !verify_client_proof(&s, &c, &other, &p),
            "proof is bound to the laptop's cert"
        );
        assert!(!verify_client_proof(&[0; 32], &c, &srv, &p), "wrong secret");
        assert!(
            !verify_server_proof(&s, &c, &srv, &p),
            "client proof is not a server proof"
        );
        assert_eq!(derive_pair_key(&s, &c, &srv), derive_pair_key(&s, &c, &srv));
        assert_ne!(
            derive_pair_key(&s, &c, &srv),
            derive_pair_key(&s, &other, &srv)
        );
    }
}
