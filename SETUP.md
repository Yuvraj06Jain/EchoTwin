# PhoneLink: setup, test checklist and troubleshooting

This guide covers **version 1**: installing on the laptop and phone, pairing, the numbered
test checklist, and troubleshooting. For what PhoneLink does see [README.md](README.md); for
how it works inside and where each feature lives in the code see [docs/](docs/ARCHITECTURE.md)
([architecture](docs/ARCHITECTURE.md), [features](docs/FEATURES.md), [files](docs/FILES.md),
[protocol](docs/PROTOCOL.md), [developing](docs/DEVELOPING.md)).

**Not in v1:** starting a brand-new SMS conversation from the laptop, MMS pictures, sending
folders, alarms/reminders to the phone.

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
5. **SMS on the laptop:** in the PhoneLink phone app tap **Allow SMS access** and allow SMS and Contacts. If Android says the setting is restricted, use *App info → ⋮ → Allow restricted settings* first (as for notification access).
6. **Bluetooth backup (optional):**
   1. Pair the phone and the laptop in the normal Bluetooth settings (phone: Settings → Connections → Bluetooth; laptop: KDE System Settings → Bluetooth).
   2. In the PhoneLink phone app tap **Allow Nearby devices**.
   That's all: the laptop tells the phone its Bluetooth address over the encrypted Wi-Fi link.
7. **Optional: Quick Settings tiles.** Pull down the quick panel twice, tap the pencil (Edit), and drag **Lock computer** and/or **Send clipboard** into your tiles.

