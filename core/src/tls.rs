//! Mutual TLS 1.3 (inside QUIC) with fingerprint pinning instead of certificate authorities.
//!
//! * Server side: a client certificate is mandatory. It must be pinned, or the user must
//!   currently have the pairing window open. Anything else fails *inside the handshake*,
//!   so no application code ever processes bytes from a stranger.
//! * Client side: we only accept the exact fingerprint we expect (pinned peer, or the one
//!   printed in the QR code), which also defeats man-in-the-middle during pairing.
//! * The peer must still prove possession of the private key (TLS 1.3 CertificateVerify),
//!   so copying someone's certificate is useless.

use std::{sync::Arc, time::Duration};

use quinn::{
    crypto::rustls::{QuicClientConfig, QuicServerConfig},
    IdleTimeout, TransportConfig,
};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{verify_tls13_signature, CryptoProvider, WebPkiSupportedAlgorithms},
    pki_types::{CertificateDer, ServerName, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
    CertificateError, DigitallySignedStruct, DistinguishedName, SignatureScheme,
};

use crate::{
    trust::{AccessGate, Admission},
    Error, Fingerprint, Identity, Result,
};

pub const ALPN: &[u8] = b"phonelink/1";
/// We never use DNS names; this constant only satisfies the API.
pub const SERVER_NAME: &str = "phonelink";

pub const KEEP_ALIVE: Duration = Duration::from_secs(4);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(20);

pub fn crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn rejected() -> rustls::Error {
    rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
}

fn tls12_refused() -> rustls::Error {
    rustls::Error::General("TLS 1.2 is not supported".into())
}

fn transport() -> Arc<TransportConfig> {
    let mut t = TransportConfig::default();
    t.keep_alive_interval(Some(KEEP_ALIVE));
    t.max_idle_timeout(Some(
        IdleTimeout::try_from(IDLE_TIMEOUT).expect("valid idle timeout"),
    ));
    Arc::new(t)
}

/// Client side: accept exactly one fingerprint.
#[derive(Debug)]
pub struct PinnedServerVerifier {
    expected: Fingerprint,
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if intermediates.is_empty() && Fingerprint::of(end_entity) == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rejected())
        }
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(tls12_refused())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// Server side: pinned peers always; unknown certs only while pairing is open.
#[derive(Debug)]
pub struct PinnedClientVerifier {
    gate: Arc<AccessGate>,
    algs: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for PinnedClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        if !intermediates.is_empty() {
            return Err(rejected());
        }
        match self.gate.admit(&Fingerprint::of(end_entity)) {
            Admission::Trusted | Admission::PairingOnly => Ok(ClientCertVerified::assertion()),
            Admission::Denied => {
                tracing::debug!("refused handshake from unpaired certificate");
                Err(rejected())
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(tls12_refused())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

pub fn server_config(
    identity: &Identity,
    gate: Arc<AccessGate>,
    provider: Arc<CryptoProvider>,
) -> Result<quinn::ServerConfig> {
    let verifier = Arc::new(PinnedClientVerifier {
        gate,
        algs: provider.signature_verification_algorithms,
    });
    let mut tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![identity.cert()], identity.key())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.max_early_data_size = 0; // no 0-RTT: it is replayable
    let crypto = QuicServerConfig::try_from(tls).map_err(|e| Error::Protocol(e.to_string()))?;
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    cfg.transport_config(transport());
    cfg.max_incoming(256);
    cfg.incoming_buffer_size(64 * 1024);
    cfg.incoming_buffer_size_total(4 * 1024 * 1024);
    Ok(cfg)
}

pub fn client_config(
    identity: &Identity,
    expected: Fingerprint,
    provider: Arc<CryptoProvider>,
) -> Result<quinn::ClientConfig> {
    let verifier = Arc::new(PinnedServerVerifier {
        expected,
        algs: provider.signature_verification_algorithms,
    });
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous() // "dangerous" = custom verifier; ours is stricter than CA validation
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![identity.cert()], identity.key())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.enable_early_data = false;
    let crypto = QuicClientConfig::try_from(tls).map_err(|e| Error::Protocol(e.to_string()))?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(crypto));
    cfg.transport_config(transport());
    Ok(cfg)
}

/// Fingerprint of the certificate the remote side authenticated with.
pub fn peer_fingerprint(conn: &quinn::Connection) -> Option<Fingerprint> {
    let any = conn.peer_identity()?;
    let certs = any.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    certs.first().map(|c| Fingerprint::of(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::{PeerRecord, TrustStore};

    fn verifier(gate: Arc<AccessGate>) -> PinnedClientVerifier {
        PinnedClientVerifier {
            gate,
            algs: crypto_provider().signature_verification_algorithms,
        }
    }

    #[test]
    fn server_refuses_unknown_client_certs_inside_the_handshake() {
        let gate = Arc::new(AccessGate::new(TrustStore::in_memory()));
        let v = verifier(gate.clone());
        let (friend, stranger) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        gate.trust()
            .upsert(PeerRecord {
                fingerprint: friend.fingerprint(),
                name: "friend".into(),
                pair_key: [0; 32],
                paired_at: 0,
                last_addrs: vec![],
            })
            .unwrap();
        let now = UnixTime::now();
        assert!(v.verify_client_cert(&friend.cert(), &[], now).is_ok());
        assert!(v.verify_client_cert(&stranger.cert(), &[], now).is_err());
        assert!(
            v.verify_client_cert(&friend.cert(), &[stranger.cert()], now)
                .is_err(),
            "no chains"
        );

        gate.open_pairing(Duration::from_secs(10));
        assert!(
            v.verify_client_cert(&stranger.cert(), &[], now).is_ok(),
            "may attempt pairing"
        );
        gate.close_pairing();
        assert!(v.verify_client_cert(&stranger.cert(), &[], now).is_err());
    }

    #[test]
    fn client_accepts_only_the_expected_server() {
        let (laptop, impostor) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let v = PinnedServerVerifier {
            expected: laptop.fingerprint(),
            algs: crypto_provider().signature_verification_algorithms,
        };
        let name = ServerName::try_from(SERVER_NAME).unwrap();
        let now = UnixTime::now();
        assert!(v
            .verify_server_cert(&laptop.cert(), &[], &name, &[], now)
            .is_ok());
        assert!(v
            .verify_server_cert(&impostor.cert(), &[], &name, &[], now)
            .is_err());
    }
}
