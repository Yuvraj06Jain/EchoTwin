//! Framing on QUIC streams: `[u32 big-endian length][postcard payload]`.
//!
//! Compatibility rules (append-only):
//! * New `Control` variants are only ever appended. An older peer that receives an
//!   unknown variant gets [`Frame::Unknown`] and skips it; the length prefix keeps the
//!   stream in sync.
//! * New fields are only appended to the end of a variant; postcard ignores trailing bytes,
//!   so older peers decode the prefix they know.
//! * Features are gated by capability strings exchanged in `Hello`.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MIN_PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_LEN: usize = 1024 * 1024;
/// Largest application body that fits in a `Deliver` frame.
pub const MAX_BODY_LEN: usize = MAX_FRAME_LEN - 128;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MsgId(pub [u8; 16]);

impl MsgId {
    pub fn random() -> Self {
        Self(rand::random())
    }
}

impl fmt::Debug for MsgId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MsgId({})", hex::encode(self.0))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Control {
    Hello {
        version: u16,
        name: String,
        capabilities: Vec<String>,
    },
    /// Application message that must survive disconnects; answered by `Ack`.
    Deliver {
        id: MsgId,
        body: Vec<u8>,
    },
    Ack {
        id: MsgId,
    },
    PairRequest {
        name: String,
        proof: [u8; 32],
    },
    PairResponse {
        name: String,
        proof: [u8; 32],
    },
    Reject {
        reason: String,
    },
}

#[derive(Debug, PartialEq)]
pub enum Frame {
    Known(Control),
    /// Well-framed but not understood (sent by a newer version). Safe to skip.
    Unknown,
}

pub fn encode(msg: &Control) -> Result<Vec<u8>> {
    let payload = postcard::to_stdvec(msg)?;
    if payload.len() > MAX_FRAME_LEN {
        return Err(Error::Protocol("frame too large".into()));
    }
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

pub fn decode(payload: &[u8]) -> Frame {
    match postcard::from_bytes(payload) {
        Ok(c) => Frame::Known(c),
        Err(_) => Frame::Unknown,
    }
}

/// `Ok(None)` on a clean end of stream at a frame boundary.
pub async fn read_frame(recv: &mut quinn::RecvStream) -> Result<Option<Frame>> {
    let mut len = [0u8; 4];
    match recv.read_exact(&mut len).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME_LEN {
        return Err(Error::Protocol(format!("frame of {n} bytes exceeds limit")));
    }
    let mut buf = vec![0u8; n];
    recv.read_exact(&mut buf).await?;
    Ok(Some(decode(&buf)))
}

pub async fn write_frame(send: &mut quinn::SendStream, msg: &Control) -> Result<()> {
    send.write_all(&encode(msg)?).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let m = Control::Deliver {
            id: MsgId::random(),
            body: b"hi".to_vec(),
        };
        let bytes = encode(&m).unwrap();
        assert_eq!(
            u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize,
            bytes.len() - 4
        );
        assert_eq!(decode(&bytes[4..]), Frame::Known(m));
    }

    #[test]
    fn unknown_variant_from_newer_peer_is_skippable() {
        // Variant index 200 does not exist in this version.
        let future = [200u8, 1, 2, 3];
        assert_eq!(decode(&future), Frame::Unknown);
    }

    #[test]
    fn appended_fields_are_ignored_by_older_decoders() {
        let mut bytes = postcard::to_stdvec(&Control::Ack { id: MsgId([7; 16]) }).unwrap();
        bytes.extend_from_slice(&[42, 42, 42]); // a field a future version appended
        assert_eq!(
            decode(&bytes),
            Frame::Known(Control::Ack { id: MsgId([7; 16]) })
        );
    }
}