### What the phone app can and can't access
- **Microphone:** never. The permission isn't in the app, and the build fails if anything adds it.
- **Camera:** only while you scan the pairing QR code. Afterwards the app gives the permission back, and Android removes it as soon as the app closes. Check under Settings → Apps → PhoneLink → Permissions.
- **Location, storage, Bluetooth scanning:** never.
- **SMS and contacts:** only after you tap *Allow SMS access*. Conversations go only to your paired laptop.
- **Nearby devices (Bluetooth):** only to connect to your paired laptop when Wi-Fi is gone.
- **Clipboard:** read only when you tap *Send clipboard* (or share text to *Copy to computer*). Text from the laptop is put on the phone's clipboard, marked as sensitive so Android's preview pop-up doesn't show it. Switch it off under *Share clipboard* in the phone app.
- **Notifications:** read only after you grant access. They're sent only to your paired laptop, encrypted.
- **Files:** only files you share to PhoneLink or pick in its *Send files* screen are read. Files from the laptop are written to Download/PhoneLink through Android's own download store, so the app needs no storage permission.
- **Music:** the same notification access lets PhoneLink see what the phone's player publishes (title, artist, album art, position) and press its buttons. No audio is ever recorded or sent.
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
| 21 | Phone app → **Lock computer** | the laptop locks within a second or two; the phone shows "… is locked" |
| 22 | Add the **Lock computer** Quick Settings tile (section 2, step 5) and tap it with the phone app closed | the laptop locks |
| 23 | Laptop app → Settings: switch off *Let your phone lock this computer*, then tap Lock on the phone | nothing happens to the laptop; the phone says it's switched off |
| 24 | Copy some text on the laptop, then paste on the phone (long-press a text field → Paste) | the laptop's text is pasted |
| 25 | Copy text on the phone, then tap **Send clipboard** on the PhoneLink notification | a "Copied from …" popup on the laptop; Ctrl+V pastes it |
| 26 | Copy a password from KeePassXC on the laptop, then paste on the phone | the password is **not** there (password-manager copies are never sent) |
| 27 | Play a song in Spotify / YouTube Music on the phone | within a second the laptop app shows a *Now playing* card with the title, artist, cover and a moving progress bar |
| 28 | In that card: pause, play, next, previous | the phone does the same |
| 29 | Drag the volume slider in the card; then press the phone's volume keys | the phone's media volume follows the slider; the slider follows the keys |
| 30 | Close the music app on the phone (swipe its notification away) | the card disappears |
| 31 | Drag a photo onto the laptop's PhoneLink window | the Files tab shows it sending; it appears on the phone in *My Files → Download → PhoneLink* |
| 32 | In Dolphin, right-click a PDF → **Send to phone (PhoneLink)** | a "Sending…" popup, then the PDF on the phone |
| 33 | On the phone, open Gallery → a photo → Share → **Send to computer** | a "Received …" popup on the laptop; clicking it opens the Files tab; *Show in folder* opens ~/Downloads/PhoneLink |
| 34 | Send a big video (1 GB+) from the phone, and turn Wi-Fi (or the hotspot) off at about half way; turn it back on | it continues from where it stopped (not from 0%) and the video plays |
| 35 | Start a big transfer and press **Cancel** on the laptop | it stops on both devices; no partial file is left anywhere |
| 36 | Send the same file twice | the second is saved as `name (2).ext`; nothing is overwritten |
| 37 | Allow SMS access on the phone; open the laptop's Messages tab | your recent conversations, with names |
| 38 | Get an SMS (e.g. an OTP or ask someone to text you) | it appears in the conversation within a few seconds, with an unread dot |
| 39 | Reply from the laptop | *Sending…* then *Sent*; the text is in Samsung Messages on the phone, sent from your default SIM |
| 40 | Turn on airplane mode on the phone (Wi-Fi off), reply from the laptop | refused with a reason; nothing is sent later by surprise |
| 41 | Pair phone and laptop in Bluetooth settings, allow Nearby devices; then turn **Wi-Fi/hotspot off** | within ~20 s the laptop app shows *Connected · Bluetooth*; a WhatsApp notification still pops up |
| 42 | Turn Wi-Fi/hotspot back on | back to *Connected* (Wi-Fi) within a minute, without touching anything |
| 43 | New phone app: open it | dark screens with **Home · Files · Settings** at the bottom; *Finish setup* appears only while a permission is missing |
| 44 | Laptop off (or away), get a WhatsApp message, then reconnect | default: no late popup for it on the laptop (Settings → *Queue notifications while disconnected* is off) |
| 45 | Turn that switch **on**, repeat 44 | the notification is delivered when the laptop reconnects |
| 46 | Re-run `./packaging/install.sh`, then right-click the panel → **Add Widgets…** → *PhoneLink* | a small phone outline appears in the panel: green when connected, grey offline |
| 47 | Get a WhatsApp message | a green dot appears on the outline; it goes away when you open the popup |
| 48 | Click the outline | popup with battery, 5G/4G and signal; tabs Notifications · Messages · Files |
| 49 | Play music on the phone, open the popup | a music card on top: previous / play-pause / next and volume work |
| 50 | Drag a file from Dolphin onto the outline (phone offline, then online) | "Queued 1 file…"; it's sent when the phone connects |
| 51 | Phone app → Finish setup → **Phone status** → Allow | the laptop shows *5G* (or *4G*) with signal bars, in the widget and the desktop app |
| 52 | Look at the phone app's Home | it shows the laptop's battery, e.g. "Computer battery 76% · charging" |
| 53 | `git pull`, `./packaging/install.sh`, then `plasmashell --replace & disown` | the panel icon is smaller (a bit below the round icons); the popup is wider and shorter, translucent with blur; empty tabs show a small green icon |
| 54 | Right-click the widget → Configure PhoneLink… → Appearance | an opacity slider (30–100%) changes the popup's background; "Open KDE blur settings…" opens the Blur effect |
| 55 | PhoneLink app → Settings → Appearance | window opacity slider applies live (text stays solid); Blur behind on/off; it's remembered after restarting the app |

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

### Lock and clipboard

| Symptom | Fix |
|---|---|
| *Lock computer* is greyed out on the phone | the laptop is offline, or still runs an older PhoneLink: re-run `./packaging/install.sh` with the new zip |
| the phone says "could not lock its screen" | the desktop's screen locker didn't answer. On KDE, check System Settings → Screen Locking works with Meta+L. `journalctl --user -u phonelinkd -n 30` shows what was tried |
| "The request arrived too late" | the lock request waited more than 2 minutes (the connection dropped right then), or the phone's and laptop's clocks differ by more than 2 minutes: turn on automatic time on both |
| laptop copies don't reach the phone | laptop app → Settings → *Share clipboard with your phone* must be on, and so must *Share clipboard* in the phone app. Copies are sent only while connected; something copied while offline isn't sent later. Clipboard sharing uses KDE Plasma's clipboard, which must be running (the clipboard icon in the system tray); other desktops aren't supported yet |
| *Send clipboard* says "The clipboard is empty" | copy the text again; some apps (banking, password managers) block other apps from reading what they copy |
| text over 64 KB isn't shared | by design: very long copies are skipped rather than cut off |

### Files

