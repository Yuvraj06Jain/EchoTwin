# PhoneLink: setup, test checklist and troubleshooting

This guide covers the current version (0.1).

**What works today:**
- pairing by QR code
- automatic connection and reconnection over your Wi-Fi
- text notes in both directions
- desktop notifications on the laptop

**Not built yet:** mirroring your phone's notifications, SMS, file transfer, clipboard and a desktop window. See the README roadmap.

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

### What the installer does
- Copies `phonelinkd` (the background service) and `phonelink` (the command) to `~/.local/bin`.
- Installs and starts the systemd **user** service `phonelinkd`. It starts automatically every time you log in.
- Nothing runs as root. Nothing is installed system-wide.

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

---

## 3. Pairing

Both devices must be on the **same network**: the same Wi-Fi, or the laptop connected to the phone's hotspot.

1. Laptop:
   ```bash
   phonelink pair
   ```
   A QR code appears with "this computer's id", e.g. `405F-AEB0-D952-DF66`.
2. Phone: **Pair a computer**, then scan the code.
3. The phone shows "Pair with *laptop name*?" and an id. **Check that it matches the terminal's id exactly**, then tap **Pair**.
4. The terminal prints `Paired with Galaxy S24 FE`, and the laptop shows a "Galaxy S24 FE connected" notification.

The QR code is valid for 2 minutes and works only once. If the id doesn't match, **don't pair**: something else answered on the network. Run `phonelink pair` again.

---

## 4. Test checklist (please try these and report results)

| # | Do this | Expected |
|---|---|---|
| 1 | Pair (section 3) | both sides show each other; laptop notification "… connected" |
| 2 | `phonelink devices` | `Galaxy S24 FE  connected` |
| 3 | `phonelink send-text hello from laptop` | notification on the phone + text under *Received* in the app |
| 4 | In the app, type a message and tap **Send to computer** | KDE notification "Message from Galaxy S24 FE" |
| 5 | Run `phonelink watch` in a terminal, then do 3 and 4 again | live events printed |
| 6 | Turn the phone's Wi-Fi off for 30 s and back on | it shows *connected* again within a few seconds, without you doing anything |
| 7 | With the laptop off or asleep, send 3 messages from the phone, then wake the laptop | all 3 arrive once, in order |
| 8 | Lock the phone and leave it for 30+ min, then `phonelink send-text still there?` | arrives on the phone |
| 9 | Reboot the phone (don't open the app) | laptop shows *connected* again after boot |
| 10 | Reboot the laptop | reconnects after login |
| 11 | On college Wi-Fi, check `phonelink devices` | *connected*; if *offline*, see **client isolation** below |

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
| `phonelink: command not found` | `~/.local/bin` not in PATH yet | log out and back in (Kubuntu adds it at login), or run `~/.local/bin/phonelink` |
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
6. **Laptop asleep.** A suspended laptop can't be reached. Messages from the phone wait and arrive after it wakes (checklist #7). That's expected.

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
- The GitHub repository holds only source code. Keys, pairings and messages never leave your devices. The repository is currently **public**; you can make it private in GitHub → Settings without affecting anything.
