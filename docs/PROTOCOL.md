# Protocol

What goes over the wire between the phone and the laptop, and the rules for changing it.
Source of truth: `core/src/wire.rs` (framing, `Control`) and `core/src/message.rs`
(`AppMessage`, capabilities, limits).

Contents
- [Layers](#layers)
- [Frames and Control messages](#frames-and-control-messages)
- [Application messages (AppMessage)](#application-messages-appmessage)
- [Capabilities](#capabilities)
- [Queued vs right now](#queued-vs-right-now)
- [Limits and freshness](#limits-and-freshness)
- [File data streams](#file-data-streams)
- [Compatibility rules (read before changing anything)](#compatibility-rules-read-before-changing-anything)
- [The local control socket (laptop only)](#the-local-control-socket-laptop-only)
- [The touchpad session](#the-touchpad-session)

## Layers

```
 UDP ── QUIC (quinn) ── TLS 1.3, both certificates pinned, ALPN "phonelink/1"
                         ├─ control stream: Frame(Control) …  Hello → Deliver/Ack …
                         └─ data streams (one per file): FileStart → FileResume → bytes → hash → FileResult
 RFCOMM (Bluetooth) ── TLS 1.3 (tokio-rustls), ALPN "phonelink-stream/1" ── same control frames + Ping
```

## Frames and Control messages

Every frame on a stream is `[u32 big-endian length][postcard-encoded Control]`, at most
1 MiB (`MAX_FRAME_LEN`). `read_frame` / `write_frame` work on any `AsyncRead`/`AsyncWrite`.
Unknown variants decode as `Frame::Unknown` and are skipped.

| # | Control | Fields | Meaning |
|---|---|---|---|
| 0 | `Hello` | `version, name, capabilities` | First frame of a session, both ways |
| 1 | `Deliver` | `id: MsgId, body` | An `AppMessage` (encoded) that must survive disconnects |
| 2 | `Ack` | `id` | "Stored/processed `id`" → sender deletes it from its outbox |
| 3 | `PairRequest` | `name, proof` | Phone → laptop during pairing (HMAC proof) |
| 4 | `PairResponse` | `name, proof` | Laptop → phone during pairing |
| 5 | `Reject` | `reason` | Session refused (e.g. protocol too old); reason `"paused"` = the connection was switched off there |
| 6 | `Ping` | | Keep-alive on Bluetooth stream links (QUIC has its own) |
| 7 | `Links` | `bluetooth: Option<String>` | Laptop → phone after Hello: Bluetooth adapter address for the backup |
| 8 | `FileStart` | `id, name, size, mime` | First frame on a file data stream |
| 9 | `FileResume` | `offset` | Receiver: "I already have `offset` bytes" |
| 10 | `FileResult` | `ok, message` | Receiver, after the hash: stored, or failed |
| 11 | `Fleeting` | `body` | An `AppMessage` sent only if the session is open now: no outbox, no `Ack`, dropped if over 256 bytes (`MAX_FLEETING_LEN`). `Node::send_fleeting`. Used for `Pointer`. Older peers skip it as unknown |

`PROTOCOL_VERSION` is 1.

## Application messages (AppMessage)

Carried inside `Deliver.body`, postcard-encoded. Anything this build doesn't know decodes as
`Unsupported` and is ignored. Direction: P = phone, C = computer.

| # | AppMessage | Dir | Capability | Handled on the laptop in | Handled on the phone in |
|---|---|---|---|---|---|
| 0 | `Text { text }` | both | `messages/1` | `daemon.rs` event_pump → popup + `WatchEvent::Text` | `LinkEvent::Text` → `Notifications.message` |
| 1 | `Unsupported` | — | — | ignored | ignored |
| 2 | `NotificationPosted(PhoneNotification)` | P→C | `notifications/1` | `notifications::Store::post`, popup | sent by `post_notification` |
| 3 | `NotificationRemoved { key }` | P→C | `notifications/1` | `Store::removed_on_phone` | `remove_notification` |
| 4 | `NotificationSnapshot { items }` | P→C | `notifications/1` | `Store::snapshot` | `send_snapshot` on connect |
| 5 | `AppIcon { package, png }` | P→C | `notifications/1` | `save_icon` | `send_app_icon` |
| 6 | `DismissNotification { key }` | C→P | `notifications/1` | sent on dismiss when sync is on | `LinkEvent::DismissRequested` → `MirrorService.dismiss` |
| 7 | `ReplyNotification { key, text }` | C→P | `notifications/1` | `Request::ReplyNotification` | `LinkEvent::ReplyRequested` → `MirrorService.reply` |
| 8 | `NotificationAction { key, index }` | C→P | `notifications/1` | `Request::NotificationAction` | `LinkEvent::ActionRequested` → `MirrorService.press` |
| 9 | `ActionResult { key, ok, message }` | both | — | `WatchEvent::ActionResult` (and lock result to phone) | `send_action_result`; `LinkEvent::RequestResult` |
| 10 | `LockComputer { sent_ms }` | P→C | `lock/1` | `State::lock_requested` → `lock.rs` | `lock_computer` |
| 11 | `Clipboard { text, copied_ms }` | both | `clipboard/1` | `ClipboardSync::set` / `share_copies` | `send_clipboard`; `LinkEvent::ClipboardReceived` |
| 12 | `NowPlaying(Option<NowPlaying>)` | P→C | `media/1` | `MediaStore::update` | `send_now_playing` (MediaWatcher) |
| 13 | `MediaArt { key, image }` | P→C, and C→P | `media/1`; C→P `remoteart/1` | P→C: `MediaStore::set_art`. C→P: sent by the `computer_media` task before the `ComputerNowPlaying` naming `key` | P→C: `send_media_art`. C→P: `LinkEvent::ComputerMediaArt` (checked `is_art_image`, key ≤ 64) |
| 14 | `MediaControl { action }` | C→P | `media/1` | `Request::MediaControl` | `LinkEvent::MediaControlRequested` → `MediaWatcher.control` |
| 15 | `FileCancel { id }` | both | `files/1` | `Transfers::on_message` | `Transfers::on_message` (in core) |
| 16 | `SmsSnapshot { threads }` | P→C | `sms/1` | `SmsStore::snapshot` | `sms_snapshot` on connect |
| 17 | `SmsThreadUpdate(SmsThread)` | P→C | `sms/1` | `SmsStore::update` | `sms_thread_update` |
| 18 | `SmsLoadOlder { thread, before_ms }` | C→P | `sms/1` | `Request::SmsLoadOlder` | `LinkEvent::SmsOlderRequested` → `SmsSync.older` |
| 19 | `SmsOlder { thread, messages, has_older }` | P→C | `sms/1` | `SmsStore::older` | `sms_older` |
| 20 | `SmsSend { client_id, thread, address, body }` | C→P | `sms/1` | `State::send_sms` | `LinkEvent::SmsSendRequested` → `SmsSync.send` |
| 21 | `SmsSendResult { client_id, state, message }` | P→C | `sms/1` | `SmsStore::send_result` | `sms_send_result` |
| 22 | `PhoneStatus(PhoneStatus)` | P→C | `status/1` | `phone_status` map → `DeviceInfo.status` | `send_phone_status` (StatusWatcher) |
| 23 | `ComputerStatus { battery, charging }` | C→P | `status/1` | sent by `send_battery` / `battery_watch` | `LinkEvent::ComputerStatus` → `Device.battery` |
| 24 | `ClipboardImage { transfer_id, copied_ms }` | C→P | `clipimage/1` | `State::send_clipboard_image` (after `send_file`) | `LinkEvent::ClipboardImageIncoming` → `FileShare.expectClipboard` → clipboard |
| 25 | `CallState { state: CallPhase, caller, key }` | P→C | `calls/1` | `State::call_changed` (popup, pause media, `call_changed` event) | `send_call_state` (CallWatcher) |
| 26 | `CallDecline { key }` | C→P | `calls/1` | `State::decline_call` (`DeclineCall`, popup button) | `LinkEvent::CallDeclineRequested` → `CallWatcher.decline` |
| 27 | `OpenUrl { url, sent_ms }` | both | `links/1` | received: `State::open_link_from`; sent: `Request::OpenUrl` (queued) | received: `LinkEvent::OpenUrlRequested` → `Notifications.link`; sent: `open_url` |
| 28 | `Ring { on, sent_ms }` | both | `ring/1` | received: `State::rung_by` → `Effects::ring`; sent: `Request::Ring` | received: `LinkEvent::RingRequested` → `Ringer`; sent: `ring_computer` |
| 29 | `ComputerNowPlaying(Option<NowPlaying>)` | C→P | `remote/1` | `computer_media` task (from `mpris::watch`), on connect | `LinkEvent::ComputerMedia` → Remote screen |
| 30 | `ComputerMediaControl { action }` | P→C | `remote/1` | `mpris::control` (volume via `Effects::volume`) | `computer_media_control` |
| 31 | `RemoteCommands { items: Vec<RemoteCommand { id, name }> }` | C→P | `remote/1` | `State::remote_commands` (on connect, after `SetCommands`) | `LinkEvent::RemoteCommands` |
| 32 | `RunCommand { id, sent_ms }` | P→C | `remote/1` | `State::run_command` → `Effects::run_command`; answer `ActionResult` `cmd:<id>` | `run_command` |
| 33 | `PresentKey { key: PresentKeyKind, sent_ms }` | P→C | `present/1` | `present::Presenter` (portal RemoteDesktop); errors as `ActionResult` `present` | `present_key` |
| 34 | `ContactsQuery { req, query }` | C→P | `contacts/1` | `State::contacts` (`Request::Contacts`) | `LinkEvent::ContactsRequested` → `SmsSync.searchContacts` |
| 35 | `ContactsResult { req, items: Vec<Contact { name, number }> }` | P→C | `contacts/1` | resolves the waiting `Contacts` request | `contacts_result` |
| 36 | `ApproveRequest { id, what, host, nonce_hex, sent_ms }` | — | — | retired in v3 (sudo approval removed), never sent; ignored if received | — |
| 37 | `ApproveResponse { id, approved, signature }` | — | — | retired in v3, never sent; ignored | — |
| 38 | `ApproveEnroll { public_key }` | — | — | retired in v3, never sent; ignored | — |
| 39 | `CallInfo { key, caller, phase: CallPhase, actions: Vec<CallActionKind> }` | P→C | `calls/2` | `State::call_changed` (card or notification, pause media, call strip) | `send_call_info` (`CallWatcher`) |
| 40 | `CallAction { key, action: CallActionKind }` | C→P | `calls/2` | `State::call_action` (`Request::CallAction`, the card, popup buttons) | `LinkEvent::CallActionRequested` → `CallWatcher.act` |
| 41 | `Pointer { x, y, visible, shape: PointerShape, color, size }` | P→C | `pointer/1` | fleeting only; clamped (x, y ≤ 10000, size ≤ 2, color & 0xFFFFFF), needs `presentation_remote`; → `WatchEvent::Pointer` for the app's overlay | `pointer` (the Presentation screen's touchpad) |
| 42 | `UnlockComputer { sent_ms }` | P→C | `unlock/1` | `State::unlock_requested`: needs `allow_unlock` (default off) and < 30 s; logind `Session.Unlock`; popup; `ActionResult` `unlock` | `unlock_computer` (fleeting, after the fingerprint prompt) |
| 43 | `LockState { locked, unlock_allowed }` | C→P | `lockstate/1` | `send_lock_state` (fleeting: on connect, on every lock change, when `allow_unlock` changes) | `LinkEvent::ComputerLockChanged`, `Device.locked` / `unlock_allowed` (cleared on disconnect) |

Payload types (`message.rs`): `PhoneNotification { key, package, app, title, text,
posted_ms, can_reply, actions }`, `NowPlaying { app, package, title, artist, album,
duration_ms, position_ms, playing, can_previous, can_next, volume, max_volume, art_key }`,
`SmsThread { id, address, name, messages, has_older }`, `SmsMessage { id, from_me, body,
date_ms, state, read }`, `SmsState { Received, Sent, Sending, Failed }`,
`PointerShape { Dot, Ring, Spotlight }`, `CallActionKind { Answer, Decline, End, Mute, Speaker, Hold }` (at most `MAX_CALL_ACTIONS` = 6 in a `CallInfo`; since v3.1 phones send only Answer / Decline / End and laptops use only those), `MediaAction { PlayPause, Next, Previous, VolumeUp, VolumeDown, SetVolume(u32), ToggleMute }`, `PresentKeyKind { Next, Previous, Start, End, Blank, StartSlides, StartCanva, StartFullscreen }`,
`PhoneStatus { battery: Option<u8>, charging, network, signal }`,
`CallPhase { Idle, Ringing, Active }`.

## Capabilities

Sent in `Hello`. A feature is used only if the *other* side listed it
(`Node::peer_supports`, `has_cap` in core-ffi, `connected_with` in the daemon), and the UIs
hide what's missing. That is how an older phone app keeps working with a newer laptop.

| String | Constant | Feature |
|---|---|---|
| `messages/1` | `node::CAP_MESSAGES` | Notes (Text), always present |
| `notifications/1` | `CAP_NOTIFICATIONS` | Notification mirroring + reply/action/dismiss |
| `lock/1` | `CAP_LOCK` | Lock the computer from the phone |
| `lockstate/1` | `CAP_LOCKSTATE` | The computer says whether its screen is locked (the phone's Lock / Unlock tile) |
| `unlock/1` | `CAP_UNLOCK` | Unlock the computer from the phone (after the phone's fingerprint or screen lock) |
| `clipboard/1` | `CAP_CLIPBOARD` | Clipboard both ways |
| `media/1` | `CAP_MEDIA` | Now playing + controls |
| `files/1` | `transfer::CAP_FILES` | File transfers |
| `sms/1` | `CAP_SMS` | SMS |
| `status/1` | `CAP_STATUS` | Battery and network both ways |
| `clipimage/1` | `CAP_CLIPBOARD_IMAGE` | Images copied on the laptop go to the phone's clipboard |
| `calls/1` | `CAP_CALLS` | Call popups with Decline |
| `pointer/1` | `CAP_POINTER` | The presentation pointer (`Pointer` as fleeting messages) |
| `calls/2` | `CAP_CALLS2` | `CallInfo` with the call notification's buttons, and `CallAction` |
| `links/1` | `CAP_LINKS` | Open a web link on the other device |
| `ring/1` | `CAP_RING` | Ring the other device |
| `remote/1` | `CAP_REMOTE` | The phone as a remote: the laptop's music card and My buttons |
| `present/1` | `CAP_PRESENT` | Presentation clicker (Next, Previous, Start = F5, End, Blank) |
| `present/2` | `CAP_PRESENT2` | The per-app start keys `StartSlides`, `StartCanva`, `StartFullscreen` |
| `mute/1` | `CAP_MUTE` | `MediaAction::ToggleMute` on the computer (phone → computer only) |
| `remoteart/1` | `CAP_REMOTE_ART` | The computer sends its album art (`MediaArt` C→P) |
| `contacts/1` | `CAP_CONTACTS` | Contact search for a new SMS |

Lists: laptop `start_node` in `desktop/daemon/src/daemon.rs`; phone `PhoneLink::start` in
`core-ffi/src/lib.rs`.

## Queued vs right now

`Node::send` always writes to the outbox, so a message survives a link dropping *mid-flight*.
The difference is **who gets it**:

| Kind | Rule | Messages |
|---|---|---|
| **Queued** for every paired device (arrives after reconnect, up to 7 days) | `send_all`, or `node.send` to a chosen device | `Text` (notes), `NotificationSnapshot`, `ActionResult`, `FileCancel`, laptop → phone requests (Dismiss/Reply/Action) |
| **Notifications** | queued only if the phone's *Queue notifications while disconnected* is on (default off); otherwise connected computers only | `NotificationPosted`, `NotificationRemoved`, `AppIcon` |
| **Connected only** (sent again on every connect) | `send_connected(cap, …)` / `connected_with(cap)` | `NowPlaying`, `MediaArt`, `SmsSnapshot`, `SmsThreadUpdate`, `PhoneStatus`, `ComputerStatus`, `CallState` |
| **Now or never** (refused when offline, plus a timestamp check) | `connected_one` / `is_connected` + `is_fresh` | `LockComputer` (2 min), `Clipboard` (2 min), `SmsSend` (laptop refuses if not connected), `CallDecline`, `Ring` (2 min), `OpenUrl` phone → laptop (2 min) |
| **Queued, no time limit** | `node.send` | `OpenUrl` laptop → phone (it becomes a notification, so late is fine) |

## Limits and freshness

All in `message.rs` unless noted.

| Constant | Value | Use |
|---|---|---|
| `MAX_FRAME_LEN` (`wire.rs`) | 1 MiB | largest frame |
| `MAX_NOTIFICATION_TEXT` | 4000 bytes (2000 before v3) | notification text; the phone keeps the newest lines of a long chat and starts it with "…" |
| `MAX_SNAPSHOT_ITEMS` | 100 | notifications per snapshot |
| `MAX_CLIPBOARD_TEXT` | 64 KiB | clipboard text |
| `MAX_ART_BYTES` | 64 KiB | album art (JPEG/PNG only, `is_art_image`) |
| `SMS_MAX_THREADS` / `SMS_MAX_MESSAGES` / `SMS_MAX_BODY` | 30 / 30 / 4000 | SMS sync sizes |
| `UNLOCK_MAX_AGE_MS` | 30 s | an older unlock request is refused |
| `LOCK_MAX_AGE_MS`, `CLIPBOARD_MAX_AGE_MS`, `LINK_MAX_AGE_MS`, `RING_MAX_AGE_MS` | 2 min | `is_fresh` window (either clock direction) |
| `MAX_CALLER` | 100 bytes | caller name |
| `MAX_COMMAND_NAME`, `MAX_COMMANDS` | 40 bytes, 20 | Remote buttons |
| `MAX_CONTACT_QUERY`, `MAX_CONTACTS` | 60 bytes, 20 | contact search |
| `COMMAND_MAX_AGE_MS`, `PRESENT_MAX_AGE_MS` | 2 min, 10 s | a button press / slide key older than this is refused |
| `MAX_URL` | 2048 bytes | links; `is_web_link` also requires `http(s)://`, a host, no spaces or control characters |
| `POPUP_MAX_AGE_MS` (`desktop/daemon/src/notifications.rs`) | 2 min | older notifications go to the list without a popup |
| `MAX_STORED` (`notifications.rs`) | 200 | notifications kept on the laptop |
| `PENDING_RETENTION_SECS` (`node.rs`) | 7 days | unsent messages are dropped after this |

Receivers also clamp what they get (e.g. battery ≤ 100, signal ≤ 4, network label ≤ 16
bytes in `daemon.rs`; `safe_package` for icon file names).

## File data streams

`core/src/transfer.rs`. The control channel only carries `FileCancel`; the file itself goes
on its own QUIC stream (`Node::open_data` / `Node::data_streams`):

1. Sender: `FileStart { id, name, size, mime }`.
2. Receiver: finds or creates `.<name>.part`, answers `FileResume { offset }`.
3. Sender: raw bytes from `offset` to `size` (256 KiB chunks), then the 32-byte BLAKE3 hash
   of the whole file. The receiver fsyncs every 8 MiB.
4. Receiver: compares the hash, renames atomically to a safe unique name, answers
   `FileResult { ok, message }`. A mismatch deletes the part and retries (max 3 attempts).

State (offset, paths, status) is in `transfers.sqlite3`, so restarts resume. On a Bluetooth
session transfers wait (`waiting`) until QUIC is back.

## Compatibility rules (read before changing anything)

1. **Append only.** postcard identifies enum variants by **position**. Never insert,
   reorder or remove variants of `Control` or `AppMessage`; never reorder or remove fields
   of a struct that is already sent. Add new variants at the end; add new data as a new
   variant (or a new struct used by a new variant).
2. **New feature ⇒ new capability string** (or `…/2` for an incompatible change), checked
   before sending.
3. Unknown → `Unsupported` / `Frame::Unknown`, ignored. Don't make that an error.
4. Validate everything the other device sends (sizes, ranges, names).
5. After changing anything in `core-ffi`, regenerate the Kotlin bindings
   (`scripts/gen-kotlin-bindings.sh`) — see [DEVELOPING.md](DEVELOPING.md).

## The local control socket (laptop only)

`desktop/daemon/src/ipc.rs`. Unix socket `$XDG_RUNTIME_DIR/phonelink.sock`, mode 0600, and
the daemon checks the peer's uid. One JSON object per line.

- **Request** (`{"cmd": "...", …}`): `Status`, `Devices`, `Pair`, `Unpair`, `SendText`,
  `Watch`, `SetNotifications`, `Notifications`, `AppIcons`, `DismissNotification`,
  `ClearNotifications`, `ReplyNotification`, `NotificationAction`, `SetSyncDismissals`,
  `SetAppMuted`, `Reconnect`, `SetClipboardSync`, `SetAllowLock`, `Media`, `MediaControl`,
  `Transfers`, `SendFiles`, `CancelTransfer`, `ClearTransfers`, `OpenTransfer`,
  `SmsThreads`, `SendSms`, `SmsMarkRead`, `SmsLoadOlder`, `SetBluetoothBackup`.
- **Reply** (`{"type": "...", …}`): one per request (`Error { message }` on failure).
- **WatchEvent** (`{"kind": "...", …}`, after `Watch`): `Paired`, `Connected`,
  `Disconnected`, `Text`, `NotificationPosted`, `NotificationRemoved`,
  `NotificationsChanged`, `ActionResult`, `AppIcon`, `ClipboardReceived`, `LockedByPhone`,
  `MediaChanged`, `PhoneStatusChanged`, `TransferChanged`, `SmsChanged`, `SmsSendResult`.

Try it: `phonelink --json status`, `phonelink --json watch`.

## The touchpad session

The Touchpad tab talks to the laptop's `phonelink-touchpad` service, not to `phonelinkd`:
a session of its own on the same transport (QUIC, pinned mutual TLS, `Hello`), with its own
identities, its own trust stores and port 47830. Only capability `touchpad/1` is offered.
Source of truth: `core/src/touchpad.rs`.

- **Pairing code:** `phonelink:touchpad?v=1&fp=…&s=…&a=…&n=…` (the same fields as the main
  code). `PairingOffer::from_uri` refuses it and `touchpad::offer_from_pair_uri` refuses a
  main `phonelink:pair?` code, so neither pairing can be made with the other's QR.
- **Finger frames:** QUIC datagrams (`Node::send_datagram`, at most 512 bytes), postcard
  `TouchFrame { seq: u32, contacts: Vec<Contact { slot: u8, id: u16, x: u16, y: u16 }> }`.
  Each frame is a full snapshot; at most 5 contacts, slots 0–4 and unique, coordinates
  0–10000; anything else is dropped. A frame not newer than the last one applied (wrapping
  `seq`) is ignored. Only a node started with `NodeConfig::datagrams` reads them.
- **Messages:** postcard `touchpad::Msg` in `Control::Fleeting` on the control stream: sent
  only while connected, in order, never queued or repeated.

| # | Message | Direction | Meaning |
|---|---|---|---|
| 0 | `PadInfo { width_dmm, height_dmm }` | phone → laptop | the pad is on screen at this size (tenths of a mm); sent on every connect and size change |
| 1 | `Release` | phone → laptop | the pad left the screen: lift every finger, let go of both buttons |
| 2 | `PadStatus { ok, message }` | laptop → phone | the virtual touchpad is ready, or why not (message ≤ 200 bytes) |
| 3 | `Button { button: Left \| Right, down }` | phone → laptop | Left click / Right click pressed or released |

- **Liveness:** while a finger or a button is down the phone sends a frame at least every
  100 ms (empty frames while only a button is held). After 400 ms without one, the laptop
  lifts every finger and releases both buttons. Also on disconnect and on `Release`.
