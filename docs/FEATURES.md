# Features: how each one works and where it lives

One section per feature. Each has the same parts: **what it does**, **how it works**, a
**where it lives** table (layer → file → functions), **settings**, **tests**, and **to
change…** hints. Paths are relative to the repo root; `android/…/` means
`android/app/src/main/java/dev/phonelink/app/`, `plasmoid/…/` means
`packaging/plasmoid/dev.phonelink.widget/contents/`.

Every message takes the same path: a watcher on one side → the shared Rust core (outbox,
encrypted session) → the other side's handler → the UI. See [PROTOCOL.md](PROTOCOL.md) for
the message list.

Contents
1. [Pairing](#1-pairing)
2. [Staying connected and reconnecting](#2-staying-connected-and-reconnecting)
3. [Bluetooth backup](#3-bluetooth-backup)
4. [Notification mirroring (reply, buttons, dismiss, mute, queue switch)](#4-notification-mirroring)
5. [Notes (send a text)](#5-notes-send-a-text)
6. [Lock and unlock the laptop from the phone](#6-lock-and-unlock-the-laptop-from-the-phone)
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

**How:** the QR holds the
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
/24 search, with backoff reset by nudges.

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

### Connection switch (on/off)

**What:** switch the connection off on either device; nothing connects or searches until
it's switched back on, and the other device stops searching too. Laptop: window (left
panel / Settings), widget header button, `phonelink off|on`. Phone: Home card / Settings
switch (also stops the background service).

**How:** `Node::set_paused` sends `Control::Reject { reason: "paused" }` on every session,
closes it, refuses incoming sessions the same way and makes `maintain` wait. A peer that
receives "paused" marks it (`peer_paused`) and stops dialling until the other side connects
back (rechecks every 10 min).

| Layer | File | Functions |
|---|---|---|
| Core | `core/src/node.rs` | `set_paused`, `is_paused`, `peer_paused`, `mark_peer_paused`, `PAUSED_REASON`, `Event::PeerPaused` |
| Laptop | `desktop/daemon/src/daemon.rs` | `Request::SetConnection`, `State::update_lan` (mDNS off while off), Bluetooth profile unregistered; config `connection` |
| Window / widget / CLI | `main.rs` `set_connection`; `app.js` `connectionOff`, `toggle-connection`; `FullRepresentation.qml` power button; `phonelink on/off` | |
| Phone | `core-ffi` `set_paused`, `Device.paused_by_peer`; `LinkManager.setConnection`; `Prefs.connection`; `HomeScreen` `ComputerCard` switch | |

**Tests:** `core/tests/sessions.rs` `switching_the_connection_off_and_on`,
`desktop/daemon/tests/status.rs` `the_connection_switch`, FFI `status_both_ways`.

### Battery saver (laptop)

**What:** a laptop switch (Settings → General, `phonelink saver on|off`, config
`battery_saver`, default on) for fewer background wake-ups. On: QUIC keep-alive 10 s / idle
timeout 30 s instead of 4 s / 20 s (a phone that left is noticed ~10 s later), reconnect
backoff up to 60 s instead of 30 s, and while the phone is away the /24 search runs after
3, 6, 12, 24, then every 30 min (back to 3 min after any nudge or connection). The phone's
own timers never change.

**Turns on by itself:** below a laptop battery level (Settings → General → "Turn on by
itself", slider 0–50 %, default 20 %, 0 = never; `phonelink saver auto 20|off`; config
`auto_saver_below`). On at or below the level while not charging, off again above level + 5 %
or as soon as the charger is in (`battery::auto_saver`, unit-tested). It never changes the
saved switch; the window's Saver tile and the widget's Saver button read "Saver · auto" and
can't be switched off while it lasts. A popup says when it turned on. It stays on Wi-Fi:
moving to Bluetooth would save little or nothing (Wi-Fi stays on for the internet anyway)
and would stop file transfers.

**How:** `core/src/node.rs` `Timings` (`default`, `battery_saver`) in `NodeConfig.timings`;
`Node::set_timings` swaps the endpoint's server config (new connections) and the reconnect
loop reads backoff/search gaps each round (`ProbeClock`). The daemon applies it at start
(`start_node`) and from `State::apply_saver` (start-up, every battery check in
`battery_watch`, `Request::SetBatterySaver`, `Request::SetAutoSaver`, `Request::Status`);
`Status.battery_saver` (the switch), `Status.saver_active` (in effect), `auto_saver_below`;
`WatchEvent::SaverChanged`. Window: `main.rs` `set_battery_saver`, `set_auto_saver`,
`tauri-api.js` `batterySaver`, `autoSaverBelow`, `saverAuto`.

Always on, regardless of the switch: phone signal-bar-only changes are announced at most every
10 s (`State::announce_status`); the window redraws only the device panel on
`phone_status_changed`; the outbox uses `synchronous=NORMAL`; the transfer sender sleeps
until woken (no 1 s tick); mDNS address check every 30 s on the laptop; network watch every
10 s; clipboard thread sleeps 60 s while Klipper is connected; the widget waits for changes
instead of polling; the window's 1 s timers run only while needed and visible.

**Tests:** `status.rs` `the_battery_saver_switch_is_saved`, `the_battery_saver_turns_on_by_itself_when_the_laptop_is_low`, `wobbling_signal_bars_are_announced_rarely`; `battery.rs` `the_saver_turns_on_when_low_and_off_with_some_room`;
`node.rs` `subnet_search_slows_down_only_with_the_battery_saver`; `transfer.rs`
`the_sender_sleeps_while_idle_and_wakes_for_retries`.

## 3. Bluetooth backup

**What:** when Wi-Fi/hotspot is gone, the phone stays connected over Bluetooth; notifications,
SMS, clipboard keep working; files wait for Wi-Fi.

**How:** without Wi-Fi the phone opens an RFCOMM socket to the laptop and runs the same
pinned-TLS session over it; QUIC takes over again when it returns. If the two are paired in
Bluetooth settings it does so after 15 s; if not, after 2 minutes, with an *insecure*
RFCOMM socket (no Bluetooth pairing, no popup). The laptop's profile has
`RequireAuthentication: false` for this. That is safe: the session on top is mutual TLS
with pinned keys, and pairing is never allowed over the stream (`core/tests/stream.rs`
`strangers_cannot_use_the_bluetooth_link_even_while_pairing_is_open`). Both devices still
need Bluetooth switched on; Android 13+ doesn't let an app switch it on.

| Layer | File | Functions |
|---|---|---|
| Core | `core/src/node.rs` | `connect_stream`, `accept_stream`, `set_bluetooth_address`, `links_frame`, `peer_links`, `Transport::{Quic, Stream}` |
| Core | `core/src/tls.rs` | `stream_server_config`, `stream_client_config` |
| Laptop | `desktop/daemon/src/bluetooth.rs` | `register` (BlueZ `ProfileManager1`), `Registered::current_address` |
| Laptop | `desktop/daemon/src/daemon.rs` | `bluetooth_backup` task, `Request::SetBluetoothBackup` |
| FFI | `core-ffi/src/lib.rs` | `open_stream`, `StreamPipe::{received, closed}`, trait `StreamSink` |
| Phone | `android/…/BluetoothBackup.kt` | `onDisconnected` (15 s timer; `unreachableSince` for the 2-minute unpaired rule), `target`, `open` (secure or insecure socket), `onConnected`, RFCOMM socket loop |
| UI | `app.js` `renderDevice` ("Connected over Bluetooth"); phone `HomeScreen` warning card; widget header | |

**Settings:** laptop `bluetooth_backup` (Settings → Bluetooth backup). Phone permission
"Nearby devices" (`BLUETOOTH_CONNECT`).
**Tests:** `core/tests/stream.rs`, `desktop/daemon/tests/bluetooth.rs`.

## 4. Notification mirroring

**What:** phone notifications appear as KDE popups and in the Notifications tab / widget.
You can reply, press buttons, dismiss. Optional "dismiss on both devices"; per-app mute;
phone-side "Queue notifications while disconnected" (default off).

**How:** the phone's notification listener sends each one through the core; the laptop service stores it in memory, shows a popup and tells the window and widget.
On every connect the phone sends a snapshot of what's currently shown, so the laptop list
matches the phone after restarts.

**The whole message:** the laptop gets everything the app put into the notification, i.e.
what the phone shows when the notification is pulled down: every message of a chat
(MessagingStyle), every line of an inbox-style notification (`EXTRA_TEXT_LINES`), else the big
text. Up to 4000 bytes (`MAX_NOTIFICATION_TEXT`); a longer chat keeps its newest lines and
starts with "…" (`MirrorService.joinNewest`). Line breaks are kept in the window (`white-space:
pre-line`) and the widget's list (no line limit). Limit: if the phone's own pulled-down
notification ends with "…" (Gmail and Outlook do this for long mails), the app never gave
Android more, so there is nothing more to mirror.

| Layer | File | Functions |
|---|---|---|
| Phone | `android/…/MirrorService.kt` | `onNotificationPosted`, `onNotificationRemoved`, `isMirrorable` (noise filter), `toMirrored` (`messagingText`, `inboxText`, `joinNewest`), `snapshot`, `reply`, `press`, `dismiss` |
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
`notification_mirroring_call_sequence` (includes the queue switch and that a snapshot of 100
maximum-length notifications fits one frame); UI suite ("a three-line note shows three lines").
**To change** which notifications are skipped: `MirrorService.isMirrorable`; the popup age
limit: `POPUP_MAX_AGE_MS`.

## 5. Notes (send a text)

**What:** short notes either way (phone "Note" button; laptop "Send a note", `phonelink
send-text`). Queued if the other side is offline. Every note has a **Copy** button on both
devices: in the laptop's popup and window, on the phone's Home list and its notification.

| Layer | File | Functions |
|---|---|---|
| Core | `AppMessage::Text` | |
| Phone | `core-ffi` `send_text`; `MainActivity.sendText`; `HomeScreen` `NoteDialog`; incoming → `LinkManager.messages` (`Note(from, text)`) and `Notifications.message(copy = true)` | Copy: `HomeScreen` `NoteCard` → `MainActivity.copyNote`; notification button → `CopyReceiver` (not exported) |
| Laptop | `daemon.rs` `Request::SendText`, event_pump `Text` → `note_popup` (button `note-copy`, last 20 texts kept in `State.notes`) + `WatchEvent::Text` | Copy: `popup_action` → Klipper, marked as ours so it isn't sent back; `Request::PopupAction` presses a popup button from a client (tests) |
| Window | `main.rs` `send_note`; `app.js` `openNoteDialog`, `submitNote`; `copy-note` action | |

**Tests:** `tests/lock_clipboard.rs` `a_note_popup_has_copy_which_fills_the_clipboard_without_an_echo`;
UI suite ("Copy puts the note on the clipboard").

## 6. Lock and unlock the laptop from the phone

**What:** Lock button (phone Home, Quick Settings tile, status notification). **Unlock**
tile on phone Home: Android's own prompt asks for the phone's fingerprint or screen lock,
then the laptop unlocks. Unlocking is off until you turn on laptop Settings → "Let your
phone unlock this computer", and the laptop shows a popup "Unlocked by <phone>" every time
(even with notifications off). Honest risk: anyone who can unlock your phone could then
unlock the laptop while that switch is on.

**How:** phone sends `LockComputer { sent_ms }` only if a computer is connected; the laptop
refuses stale requests (> 2 min) or when the setting is off, locks via the ScreenSaver
D-Bus API (logind fallback), and answers with `ActionResult { key: "lock" }`.

Unlock: after the prompt (platform `BiometricPrompt`, BIOMETRIC_STRONG or the screen lock;
refused if the phone has no screen lock) the phone sends `UnlockComputer { sent_ms }` as a
fleeting message (only while connected, never queued). The laptop needs `unlock/1`, the
`allow_unlock` switch (default off) and a request under 30 s old, then calls logind
`Session.Unlock` on this user's graphical sessions (what `loginctl unlock-session` does; no
root, polkit lets you unlock your own active session; KDE's lock screen obeys it, as KDE
Connect users rely on). KDE bug 506343 reports it can take ~5 s on Plasma 6.4. It can't work
where logind is missing (non-systemd) or polkit forbids it. Answer: `ActionResult { key:
"unlock" }`; `WatchEvent::UnlockedByPhone`.

| Layer | File | Functions |
|---|---|---|
| Phone | `android/…/LinkManager.kt` `lockComputer`, `unlockComputer`; `UnlockPrompt.kt`; `Tiles.kt` `LockTileService`; `LockReceiver.kt`; `HomeScreen` Lock and Unlock tiles | |
| FFI | `core-ffi/src/lib.rs` | `lock_computer` (uses `connected_one`), `unlock_computer` (fleeting), `Device.can_unlock` |
| Laptop | `desktop/daemon/src/daemon.rs` | `State::lock_requested`, `State::unlock_requested`, `Request::SetAllowLock` |
| Laptop | `desktop/daemon/src/lock.rs` | `lock_screen`, `lock_with_logind`, `unlock_screen`, `session_call` |

**Settings:** laptop `allow_lock`, `allow_unlock` (off). **Permission:** `USE_BIOMETRIC` (no
prompt). **Tests:** `desktop/daemon/tests/lock_clipboard.rs` (incl.
`the_phone_unlocks_only_when_allowed_fresh_and_it_is_announced`, stand-in logind), FFI test,
UI suite (the switch, off by default).

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

**Images (laptop → phone):** a copied picture is read with `arboard` (`clipboard.rs` `SystemImages`, saved as PNG) or, for a copied image file, sent as that file; `State::send_clipboard_image` sends it with the file engine at full quality plus `ClipboardImage`, and the phone (`FileShare.toClipboard`) puts it on the clipboard through a `FileProvider` instead of Downloads.

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
`phonelink send-file`. Phone: Share → EchoTwin, "Send files". Received on the laptop in
`~/Downloads/EchoTwin`, on the phone in `Download/EchoTwin`, unless you choose another folder:

- **Laptop:** window → Settings → *Received files* → **Change…** (the system's folder
  chooser) or **Default**; or `phonelink download-dir <folder>` / `--default`. The service
  checks the folder is absolute and writable (it is made if missing), switches at once (a file
  that is already arriving finishes there too; across disks it is copied), and saves it as
  `download_dir` in config.toml. Popups say where each file went.
- **Phone:** Settings → *Received files* → **Change** opens Android's own folder chooser;
  EchoTwin gets access to that one folder only (kept across restarts; no storage permission)
  and gives it back on **Default** or another choice. If the folder is deleted or the access
  taken away, files go to `Download/EchoTwin` and the notification says so.

**How:** [PROTOCOL → File data streams](PROTOCOL.md#file-data-streams). The engine is in the
core and runs on both sides; the phone only provides file descriptors and publishes
results.

| Layer | File | Functions |
|---|---|---|
| Core | `core/src/transfer.rs` | `Transfers::{start, send_file, cancel, on_message, list, clear_finished, nudge, set_incoming_dir, incoming_dir}`, `safe_name`, `reserve_unique`, `move_file`, `SourceOpener`, `PathOpener` |
| Core | `core/src/node.rs` | `open_data`, `data_streams` |
| Laptop | `desktop/daemon/src/daemon.rs` | `State::send_files`, `transfer_updates`, `set_download_dir`, requests `Transfers`, `SendFiles`, `CancelTransfer`, `ClearTransfers`, `OpenTransfer`, `SetDownloadDir`; `Reply::Status.download_dir` |
| Laptop | `desktop/daemon/src/open.rs` | `open_file`, `reveal_file`, `guess_mime` |
| Window | `desktop/app/src/main.rs` | `send_files`, `pick_files`, `pick_download_dir`, `set_download_dir`, `cancel_transfer`, `clear_transfers`, `open_transfer`, `WindowEvent::DragDrop` → `phonelink://drop` |
| Window UI | `app.js` | `sendFiles`, `sendPaths`, `upsertTransfer`, `renderFiles`, `transferRow`, `downloadSection` |
| Widget | `CompactRepresentation.qml` / `FullRepresentation.qml` `DropArea`, `FileDialog` → `main.qml` `sendUrls` |
| Dolphin | `packaging/dolphin/phonelink-send.desktop.in` | |
| FFI | `core-ffi/src/lib.rs` | `send_file`, `cancel_transfer`, `transfers`, `clear_transfers`, trait `FileSource`, `LinkEvent::TransferChanged` |
| Phone | `android/…/FileShare.kt` | `open` (URI → fd), `send`, `onChanged`, `publish` (`toTree` for a chosen folder, else `toDownloads` via MediaStore; notification), `folderName`, `publishLeftovers`; `Prefs.receiveFolder`; `MainActivity.pickFolder` (OpenDocumentTree) |
| Phone | `ShareActivity.kt`, `MainActivity` file picker, `ui/OtherScreens.kt` `FilesScreen`, `HomeScreen` `TransferCard` | |

Speed and time left: `transfer.rs` keeps a smoothed rate per running transfer
(`smooth_rate`, `eta`) and reports `TransferInfo.rate` / `eta_secs`; shown by `app.js`
`transferStatus`/`fmtEta`, the widget (`Shell.left`), the CLI and the phone (`formatEta`).
Files are sent byte-for-byte, never compressed.
**Tests (folder choice):** `core/tests/files.rs` `received_files_go_to_the_folder_chosen_last`,
`desktop/daemon/tests/files.rs` `the_folder_for_received_files_can_be_changed_and_is_remembered`,
UI suite ("Received files folder").

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
EchoTwin window at the right tab (and focuses it on Wayland).

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

**What:** a "live" phone outline in the panel: it fills with the phone's battery level
(amber under 20%, grey and empty offline), shows a count of new notifications and unread
texts, pulses once when something arrives and draws a ring while a file moves. Click: the
**Tile dashboard** (design: `docs/design/widget-concepts.html`, style 1): a battery ring
with network and signal, now playing (blurred album art with Qt 6.5+, a tint otherwise),
the newest notifications (app icon, Copy-code chip), recent texts with avatars, a drop tile
that shows the running transfer, and quick buttons Ring · Note · Saver · Open. Captions
open the full lists (back arrow); the popup always reopens on the dashboard. Drop files on
the icon or anywhere on the popup. Middle-click opens the window. Configurable background
opacity.

**How:** pure QML, no compiled plugin. `main.qml` runs `phonelink widget` once through
Plasma's `executable` data source, then always keeps one `phonelink widget --wait <hash>`
running: it subscribes to the service's events and prints the JSON only when its `hash`
changes (or after 60 s), and the next call starts at once (at most one a second; 10 s
retry while the service is down). Opening the popup and actions also read it once. Actions
run `phonelink <args>` with every argument shell-quoted by `shell.js`. The popup's
translucency fades Plasma's own popup background item (`applyOpacity`); KDE's Blur effect
blurs behind it.

| File | What to edit there |
|---|---|
| `plasmoid/…/ui/main.qml` | waiting loop (`poll`, `apply`, `refresh`), `run`, `sendUrls`, `openApp`, `newCount` (`seenMs`), colours, tooltip |
| `plasmoid/…/ui/CompactRepresentation.qml` | the live icon: `body` + battery fill, badge, `pulse`, transfer ring, drop handling |
| `plasmoid/…/ui/FullRepresentation.qml` | popup size (`Layout.preferred*`), header pill, the dashboard rows, `openList` + the three list pages, `CodeChip`, `applyOpacity` |
| `plasmoid/…/ui/Tile.qml`, `ListTile.qml`, `ListPage.qml` | the tile frame, a tile with a caption, a full-list page |
| `plasmoid/…/ui/BatteryRing.qml`, `NowPlayingTile.qml`, `ArtBackdrop.qml`, `DropTile.qml`, `QuickButtons.qml`, `Chip.qml` | one tile or piece each (`ArtBackdrop` is loaded through a `Loader` so a Qt without `QtQuick.Effects` falls back) |
| `plasmoid/…/ui/Empty.qml` | empty-state look |
| `plasmoid/…/ui/shell.js` | `quote`, `command`, `localPath` (security-relevant) |
| `plasmoid/…/ui/configAppearance.qml`, `config/main.xml`, `config/config.qml` | settings page and keys |
| `desktop/daemon/src/bin/phonelink.rs` `widget_state`, `widget_with_hash`, `widget_wait` | the JSON the widget reads (no moving media position, so the hash is stable) |

Install/update: `./packaging/install.sh` (uses `kpackagetool6`), then
`plasmashell --replace & disown`. **Tests:** `desktop/daemon/tests/status.rs` (widget JSON, `the_widget_waits_for_changes`);
the quoting was tested with hostile names; QML is parse-checked (`qmlformat`);
`packaging/plasmoid/tests/preview/render.sh out.png` renders the popup and icon with sample
data and stand-in KDE modules, to check layout without Plasma.

## 14. Desktop app window

**What:** the EchoTwin window in the phone app's style (design:
`docs/design/desktop-concepts.html`, layout A "Twin sidebar"). The sidebar is the phone's
Home screen: the twin tile (phone and this computer drawn and filled to their battery
levels, the link pill Wi-Fi / Bluetooth / Offline / Off between them; the dashes drift only
while connected and the window has focus), the connection tile, action tiles (Note, Send
files, Ring phone, Clipboard, Saver; toggles are tinted while on) and the now-playing tile
(blurred album art behind). Right side: pill tabs Notifications / Messages / Files, 18 px
cards. Settings sheet, pairing screen, "service not running" screen. Translucent with
optional blur.

**How:** `desktop/ui` is a framework-free page; `tauri-api.js` implements
`window.PhoneLinkAPI` with `invoke(...)`; `app.js` holds state and renders. Every command
in `desktop/app/src/main.rs` opens the socket, sends one `Request`, maps the `Reply`.
`watch_daemon` keeps one `Watch` subscription and emits `phonelink://event` /
`phonelink://daemon`.

| To change… | Edit |
|---|---|
| Colours, spacing | `desktop/ui/styles.css` `:root` tokens (`--r-tile` for tiles); shared classes `.tile`, `.icon-square`, `.pill` |
| Sidebar | `app.js` `renderDevice` (`phoneSvg`, `laptopSvg`, `linkState`), `renderMedia`; this computer's battery comes from `Reply::Status.battery` and `computer_battery_changed` |
| A tab | `app.js` `renderNotifications` / `renderThreads` + `renderConversation` / `renderFiles` |
| Settings sheet | `app.js` `settingsBody`, `appearanceSection`; click handling in the `onClick` switch (`case 'toggle-setting'` …) |
| New backend call | command in `main.rs` (+ register in `generate_handler!`), method in `tauri-api.js`, fake in `mock-api.js` |
| Opacity / blur (default 100 %, blur off; no blur requested while solid) | `app.js` `applyAppearance`; `styles.css` `body.in-app .window` (`--alpha`); `desktop/app/src/blur.rs` |
| Window size/frame | `main.rs` `WebviewWindowBuilder` in `main` |

**Tests:** UI suites (Playwright) in the session scratchpad (`test.js` preview, `test-app.js`
with a fake Tauri); see [DEVELOPING.md](DEVELOPING.md#ui-tests).

## 15. Phone app UI and setup checklist

**What:** dark Compose app with Home · Files · Settings. Home is the **Twin link** design
(`docs/design/phone-concepts.html`, style 3): this phone and the computer drawn side by side
and filled to their battery levels, the link between them (Wi-Fi green, Bluetooth amber,
grey when off; the dashes drift only while connected and on screen), then the connection
tile, the laptop's music card (§23), action tiles (Lock, Clipboard, Send files, Note, Ring,
Mute, My buttons; each only when usable), the full-width Presentation tile as the last row,
chips for what shows on the computer, and notes from the computer with a Copy button. The
v3 layout is in `docs/design/v3-preview.html`. A "Finish setup" checklist for
notification access, background running, SMS, Nearby devices, Phone status; pairing screen
when nothing is paired.

| To change… | Edit |
|---|---|
| Colours, shared widgets | `android/…/ui/Theme.kt` (`Pl`, `Panel` (18 dp tiles), `SwitchRow`, `Chip`) |
| The phone + laptop picture | `android/…/ui/TwinHeader.kt` (`TwinHeader`, `Link`, `PhoneOutline`, `LaptopOutline`, `LinkLine`) |
| Home | `android/…/ui/HomeScreen.kt` (`ConnectionTile`, `ComputerMusicCard`, `ActionGrid`, `ActionTile`, `PresentationTile`, `ShowChip`, `TransferCard`, `NoteCard`, `NoteDialog`) |
| This phone's battery/network on Home | `StatusWatcher.mine` (a `StateFlow`) → `UiState.my*` |
| Files / Settings / Setup / Pair | `android/…/ui/OtherScreens.kt` (`FilesScreen`, `SettingsScreen`, `SetupScreen` + `Step`, `PairScreen`) |
| What a button does | `MainActivity.kt` `actions` object (implements `ui/Model.kt` `Actions`) |
| Setup items | `ui/Model.kt` `Access` + `MainActivity.onResume` (reads permission state) |
| Phone switches | `Prefs.kt` |
| Persistent / transfer notifications | `Notifications.kt` |

## 16. CLI

`desktop/daemon/src/bin/phonelink.rs`. Commands: `status`, `devices`, `pair [--ttl]`,
`unpair <id>`, `send-text [--to] <text>`, `watch`, `notifications`, `reconnect`,
`send-file [--to] [--notify] <files>`, `transfers`, `cancel-transfer <id>`,
`media [action] [level]`, `widget [--wait <hash>] [--timeout <s>]`, `dismiss <id>`,
`sms-read <thread>`, `on`, `off`, `saver on|off`. Add `--json` for raw
replies. To add one: a variant in `enum Cmd`, a match arm in `run`, usually one `Request`.

## 17. Privacy guard rails

- `AndroidManifest.xml` removes microphone/location permissions that libraries could add.
- `scripts/audit-apk-permissions.sh` (run by CI) fails the build if the APK asks for
  anything outside its allow-list. **Adding a permission = add it to the manifest *and* the
  allow-list**, deliberately.
- Camera is handed back after the scan (`MainActivity.giveUpCamera`).
- Laptop stores notification/SMS/clipboard/music content in memory only.

## 18. One-time codes (OTP copy)

**What:** a notification carrying a one-time code ("482913 is your OTP…") gets a
**Copy 482913** button: on the popup, on the card in the window, and in the widget. The
code goes on the laptop clipboard (never back to the phone). Nothing is copied by itself.

**How:** `otp::find_code(title, text)` only answers when the text talks about a code
(keyword), takes 4–8 digit runs (also `482 913`, `4821-2290`, `G-482913`) and skips amounts,
card/account digits, dates, times, phone numbers. The closest candidate to a keyword wins.
The result is `NotificationInfo.code`.

| Layer | File | Functions |
|---|---|---|
| Laptop | `desktop/daemon/src/otp.rs` | `find_code` |
| Laptop | `desktop/daemon/src/daemon.rs` | `maybe_popup` (button `otp:<id>`), `copy_code`, `popup_action`, `Request::CopyCode` |
| Window | `app.js` `renderNotifications` (`copy-code`); `tauri-api.js` `copyCode` | |
| Widget | `FullRepresentation.qml` (copy button → `phonelink copy-code <id>`) | |

**Tests:** `otp.rs` unit tests (16 real-world samples), `tests/notifications.rs`
`one_time_codes_get_a_copy_button`, `tests/lock_clipboard.rs` (clipboard set, nothing sent
back), UI suite.

## 19. Battery alerts

**What:** "Galaxy S24 FE battery low · 15% left" and "… is fully charged" on the laptop;
"Battery low" / "Fully charged" for the laptop on the phone.

**How:** one rule set for both sides in `core/src/alerts.rs` (`BatteryAlerts::update`):
low at ≤ 15% while not charging, once per discharge (re-armed above 20% or when plugged
in); full at 100% while charging, once per charge and only if this charge was seen below
100%. The laptop keeps one per phone (`State::battery_alert`); the phone gets
`LinkEvent::ComputerStatus.alert` from the core. Switches: `battery_alerts` (laptop),
Settings → Battery alerts (phone, `Prefs.batteryAlerts`).

**Tests:** `alerts.rs` unit tests, `tests/status.rs` `low_and_full_battery_popups_come_once`,
FFI `status_both_ways`.

## 20. Open a link on the other device

**What:** phone: Share → **Open on computer** opens the link in the laptop's browser.
Laptop: `phonelink open-url <url>`, or a note that is only a link, arrives on the phone as
a "Link from …" notification; tapping opens it (Android doesn't let a background app open
a page by itself).

**How:** `AppMessage::OpenUrl`. Both sides accept only `is_web_link` (http/https, a host, no
spaces or control characters, ≤ 2048 bytes). The laptop also needs it fresh (2 min) and the
`open_links` switch on, then runs `xdg-open <url>` (one argument, no shell) through
`Effects::open_url`. Laptop → phone is queued.

| Layer | File | Functions |
|---|---|---|
| Phone | `OpenLinkActivity.kt`, `LinkManager.openOnComputer`, `Notifications.link` | |
| FFI | `core-ffi/src/lib.rs` | `open_url`, `LinkEvent::OpenUrlRequested` |
| Laptop | `daemon.rs` `open_link_from`, `Request::OpenUrl`; `effects.rs`; `open.rs` `open_url` | |
| Window | `app.js` `submitNote` (a link-only note → `openUrl`) | |

**Tests:** `message.rs` `only_plain_web_links_are_accepted`, `tests/status.rs` `links_…`,
FFI `lock_and_clipboard_call_sequence`, UI suite.

## 21. Ring my phone / laptop

**What:** find a device by making it ring. The phone plays its alarm sound at full alarm
volume (heard on silent) and vibrates, with a Stop button; the laptop loops a two-tone
sound loud and unmuted, with a Stop popup. Both stop after a minute and put the volume
back.

**How:** `AppMessage::Ring { on, sent_ms }`, connected only, fresh within 2 min. Laptop:
`ring.rs` writes `ring.wav` once (generated, no files shipped), plays it with `pw-play`,
`paplay` or `aplay`, sets the volume with `wpctl` (remembered and restored). `allow_ring`
switch. Controls: window quick action, widget header button, `phonelink ring [stop]`;
phone Home "Ring" tile.

| Layer | File | Functions |
|---|---|---|
| Phone | `Ringer.kt` (`Ringer`, `StopRingReceiver`), `LinkManager.ringComputer` | |
| FFI | `core-ffi/src/lib.rs` | `ring_computer`, `LinkEvent::RingRequested`, `Device.can_ring` |
| Laptop | `ring.rs` `Ringer`, `ring_wav`; `effects.rs`; `daemon.rs` `rung_by`, `stop_ringing`, `Request::Ring` | |

**Permission:** `VIBRATE` (no prompt). **Tests:** `ring.rs` unit tests (WAV header,
`wpctl` parsing), `tests/status.rs` `ringing_both_ways`, FFI test, UI suite.

## 22. Calls

**What:** nothing on screen until the phone rings. Then a **card slides down at the top
centre of the laptop screen** (also over a full-screen presentation): the caller's initial,
name ("Unknown caller" without one), "Incoming call · <phone>", a red **Decline** and a green
**Answer**. Answering keeps the call on the phone (audio stays there); the card goes, and the
**call strip** shows at the top of the widget popup and the window's sidebar: caller, "On call ·
mm:ss", and the buttons the phone's call notification offers right now (**End** always;
**Mute**, **Speaker**, **Hold** when the dialer shows them). Music or video playing on the
laptop pauses during the call and resumes after. The panel icon gets a small green handset
during a call. Off (`call_popup`, window → Settings → "Show calls at the top of the screen"):
a KDE notification with Answer / Decline instead.

**How:** the phone reads call notifications (`Notification.CATEGORY_CALL`: Samsung's dialer,
WhatsApp and others): incoming if `android.callType` is 1 or a button says Answer/Accept;
caller from `android.callPerson`, else the title; each button is classified by its title
(answer|accept|pick up · decline|reject|dismiss · hang up|end|disconnect · mute · speaker ·
hold|resume). It sends `CallInfo` (`calls/2`) with those buttons whenever the call, the caller
or the buttons change; an older laptop (`calls/1`) gets `CallState`. A press on the laptop
sends `CallAction`; the phone presses the same notification button (on Android 14+ allowed to
open the call screen from the background). If no call notification shows up within 1.5 s of
the phone's own "ringing" state, it sends Ringing without a name or buttons.

On the laptop, a new ring with `call_popup` on starts the app in the background (`Effects::
start_app`, at most every 30 s; `phonelink-app --background` opens no main window and quits
a minute after its last window hid), which shows the card as a layer-shell overlay
(`overlay.rs`, see below). The card window is made after the call was announced, so the page
asks for it on load (`phonelink://call-card-ready`; the app answers with the latest call it
kept in `LAST_CALL`, or from `Reply::Status.call` when it was started for this ring), and says
`phonelink://call-card-visible` once the card is really on screen (logged with the window's
mapped size). **The KDE notification with Answer / Decline shows on every ring regardless**
(even with "Desktop notifications" off), so a call can never go unseen; the card is an extra
on top. Both close on answer, decline, end, or when the phone leaves. The service logs each
call change and the app's overlay steps (`journalctl --user -u phonelinkd | grep -i call`).

In-call controls are **End only** (plus Answer / Decline while ringing): the phone reads those
three from the call notification's buttons by title; the laptop ignores anything else an older
phone reports (Mute, Speaker, Hold), and `phonelink call` takes `answer|decline|end`.

The timer counts from this computer's clock when the call became active
(`active_ms`), and ticks only while a call is active and the window or popup is open (the
widget hash never changes because of it). The laptop pauses every MPRIS player that is
*Playing* and later resumes only those, if they are still paused (`mpris::CallPause`). A phone
that disconnects mid-call counts as the call ending. Switches: `call_popup`,
`pause_media_on_call`. CLI: `phonelink call answer|decline|end`.

**Overlays (`desktop/app/src/overlay.rs`):** Wayland doesn't let a normal window place itself
or stay above a full-screen window, so the card (and the presentation pointer, §24) are
**layer-shell** surfaces on the Overlay layer (KWin supports the protocol; Plasma's panels use
it), never taking the keyboard: needs `libgtk-layer-shell` (cargo feature `layer-shell`, on by
default; `install.sh` builds without it and says so if the headers are missing). On X11 they
are always-on-top windows placed on the primary monitor. Check on your desktop with
`phonelink-app --overlay-check`: always its own process (also while EchoTwin runs), it shows a
sample card and pointer for 20 s through the same steps as a real call and prints each step
(display, layer-shell built in and supported, realized too early?, layer surface, page shown,
window mapped size) and a result line, to paste when asking for help. Verified here under sway (headless):
the card sits at the top centre above a full-screen window.

| Layer | File | Functions |
|---|---|---|
| Phone | `CallWatcher.kt` (`posted`, `kindOf`, `act`, `decline`, `removed`, `telephony`, `resend`), `MirrorService`, `StatusWatcher`, `LinkManager.sendCallInfo` | |
| FFI | `core-ffi/src/lib.rs` | `send_call_info` (`calls/2` or `CallState`), `send_call_state`, `LinkEvent::{CallActionRequested, CallDeclineRequested}`, `CallActionFfi` |
| Laptop | `daemon.rs` `call_changed` (`CALL_ACTIONS`), `call_notification`, `call_card_shown`, `call_action`, `call_view`, `start_app`; `Request::{CallAction, CallPopupShown, DeclineCall, AppLog}`; `WatchEvent::CallChanged` (`actions`, `active_ms`, `card`); `Reply::Status.call` | `mpris.rs` `CallPause`; `popups.rs` `start_app_background` |
| App | `desktop/app/src/main.rs` `call_card`, `call_card_on_start`, `call_action`, `--background`, `exit_when_idle`, `--overlay-check`; `overlay.rs` | |
| Window | `call.html` + `call.js` (the card); `app.js` `callTile`, `updateCallTimer`, `call-action` | |
| Widget | `CallStrip.qml`, `CompactRepresentation.qml` (handset badge); `phonelink widget` `call` | |

**No "Silence":** Android only lets an app mute the ringer with Do Not Disturb access. If
Answer doesn't work on a phone, the fallback (`TelecomManager.acceptRingingCall`) needs the
ANSWER_PHONE_CALLS permission, which EchoTwin doesn't ask for.
**Tests:** `tests/calls.rs` (stand-in media players; the card-or-notification fallback after
4 s, buttons reaching the phone, refusals, `active_ms`, a `calls/1` phone), FFI test, UI suites
(the window's call tile and timer; `overlay.test.js` for the card).

## 23. Remote: the phone controls the laptop

**What:** on the phone's Home, right under the Connection tile, a **music card** for what
plays on the laptop (any MPRIS player: browsers, Spotify, VLC…): the album cover blurred
behind it, title, "Playing on <laptop> · <player>", and − ⏮ ▶/❚❚ ⏭ +. It shows while
something plays and for 5 minutes after it last played (so you can resume), then hides. A
**Mute** tile mutes or unmutes the laptop. **My buttons** (a tile only when the laptop has at
least one) are name + command pairs kept on the laptop (window → Settings → Phone buttons;
config `[[commands]]`). Since v3 there are no default buttons: the old "Suspend" and
"Mute / unmute" defaults are dropped from existing configs when they load; buttons you made
stay.

**How:** `mpris::watch` follows players by D-Bus signals (`PropertiesChanged`,
`NameOwnerChanged`; no polling) and reports the active one (playing, else the last shown if
paused), with its `mpris:artUrl`. Album art: only `file://` URLs (Firefox, Chrome, VLC give
these); `art.rs` makes a JPEG of at most 256 px and 64 KiB (once per file, `ArtCache`), its
key is 16 hex digits of BLAKE3. The laptop sends `MediaArt` to phones with `remoteart/1`
**before** the `ComputerNowPlaying` that names the key, and again on connect. The daemon never
fetches from the web, so players that only give `https://` art (Spotify) show no picture.
The phone sends `ComputerMediaControl`; volume runs `wpctl set-volume` (fallback `pactl`),
`ToggleMute` (`mute/1`) runs `wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle` (fallback `pactl
set-sink-mute`). Buttons: the phone only gets ids (8 hex digits of BLAKE3(name)) and names; a
`RunCommand` runs `sh -c <command>` in the desktop session, detached, only if
`allow_commands` is on, the id matches and the press is under 2 minutes old. A popup says
what ran. **The phone can only press buttons you wrote on the laptop.**

| Layer | File | Functions |
|---|---|---|
| Laptop | `desktop/daemon/src/mpris.rs` | `watch`, `ArtCache`, `active_player`, `control` |
| Laptop | `desktop/daemon/src/art.rs` | `art_path`, `thumbnail`, `key` |
| Laptop | `desktop/daemon/src/effects.rs` | `volume`, `mute_toggle` |
| Laptop | `desktop/daemon/src/config.rs` | `CommandEntry` (`id`), `check_commands`, `Config::load` (drops the old defaults) |
| Laptop | `daemon.rs` | `computer_media` task (art first), `remote_commands`, `run_command`, `Request::{Commands, SetCommands}` |
| Window | `app.js` `commandsSection`, `submitCommand`, `saveCommands` | |
| FFI | `core-ffi/src/lib.rs` | `computer_media_control` (`ToggleMute` needs `mute/1`), `run_command`, `LinkEvent::{ComputerMedia, ComputerMediaArt, RemoteCommands}`, `Device.{can_remote, can_mute}` |
| Phone | `ui/HomeScreen.kt` (`ComputerMusicCard`, `ActionGrid`), `ui/RemoteScreen.kt` (`ButtonsScreen`), `LinkManager.{computerMedia, computerArt, computerPlayedMs, commands, runCommand}` | |

**Tests:** `desktop/daemon/tests/remote.rs` (stand-in MPRIS player on a private D-Bus;
`the_laptops_album_art_reaches_the_phone_before_the_track_and_mute_works`: art before track,
none for `https://`, Mute), `art.rs` unit test, `config.rs`
`the_old_default_buttons_are_dropped_but_own_buttons_stay`, FFI test, UI suite.

## 24. Presentation remote

**What:** Home → **Presentation** (a full-width tile, always the last row): Previous and
Next side by side and the same size, one **Start / End** button and **Black**; the screen
stays on and the phone's volume keys work as a clicker (down = next, up = previous). A chip
at the top picks the app, because each starts differently:

| Chip | Start | End |
|---|---|---|
| PowerPoint / Impress · F5 (default) | F5 | Esc |
| Google Slides · Ctrl+F5 | Ctrl+F5 | Esc |
| Canva · Ctrl+Alt+P | Ctrl+Alt+P | Esc |
| Full screen · F11 | F11 | F11 (toggles) |

Start always pressed F5 before v3; in a browser F5 reloads the page, which is why Canva never
started and a second press ended the presentation. The Start button turns into a red
**■ End** once pressed and back after; it starts as Start each time the screen opens. The
chip (stored in `Prefs` `present_app`) shows only for laptops with `present/2`. Honest
limits: Canva's Ctrl+Alt+P comes from a third-party shortcut list, not Canva's own help (the
chip shows the key so you can try another); Black (B) works in PowerPoint, Impress and Google
Slides. The first press makes KDE ask once to allow remote control; it is remembered.

**How:** `present.rs` uses xdg-desktop-portal `org.freedesktop.portal.RemoteDesktop`
(keyboard only, `persist_mode` 2): `CreateSession` → `SelectDevices` (with the saved
`restore_token`) → `Start` → `NotifyKeyboardKeycode` press/release. `keys()` gives a chord
per key, pressed in order and released in reverse (also after an error, so nothing stays
held): Page Down 109, Page Up 104, F5 63, Esc 1, B 48, Ctrl+F5 29+63, Ctrl+Alt+P 29+56+25,
F11 87. One thread owns the session; keys are queued in arrival order; a closed session is
reopened once. The restore token lives in `<data dir>/remote-control.token` (0600). No portal
on X11 → `xdotool key …` (`ctrl+F5`, `ctrl+alt+p`, `F11`) if installed. Refused when
`presentation_remote` is off or the key is over 10 s old. **The phone can only send these
keys.**

**Pointer:** the space between the keys and the bottom row is a **touchpad**: drag on it and
a pointer moves on the laptop's screen, above the presentation (absolute: where the finger is
on the pad's 16:9 area is where the pointer is). Lifting the finger hides it. The square
button next to Black picks the look: **Dot**, **Ring** or **Spotlight** (the screen darkens
around a clear circle), five colours, three sizes (saved on the phone). The phone sends at
most one position every 33 ms as a *fleeting* message (`Control::Fleeting`: sent only while
connected, never queued: a late pointer is worse than none); the laptop clamps the values,
checks `presentation_remote`, starts the app if needed and broadcasts `WatchEvent::Pointer`.
The app draws it in a full-screen, click-through overlay (`pointer.html`; layer-shell, see
§22) and hides the window when the finger lifts or nothing came for 3 s. `phonelink widget
--wait` skips pointer events without reading the state again, and the window ignores them.
If the overlay can't be made, the phone hears so once per connection
(`Request::PointerUnavailable`). Verified here under sway (headless): drawn at the right spot
above a full-screen window; click-through and smoothness need a real desktop.

| Layer | File | Functions |
|---|---|---|
| Core | `wire.rs` `Control::Fleeting`, `MAX_FLEETING_LEN`; `node.rs` `send_fleeting` | |
| Phone | `ui/RemoteScreen.kt` `PointerPad`, `area`, `PointerStyleDialog`; `LinkManager.pointer`; `Prefs.pointerStyle` | |
| FFI | `pointer`, `PointerShapeFfi`, `Device.can_point` | |
| Laptop | `daemon.rs` `AppMessage::Pointer` arm, `Request::PointerUnavailable`; `bin/phonelink.rs` `widget_wait` (skips pointer events) | |
| App | `main.rs` `pointer_moved`, `show_pointer`, `pointer_watchdog`; `pointer.html` + `pointer.js` | |

**Tests:** `desktop/daemon/tests/present.rs` (stand-in portal: sequence, chords, keyboard
only, remembered permission, session reuse, refusal), `present.rs` unit test (key table),
`remote.rs` `presentation_keys_are_refused_when_late_or_switched_off` and
`the_pointer_moves_reach_the_app_clamped_and_never_wake_the_widget`, `core/tests/sessions.rs`
`fleeting_messages_go_only_while_connected_and_are_never_queued` (and over Bluetooth in
`stream.rs`), FFI test, `overlay.test.js` (pointer position, shapes, hiding).

## 25. New SMS from the laptop

**What:** Messages → **New**: type a name (the phone suggests contacts) or a number, write,
Send (Ctrl+Enter). The new conversation appears once the phone has sent it.

**How:** `Request::SendSmsTo` checks the number (`sms::valid_address`: 3–20 characters of
digits, spaces, `+ - ( )`, at least 3 digits) and sends `SmsSend` with thread 0; the phone
sends it like any reply and its SMS database change brings the conversation over. Contacts:
`Request::Contacts` asks the phone (`ContactsQuery`), waits up to 3 s, returns at most 20; the
phone searches `ContactsContract.CommonDataKinds.Phone.CONTENT_FILTER_URI` (READ_CONTACTS,
already asked for with SMS). Nothing is stored on the laptop.

**Tests:** `desktop/daemon/tests/sms.rs` `a_new_conversation_can_be_started_from_the_computer`,
`without_contacts_support_the_number_is_typed_by_hand`; FFI test; UI suite ("New SMS").
