# File map

Every file in the repository and what it is for. Use this to find *where* something lives;
[FEATURES.md](FEATURES.md) explains *how* each feature works across these files.

Contents
- [Top level](#top-level)
- [core/ (shared Rust library)](#core-shared-rust-library)
- [core-ffi/ (Android bridge)](#core-ffi-android-bridge)
- [desktop/daemon/ (phonelinkd + phonelink CLI)](#desktopdaemon-phonelinkd--phonelink-cli)
- [desktop/app/ (Tauri window)](#desktopapp-tauri-window)
- [desktop/ui/ (window's HTML/CSS/JS)](#desktopui-windows-htmlcssjs)
- [android/ (phone app)](#android-phone-app)
- [packaging/ (install, KDE widget, menus)](#packaging-install-kde-widget-menus)
- [scripts/ and CI](#scripts-and-ci)

```
phonelink/
├── core/                 Rust: identity, pairing, TLS, QUIC sessions, outbox, discovery, files
├── core-ffi/             Rust → Kotlin bindings (UniFFI) used by the Android app
├── desktop/
│   ├── daemon/           phonelinkd (background service) + phonelink (CLI), Rust
│   ├── app/              PhoneLink window (Tauri 2), Rust shell around desktop/ui
│   └── ui/               the window's HTML/CSS/JS (no framework, no build step)
├── android/              phone app (Kotlin, Jetpack Compose)
├── packaging/            install.sh, systemd unit, KDE widget (plasmoid), Dolphin menu
├── scripts/              Android build, bindings generation, APK permission audit
├── .github/workflows/    CI that builds the APK and publishes the latest-apk release
└── docs/                 these documents
```

---

## Top level

| File | What it does |
|---|---|
| `Cargo.toml` | Rust workspace: `core`, `core-ffi`, `desktop/daemon`, `desktop/app`. Shared version/edition/license. |
| `Cargo.lock` | Exact Rust dependency versions (committed so builds are reproducible). |
| `.cargo/config.toml` | Android builds: align native code to 16 KB pages so it loads on Android 15+ devices. |
| `.gitignore` | Keeps build output (`target/`, Android `build/`, `jniLibs/`), SQLite files and keystores out of git. |
| `README.md` | Repo front page: what EchoTwin is, security in brief, features, screenshots. |
| `LICENSE` | GPL-3.0 license text. |
| `SETUP.md` | Install guide for the laptop and phone, the numbered device test checklist (rows 1–55), troubleshooting. |
| `docs/` | File map (this file), features, protocol, developer guide; `docs/img/` screenshots. |

## core/ (shared Rust library)

The same code runs on the laptop (inside `phonelinkd`) and on the phone (inside the APK via
`core-ffi`). Crate name `phonelink-core`.

| File | What it does | Key items |
|---|---|---|
| `core/Cargo.toml` | Dependencies: quinn (QUIC), rustls (TLS 1.3), rcgen/ed25519, rusqlite, mdns-sd, blake3, postcard, tokio. | |
| `core/src/lib.rs` | Module list and the public re-exports used by the other crates. | `AppMessage`, `Node`, `NodeConfig`, `Event`, `CAP_*` |
| `core/src/identity.rs` | This device's long-term Ed25519 key + self-signed certificate; peers are identified by the SHA-256 fingerprint of the certificate. Files `identity.cert.pem` / `identity.key.pem`. | `Identity::load_or_create`, `Fingerprint::{of, short, to_hex}` |
| `core/src/pairing.rs` | QR pairing: the offer in the QR code (`phonelink://pair?...`), HMAC proofs both ways, HKDF-derived `pair_key`. | `PairingOffer::{to_uri, from_uri}`, `client_proof`, `server_proof`, `derive_pair_key` |
| `core/src/trust.rs` | Who may connect: pinned peers (`peers.json`, with last addresses and Bluetooth address) and the pairing window (one-time secret, TTL). | `TrustStore`, `PeerRecord`, `AccessGate::{admit, open_pairing, consume_pairing}` |
| `core/src/tls.rs` | Mutual TLS 1.3 with fingerprint pinning instead of certificate authorities. QUIC config (ALPN `phonelink/1`) and byte-stream config for Bluetooth (ALPN `phonelink-stream/1`, pairing never allowed). Keep-alive 4 s, idle timeout 20 s. | `PinnedServerVerifier`, `PinnedClientVerifier`, `server_config`, `client_config`, `stream_server_config`, `stream_client_config` |
| `core/src/wire.rs` | Framing `[u32 length][postcard]` on any byte stream; the `Control` enum (Hello, Deliver, Ack, pairing, Ping, Links, file stream frames). Append-only. | `Control`, `Frame`, `MsgId`, `read_frame`, `write_frame`, `MAX_FRAME_LEN` |
| `core/src/message.rs` | Application messages carried inside `Deliver` (notifications, SMS, media, clipboard, lock, status, …), capability strings, size limits, freshness rules. Append-only. | `AppMessage`, `PhoneNotification`, `NowPlaying`, `SmsThread`, `PhoneStatus`, `CAP_*`, `is_fresh`, `truncate_text` |
| `core/src/node.rs` | The heart: one QUIC endpoint that listens and dials, sessions (QUIC or Bluetooth stream), Hello/capabilities, durable delivery with acks, reconnect loop with backoff, address probing, pairing on the wire, data streams for files. | `Node::{start, send, maintain, nudge, pair_with, start_pairing, connect_stream, accept_stream, open_data, data_streams, peer_supports, transport, unpair, shutdown}`, `Event`, `Transport` |
| `core/src/outbox.rs` | SQLite (WAL) durable outbox and duplicate filter: messages are stored before sending, deleted on ack, re-sent after reconnect; pending kept 7 days, seen-IDs 30 days. File `outbox.sqlite3`. | `Outbox::{enqueue, pending, ack, first_seen, prune_pending}` |
| `core/src/transfer.rs` | Resumable file transfers over separate QUIC streams: `.part` files, resume offset, BLAKE3 check, cancel both ways, safe file names, free-space reserve, state in `transfers.sqlite3`. | `Transfers::{start, send_file, cancel, on_message, list, nudge}`, `SourceOpener`, `PathOpener`, `safe_name`, `CAP_FILES` |
| `core/src/discovery.rs` | Rotating anonymous tokens `HMAC(pair_key, 10-min window)` so the LAN never sees a name or stable ID. | `token_at`, `matches` |
| `core/src/lan.rs` | mDNS/DNS-SD advertise + browse (`_phonelink._udp`), random instance/host names re-rolled every 10 min. | `Lan::{new, advertise, browse}`, `build_advertisement`, `recognise` |
| `core/src/probe.rs` | Finding a peer whose address changed without multicast: default gateway (phone hotspot) and own /24. | `default_gateways`, `parse_route_table`, `local_subnets`, `subnet_candidates` |
| `core/src/limiter.rs` | Handshake flood protection: 20 handshakes per IP per 10 s, at most 64 in flight. | `HandshakeLimiter` |
| `core/src/error.rs` | The crate's error type. | `Error`, `Result` |
| `core/src/util.rs` | Small helpers (serde for byte arrays, time). | |
| `core/examples/fake_phone.rs` | A stand-in phone for testing the laptop without Android: `cargo run -p phonelink-core --example fake_phone -- '<pairing URI>'`. | |
| `core/tests/common/mod.rs` | Test helpers: start nodes on localhost, pair them. | |
| `core/tests/sessions.rs` | Pairing, messaging both ways, offline queue survives restarts (exactly once, in order), reconnect, simultaneous dials, unpair. | |
| `core/tests/attacker.rs` | A stranger on the network: unpaired certs refused, no pairing without the QR secret, single-use QR, expired window, impostor laptop, handshake flood. | |
| `core/tests/discovery.rs` | Laptop moves to a new address: address hints, live mDNS, subnet search (and that it fails without it). | |
| `core/tests/stream.rs` | Bluetooth backup over an in-memory pipe: same pinned TLS, failover and back, strangers refused, malformed Bluetooth addresses ignored. | |
| `core/tests/files.rs` | File transfers: integrity, resume after cuts and restarts, cancel both sides, damaged data, Bluetooth-only waits, full disk, hostile names. | |

## core-ffi/ (Android bridge)

| File | What it does |
|---|---|
| `core-ffi/Cargo.toml` | Crate `phonelink-ffi` (cdylib for Android). Feature `bindgen` builds the bindings generator. |
| `core-ffi/uniffi.toml` | UniFFI settings (Kotlin package `dev.phonelink.core`). |
| `core-ffi/src/lib.rs` | The Kotlin-facing API. Object `PhoneLink` (`start`, `pair`, `devices`, `send_text`, `post_notification`, `set_queue_notifications`, `send_snapshot`, `lock_computer`, `send_clipboard`, `send_now_playing`, `send_media_art`, `send_file`, `cancel_transfer`, `sms_*`, `send_phone_status`, `open_stream`, `on_network_changed`, `shutdown`, …). Records (`Device`, `MirroredNotification`, `PhoneMedia`, `PhoneSms`, `PhoneSmsThread`, `FileTransfer`, `PairingCode`). `LinkEvent` enum delivered to Kotlin. Callback traits `EventListener`, `FileSource`, `StreamSink`; object `StreamPipe`. Stable phone port in `port.txt`. Contains the FFI tests (`mod tests`). |
| `core-ffi/src/bin/uniffi-bindgen.rs` | The generator binary used by `scripts/gen-kotlin-bindings.sh`. |
| `core-ffi/kotlin-test/…` | A JVM test (`EndToEndTest.kt`) that calls the generated Kotlin bindings against the host-built library (`scripts/test-kotlin-bindings.sh`). |

## desktop/daemon/ (phonelinkd + phonelink CLI)

Crate `phonelink-desktop`. `phonelinkd` owns the `Node`; everything else (CLI, window,
widget) talks to it over a private Unix socket with line-delimited JSON.

| File | What it does | Key items |
|---|---|---|
| `src/bin/phonelinkd.rs` | Service entry point: loads config, starts `daemon::run`, stops on SIGTERM/Ctrl-C. | `main` |
| `src/bin/phonelink.rs` | The CLI (`phonelink status/devices/pair/unpair/send-text/watch/notifications/reconnect/send-file/transfers/cancel-transfer/media/widget/dismiss/sms-read`, `--json`). `widget` prints the one JSON object the KDE widget polls. | `Cmd`, `run`, `widget_state`, `print_event` |
| `src/lib.rs` | Module list. | |
| `src/daemon.rs` | The service: `State`, node start (capabilities), event pump (every incoming `AppMessage` handled here), IPC request handler, background tasks (network watch, clipboard, transfers, Bluetooth backup, battery). | `run`, `event_pump`, `handle_client`, `State::{send_files, send_sms, lock_requested, maybe_popup, send_battery, connected_with}`, `battery_watch`, `bluetooth_backup`, `network_watch`, `share_copies`, `transfer_updates` |
| `src/ipc.rs` | The local protocol types: `Request` (what clients can ask), `Reply`, `WatchEvent` (pushed to `Watch` subscribers), `DeviceInfo`, `TransferView`. | |
| `src/client.rs` | Client side of the socket (used by CLI, window, tests). | `Client::{connect, request, send, next}` |
| `src/config.rs` | `~/.config/phonelink/config.toml` (all keys optional). | `Config` |
| `src/paths.rs` | XDG paths, each overridable by env var for tests (`PHONELINK_SOCKET`, …). | `data_dir`, `config_file`, `download_dir`, `socket_path` |
| `src/notifications.rs` | In-memory store of mirrored notifications (max 200), dismiss-sync logic, app icons cache, markup escaping. | `Store::{post, removed_on_phone, snapshot, dismiss, clear}`, `save_icon`, `POPUP_MAX_AGE_MS` |
| `src/popups.rs` | Desktop popups over the freedesktop notification D-Bus API on one connection/thread; clicking opens the window at the right tab (activation token passed through). | `Popups`, `PopupCmd`, `Tab`, `open_app`, `session_env` |
| `src/lock.rs` | Lock the screen: `org.freedesktop.ScreenSaver.Lock`, then logind `LockSession`. | `lock_screen`, `Buses` |
| `src/clipboard.rs` | KDE clipboard via Klipper D-Bus: watch copies, set text, never echo back. | `ClipboardSync::{start, set}` |
| `src/media.rs` | What plays on each phone (memory only), position extrapolation, album art. | `MediaStore::{update, set_art, view}`, `MediaInfo` |
| `src/sms.rs` | SMS conversations from the phone (memory only), pending sends, read marks. | `SmsStore`, `SmsThreadView`, `thread_key` |
| `src/bluetooth.rs` | Registers the PhoneLink RFCOMM profile with BlueZ (`ProfileManager1`), gets each connection as a socket fd, reports the adapter address. UUID `7a1d5c3e-9b4f-4e8a-a2c6-5f0e3d1b8c94`. | `register`, `Registered`, `PROFILE_UUID` |
| `src/battery.rs` | This computer's battery from `/sys/class/power_supply` (ignores mouse/keyboard batteries). | `read` |
| `src/netaddrs.rs` | Which LAN addresses go into the QR code; detects address changes to nudge reconnects. | `lan_addrs`, `NetWatch` |
| `src/open.rs` | Open a received file or show it in the file manager (`FileManager1.ShowItems`), MIME guess. | `open_file`, `reveal_file`, `guess_mime` |
| `tests/daemon.rs` | Real daemon over its socket with a simulated phone: pairing, messages, CLI errors. | |
| `tests/notifications.rs` | Notification flow end to end (post, update, remove, snapshot, reply/action/dismiss, mute). | |
| `tests/popups.rs` | Popups against a stand-in notification server on a private D-Bus (replace, close, click). | |
| `tests/lock_clipboard.rs` | Lock and clipboard against stand-in ScreenSaver/logind/Klipper services. | |
| `tests/media.rs` | Now playing + controls with a simulated phone. | |
| `tests/files.rs` | File transfers through the service, refusal reasons, cancel from the app. | |
| `tests/sms.rs` | Conversations, sending, older messages. | |
| `tests/bluetooth.rs` | Bluetooth backup against a stand-in BlueZ (socketpair as RFCOMM). | |
| `tests/status.rs` | Battery/network both ways and the widget JSON through the real CLI. | |

## desktop/app/ (Tauri window)

| File | What it does |
|---|---|
| `Cargo.toml` | Crate `phonelink-app`; Tauri 2, single-instance and dialog plugins, GTK + Wayland crates (Linux). |
| `build.rs` | `tauri_build::build()` (embeds the UI and generates ACL schemas). |
| `tauri.conf.json` | App id, bundle settings, `frontendDist: ../ui` (the UI is compiled in; no dev server). |
| `capabilities/default.json` | What the page may call (Tauri ACL). |
| `gen/schemas/*.json` | Generated by `tauri-build`; do not edit. |
| `src/main.rs` | The window: one `#[tauri::command]` per action (each forwards a `Request` to `phonelinkd`), watch-event forwarding to the page (`phonelink://event`), drag-and-drop, file picker, single-instance handling (`--show=<tab>`, `--activation-token=`), starting the service, window creation (frameless, transparent). |
| `src/blur.rs` | Blur behind the window on KDE: Wayland `org_kde_kwin_blur` on GTK's surface, X11 `_KDE_NET_WM_BLUR_BEHIND_REGION`. |

## desktop/ui/ (window's HTML/CSS/JS)

Plain files, no framework. The same files run as a clickable browser preview (with
`mock-api.js`) and inside the app (with `tauri-api.js`).

| File | What it does |
|---|---|
| `index.html` | Window skeleton: title bar, left device panel, tabs (Notifications, Messages, Files), dialogs, toasts. Loads the API script then `app.js`. |
| `app.js` | All UI logic, in sections (search for `// ---`): helpers, state, loading, rendering (device panel, battery/signal), music card, tabs & SMS threads, notifications, files, dialogs (send note, settings incl. **Appearance**), pairing screen, service-not-running screen, events (`api.onEvent` → refresh), timers, start. |
| `tauri-api.js` | The real backend: maps `window.PhoneLinkAPI` calls to Tauri `invoke(...)` commands and service events to UI events; converts service JSON to UI shapes (`toDevice`, `toTransfer`, …). |
| `mock-api.js` | Sample data and a fake API for the browser preview (`xdg-open desktop/ui/index.html`). |
| `qr-sample.js` | A pre-rendered QR used by the preview's pairing screen. |
| `styles.css` | Dark theme tokens (`:root` variables), layout, components, translucent window (`--alpha`). |
| `README.md` | How to open the preview. |
| `tests/preview.test.js` | Playwright checks of the browser preview (mock API): every tab, dialogs, keyboard, overflow, pairing QR decodes. |
| `tests/app.test.js` | Playwright checks of the real UI code against a fake Tauri `invoke`: every command, events, settings, appearance, status display. |
| `tests/package.json`, `tests/.gitignore` | `npm test` runs both; screenshots go to `tests/shots/`. |

## android/ (phone app)

Package `dev.phonelink.app` in `android/app/src/main/java/dev/phonelink/app/`.

| File | What it does |
|---|---|
| `android/build.gradle.kts`, `settings.gradle.kts`, `gradle.properties` | Gradle project setup (AGP, Kotlin, Compose). |
| `android/app/build.gradle.kts` | App module: SDK levels (min 29), Compose, JNA (for UniFFI), ZXing scanner, signing from CI secrets. |
| `android/app/proguard-rules.pro` | Keep rules for JNA/UniFFI. |
| `AndroidManifest.xml` | Permissions (camera for the scan, notifications, SMS, contacts, Bluetooth connect, phone state, foreground service), services, receivers, tiles, share targets. Microphone/location are explicitly removed. |
| `res/…` | Icons (`ic_*`), colours, strings, backup exclusion rules (`data_extraction_rules.xml`). |
| `PhoneLinkApp.kt` | Application: `LinkManager.init` and `Notifications.createChannels` at process start. |
| `LinkManager.kt` | **Process-wide owner of the Rust core** (`PhoneLink`). Starts it, exposes flows (`devices`, `transfers`, `messages`, `myId`), routes every `LinkEvent` (reply/dismiss/action requests, clipboard, media control, SMS send, transfers, computer status) and wraps every send on one background thread. |
| `LinkService.kt` | Foreground service (type `connectedDevice`) that keeps the link alive; multicast lock for mDNS; network callback → `onNetworkChanged`; unlock receiver → nudge; starts `SmsSync`, `StatusWatcher`, discovery. |
| `BootReceiver.kt` | Starts the service after boot/app update if a computer is paired. |
| `MirrorService.kt` | `NotificationListenerService`: mirrors notifications (filters noise), snapshot on connect, performs reply/action/dismiss requested by the computer. |
| `MediaWatcher.kt` | Active media sessions (via notification access) → now playing + album art; executes play/pause/next/previous/volume. |
| `SmsSync.kt` | Reads conversations from `content://sms` (+ contact names), watches for changes, sends SMS (`SmsManager`, multipart, sent results). |
| `FileShare.kt` | Opens files the core sends (content URI → fd), publishes received files to `Download/PhoneLink` (MediaStore), progress notification. |
| `ShareActivity.kt` | "PhoneLink" in Android's Share menu (files → send; text → computer clipboard). |
| `ClipboardActivity.kt` | Invisible activity that reads the phone clipboard while focused and sends it. |
| `Tiles.kt` | Quick Settings tiles: Lock computer, Send clipboard. |
| `LockReceiver.kt` | "Lock computer" button on the status notification. |
| `BluetoothBackup.kt` | After 15 s without Wi-Fi link: RFCOMM socket to the bonded laptop, piped into the core (`open_stream`). |
| `StatusWatcher.kt` | Battery (sticky broadcast), Wi-Fi/mobile (ConnectivityManager), 5G/4G + signal (TelephonyCallback) → `send_phone_status`. |
| `Notifications.kt` | The phone's own notifications: channels, persistent status, transfers, received notes. |
| `Prefs.kt` | Phone-side switches (clipboard sharing, queue notifications while disconnected). |
| `MainActivity.kt` | Activity: permission launchers, QR scan + pairing confirm, bottom navigation, implements `ui.Actions`. |
| `ui/Theme.kt` | Dark palette (`Pl`), `PhoneLinkTheme`, shared components (`Panel`, `SwitchRow`, `Chip`, `Hint`, `SectionTitle`). |
| `ui/Model.kt` | `UiState`, `Access` (setup checklist state), `Actions` interface, transfer helpers. |
| `ui/HomeScreen.kt` | Home: computer card (status chip, laptop battery, Reconnect, Lock/Clipboard/Send files/Note), Bluetooth warning, Now (active transfers), "On your computer" switches, note dialog. |
| `ui/OtherScreens.kt` | Files tab, Settings tab (clipboard, queue switch, computers, phone id), Finish-setup checklist, pairing screen, confirm-pair dialog. |
| `android/app/src/main/java/dev/phonelink/core/phonelink_ffi.kt` | **Generated** Kotlin bindings (by `scripts/gen-kotlin-bindings.sh`). Never edit by hand; regenerate after any change to `core-ffi/src/lib.rs`. |

## packaging/ (install, KDE widget, menus)

| File | What it does |
|---|---|
| `install.sh` | Builds (or uses prebuilt) binaries into `~/.local/bin`, installs the systemd user unit, Dolphin service menu, desktop entry + icons, the KDE widget (`kpackagetool6`), then starts `phonelinkd`. |
| `uninstall.sh` | Removes all of the above (`--purge` also deletes keys and pairings). |
| `systemd/phonelinkd.service` | User service (`systemctl --user … phonelinkd`). |
| `desktop/phonelink.desktop.in` | Application menu entry for the window. |
| `dolphin/phonelink-send.desktop.in` | Dolphin right-click → "Send to phone" (`phonelink send-file --notify %F`). |
| `plasmoid/dev.phonelink.widget/metadata.json` | Plasma 6 widget package metadata (id `dev.phonelink.widget`). |
| `…/contents/ui/main.qml` | Widget root: polls `phonelink widget` (2 s open / 5 s closed) through Plasma's executable data source, runs actions, unread logic, tooltip. |
| `…/contents/ui/CompactRepresentation.qml` | The panel icon: green phone outline, unread dot, click/middle-click, drop files. |
| `…/contents/ui/FullRepresentation.qml` | The popup: header status, music card, tabs Notifications/Messages/Files, file picker, drop area, translucent background. |
| `…/contents/ui/Empty.qml` | Small green empty-state. |
| `…/contents/ui/shell.js` | Safe shell quoting for every command, `file://` → path, helpers. |
| `…/contents/ui/configAppearance.qml` | Widget settings page: background opacity, link to KDE blur settings. |
| `plasmoid/tests/shell.test.js` | Node test: hostile file names (quotes, `$(…)`, backticks, newlines) survive `shell.js` quoting unchanged and run nothing. |
| `…/contents/config/main.xml`, `config.qml` | Widget settings schema (`backgroundOpacity`, default 75) and the settings page list. |

## scripts/ and CI

| File | What it does |
|---|---|
| `scripts/build-android.sh` | Builds the Rust `.so` for arm64/x86_64 with cargo-ndk, then `assembleDebug`. |
| `scripts/gen-kotlin-bindings.sh` | Regenerates `phonelink_ffi.kt` from `core-ffi`. Run after every FFI change. |
| `scripts/test-kotlin-bindings.sh` | Runs `core-ffi/kotlin-test` (JVM) against the host library. |
| `scripts/audit-apk-permissions.sh` | Fails if the built APK requests any permission outside the allow-list. |
| `.github/workflows/android.yml` | CI: builds the APK on pushes touching `android/`, `core/`, `core-ffi/`, `Cargo.*` (or manual run), audits permissions, publishes release `latest-apk` (`PhoneLink-<sha>.apk`), writes `status.txt`/`build.log` to the `ci-logs` branch. Stable signing from repo secrets. |
