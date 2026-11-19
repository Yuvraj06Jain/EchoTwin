# Touchpad tab in EchoTwin (replacing the TouchTwin project)

## Context
TouchTwin (`~/Desktop/Projects/PhoneMouse`) works and has been verified live:
- The phone streams raw fingers to a uinput virtual touchpad, so libinput and InputActions treat it like the real trackpad. The 3-finger swipe raised the volume, and settings are mirrored through KWin.
- The laptop service runs as its own system user. Only its group can open `/dev/uinput`, pairing needs polkit, and the pad node is read-only for the user.

The user now wants this **inside EchoTwin**, not as a separate app. Phone side:
- a new **Touchpad tab**
- styled like the presentation `PointerPad` but filling the screen
- two **Left click / Right click** buttons at the top, with the trackpad below them

The TouchTwin project is then removed. The user chose to **keep the secure touchpad service**. EchoTwin's `phonelinkd` runs as the user, so any program of the user's could drive it. It therefore must not be what creates input. The touchpad keeps its own small hardened laptop service and its own pairing, one extra QR scan. The phone side is just a tab in the EchoTwin app.

This plan is executed in the user's EchoTwin chat. After approval it is saved to `EchoTwin/docs/plans/touchpad-tab.md` for that chat to pick up.

## Architecture
```
EchoTwin app ── Touchpad tab ──(own identity + pinned TLS, QUIC datagrams)──▶ phonelink-touchpad.service
   (TouchpadLink FFI object,                                                   (system user, only it may open
    separate from PhoneLink)                                                    /dev/uinput, polkit pairing)
                                                                                      │ uinput: touchpad + 2 buttons
                                                                                      ▼
                                                                    libinput ─▶ KWin + InputActions
phonelink-touchpad mirror (user service, unprivileged) ── copies the real touchpad's settings through KWin D-Bus
```
The touchpad service has its own certificate, trust store (`/var/lib/phonelink-touchpad`) and port (47830). The phone's Touchpad tab pins that certificate, separately from the main EchoTwin pairing. The user-level `phonelinkd` holds no touchpad power.

## Changes

### 1. `core` (phonelink-core): datagrams and the touchpad protocol
These are the minimal additions. TouchTwin's copies of the security files are not brought back: EchoTwin has the originals.
- **`core/src/node.rs`:**
  - Add `Event::Datagram { peer, body: Bytes }`.
  - Add `Node::send_datagram(peer, body) -> Result<bool>`, QUIC only; `Link::Stream` returns `Ok(false)`.
  - Add a datagram reader task in `start_session` for `Link::Quic`.
  - Port from `PhoneMouse/touchtwin-core/src/node.rs` (`send_datagram`, the reader in `start_session`, `MAX_DATAGRAM_LEN = 512`).
  - Existing `match` on `Event` in `desktop/daemon` and `core-ffi` gets a `Datagram` arm, which is ignored there.
  - Add `bytes = "1"` to `core/Cargo.toml`.
  - quinn's default transport already enables datagrams, so `tls.rs` is unchanged.
- **New `core/src/touchpad.rs`:** port of `PhoneMouse/touchtwin-core/src/proto.rs`.
  - Keep: `CAP_TOUCHPAD = "touchpad/1"`, `Msg { PadInfo, Release, PadStatus }`, `TouchFrame`, `Contact`, `seq_newer`, and the strict `decode` validation, with its tests.
  - **New:** `Msg::Button { button: Left | Right, down: bool }`, appended last.
  - Uses its own `Msg` enum, not `AppMessage`, because it only ever travels to the touchpad service.

### 2. New crate `desktop/touchpad` (binary `phonelink-touchpad`)
Move these from `PhoneMouse/daemon/src/` with minimal edits, swapping `touchtwin_core` for `phonelink_core`:
- **`pad.rs`:** the snapshot-to-MT-protocol-B state machine. Keep its tests.
- **`uinput.rs`:** the device becomes **"EchoTwin Touchpad"**, a touchpad with two physical buttons:
  - add `BTN_RIGHT`
  - **drop `INPUT_PROP_BUTTONPAD`**, so libinput treats `BTN_LEFT`/`BTN_RIGHT` as real left and right buttons
  - holding Left while moving a finger is a drag