| Symptom | Fix |
|---|---|
| a transfer says *Waiting for Galaxy S24 FE to connect* | it continues by itself when the phone is back; *Reconnect now* speeds that up |
| *Waiting for Wi-Fi (connected over Bluetooth)* | by design: files don't go over the slow Bluetooth backup |
| *Update PhoneLink on …* | the other side still runs an older PhoneLink: install the new APK / zip |
| *Can't read the file any more. Share it again.* | a file shared from another app can only be read while PhoneLink keeps running; if the phone closed it mid-way, share the file again. Files picked with *Send files* in the PhoneLink app don't have this problem |
| *Not enough free space on …* | free some space on that device (PhoneLink keeps 512 MB free on the laptop and 256 MB on the phone) |
| *Send to phone* is missing in Dolphin | re-run `./packaging/install.sh`, then restart Dolphin. It's installed in `~/.local/share/kio/servicemenus` |
| dropping files on the window does nothing | drop them inside the PhoneLink window (the drop zone lights up); folders can't be sent yet, zip them first |
| received files go somewhere else | set `download_dir = "/path"` in `~/.config/phonelink/config.toml`, then `systemctl --user restart phonelinkd` |

### SMS

| Symptom | Fix |
|---|---|
| Messages tab says *No conversations yet* | phone app: tap **Allow SMS access** (and allow restricted settings if asked). Then reconnect |
| names show as numbers | allow Contacts too (phone: Settings → Apps → PhoneLink → Permissions → Contacts) |
| some chats are missing | RCS chats (blue "chat" bubbles in Samsung Messages / Google Messages) can't be read by other apps; only SMS appear |
| *Message not sent: … not connected* | the phone must be connected at that moment; texts are never queued to be sent later |
| *No mobile service* / *Airplane mode is on* | what the phone's network said; try again when it has signal |
| group conversations can't be answered | sending to groups isn't supported yet |
| marking read on the laptop doesn't mark it on the phone | by design: only the default SMS app may change that |

### Bluetooth backup

| Symptom | Fix |
|---|---|
| laptop Settings: *Bluetooth backup — not available: no Bluetooth adapter found* | turn Bluetooth on (KDE system tray). `bluetoothctl show` should list a controller |
| phone shows *Offline · Pair this phone with … in Bluetooth settings first* | pair them in the system Bluetooth settings (PhoneLink never starts pairing itself) |
| *Offline · Allow "Nearby devices"* | phone app: tap **Allow Nearby devices** |
| *Offline · The computer has the Bluetooth backup off* | laptop app → Settings → *Bluetooth backup* on |
| it never switches to Bluetooth | the backup starts 15 s after Wi-Fi is gone, then retries every 30 s–2 min. Check `journalctl --user -u phonelinkd -n 30` for "connected over Bluetooth" |
| files say *Waiting for Wi-Fi* | by design over Bluetooth; they continue when Wi-Fi is back |

### Music controls

| Symptom | Fix |
|---|---|
| no *Now playing* card | the phone app needs **notification access** (section 2, step 4), the same switch as for notifications. Also update PhoneLink on both sides |
| the card shows the wrong player | it shows the player that is playing; if none is, the most recently paused one. Close the other app's media notification |
| buttons do nothing for one app | that app ignores remote buttons; the phone's own media controls in the notification shade won't work for it either |
| the volume slider moves but nothing changes | the phone is in *Do not disturb* with media muted, or the sound goes to a Bluetooth speaker that has its own volume |
| no cover art | the app doesn't publish one (some podcast and browser players) |

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
- Files go directly between your two devices, encrypted, never through a server. Partial files are kept hidden (`.phonelink-….part`) until complete and checked, and removed if you cancel.
- SMS conversations are kept in memory only on the laptop (never written to disk) and resent by the phone when it connects. Texts are only sent when you press Send, never queued.
- The Bluetooth backup carries the same mutual-TLS session as Wi-Fi: a device that is merely Bluetooth-paired to your laptop still can't talk to PhoneLink. The phone never scans for devices.
- Shared clipboard text goes only to your paired devices, encrypted, and only while they are connected. It isn't stored by PhoneLink on either side. Passwords copied from a password manager on the laptop are skipped (KDE marks them and PhoneLink never sees them).
- See *What the phone app can and can't access* in section 2 for camera, microphone and uninstall.
- The GitHub repository holds only source code. Keys, pairings and messages never leave your devices. The repository is currently **public**; you can make it private in GitHub → Settings without affecting anything.
