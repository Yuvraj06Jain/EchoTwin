//! Long-term device identity.
//!
//! Every device owns one Ed25519 key and a self-signed certificate. Nothing about the
//! device (name, model, user) is put in the certificate. Peers are identified purely by
//! the SHA-256 fingerprint of the certificate, which is what gets pinned at pairing time.

use std::{fmt, path::Path};

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ED25519};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::{util, Error, Result};

const CERT_FILE: &str = "identity.cert.pem";
const KEY_FILE: &str = "identity.key.pem";

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    pub fn of(cert_der: &[u8]) -> Self {
        Self(Sha256::digest(cert_der).into())
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_hex(s: &str) -> Result<Self> {
        let v =
            hex::decode(s.trim()).map_err(|_| Error::Protocol("fingerprint is not hex".into()))?;
        let arr: [u8; 32] = v
            .try_into()
            .map_err(|_| Error::Protocol("fingerprint must be 32 bytes".into()))?;
        Ok(Self(arr))
    }

    /// Short human-comparable form, e.g. `3F2A-91C0-77DE-0B14`.
    pub fn short(&self) -> String {
        self.0[..8]
            .chunks(2)
            .map(hex::encode_upper)
            .collect::<Vec<_>>()
            .join("-")
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({})", self.short())
    }
}

impl Serialize for Fingerprint {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Fingerprint {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Fingerprint::from_hex(&s).map_err(D::Error::custom)
    }
}

pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
    fingerprint: Fingerprint,
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl Identity {
    /// Create a brand-new random identity (not persisted).
    pub fn generate() -> Result<Self> {
        let key = KeyPair::generate_for(&PKCS_ED25519)?;
        let mut params = CertificateParams::new(vec!["phonelink".to_string()])?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "phonelink");
        params.distinguished_name = dn;
        let cert = params.self_signed(&key)?;
        Ok(Self::from_parts(
            cert.der().clone(),
            PrivatePkcs8KeyDer::from(key.serialize_der()),
        ))
    }

    /// Load the identity stored in `dir`, creating and saving a new one on first run.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let (cert_path, key_path) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
        if cert_path.exists() && key_path.exists() {
            let cert = CertificateDer::from_pem_file(&cert_path)?;
            let key = PrivatePkcs8KeyDer::from_pem_file(&key_path)?;
            return Ok(Self::from_parts(cert, key));
        }
        let id = Self::generate()?;
        id.save(dir)?;
        Ok(id)
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        // Key first: if we crash in between, load_or_create sees no cert and regenerates.
        util::write_atomic(
            &dir.join(KEY_FILE),
            util::pem_encode("PRIVATE KEY", self.key.secret_pkcs8_der()).as_bytes(),
            true,
        )?;
        util::write_atomic(
            &dir.join(CERT_FILE),
            util::pem_encode("CERTIFICATE", &self.cert).as_bytes(),
            false,
        )?;
        Ok(())
    }

    fn from_parts(cert: CertificateDer<'static>, key: PrivatePkcs8KeyDer<'static>) -> Self {
        let fingerprint = Fingerprint::of(&cert);
        Self {
            cert,
            key,
            fingerprint,
        }
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    pub fn cert(&self) -> CertificateDer<'static> {
        self.cert.clone()
    }

    pub fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(self.key.clone_key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_persists_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let a = Identity::load_or_create(dir.path()).unwrap();
        let b = Identity::load_or_create(dir.path()).unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_ne!(a.fingerprint(), Identity::generate().unwrap().fingerprint());
    }

    #[cfg(unix)]
    #[test]
    fn private_key_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        Identity::load_or_create(dir.path()).unwrap();
        let mode = std::fs::metadata(dir.path().join(KEY_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn fingerprint_hex_roundtrip() {
        let fp = Identity::generate().unwrap().fingerprint();
        assert_eq!(Fingerprint::from_hex(&fp.to_hex()).unwrap(), fp);
        assert_eq!(fp.short().len(), 19);
    }
}