- **`main.rs`:**
  - Handles `Msg::Button` by emitting `BTN_LEFT`/`BTN_RIGHT` plus SYN, tracking button state.
  - Releases fingers **and buttons** on disconnect, on `Release`, and in the 400 ms watchdog. The watchdog also covers a button held with no finger down: the phone heartbeats while a button is held.
  - Keeps `run`, `mirror`, `pair [--uri]`, `devices`, `unpair` and `demo`.
- **Also moved:** `control.rs` (socket `/run/phonelink-touchpad/control.sock`, polkit-gated pair/unpair), `polkit.rs`, `kwin.rs` (matches the new device name) and `netaddrs.rs`. `netaddrs.rs` is already EchoTwin's own; reuse `desktop/daemon/src/netaddrs.rs` through a path module or a small shared module, not a second copy.
- **`Cargo.toml` dependencies:** `evdev 0.13`, `zbus 5` (tokio feature), `qrcode`, `serde_json`, `futures-lite`. Add the crate to the workspace members.
- **`phonelink` CLI (`desktop/daemon/src/bin/phonelink.rs`):** add `phonelink touchpad pair|devices|unpair`, which forwards to `phonelink-touchpad`. The user never needs a second command name.

### 3. Packaging (`packaging/touchpad/`)
Port from `PhoneMouse/packaging` and rename:
- sysusers user `phonelink-touchpad`
- `60-phonelink-uinput.rules` (`GROUP="phonelink-touchpad", MODE="0660"`)
- `71-touchpad-phonelink.rules` and `72-phonelink-pad.rules.in`: hide `ID_INPUT_TOUCHPAD` before InputActions' `71-touchpad.rules`, then restore it with a **read-only** `setfacl ... $devnode`
- `phonelink-touchpad.service` (hardened system unit)
- `phonelink-touchpad-mirror.service` (user unit)
- `dev.echotwin.touchpad.policy`: `auth_admin`, **plus** `<annotate key="org.freedesktop.policykit.owner">unix-user:phonelink-touchpad</annotate>`. Without it polkit refuses the non-root daemon, as found live.

Scripts:
- `packaging/install-touchpad.sh`: port of `install-system.sh`, run as the user, using sudo inside.
- `packaging/uninstall-touchpad.sh`.

### 4. `core-ffi`: a separate `TouchpadLink` object
Port `PhoneMouse/touchtwin-ffi/src/lib.rs` into a new module (`core-ffi/src/touchpad.rs`), exported alongside `PhoneLink`:
- **Own node and identity:** its own `Node` and data dir (`filesDir/touchpad`), with its own identity and trust store, never shared with `PhoneLink`.
- **Kept as is:** `start`, `pair`, `laptops`, `unpair`, `reconnect_now`, `open_pad`, `close_pad`, `touch`, and `latency_ms`. The heartbeat (100 ms) and the repeated lift also stay.
- **New:** `button(left: bool, down: bool) -> bool`, sent reliably. The heartbeat keeps running while a button is held.
- Its listener reports `PadEvent` (`Connected`, `Disconnected`, `PadReady`, `PadFailed`, `Paired`).
- Port the end-to-end FFI test, adding button down/up, plus the `fake_phone` example (`core-ffi/examples/fake_touchpad_phone.rs`).
- Regenerate the bindings with `scripts/gen-kotlin-bindings.sh`.

