# Developing EchoTwin

How to build, test, debug and change the code. For what each file does see
[FILES.md](FILES.md); for features see [FEATURES.md](FEATURES.md).

Contents
- [Build and run on the laptop](#build-and-run-on-the-laptop)
- [The Android APK (GitHub Actions)](#the-android-apk-github-actions)
- [Tests](#tests)
- [Debugging](#debugging)
- [Configuration keys](#configuration-keys)
- [Recipes](#recipes)
- [Pitfalls learned in v1](#pitfalls-learned-in-v1)

## Build and run on the laptop

```bash
cargo build --release            # phonelinkd, phonelink, phonelink-app
./packaging/install.sh           # installs to ~/.local/bin, systemd unit, menus, KDE widget; starts the service
```

- Restart the service after rebuilding: `systemctl --user restart phonelinkd`
  (install.sh does this).
- Reload the KDE widget after changing QML: re-run `install.sh` (it runs
  `kpackagetool6 -u`), then `plasmashell --replace & disown`.
- The window's UI is compiled into `phonelink-app`; after editing `desktop/ui/*` rebuild the
  app. For quick UI work open the browser preview instead: `xdg-open desktop/ui/index.html`
  (uses `mock-api.js`, "Preview" switcher at the top).
- Try the laptop side without a phone:
  `cargo run -p phonelink-core --example fake_phone -- '<URI from phonelink pair --json>'`.

Build dependencies: see [SETUP.md](../SETUP.md) section 1 (Rust stable, `build-essential`,
WebKitGTK). Building the window from source also needs the WebKitGTK/GTK development
packages (`libwebkit2gtk-4.1-dev`, `libgtk-3-dev`; `install.sh` tells you if they're missing).

## The Android APK (GitHub Actions)

You don't need Android Studio. `.github/workflows/android.yml` builds on GitHub:

1. Push to `main` touching `android/**`, `core/**`, `core-ffi/**`, `Cargo.*` or the workflow
   — or run it by hand: GitHub → **Actions** → **Android APK** → **Run workflow** → `main`.
2. ~3–5 min later: **Releases → latest-apk** has `EchoTwin-<commit>.apk`. Install it over the
   old one; pairing is kept because every build is signed with the same key (repo secrets
   `ANDROID_KEYSTORE_B64`, `ANDROID_KEYSTORE_PASSWORD`).
3. The run also writes `status.txt` and `build.log` to the **`ci-logs`** branch
   (`git fetch origin ci-logs && git show origin/ci-logs:build.log | grep '^e: '` shows Kotlin
   errors) and runs `scripts/audit-apk-permissions.sh`.

Local build (needs Android SDK + NDK): `./scripts/build-android.sh`.

## Tests

| What | Command | Notes |
|---|---|---|
| All Rust tests | `cargo test --workspace` | core (sessions, attacker, discovery, stream, files), FFI call sequences, daemon end-to-end with simulated phones and stand-in D-Bus services |
| One test file | `cargo test -p phonelink-desktop --test sms` | |
| Lint | `cargo clippy --workspace --all-targets` | keep it warning-free |
| Kotlin bindings on the JVM | `./scripts/test-kotlin-bindings.sh` | |
| <a id="ui-tests"></a>Window UI (preview + fake Tauri) | `cd desktop/ui/tests && npm install && npx playwright install chromium && npm test` | ~115 + ~148 checks, screenshots in `desktop/ui/tests/shots/` |
| Widget shell quoting | `node packaging/plasmoid/tests/shell.test.js` | hostile file names must stay names |
| QML syntax | `qmlformat <file>.qml > /dev/null` (package `qt6-declarative-dev-tools`) | Plasma itself can't run in CI |

How the laptop tests work without hardware:
- **Simulated phone:** a second `Node` in the test pairs with the real daemon through its
  socket (see `desktop/daemon/tests/media.rs` `setup`).
- **Stand-in desktop services:** tests start a private `dbus-daemon` and register fake
  ScreenSaver / logind / Klipper / notification server / BlueZ objects
  (`tests/lock_clipboard.rs`, `tests/popups.rs`, `tests/bluetooth.rs`), passed in through
  `DaemonOptions.buses` / `Popups::start_with`.
- **Fake battery:** `Config.power_supply_dir` points at a temp folder (`tests/status.rs`).
- **Paths:** `PHONELINK_DATA_DIR`, `PHONELINK_CONFIG`, `PHONELINK_DOWNLOAD_DIR`,
  `PHONELINK_SOCKET` override every location (`desktop/daemon/src/paths.rs`).

Only real devices can test: Android behaviour (One UI background limits, permissions,
notification access), real Bluetooth, KDE Plasma (widget look, popup translucency, Wayland
blur). Use the numbered checklist in [SETUP.md](../SETUP.md).

## Debugging

- Service log: `journalctl --user -u phonelinkd -f`. More detail:
  `systemctl --user edit phonelinkd` → `Environment=RUST_LOG=debug`, or run it by hand:
  `RUST_LOG=debug phonelinkd`.
- Live events: `phonelink --json watch`. State: `phonelink --json status`, `phonelink widget`.
- Window: run `phonelink-app` from a terminal (blur errors are printed there); right-click →
  Inspect is available in debug builds.
- Phone: `adb logcat | grep -i phonelink` (USB debugging on).
- Offline after reboot / new IP: see SETUP.md troubleshooting; `phonelink reconnect` forces
  a search.

## Configuration keys

`~/.config/phonelink/config.toml` (`desktop/daemon/src/config.rs`), all optional:

| Key | Default | Meaning |
|---|---|---|
| `port` | 47100 | UDP port (0 = any) |
| `device_name` | hostname | name shown on the phone |
| `notifications` | true | desktop popups |
| `discovery` | true | anonymous mDNS on the LAN |
| `sync_dismissals` | false | dismiss on both devices |
| `muted_apps` | [] | phone apps whose notifications don't pop up |
| `clipboard_sync` | true | share the clipboard |
| `allow_lock` | true | phone may lock this computer |
| `download_dir` | `~/Downloads/EchoTwin` | where received files go |
| `bluetooth_backup` | true | accept the Bluetooth backup link |
| `connection` | true | the connection switch (`phonelink on/off`) |
| `power_supply_dir` | `/sys/class/power_supply` | tests only |

Window appearance is stored by the page (`localStorage` keys `phonelink.opacity`,
`phonelink.blur`). Widget: `backgroundOpacity` (Plasma's widget config). Phone:
`Prefs.kt` (`clipboard_sharing`, `queue_notifications`).

## Recipes

### Add a new feature that sends something between the devices

1. **Protocol** (`core/src/message.rs`): append a new `AppMessage` variant at the **end**, add
   a capability constant (`CAP_X = "x/1"`), export it in `core/src/lib.rs`, add a round-trip
   unit test.
2. **Phone bridge** (`core-ffi/src/lib.rs`): add the capability to the list in
   `PhoneLink::start`; add a method (use `send_connected` for "now" state, `send_all`/
   `node.send` for things that must arrive later); for laptop → phone messages add a
   `LinkEvent` variant and a match arm in the event pump; add an FFI test.
3. **Regenerate bindings:** `./scripts/gen-kotlin-bindings.sh` (commit the changed
   `phonelink_ffi.kt`).
4. **Laptop service** (`desktop/daemon/src/daemon.rs`): add the capability to `start_node`;
   handle the message in `event_pump`; if the UI needs it, add a `Request`/`Reply`/
   `WatchEvent` in `ipc.rs` and handle it in `handle_client`; add a test in
   `desktop/daemon/tests/` with a simulated phone.
5. **Window:** a `#[tauri::command]` in `desktop/app/src/main.rs` (register it in
   `generate_handler!`), a method in `desktop/ui/tauri-api.js` (and a fake in
   `mock-api.js`), UI in `app.js`; react to the new `WatchEvent` kind in `api.onEvent`.
6. **Phone:** call the new method from Kotlin (`LinkManager` wraps calls on its background
   thread); handle the new `LinkEvent` in `LinkManager.listener`.
7. **Widget / CLI** if useful: extend `widget_state` and/or `enum Cmd` in `phonelink.rs`.
8. Docs: add a section in FEATURES.md and a row in PROTOCOL.md.

### Add a phone permission
Add it to `AndroidManifest.xml` **and** to the allow-list in
`scripts/audit-apk-permissions.sh` (CI fails otherwise); ask for it at the moment it's
needed (see the launchers in `MainActivity.kt`) and add a setup step (`ui/Model.kt`
`Access`, `ui/OtherScreens.kt` `SetupScreen`).

### Add a laptop setting
`config.rs` (`Config` field + `Default`), a `Request::SetX` that calls `State::persist`,
show it in `Status` if the UI needs it, then a switch in `app.js` `settingsBody` + handling in
`tauri-api.js` `updateSetting`.

### Change the widget
Edit QML in `packaging/plasmoid/dev.phonelink.widget/contents/ui/`; new data → extend
`widget_state` in `desktop/daemon/src/bin/phonelink.rs`; every command argument must go
through `Shell.command`/`Shell.quote`.

## Pitfalls learned in v1

- **Regenerate the Kotlin bindings after any change to `core-ffi/src/lib.rs`**, or the APK
  build fails with "Unresolved reference".
- **Never reorder or remove protocol variants** (postcard is positional). Append only.
- **Tauri:** synchronous `#[tauri::command]`s run on the main thread; anything that waits for
  the main thread (`run_on_main_thread`) must be an `async` command.
- **Tokio tests:** `#[tokio::test]` is single-threaded; a blocking call (e.g.
  `std::process::Command::output`) while the daemon runs in the same runtime deadlocks. Use
  `flavor = "multi_thread"` and `spawn_blocking`.
- **Don't hold two `lock(&state.x)` guards of the same mutex in one expression** (an earlier
  `Status` deadlock); read settings once.
- **Kotlin:** a UniFFI `u8` is `UByte` in Kotlin (`level.toInt() < 20`, not `level < 20`).
- **Git identity:** commits use `220278130+Yuvraj06Jain@users.noreply.github.com`
  (repo-local config) so the real email is never published.
- A force-push may not trigger CI; start the workflow by hand.
