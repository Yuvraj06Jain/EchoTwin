//! Framing on byte streams (QUIC streams, or TLS over Bluetooth): `[u32 big-endian length][postcard payload]`.
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
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

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
    /// Keep-alive on byte-stream links (QUIC has its own). Ignored on receipt.
    Ping,
    /// The sender's other ways to be reached, sent after `Hello` (laptop → phone).
    /// `bluetooth`: the adapter address ("AA:BB:CC:DD:EE:FF") for the backup link, or
    /// `None` when the backup is switched off.
    Links {
        bluetooth: Option<String>,
    },
    /// First frame on a file data stream (sender → receiver). Then, after `FileResume`,
    /// the sender writes the file's bytes from `offset` to `size` raw, followed by the
    /// 32-byte BLAKE3 hash of the whole file.
    FileStart {
        id: [u8; 16],
        name: String,
        size: u64,
        mime: String,
    },
    /// Receiver → sender: "I already have the first `offset` bytes; send the rest."
    FileResume {
        offset: u64,
    },
    /// Receiver → sender, after the hash: stored (`ok`), or start over / give up.
    FileResult {
        ok: bool,
        message: String,
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
///
/// Works on any byte stream: a QUIC stream, or TLS over a Bluetooth socket.
pub async fn read_frame<R: AsyncRead + Unpin + ?Sized>(recv: &mut R) -> Result<Option<Frame>> {
    let mut len = [0u8; 4];
    // The first byte decides between "clean end" and "cut off mid-frame".
    if recv.read(&mut len[..1]).await? == 0 {
        return Ok(None);
    }
    recv.read_exact(&mut len[1..]).await?;
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME_LEN {
        return Err(Error::Protocol(format!("frame of {n} bytes exceeds limit")));
    }
    let mut buf = vec![0u8; n];
    recv.read_exact(&mut buf).await?;
    Ok(Some(decode(&buf)))
}

/// Writes one frame and flushes it (TLS streams buffer until flushed).
pub async fn write_frame<W: AsyncWrite + Unpin + ?Sized>(send: &mut W, msg: &Control) -> Result<()> {
    send.write_all(&encode(msg)?).await?;
    send.flush().await?;
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

    #[tokio::test]
    async fn frames_over_a_byte_stream_with_clean_and_dirty_ends() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let msgs = [
            Control::Ping,
            Control::Links {
                bluetooth: Some("AA:BB:CC:DD:EE:FF".into()),
            },
            Control::Ack { id: MsgId([1; 16]) },
        ];
        let sent = msgs.clone();
        let w = tokio::spawn(async move {
            for m in &sent {
                write_frame(&mut a, m).await.unwrap();
            }
            // dropping `a` = clean end at a frame boundary
        });
        for m in msgs {
            assert_eq!(read_frame(&mut b).await.unwrap(), Some(Frame::Known(m)));
        }
        w.await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), None);

        // Cut off in the middle of a frame: an error, not a clean end.
        let (mut a, mut b) = tokio::io::duplex(64);
        let bytes = encode(&Control::Ping).unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut a, &bytes[..bytes.len() - 1])
            .await
            .unwrap();
        drop(a);
        assert!(read_frame(&mut b).await.is_err());
    }
}