### 5. Android: the Touchpad tab
- **`MainActivity.kt` `BottomBar` (`:471-494`):** add `"Touchpad" to Icons.Filled.TouchApp`, plus a `tab == 3` branch in the routing `when` (`:223-231`). Keep the existing indexes for Files and Settings.
- **New `ui/TouchpadScreen.kt`:**
  - **Not paired yet:** a setup card ("On the laptop run `phonelink touchpad pair`, then scan") with a Scan button. It reuses the zxing `ScanContract` the app already uses (`MainActivity.kt` `scan`).
  - **Paired:** a column with:
    - a thin status line (laptop name, a connected dot, latency, the error from `PadFailed`)
    - a **row of two large buttons, "Left click" and "Right click"**, sized about 72 dp tall and half the width each. Each one uses `pointerInput`: press calls `button(down=true)` and release calls `button(down=false)`. Holding it allows drag, and a light haptic plays on press.
    - **the trackpad filling the rest:** the same look as `PointerPad` (`RemoteScreen.kt:241-297`), with a dark surface, a dashed rounded outline and a soft dot under each finger. There's no 16:9 crop; it uses the full remaining area.
  - Touch handling ports `PhoneMouse/.../TouchPadView.kt`: raw multitouch with stable slots and per-touch ids, every finger forwarded. It's embedded with `AndroidView`, because Compose's `pointerInput` in `PointerPad` only tracks one finger.
  - The tab keeps the screen on, blocks the back gesture on the pad edges, calls `open_pad` with the size in mm when shown, and `close_pad` when the tab is left or the app is backgrounded.
- **`LinkManager.kt`:**
  - Holds the `TouchpadLink`, started lazily like `link()` (`:63-75`).
  - Touch calls go **directly**, not through the shared `io` executor (`:192`). They don't block, and queueing behind notification work would add lag.
  - `pair` and `start` run on a background thread.
- **State:** add touchpad fields to `UiState` (`ui/Model.kt:20-56`) and `Actions` (`:59-101`): `touchpadPaired`, `touchpadStatus`, `scanTouchpad()`, `touch(fingers)`, `button(left, down)`.

### 6. Remove TouchTwin (only after the EchoTwin version is verified live)
1. Run `~/Desktop/Projects/PhoneMouse/scripts/uninstall-system.sh`. It removes `touchtwind.service`, the `touchtwin` user, the udev rules, the polkit file and `/var/lib/touchtwin`.
2. Confirm nothing remains:
   - `ls /etc/udev/rules.d | grep touchtwin`
   - `getent passwd touchtwin`
   - `/dev/uinput` ends up back to root-only until the new rules apply
3. Delete `~/Desktop/Projects/PhoneMouse`. **Ask the user first.** It isn't a git repo, so the deletion is permanent.
4. Update the memory note `touchtwin-project.md` to say it now lives in EchoTwin.

Order matters: install the EchoTwin touchpad service **after** uninstalling TouchTwin. Both use port 47830 and both own `/dev/uinput`'s group.

## Verification
- **Tests:**
  - `cargo test --workspace` in EchoTwin: the new datagram session test in `core/tests`, the ported `pad`, `polkit`, `touchpad` and FFI tests, and all existing tests still passing.
  - `npm test` for the desktop UI, unchanged.
- **Install:** run `packaging/install-touchpad.sh`. Its checks must print:
  - `/dev/uinput` = `root phonelink-touchpad 0660`, not writable by the user
  - the service active
  - `systemd-analyze security phonelink-touchpad` about 1.9
- **Pairing:** `phonelink touchpad pair --uri`, approve the KDE polkit dialog, then pair with `fake_touchpad_phone`.
- **Gestures with `fake_touchpad_phone gestures`:**
  - the cursor moves
  - a tap clicks
  - two fingers scroll
  - three fingers up raises the volume through InputActions (restore the volume afterwards)
- **Buttons:**
  - Right down/up opens a context menu
  - Left down plus a finger move plus Left up drags
- **Device node:**
  - readable but write is denied for the user
  - `getfacl` shows `user:<you>:r--`, with no `uaccess` tag
  - InputActions `deviceList` shows "EchoTwin Touchpad" with `multiTouch: true`
  - KWin's D-Bus shows the mirrored `pointerAcceleration` and `scrollFactor`
- **Cleanup:** unpair the fake phone afterwards (polkit) and delete its keys.
- **Android:** `scripts/build-android.sh` needs the Android SDK/NDK, which isn't installed on this machine yet. Then on the phone:
  - the Touchpad tab appears
  - scan `phonelink touchpad pair`
  - pad gestures and both buttons work
  - switching tabs lifts every finger and button
