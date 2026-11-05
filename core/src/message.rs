//! Application-level messages carried in the durable `Deliver` channel.
//!
//! Same compatibility rules as `wire.rs`: variants and fields are append-only, and an
//! older build turns unknown variants into [`AppMessage::Unsupported`] instead of failing.

use serde::{Deserialize, Serialize};

use crate::Result;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppMessage {
    /// A plain text note (also used as a connectivity test).
    Text { text: String },
    /// Placeholder for whatever a newer version sent that we don't understand.
    /// Never sent on the wire by this version.
    Unsupported,
}

impl AppMessage {
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(postcard::to_stdvec(self)?)
    }

    pub fn decode(bytes: &[u8]) -> Self {
        postcard::from_bytes(bytes).unwrap_or(AppMessage::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_forward_compat() {
        let m = AppMessage::Text { text: "hi".into() };
        assert_eq!(AppMessage::decode(&m.encode().unwrap()), m);
        assert_eq!(AppMessage::decode(&[250, 1, 2]), AppMessage::Unsupported);
    }
}
