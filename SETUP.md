# PhoneLink: setup, test checklist and troubleshooting

This guide covers the current version (0.2).

**What works today:**
- the **PhoneLink desktop app** (open it from your application menu)
- pairing by QR code
- automatic connection and reconnection over your Wi-Fi
- text notes in both directions
- **your phone's notifications on the laptop**, both as KDE popups and in the app's Notifications tab:
  - reply where the phone app allows it (WhatsApp, Messages…)
  - press their buttons ("Mark as read", "Archive"…)
  - dismiss them
  - click a popup to open the PhoneLink window on its Notifications tab
- settings in the laptop app: *Dismiss on both devices* (off by default), which apps to show, and desktop popups on/off

Mirroring is **one-way**: phone → laptop. The laptop never sends its own notifications to the phone (notes you type with *Send a note* are the only thing that appears on the phone).

**Not built yet:** SMS conversations, file transfer and clipboard. The app has their tabs and says "coming in the next update" until each one exists.

---

## 1. Laptop setup (Kubuntu, Arch, Fedora: any distro with systemd)

### Option A: prebuilt binaries (fastest)
The laptop zip contains ready-made binaries in `bin/`. They need an x86_64 CPU and
**glibc 2.39 or newer**. That covers Kubuntu 24.04 and newer, Arch, and Fedora 40 and newer.

1. Unzip it, for example to `~/Desktop/Projects/Phone-Link`.
2. In a terminal inside that folder, run:
   ```bash
   ./packaging/install.sh
   ```

The script picks the prebuilt binaries automatically. On an older system it builds from source instead (Option B).

### Option B: build from source
1. Install the build tools.
   Kubuntu/Ubuntu:
   ```bash
   sudo apt install build-essential curl git
   ```
   Arch:
   ```bash
   sudo pacman -S base-devel git
   ```
2. Install Rust:
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
   Then open a new terminal.
3. Build and install:
   ```bash
   ./packaging/install.sh
   ```
   The first build takes about 3–5 minutes.

