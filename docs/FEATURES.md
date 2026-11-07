# Features: how each one works and where it lives

One section per feature. Each has the same parts: **what it does**, **how it works**, a
**where it lives** table (layer → file → functions), **settings**, **tests**, and **to
change…** hints. Paths are relative to the repo root; `android/…/` means
`android/app/src/main/java/dev/phonelink/app/`, `plasmoid/…/` means
`packaging/plasmoid/dev.phonelink.widget/contents/`.

Read [ARCHITECTURE.md](ARCHITECTURE.md) first for the common path every message takes, and
[PROTOCOL.md](PROTOCOL.md) for the message list.

Contents
1. [Pairing](#1-pairing)
2. [Staying connected and reconnecting](#2-staying-connected-and-reconnecting)
3. [Bluetooth backup](#3-bluetooth-backup)
4. [Notification mirroring (reply, buttons, dismiss, mute, queue switch)](#4-notification-mirroring)
5. [Notes (send a text)](#5-notes-send-a-text)
6. [Lock the laptop from the phone](#6-lock-the-laptop-from-the-phone)
7. [Clipboard](#7-clipboard)
8. [Music controls](#8-music-controls)
9. [File sharing](#9-file-sharing)
10. [SMS](#10-sms)
11. [Battery and network status](#11-battery-and-network-status)
12. [Desktop popups and opening the window](#12-desktop-popups-and-opening-the-window)
13. [KDE panel widget](#13-kde-panel-widget)
14. [Desktop app window (UI, appearance)](#14-desktop-app-window)
15. [Phone app UI and setup checklist](#15-phone-app-ui-and-setup-checklist)
16. [CLI](#16-cli)
17. [Privacy guard rails (permissions audit)](#17-privacy-guard-rails)

---

## 1. Pairing

**What:** the laptop shows a QR code; the phone scans it; both screens show the same id; from
then on only these two devices can talk.

**How:** see [ARCHITECTURE §3](ARCHITECTURE.md#3-connection-lifecycle). The QR holds the
laptop's fingerprint, LAN addresses, name and a one-time secret; HMAC proofs both ways;
both pin each other in `peers.json`.

| Layer | File | Functions / types |
|---|---|---|
| Core | `core/src/pairing.rs` | `PairingOffer::{to_uri, from_uri}`, `client_proof`, `server_proof`, `derive_pair_key` |
| Core | `core/src/trust.rs` | `AccessGate::{open_pairing, consume_pairing, close_pairing}`, `TrustStore::upsert` |
| Core | `core/src/node.rs` | `Node::start_pairing`, `cancel_pairing`, `pair_with`, `try_pair`, `serve_pairing` |
| Laptop service | `desktop/daemon/src/daemon.rs` | `Request::Pair` in `handle_client` (streams `PairStarted` → `Paired`/`PairExpired`); QR addresses from `netaddrs::lan_addrs` |
| CLI | `desktop/daemon/src/bin/phonelink.rs` | `phonelink pair` prints the QR in the terminal (`qr`) |
| Window | `desktop/app/src/main.rs` `start_pairing`, `cancel_pairing`, `qr_path`; `desktop/ui/app.js` `startPairing`, `renderPairing` | |
| FFI | `core-ffi/src/lib.rs` | `PhoneLink::read_pairing_code` (shows name+id before confirming), `pair`, `unpair` |
| Phone | `android/…/MainActivity.kt` | `startScan` (ZXing), `onScanned`, `pair`, `giveUpCamera`; `ui/OtherScreens.kt` `PairScreen`, `ConfirmPairDialog` |

**Tests:** `core/tests/sessions.rs`, `core/tests/attacker.rs`, `desktop/daemon/tests/daemon.rs`.
**To change** the QR contents: `PairingOffer`; the pairing window length: `ttl` in
`Request::Pair` (CLI `--ttl`, default 120 s).

## 2. Staying connected and reconnecting

**What:** after a reboot, a new IP, the phone sleeping or Wi-Fi changes, the devices find each
other again by themselves, usually within seconds.

**How:** `Node::maintain` per peer, dialing remembered addresses + mDNS hints + gateway +
/24 search, with backoff reset by nudges. Details in
[ARCHITECTURE §5](ARCHITECTURE.md#5-finding-each-other-again-reconnect).

| Layer | File | Functions |
|---|---|---|
| Core | `core/src/node.rs` | `maintain`, `connect_any`, `candidate_addrs`, `reconnect_candidates`, `nudge`, `add_addr_hints` |
| Core | `core/src/lan.rs`, `discovery.rs` | `Lan::{advertise, browse}`, `token_at`, `matches` |
| Core | `core/src/probe.rs` | `default_gateways`, `local_subnets`, `subnet_candidates` |
| Laptop | `desktop/daemon/src/daemon.rs` | `State::keep_connected`, `network_watch` (address change / suspend → nudge), `Request::Reconnect` |
| Laptop | `desktop/daemon/src/netaddrs.rs` | `NetWatch::check` |
| Phone | `core-ffi/src/lib.rs` | stable port (`port.txt`), `keep_connected`, `on_network_changed`, `start_discovery` |
| Phone | `android/…/LinkService.kt` | network callback, unlock receiver, multicast lock; `LinkManager.nudge` on app open; `BootReceiver.kt` |
| UI | `app.js` "Reconnect now" (`data-action="reconnect"`); phone Home "Reconnect"; widget refresh button | |

**Settings:** `discovery` and `port` in `config.toml`.
**Tests:** `core/tests/discovery.rs`, `core/tests/sessions.rs`.
**To change** timings: constants at the top of `core/src/node.rs` (`BACKOFF_MAX`,
`PROBE_AFTER_FAILURES`, `PROBE_INTERVAL`, …).

## 3. Bluetooth backup

**What:** when Wi-Fi/hotspot is gone, the phone stays connected over Bluetooth; notifications,
SMS, clipboard keep working; files wait for Wi-Fi.

**How:** [ARCHITECTURE §6](ARCHITECTURE.md#6-the-bluetooth-backup-link).

| Layer | File | Functions |
|---|---|---|
| Core | `core/src/node.rs` | `connect_stream`, `accept_stream`, `set_bluetooth_address`, `links_frame`, `peer_links`, `Transport::{Quic, Stream}` |
| Core | `core/src/tls.rs` | `stream_server_config`, `stream_client_config` |
| Laptop | `desktop/daemon/src/bluetooth.rs` | `register` (BlueZ `ProfileManager1`), `Registered::current_address` |
| Laptop | `desktop/daemon/src/daemon.rs` | `bluetooth_backup` task, `Request::SetBluetoothBackup` |
| FFI | `core-ffi/src/lib.rs` | `open_stream`, `StreamPipe::{received, closed}`, trait `StreamSink` |
| Phone | `android/…/BluetoothBackup.kt` | `onDisconnected` (15 s timer), `onConnected`, RFCOMM socket loop |
| UI | `app.js` `renderDevice` ("Connected over Bluetooth"); phone `HomeScreen` warning card; widget header | |

**Settings:** laptop `bluetooth_backup` (Settings → Bluetooth backup). Phone permission
"Nearby devices" (`BLUETOOTH_CONNECT`).
**Tests:** `core/tests/stream.rs`, `desktop/daemon/tests/bluetooth.rs`.

## 4. Notification mirroring

**What:** phone notifications appear as KDE popups and in the Notifications tab / widget.
You can reply, press buttons, dismiss. Optional "dismiss on both devices"; per-app mute;
phone-side "Queue notifications while disconnected" (default off).

**How:** full walkthrough in [ARCHITECTURE §2](ARCHITECTURE.md#2-life-of-a-message-a-whatsapp-notification).
On every connect the phone sends a snapshot of what's currently shown, so the laptop list
matches the phone after restarts.

| Layer | File | Functions |
|---|---|---|
| Phone | `android/…/MirrorService.kt` | `onNotificationPosted`, `onNotificationRemoved`, `isMirrorable` (noise filter), `toMirrored`, `snapshot`, `reply`, `press`, `dismiss` |
| Phone | `android/…/LinkManager.kt` | `mirrorPosted`, `mirrorRemoved`, `mirrorIcon`, `onMirrorReady`/`sendSnapshot`, `handleRequest`, `setQueueNotifications` |
| FFI | `core-ffi/src/lib.rs` | `post_notification`, `remove_notification`, `send_snapshot`, `send_app_icon`, `send_action_result`, `set_queue_notifications`, `send_notification` (queue switch) |
| Laptop | `desktop/daemon/src/notifications.rs` | `Store::{post, removed_on_phone, snapshot, dismiss, clear, apps}`, `save_icon`, `escape_markup` |
| Laptop | `desktop/daemon/src/daemon.rs` | event_pump arms `NotificationPosted/Removed/Snapshot/AppIcon/ActionResult`; `maybe_popup`; requests `Notifications`, `DismissNotification`, `ClearNotifications`, `ReplyNotification`, `NotificationAction`, `SetSyncDismissals`, `SetAppMuted`, `AppIcons` |
| Window | `desktop/app/src/main.rs` | `notifications`, `dismiss_notification`, `clear_notifications`, `reply_notification`, `notification_action`, `set_sync_dismissals`, `set_app_muted`, `app_icons` |
| Window UI | `desktop/ui/app.js` | `fetchNotifications`, `applyNotifications`, `renderNotifications`, `submitReply`, `dismiss` |
| Widget | `plasmoid/…/ui/FullRepresentation.qml` | Notifications tab; × runs `phonelink dismiss <id>` |

**Settings:** laptop `notifications` (popups), `sync_dismissals`, `muted_apps`; phone
`Prefs.queueNotifications`.
**Tests:** `desktop/daemon/tests/notifications.rs`, `popups.rs`; FFI
`notification_mirroring_call_sequence` (includes the queue switch).
**To change** which notifications are skipped: `MirrorService.isMirrorable`; the popup age
limit: `POPUP_MAX_AGE_MS`.

## 5. Notes (send a text)

**What:** short notes either way (phone "Note" button; laptop "Send a note", `phonelink
send-text`). Queued if the other side is offline.

| Layer | File | Functions |
|---|---|---|
| Core | `AppMessage::Text` | |
| Phone | `core-ffi` `send_text`; `MainActivity.sendText`; `HomeScreen` `NoteDialog`; incoming → `Notifications.message` | |
| Laptop | `daemon.rs` `Request::SendText`, event_pump `Text` → popup + `WatchEvent::Text` | |
| Window | `main.rs` `send_note`; `app.js` `openNoteDialog`, `submitNote` | |

## 6. Lock the laptop from the phone

**What:** Lock button (phone Home, Quick Settings tile, status notification). Unlocking is
deliberately impossible.

**How:** phone sends `LockComputer { sent_ms }` only if a computer is connected; the laptop
refuses stale requests (> 2 min) or when the setting is off, locks via the ScreenSaver
D-Bus API (logind fallback), and answers with `ActionResult { key: "lock" }`.

| Layer | File | Functions |
|---|---|---|
| Phone | `android/…/LinkManager.kt` `lockComputer`; `Tiles.kt` `LockTileService`; `LockReceiver.kt`; `HomeScreen` Lock tile | |
| FFI | `core-ffi/src/lib.rs` | `lock_computer` (uses `connected_one`) |
| Laptop | `desktop/daemon/src/daemon.rs` | `State::lock_requested`, `Request::SetAllowLock` |
| Laptop | `desktop/daemon/src/lock.rs` | `lock_screen`, `lock_with_logind` |

**Settings:** laptop `allow_lock`. **Tests:** `desktop/daemon/tests/lock_clipboard.rs`.

## 7. Clipboard

**What:** copy on the laptop → paste on the phone (automatic). Phone → laptop with one tap
(Clipboard button, tile, notification button, or Share → Copy to computer).

**How:** the laptop watches KDE's Klipper over D-Bus (password-manager copies are excluded by
Klipper); new text is sent to connected phones. Android lets only the focused app read the
clipboard, so the phone uses a tiny invisible activity. Both sides remember the last text
received and never echo it back; `copied_ms` must be fresh (2 min).

| Layer | File | Functions |
|---|---|---|
| Laptop | `desktop/daemon/src/clipboard.rs` | `ClipboardSync::{start, set}`, `listen`, `read`, `write` |
| Laptop | `desktop/daemon/src/daemon.rs` | `share_copies` (laptop → phone), event_pump `Clipboard` (phone → laptop), `Request::SetClipboardSync` |
| FFI | `core-ffi/src/lib.rs` | `send_clipboard`, `LinkEvent::ClipboardReceived` |
| Phone | `android/…/ClipboardActivity.kt`, `Tiles.kt` `ClipboardTileService`, `LinkManager.{sendClipboard, putOnClipboard}`, `MainActivity.sendClipboard` | |

**Settings:** laptop `clipboard_sync`; phone `Prefs.clipboardSharing`.
**Tests:** `lock_clipboard.rs` (`clipboard_goes_both_ways_without_echoes`).

## 8. Music controls

**What:** what's playing on the phone shows on the laptop (window card, widget card) with
previous / play-pause / next and volume. No controls on the phone app itself.

**How:** `MediaWatcher` reads active media sessions through notification access (no extra
permission) and sends `NowPlaying` (+ `MediaArt`) to connected computers; the laptop
extrapolates the position locally; buttons send `MediaControl` back.

| Layer | File | Functions |
|---|---|---|
| Phone | `android/…/MediaWatcher.kt` | `start`, `stop`, `resend` (on connect), `control` |
| FFI | `core-ffi/src/lib.rs` | `send_now_playing`, `send_media_art`, `LinkEvent::MediaControlRequested`, `MediaCommand` |
| Laptop | `desktop/daemon/src/media.rs` | `MediaStore::{update, set_art, view, forget}` |
| Laptop | `desktop/daemon/src/daemon.rs` | event_pump `NowPlaying`/`MediaArt`, `Request::{Media, MediaControl}`, `media_target` |
| Window | `main.rs` `media`, `media_control`; `app.js` `refreshMedia`, `renderMedia`, `tickMedia`, `setPhoneVolume` | |
| Widget | `FullRepresentation.qml` music card → `phonelink media …` | |
| CLI | `phonelink media [play-pause|next|previous|volume-up|volume-down|volume N]` | |

**Tests:** `desktop/daemon/tests/media.rs`.

## 9. File sharing

**What:** send files both ways; resumes after drops/restarts; verified; cancel from either
side. Laptop: drag onto the window or widget, "Send files", Dolphin "Send to phone",
`phonelink send-file`. Phone: Share → PhoneLink, "Send files". Received on the laptop in
`~/Downloads/PhoneLink`, on the phone in `Download/PhoneLink`.

**How:** [PROTOCOL → File data streams](PROTOCOL.md#file-data-streams). The engine is in the
core and runs on both sides; the phone only provides file descriptors and publishes
results.

| Layer | File | Functions |
|---|---|---|
| Core | `core/src/transfer.rs` | `Transfers::{start, send_file, cancel, on_message, list, clear_finished, nudge}`, `safe_name`, `reserve_unique`, `SourceOpener`, `PathOpener` |
| Core | `core/src/node.rs` | `open_data`, `data_streams` |
| Laptop | `desktop/daemon/src/daemon.rs` | `State::send_files`, `transfer_updates`, requests `Transfers`, `SendFiles`, `CancelTransfer`, `ClearTransfers`, `OpenTransfer` |
| Laptop | `desktop/daemon/src/open.rs` | `open_file`, `reveal_file`, `guess_mime` |
| Window | `desktop/app/src/main.rs` | `send_files`, `pick_files`, `cancel_transfer`, `clear_transfers`, `open_transfer`, `WindowEvent::DragDrop` → `phonelink://drop` |
| Window UI | `app.js` | `sendFiles`, `sendPaths`, `upsertTransfer`, `renderFiles`, `transferRow` |
| Widget | `CompactRepresentation.qml` / `FullRepresentation.qml` `DropArea`, `FileDialog` → `main.qml` `sendUrls` |
| Dolphin | `packaging/dolphin/phonelink-send.desktop.in` | |
| FFI | `core-ffi/src/lib.rs` | `send_file`, `cancel_transfer`, `transfers`, `clear_transfers`, trait `FileSource`, `LinkEvent::TransferChanged` |
| Phone | `android/…/FileShare.kt` | `open` (URI → fd), `send`, `onChanged` (publish to MediaStore + notification), `publishLeftovers` |
| Phone | `ShareActivity.kt`, `MainActivity` file picker, `ui/OtherScreens.kt` `FilesScreen`, `HomeScreen` `TransferCard` | |

**Settings:** laptop `download_dir`.
**Tests:** `core/tests/files.rs`, `desktop/daemon/tests/files.rs`.
**To change** chunk size / retries: constants at the top of `transfer.rs`.

## 10. SMS

**What:** Messages tab on the laptop: conversations with contact names, read and reply,
load older messages, Sending → Sent / Failed. Regular SMS only (RCS isn't readable by other
apps).

**How:** on connect the phone sends `SmsSnapshot` + one `SmsThreadUpdate` per conversation
(30 × 30); a ContentObserver sends changes; older messages on request. Sending: laptop
`SmsSend` (only if connected) → phone `SmsManager` multipart → `SmsSendResult`. Laptop keeps
it all in memory only. New texts also arrive as mirrored notifications of the Messages app.

| Layer | File | Functions |
|---|---|---|
| Phone | `android/…/SmsSync.kt` | `canRead`, `start` (observer), `snapshot`, `changedThreads`, `thread`, `older`, `send` |
| Phone | `android/…/LinkManager.kt` | `sendSmsSnapshot`, `smsChanged`, `smsSend`, `smsOlder` |
| FFI | `core-ffi/src/lib.rs` | `sms_snapshot`, `sms_thread_update`, `sms_older`, `sms_send_result`, `LinkEvent::{SmsSendRequested, SmsOlderRequested}` |
| Laptop | `desktop/daemon/src/sms.rs` | `SmsStore::{snapshot, update, older, add_pending, send_result, mark_read, view}`, `thread_key` |
| Laptop | `desktop/daemon/src/daemon.rs` | `State::send_sms`, requests `SmsThreads`, `SendSms`, `SmsMarkRead`, `SmsLoadOlder` |
| Window | `main.rs` `sms_threads`, `send_sms`, `sms_mark_read`, `sms_load_older`; `app.js` `refreshThreads`, `renderThreads`, `renderConversation`, `sendSms`, `openThread` | |
| Widget | Messages tab (click opens the window's Messages tab; `phonelink sms-read`) | |

**Permissions:** `READ_SMS`, `SEND_SMS`, `READ_CONTACTS` (asked when you allow SMS).
**Tests:** `desktop/daemon/tests/sms.rs`.

## 11. Battery and network status

**What:** phone battery, charging, 5G/4G/Wi-Fi and signal bars on the laptop (window and
widget); laptop battery on the phone's Home card.

**How:** `StatusWatcher` listens to the battery broadcast, the default network and
`TelephonyCallback` (5G shown when Android says NR/NR-NSA); sends `PhoneStatus` at most once
a second on change and on every connect. The daemon reads `/sys/class/power_supply` on
connect and every 30 s and sends `ComputerStatus` on change. Neither is queued.

| Layer | File | Functions |
|---|---|---|
| Phone | `android/…/StatusWatcher.kt` | `start`, `send`, `mobileLabel`, `current` |
| FFI | `core-ffi/src/lib.rs` | `send_phone_status`, `LinkEvent::ComputerStatus`, `Device.{battery, charging}` |
| Laptop | `desktop/daemon/src/battery.rs` | `read` |
| Laptop | `desktop/daemon/src/daemon.rs` | event_pump `PhoneStatus` → `phone_status`; `send_battery`; `battery_watch`; `DeviceInfo.status` |
| Window | `tauri-api.js` `toDevice`; `app.js` `renderDevice`, `signalBars` | |
| Widget | `FullRepresentation.qml` header line, `main.qml` tooltip | |
| Phone UI | `ui/HomeScreen.kt` `ComputerCard` ("Computer battery …") | |

**Permission:** `READ_PHONE_STATE` for the 4G/5G type (setup step "Phone status").
**Tests:** `desktop/daemon/tests/status.rs`, `battery.rs` unit test, FFI `status_both_ways`.

## 12. Desktop popups and opening the window

**What:** KDE popups for notifications, notes, connections, files; clicking opens the
PhoneLink window at the right tab (and focuses it on Wayland).

**How:** one D-Bus connection on one thread sends `Notify`, replaces/closes by id, and
listens for `ActionInvoked`/`ActivationToken`; the opener starts `phonelink-app
--show=<tab> --activation-token=…`; the single-instance plugin forwards to a running window.

| Layer | File | Functions |
|---|---|---|
| Laptop | `desktop/daemon/src/popups.rs` | `Popups`, `PopupCmd`, `popup_thread`, `start_listener`, `open_app`, `session_env` |
| Laptop | `desktop/daemon/src/daemon.rs` | `status_popup`, `popup_to`, `maybe_popup` |
| Window | `desktop/app/src/main.rs` | `launch_request`, `bring_forward`, `use_activation_token`; event `phonelink://open` → `app.js` `openFromPopup` |

**Tests:** `desktop/daemon/tests/popups.rs`.

## 13. KDE panel widget

**What:** a small green phone outline in the panel (grey offline, green dot for new
things). Click: popup with phone status, music card, Notifications / Messages / Files. Drop
files on it to send (queued if offline). Middle-click opens the window. Configurable
background opacity.

**How:** pure QML, no compiled plugin. `main.qml` runs `phonelink widget` through Plasma's
`executable` data source every 2 s (popup open) / 5 s (closed) and parses the JSON. Actions
run `phonelink <args>` with every argument shell-quoted by `shell.js`. The popup's
translucency fades Plasma's own popup background item (`applyOpacity`); KDE's Blur effect
blurs behind it.

| File | What to edit there |
|---|---|
| `plasmoid/…/ui/main.qml` | polling, `run`, `sendUrls`, `openApp`, unread logic (`seenMs`), tooltip |
| `plasmoid/…/ui/CompactRepresentation.qml` | icon size/shape (`body`), unread dot, drop handling |
| `plasmoid/…/ui/FullRepresentation.qml` | popup size (`Layout.preferred*`), header, music card, tabs, `applyOpacity` |
| `plasmoid/…/ui/Empty.qml` | empty-state look |
| `plasmoid/…/ui/shell.js` | `quote`, `command`, `localPath` (security-relevant) |
| `plasmoid/…/ui/configAppearance.qml`, `config/main.xml`, `config/config.qml` | settings page and keys |
| `desktop/daemon/src/bin/phonelink.rs` `widget_state` | the JSON the widget reads |

Install/update: `./packaging/install.sh` (uses `kpackagetool6`), then
`plasmashell --replace & disown`. **Tests:** `desktop/daemon/tests/status.rs` (widget JSON);
the quoting was tested with hostile names; QML is parse-checked (`qmlformat`).

## 14. Desktop app window

**What:** the PhoneLink window: device panel (phone art, status, battery/5G, music card,
quick actions), tabs Notifications / Messages / Files, Settings sheet, pairing screen,
"service not running" screen. Translucent with optional blur.

**How:** `desktop/ui` is a framework-free page; `tauri-api.js` implements
`window.PhoneLinkAPI` with `invoke(...)`; `app.js` holds state and renders. Every command
in `desktop/app/src/main.rs` opens the socket, sends one `Request`, maps the `Reply`.
`watch_daemon` keeps one `Watch` subscription and emits `phonelink://event` /
`phonelink://daemon`.

| To change… | Edit |
|---|---|
| Colours, spacing | `desktop/ui/styles.css` `:root` tokens |
| Device panel | `app.js` `renderDevice` |
| A tab | `app.js` `renderNotifications` / `renderThreads` + `renderConversation` / `renderFiles` |
| Settings sheet | `app.js` `settingsBody`, `appearanceSection`; click handling in the `onClick` switch (`case 'toggle-setting'` …) |
| New backend call | command in `main.rs` (+ register in `generate_handler!`), method in `tauri-api.js`, fake in `mock-api.js` |
| Opacity / blur | `app.js` `applyAppearance`; `styles.css` `body.in-app .window` (`--alpha`); `desktop/app/src/blur.rs` |
| Window size/frame | `main.rs` `WebviewWindowBuilder` in `main` |

**Tests:** UI suites (Playwright) in the session scratchpad (`test.js` preview, `test-app.js`
with a fake Tauri); see [DEVELOPING.md](DEVELOPING.md#ui-tests).

## 15. Phone app UI and setup checklist

**What:** dark Compose app with Home · Files · Settings; a "Finish setup" checklist for
notification access, background running, SMS, Nearby devices, Phone status; pairing screen
when nothing is paired.

| To change… | Edit |
|---|---|
| Colours, shared widgets | `android/…/ui/Theme.kt` (`Pl`, `Panel`, `SwitchRow`, `Chip`) |
| Home | `android/…/ui/HomeScreen.kt` (`ComputerCard`, `ActionTile`, `TransferCard`, `NoteDialog`) |
| Files / Settings / Setup / Pair | `android/…/ui/OtherScreens.kt` (`FilesScreen`, `SettingsScreen`, `SetupScreen` + `Step`, `PairScreen`) |
| What a button does | `MainActivity.kt` `actions` object (implements `ui/Model.kt` `Actions`) |
| Setup items | `ui/Model.kt` `Access` + `MainActivity.onResume` (reads permission state) |
| Phone switches | `Prefs.kt` |
| Persistent / transfer notifications | `Notifications.kt` |

## 16. CLI

`desktop/daemon/src/bin/phonelink.rs`. Commands: `status`, `devices`, `pair [--ttl]`,
`unpair <id>`, `send-text [--to] <text>`, `watch`, `notifications`, `reconnect`,
`send-file [--to] [--notify] <files>`, `transfers`, `cancel-transfer <id>`,
`media [action] [level]`, `widget`, `dismiss <id>`, `sms-read <thread>`. Add `--json` for raw
replies. To add one: a variant in `enum Cmd`, a match arm in `run`, usually one `Request`.

## 17. Privacy guard rails

- `AndroidManifest.xml` removes microphone/location permissions that libraries could add.
- `scripts/audit-apk-permissions.sh` (run by CI) fails the build if the APK asks for
  anything outside its allow-list. **Adding a permission = add it to the manifest *and* the
  allow-list**, deliberately.
- Camera is handed back after the scan (`MainActivity.giveUpCamera`).
- Laptop stores notification/SMS/clipboard/music content in memory only.
