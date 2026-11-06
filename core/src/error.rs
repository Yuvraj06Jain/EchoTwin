use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("tls: {0}")]
    Tls(#[from] rustls::Error),
    #[error("certificate generation: {0}")]
    Cert(#[from] rcgen::Error),
    #[error("pem: {0}")]
    Pem(#[from] rustls::pki_types::pem::Error),
    #[error("storage: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("encoding: {0}")]
    Encode(#[from] postcard::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("connect: {0}")]
    Connect(#[from] quinn::ConnectError),
    #[error("connection: {0}")]
    Connection(#[from] quinn::ConnectionError),
    #[error("invalid pairing code: {0}")]
    BadOffer(String),
    #[error("pairing rejected: {0}")]
    PairingRejected(String),
    #[error("peer is not paired")]
    NotPaired,
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("timed out")]
    Timeout,
    /// The peer is only reachable over the Bluetooth backup, which carries no file data.
    #[error("waiting for Wi-Fi")]
    NoNetworkLink,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