### The desktop app needs WebKitGTK
The window is drawn with WebKitGTK (KDE doesn't ship it by default). Install it once:

Kubuntu/Ubuntu:
```bash
sudo apt install libwebkit2gtk-4.1-0
```
Arch:
```bash
sudo pacman -S webkit2gtk-4.1
```
If it's missing, `install.sh` still installs the service and tells you this command.

### What the installer does
- Copies `phonelinkd` (the background service), `phonelink` (the command) and `phonelink-app` (the window) to `~/.local/bin`.
- Adds **PhoneLink** to your application menu, with its icon.
- Installs and starts the systemd **user** service `phonelinkd`. It starts automatically every time you log in.
- Nothing runs as root. Nothing is installed system-wide.

The window is only a view. Closing it does **not** disconnect your phone: the service keeps running in the background.

### Where your data lives
| Path | What | Notes |
|---|---|---|
| `~/.local/share/phonelink/` | this computer's private key, paired devices, queued messages | folder is owner-only (700). **Back this up / copy it when you switch distro or laptop**, or you'll have to pair again |
| `~/.config/phonelink/config.toml` | optional settings | see below |
| `$XDG_RUNTIME_DIR/phonelink.sock` | control socket between `phonelink` and the service | owner-only, recreated at each start |

Optional `~/.config/phonelink/config.toml`:
```toml
port = 47100                 # UDP port the service listens on
device_name = "My Laptop"    # name shown on the phone (default: hostname)
notifications = true         # desktop notifications
discovery = true             # anonymous LAN discovery so the phone finds the laptop after IP changes
```
After editing it, restart the service:
```bash
systemctl --user restart phonelinkd
```

---

## 2. Phone setup (Galaxy S24 FE, or any Android 10+)

1. On the phone, open **https://github.com/Yuvraj06Jain/Phone-Link/releases/tag/latest-apk**
   and download `PhoneLink-xxxxxxx.apk`.
2. Open the downloaded file.
   - If asked, allow **Install unknown apps** for your browser or *My Files*.
   - If Play Protect warns "unknown developer", tap *More details → Install anyway*. The app is yours and isn't on the Play Store.
3. Open **PhoneLink**:
   - Allow **notifications** (so it can show its "Connected" status and incoming messages).
   - Tap **Allow background use**, then **Allow**.
   - Then, **important on Samsung**: Settings → Battery → Background usage limits → **Never sleeping apps** → **+** → PhoneLink.
4. **Show your phone's notifications on the laptop** (after pairing, section 3). In the PhoneLink phone app, tap **Allow notification access** and switch PhoneLink on.
   - **If the switch is greyed out** ("Restricted setting"): Android blocks this for apps installed from a file.
     1. Back in PhoneLink, tap **Open App info**.
     2. Tap **⋮** (top right), then **Allow restricted settings**, and confirm with your PIN or fingerprint.
     3. Tap **Allow notification access** again; the switch now works.

### What the phone app can and can't access
- **Microphone:** never. The permission isn't in the app, and the build fails if anything adds it.
- **Camera:** only while you scan the pairing QR code. Afterwards the app gives the permission back, and Android removes it as soon as the app closes. Check under Settings → Apps → PhoneLink → Permissions.
- **Location, contacts, storage:** never.
- **Notifications:** read only after you grant access. They're sent only to your paired laptop, encrypted.
- **Uninstalling** removes everything: keys, pairings, queued messages and cached app icons all live in the app's private storage, which Android deletes. Nothing is kept in your files or in cloud backups.

---

## 3. Pairing

Both devices must be on the **same network**: the same Wi-Fi, or the laptop connected to the phone's hotspot.

1. Laptop: open **PhoneLink** from the application menu. With no phone paired, it shows a QR code and "this computer's id", e.g. `405F-AEB0-D952-DF66`.
   (Later, use **Pair another device** in the app. From a terminal, `phonelink pair` works too.)
2. Phone: **Pair a computer**, then scan the code.
3. The phone shows "Pair with *laptop name*?" and an id. **Check that it matches the terminal's id exactly**, then tap **Pair**.
4. The app switches to the main view showing the phone as *Connected*.

The QR code is valid for 2 minutes and works only once. If the id doesn't match, **don't pair**: something else answered on the network. Run `phonelink pair` again.

---

## 4. Test checklist (please try these and report results)

| # | Do this | Expected |
|---|---|---|
| 1 | Pair (section 3) | both sides show each other; laptop notification "… connected" |
| 2 | `phonelink devices` | `Galaxy S24 FE  connected` |
| 3 | Laptop app: **Send a note**, type, **Send** (or `phonelink send-text hello`) | notification on the phone + text under *Received* in the phone app |
| 4 | Phone app: type a message and tap **Send to computer** | it appears in the laptop app's Notifications tab, and as a KDE notification |
| 5 | Run `phonelink watch` in a terminal, then do 3 and 4 again | live events printed |
| 6 | Turn the phone's Wi-Fi off for 30 s and back on | it shows *connected* again within a few seconds, without you doing anything |
| 7 | With the laptop off or asleep, send 3 messages from the phone, then wake the laptop | all 3 arrive once, in order |
| 8 | Lock the phone and leave it for 30+ min, then `phonelink send-text still there?` | arrives on the phone |
| 9 | Reboot the phone (don't open the app) | laptop shows *connected* again after boot |
| 10 | Reboot the laptop | reconnects after login |
| 11 | On college Wi-Fi, check `phonelink devices` | *connected*; if *offline*, see **client isolation** below |
| 12 | Grant notification access (section 2, step 4), then get a WhatsApp message | a popup on the laptop with WhatsApp's icon, and it's listed in the app |
| 13 | In the laptop app, **Reply** to that WhatsApp notification | the reply is sent from your phone, and appears in the WhatsApp chat |
| 14 | Laptop app: press a button such as **Mark as read** | it happens on the phone |
| 15 | With *Dismiss on both devices* **off**: clear a notification on the laptop; clear another on the phone | the laptop doesn't touch the phone. The one cleared on the phone shows *Cleared on phone* in the app |
| 16 | Turn *Dismiss on both devices* **on** (laptop app → ⚙ Settings) and repeat 15 | clearing either side clears the other |
| 17 | Laptop app → Settings → switch off an app under *Show notifications from* | its notifications disappear from the app and stop popping up |
| 18 | Pair again later (or check now): after the scan, close the phone app, then look at Settings → Apps → PhoneLink → Permissions | Camera shows *Not allowed* |
| 19 | With the PhoneLink window closed, click a popup from your phone | the window opens on the Notifications tab, in front |
| 20 | With the window open but behind other windows (or on another tab), click another popup | the same window comes to the front on the Notifications tab; no second window |

When something fails, include:
- the step number
- what you saw
- the output of:
  ```bash
  journalctl --user -u phonelinkd -n 80 --no-pager
  ```

---

## 5. Troubleshooting

### Installing

| Symptom | Cause | Fix |
|---|---|---|
| `phonelink: command not found` | `~/.local/bin` not in PATH yet | log out and back in (Kubuntu adds it at login), or run `~/.local/bin/phonelink`. The app in the menu works regardless |
| `install.sh`: "Rust is not installed" | old glibc or non-x86_64, so prebuilt binaries can't be used | install Rust (Option B) and run it again |
| `Failed to connect to bus` from `systemctl --user` | running over SSH/su without a user session | run it from a normal terminal in your desktop session |
| Phone: "App not installed" / "package conflicts" | an APK signed with a different key is installed | uninstall PhoneLink first, then install. This resets the phone's pairing: see *re-pairing* |
| Phone: install button does nothing / "blocked" | Samsung **Auto Blocker** is on | Settings → Security and privacy → Auto Blocker → off (you can turn it on again after installing) |
| Play Protect "Unsafe app blocked" | unknown developer | *More details → Install anyway* |

### Laptop service

| Symptom | Fix |
|---|---|
| `PhoneLink daemon is not running` | `systemctl --user start phonelinkd`, then check `journalctl --user -u phonelinkd -n 50` |
| log: `could not listen on UDP port 47100` | another program uses the port, or a second copy is running. Set `port = 47101` in `config.toml` and restart. The phone finds the new port via discovery; if not, pair again |
| log: `phonelinkd is already running` | it already is; use `systemctl --user restart phonelinkd` instead of starting it by hand |
| no desktop notifications | KDE *Do Not Disturb* is on, or `notifications = false` in config. Check System Settings → Notifications. `phonelink watch` shows events regardless |
| need more detail in logs | `systemctl --user edit phonelinkd`, add `[Service]` and `Environment=RUST_LOG=debug`, save, restart |

### The desktop app

| Symptom | Fix |
|---|---|
| PhoneLink isn't in the application menu | the installer skipped the app because WebKitGTK was missing: install it (section 1) and run `./packaging/install.sh` again |
| "error while loading shared libraries: libwebkit2gtk-4.1.so.0" | same: install WebKitGTK (section 1) |
| the app says *The PhoneLink service isn't running* | click **Start the service**. If that fails: `journalctl --user -u phonelinkd -n 50` |
| window is blank / white | run it once from a terminal with `phonelink-app` and send me the output. The app already disables the WebKit renderer that most often causes this on NVIDIA drivers |
| black corners instead of rounded ones | desktop effects (compositing) are off. KDE: System Settings → Display and Monitor → Compositor → *Enable on startup* |
| want to see the full design with sample data | `phonelink-app --demo` |

### Phone notifications on the laptop

| Symptom | Fix |
|---|---|
| no phone notifications arrive at all | phone app: is the *Allow notification access* card still there? Grant it (section 2, step 4) |
| the notification-access switch is greyed out | *Allow restricted settings* first (section 2, step 4) |
| they're in the app but no popup appears | laptop app → Settings: *Desktop notifications* must be on, and the app not switched off under *Show notifications from*. KDE's *Do Not Disturb* also hides popups. Notifications older than 2 minutes (arriving after the laptop wakes) go to the list only, by design |
| one app's notifications never appear | some notifications are deliberately skipped: music players, navigation, downloads, and other ongoing ones. Android 15 also hides one-time codes (OTPs) from apps like PhoneLink |
| **Reply** is missing for an app | that app doesn't offer a reply box in its phone notification either |
| "Your phone couldn't do that" | the notification was already gone on the phone, or the app refused. Some apps only accept replies or buttons when the phone is unlocked |
| notifications stop after a while | battery settings (see *Can't connect*): PhoneLink must be in *Never sleeping apps* |
| clicking a popup does nothing | the window is opened as `phonelink-app` from the folder `phonelinkd` runs from (normally `~/.local/bin`): re-run `./packaging/install.sh`. Then check `journalctl --user -u phonelinkd -n 30` for "could not open the PhoneLink window". Clicks on popups shown *before* the service restarted are not received |
| the window opens but stays behind other windows | KDE's focus-stealing prevention: System Settings → Window Management → Window Behavior → *Focus stealing prevention* at *Low* (the default) allows it. Wayland needs Plasma 5.27 or newer |

### Pairing

| Message / symptom | Cause | Fix |
|---|---|---|
| "this computer has no LAN address" | laptop not on a network | connect to Wi-Fi or the phone's hotspot |
| "the code expired or was already used" | > 2 minutes, or the code was used once | run `phonelink pair` again |
| "the laptop did not answer; are you on the same network?" | different networks, guest Wi-Fi, client isolation, firewall, or a VPN | see *Can't connect* below |
| "That is not a PhoneLink pairing code" | scanned a different QR code | scan the one in the terminal |
| QR code won't scan | modules too small or terminal colours | zoom the terminal in (Konsole: Ctrl + Shift + =), maximise the window, raise screen brightness |
| ids on the two screens differ | another device answered: **possible attack** | cancel. Run `phonelink pair` again, preferably on your phone's hotspot |
| camera doesn't open | camera permission denied | Settings → Apps → PhoneLink → Permissions → Camera → Allow |

### Can't connect / keeps showing *offline*

Work through these in order:

1. **Same network?** The phone must not be on mobile data or a *guest* network, unless the laptop is on the phone's hotspot.
2. **Client isolation (college, office, hotel and some routers).** The network blocks devices from reaching each other. You can't fix this from either device.
   **Fix:** turn on the phone's **hotspot**, connect the laptop to it, then run `phonelink devices`. That's also the most private setup. Home routers may have an "AP isolation" setting you can turn off.
3. **VPN on the phone or laptop.** Many VPN apps send all traffic, including local traffic, through the VPN. Enable the VPN's "Allow LAN / local network" option, or pause the VPN.
4. **Firewall on the laptop.** Kubuntu's `ufw` is off by default. If you turned it on:
   ```bash
   sudo ufw allow from 192.168.0.0/16 to any port 47100 proto udp
   ```
   ```bash
   sudo ufw allow from 192.168.0.0/16 to any port 5353 proto udp
   ```
   Use your network's range, e.g. `10.0.0.0/8` on some networks.
5. **Battery restrictions on the phone.** The connection drops when the screen is off, or after some minutes. Do the *Never sleeping apps* step again, and check that Settings → Apps → PhoneLink → Battery is **Unrestricted**. If you haven't opened the app since installing, Samsung may have put it to "deep sleep".
6. **Phone shows *Offline* after a reboot, or after the hotspot restarted.** Both devices usually get new IP addresses then (a phone hotspot gives new ones every time it starts). PhoneLink now finds the other side by itself:
   - On a phone hotspot, the laptop tries the hotspot's gateway, which *is* the phone. That's usually instant.
   - Otherwise each side searches its local network (the /24 around it) after about a minute, and again every 3 minutes.
   - Unlocking the phone, opening either app, or a network change retries immediately.
   - To force it: **Reconnect now** in the laptop app's offline banner, **Reconnect** in the phone app, or run `phonelink reconnect`.
   - Still offline after a minute? Check `journalctl --user -u phonelinkd -n 50`. While a device stays unreachable, it logs every 3 minutes how many addresses it tried and the last error.
   - On a home router you can also give the laptop a fixed IP (a "DHCP reservation" in the router's settings).
7. **Laptop asleep.** A suspended laptop can't be reached. Messages from the phone wait and arrive after it wakes (checklist #7). That's expected.

### Re-pairing situations

| You did this | What happens | Fix |
|---|---|---|
| reinstalled the app, cleared app data, or got a new phone | the phone has a new identity; the laptop still lists the old one | on the laptop: `phonelink devices`, then `phonelink unpair <old id>`, then `phonelink pair` |
| new distro or laptop, or deleted `~/.local/share/phonelink` | the laptop has a new identity; the phone still lists the old one | on the phone: **Unpair** the old entry, then pair again. To avoid this, copy `~/.local/share/phonelink` to the new system **before** you first start the service there, and keep it owner-only (`chmod 700`) |
| lost the phone | it still holds a key the laptop trusts | `phonelink unpair <id>` immediately. It can no longer connect |

### Removing PhoneLink

On the laptop, this removes the programs but keeps your keys and pairings:
```bash
./packaging/uninstall.sh
```
This removes everything, including the keys and pairings:
```bash
./packaging/uninstall.sh --purge
```

On the phone: uninstall the app as usual. Its keys are deleted with it, and they're never included in cloud backups.

---

## 6. Privacy notes

- There is no server, no account and no cloud. Data goes directly between your two devices, encrypted with TLS 1.3, and only between devices that have scanned each other's code.
- On the network, other devices only see random identifiers that change every 10 minutes: never your laptop's name or your phone's model.
- Mirrored notifications are kept **in memory only** on the laptop (the newest 200), never written to disk. App icons are cached in `~/.local/share/phonelink/icons`.
- See *What the phone app can and can't access* in section 2 for camera, microphone and uninstall.
- The GitHub repository holds only source code. Keys, pairings and messages never leave your devices. The repository is currently **public**; you can make it private in GitHub → Settings without affecting anything.
