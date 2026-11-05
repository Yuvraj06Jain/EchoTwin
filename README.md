# PhoneLink

A private, Phone Link–style bridge between a Linux laptop and an Android phone.
Only devices you pair by scanning a QR code on your own screen can ever connect.
No accounts, no cloud, LAN only (v1).

## Status

| Milestone | State |
|---|---|
| 1. Core: identity, QR pairing, mutual-TLS QUIC, framing, capabilities, durable outbox | **done** |
| 2. Linux daemon + CLI; Android skeleton (foreground service, QR scan) | next |
| 3. Resumable file transfer, clipboard | |
| 4. Notification mirroring + replies | |
| 5. SMS | |
| 6. Desktop UI (Tauri), packaging (Flatpak, .deb, AUR) | |

## Layout

```
core/   Rust crate shared by the Linux daemon and (via UniFFI) the Android app
```

Because both apps run the *same* Rust core, messages are plain Rust types encoded with
`postcard`. No separate `.proto` schema or `protoc` toolchain is needed.
Compatibility rules for future versions are documented in `core/src/wire.rs`.

## Security model (core)

* Each device has an Ed25519 key and a self-signed cert. Peers are pinned by SHA-256 fingerprint.
* **Pairing**: the laptop opens a 2-minute, single-use window and shows a QR code containing
  its fingerprint, addresses and a 256-bit secret. The phone verifies the laptop's fingerprint
  in TLS (defeats MITM). Each side proves knowledge of the secret with an HMAC bound to both
  fingerprints, and both derive a shared `pair_key` with HKDF.
* **Two independent gates** keep strangers out: the TLS client-certificate verifier refuses
  unpinned certs during the handshake, and the session layer refuses `Hello` from untrusted peers.
  Tests check that either gate alone is sufficient.
* **Flood resistance**: QUIC Retry (address validation), per-IP handshake rate limit, cap on
  concurrent handshakes. Established sessions are unaffected.
* **Discovery privacy**: the planned mDNS advertisement carries only
  `HMAC(pair_key, 10-min window)`. It never includes a device name or a stable ID.
* **Reliability**: every message is written to an SQLite outbox before sending and removed
  only on `Ack`, with per-peer de-duplication on receive. Delivery is exactly-once and
  in-order across disconnects and restarts. `Node::maintain` reconnects with jittered backoff.

## Develop

```bash
cargo test            # unit + integration + attacker tests
RUST_LOG=debug cargo test -- --nocapture
```
