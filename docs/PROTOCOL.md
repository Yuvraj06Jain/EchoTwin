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
| 13 | `MediaArt { key, image }` | P→C | `media/1` | `MediaStore::set_art` | `send_media_art` |
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

Payload types (`message.rs`): `PhoneNotification { key, package, app, title, text,
posted_ms, can_reply, actions }`, `NowPlaying { app, package, title, artist, album,
duration_ms, position_ms, playing, can_previous, can_next, volume, max_volume, art_key }`,
`SmsThread { id, address, name, messages, has_older }`, `SmsMessage { id, from_me, body,
date_ms, state, read }`, `SmsState { Received, Sent, Sending, Failed }`,
`MediaAction { PlayPause, Next, Previous, VolumeUp, VolumeDown, SetVolume(u32) }`,
`PhoneStatus { battery: Option<u8>, charging, network, signal }`.

## Capabilities

Sent in `Hello`. A feature is used only if the *other* side listed it
(`Node::peer_supports`, `has_cap` in core-ffi, `connected_with` in the daemon), and the UIs
hide what's missing. That is how an older phone app keeps working with a newer laptop.

| String | Constant | Feature |
|---|---|---|
| `messages/1` | `node::CAP_MESSAGES` | Notes (Text), always present |
| `notifications/1` | `CAP_NOTIFICATIONS` | Notification mirroring + reply/action/dismiss |
| `lock/1` | `CAP_LOCK` | Lock the computer from the phone |
| `clipboard/1` | `CAP_CLIPBOARD` | Clipboard both ways |
| `media/1` | `CAP_MEDIA` | Now playing + controls |
| `files/1` | `transfer::CAP_FILES` | File transfers |
| `sms/1` | `CAP_SMS` | SMS |
| `status/1` | `CAP_STATUS` | Battery and network both ways |

Lists: laptop `start_node` in `desktop/daemon/src/daemon.rs`; phone `PhoneLink::start` in
`core-ffi/src/lib.rs`.

## Queued vs right now

`Node::send` always writes to the outbox, so a message survives a link dropping *mid-flight*.
The difference is **who gets it**:

| Kind | Rule | Messages |
|---|---|---|
| **Queued** for every paired device (arrives after reconnect, up to 7 days) | `send_all`, or `node.send` to a chosen device | `Text` (notes), `NotificationSnapshot`, `ActionResult`, `FileCancel`, laptop → phone requests (Dismiss/Reply/Action) |
| **Notifications** | queued only if the phone's *Queue notifications while disconnected* is on (default off); otherwise connected computers only | `NotificationPosted`, `NotificationRemoved`, `AppIcon` |
| **Connected only** (sent again on every connect) | `send_connected(cap, …)` / `connected_with(cap)` | `NowPlaying`, `MediaArt`, `SmsSnapshot`, `SmsThreadUpdate`, `PhoneStatus`, `ComputerStatus` |
| **Now or never** (refused when offline, plus a timestamp check) | `connected_one` / `is_connected` + `is_fresh` | `LockComputer` (2 min), `Clipboard` (2 min), `SmsSend` (laptop refuses if not connected) |

## Limits and freshness

All in `message.rs` unless noted.

| Constant | Value | Use |
|---|---|---|
| `MAX_FRAME_LEN` (`wire.rs`) | 1 MiB | largest frame |
| `MAX_NOTIFICATION_TEXT` | 2000 bytes | notification text is cut with "…" |
| `MAX_SNAPSHOT_ITEMS` | 100 | notifications per snapshot |
| `MAX_CLIPBOARD_TEXT` | 64 KiB | clipboard text |
| `MAX_ART_BYTES` | 64 KiB | album art (JPEG/PNG only, `is_art_image`) |
| `SMS_MAX_THREADS` / `SMS_MAX_MESSAGES` / `SMS_MAX_BODY` | 30 / 30 / 4000 | SMS sync sizes |
| `LOCK_MAX_AGE_MS`, `CLIPBOARD_MAX_AGE_MS` | 2 min | `is_fresh` window (either clock direction) |
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
