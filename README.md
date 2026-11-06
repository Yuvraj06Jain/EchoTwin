# PhoneLink

A private, Phone Link–style bridge between a Linux laptop and an Android phone.
Only devices you pair by scanning a QR code on your own screen can ever connect.
No accounts, no cloud, LAN only (v1).

**→ New here? Read [SETUP.md](SETUP.md): step-by-step setup for laptop and phone, a test checklist, and troubleshooting.**

## Status

| Milestone | State |
|---|---|
| 1. Core: identity, QR pairing, mutual-TLS QUIC, framing, capabilities, durable outbox | **done** |
| 2a. Private LAN discovery (mDNS), parallel dialing, instant reconnect | **done** |
| 2b. Linux daemon `phonelinkd` + `phonelink` CLI | **done** |
| 2c. Rust ↔ Kotlin bridge (UniFFI), tested on JVM against the real daemon | **done** |
| 2d. Android app: QR pairing, foreground service, discovery, test messages | **built** by GitHub Actions (APK in Releases); not yet tested on a real phone |
| 6a. Desktop app (Tauri): pairing, connection status, notes, settings; dark theme | **done**; tested against the real service with a simulated phone |
| 4. Phone notifications on the laptop: popups, list, reply, buttons, dismiss (sync optional), per-app filter | **done**; laptop side tested end to end, phone side compiled in CI, needs your real-phone test |
| Reconnect after a reboot / new IP (gateway + subnet search, stable phone port) | **done** |
| Click a popup to open the window | **done** |
| Lock the laptop from the phone; shared clipboard (KDE) | **done**; laptop side tested, phone side compiled in CI |
| Music controls for the phone | next |
| Resumable file transfer | planned |
| SMS | planned |
| Bluetooth backup link | core done; phone and laptop parts planned |
| Packaging (Flatpak, .deb, AUR) | |

## Layout

```
core/             Rust crate shared by the Linux daemon and (via UniFFI) the Android app
core-ffi/         UniFFI bindings for Android (+ kotlin-test: JVM end-to-end test)
desktop/daemon/   phonelinkd (systemd user service) + phonelink CLI
desktop/app/      PhoneLink desktop app (Tauri window; UI from desktop/ui, embedded)
desktop/ui/       the app's HTML/CSS/JS; open index.html in a browser for a sample-data preview
android/          Kotlin/Compose app; the generated bindings live in dev/phonelink/core
packaging/        systemd unit, install/uninstall scripts
scripts/          Android build, bindings generation, Kotlin end-to-end test
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
phonelink reconnect           # look for the phone right now
phonelink notifications       # phone notifications mirrored here
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

## Get the Android app (no build needed)

Every change to the app is built automatically on GitHub.
1. On your phone, open **https://github.com/Yuvraj06Jain/Phone-Link/releases/tag/latest-apk**
   and download `PhoneLink-xxxxxxx.apk`.
2. Open the download. When Android asks, allow **Install unknown apps** for your browser,
   then tap **Install**.
3. Open PhoneLink. Then:
   - Allow notifications.
   - Tap **Allow background use**.
   - Add PhoneLink under Settings → Battery → Background usage limits → **Never sleeping apps**.
4. On the laptop run `phonelink pair`. On the phone tap **Pair a computer**, scan the code,
   and check both screens show the same id.

Updates install over the old app and keep your pairing, provided the repository has the
signing-key secrets set (see `.github/workflows/android.yml`).

## Build and install the Android app yourself

1. On the laptop, install Android Studio. Then open
   Settings → Languages & Frameworks → Android SDK → SDK Tools and tick
   **NDK (Side by side)**.
2. Build the Rust core and the APK:
   ```bash
   ./scripts/build-android.sh
   ```
3. On the phone, enable USB debugging (Settings → About phone → Software information →
   tap *Build number* 7 times, then Developer options → USB debugging). Connect it and run:
   ```bash
   adb install -r android/app/build/outputs/apk/debug/app-debug.apk
   ```
4. Open PhoneLink on the phone. Then:
   - Allow notifications.
   - Tap **Allow background use**.
   - Add PhoneLink under Settings → Battery → Background usage limits → **Never sleeping apps**.
5. Run `phonelink pair` on the laptop, tap **Pair a computer** on the phone, scan the code,
   and check that both screens show the same id.

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
cargo test                           # unit, integration, attacker and daemon tests
cargo test --test discovery -- --ignored   # live mDNS (needs multicast)
./scripts/test-kotlin-bindings.sh    # Kotlin bindings on JVM <-> real phonelinkd
./scripts/gen-kotlin-bindings.sh     # after changing core-ffi/src/lib.rs
```
