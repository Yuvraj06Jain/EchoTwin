# PhoneLink

A private, Phone Link–style bridge between a Linux laptop and an Android phone.
Only devices you pair by scanning a QR code on your own screen can ever connect.
No accounts, no cloud, LAN only (v1).

## Status

| Milestone | State |
|---|---|
| 1. Core: identity, QR pairing, mutual-TLS QUIC, framing, capabilities, durable outbox | **done** |
| 2a. Private LAN discovery (mDNS), parallel dialing, instant reconnect | **done** |
| 2b. Linux daemon `phonelinkd` + `phonelink` CLI | **done** |
| 2c/d. Android app (Rust core via UniFFI, QR scan, foreground service) | in progress |
| 3. Resumable file transfer, clipboard | |
| 4. Notification mirroring + replies | |
| 5. SMS | |
| 6. Desktop UI (Tauri), packaging (Flatpak, .deb, AUR) | |

## Layout

```
core/             Rust crate shared by the Linux daemon and (via UniFFI) the Android app
desktop/daemon/   phonelinkd (systemd user service) + phonelink CLI
packaging/        systemd unit, install/uninstall scripts
```

## Install on Linux (Kubuntu, Arch, Fedora, ... any systemd distro)

1. Install the build tools.
   Kubuntu/Ubuntu:
   ```bash
   sudo apt install build-essential curl git
   ```
   Arch:
   ```bash
   sudo pacman -S base-devel git
   ```
2. Install Rust (current stable; distro packages are often too old):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
3. Build and install the daemon for your user (no root):
   ```bash
   ./packaging/install.sh
   ```
4. Pair your phone:
   ```bash
   phonelink pair
   ```

Everyday commands:

```bash
phonelink status              # this computer's id + paired devices
phonelink devices
phonelink send-text hello     # queued if the phone is offline
phonelink watch               # live connections and messages
phonelink unpair <device>     # id, fingerprint prefix or name
journalctl --user -u phonelinkd -f   # daemon logs
```

Optional config: `~/.config/phonelink/config.toml`

```toml
port = 47100                 # UDP
device_name = "My Laptop"    # default: hostname
notifications = true
discovery = true             # anonymous mDNS so the phone finds you after IP changes
```

### Firewall

Kubuntu ships `ufw` disabled. If you enabled it, allow PhoneLink from your local network only:

```bash
sudo ufw allow from 192.168.0.0/16 to any port 47100 proto udp
```

```bash
sudo ufw allow from 192.168.0.0/16 to any port 5353 proto udp   # mDNS discovery
```

firewalld (Fedora, and Arch if you use it):

```bash
sudo firewall-cmd --permanent --add-port=47100/udp
```

```bash
sudo firewall-cmd --permanent --add-service=mdns
```

```bash
sudo firewall-cmd --reload
```

Pairing is still required even with the port open; the firewall only adds another layer.

### College / office Wi-Fi

Many campus networks block devices from talking to each other ("client isolation").
If `phonelink devices` shows the phone as offline while both are on the same Wi-Fi, turn on
the phone's hotspot and connect the laptop to it. That's also the most private setup.

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
* **Discovery privacy**: the mDNS advertisement uses a random instance name and a random
  host name, both re-rolled every 10 minutes, plus `HMAC(pair_key, window)` tokens.
  It never includes your hostname, device name or a stable ID.
* **Local control**: the daemon's socket is mode 0600 and also checks the caller's uid
  (`SO_PEERCRED`). Keys, peers and the message store are owner-only.
* **Reliability**: every message is written to an SQLite outbox before sending and removed
  only on `Ack`, with per-peer de-duplication on receive. Delivery is exactly-once and
  in-order across disconnects and restarts. `Node::maintain` reconnects with jittered backoff.

## Develop

```bash
cargo test            # unit + integration + attacker tests
RUST_LOG=debug cargo test -- --nocapture
```
