#!/usr/bin/env bash
# Install the laptop side of EchoTwin's Touchpad tab (the phone as this laptop's touchpad).
#
# Run as yourself (not with sudo): it builds as you, then asks for sudo only for the
# system parts. What it sets up:
#   * a `phonelink-touchpad` system user; its group is the ONLY one that can open /dev/uinput
#   * phonelink-touchpad.service, running as that user, with its own keys and paired phones
#     in /var/lib/phonelink-touchpad (separate from phonelinkd, which runs as you)
#   * a polkit action: pairing or forgetting a phone asks for your password
#   * the virtual touchpad's device node readable (not writable) by you, for InputActions
#   * phonelink-touchpad-mirror (user service, no privileges): copies your touchpad settings
#
# Undo with packaging/uninstall-touchpad.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
P=packaging/touchpad

if [[ $EUID -eq 0 ]]; then
  echo "Run this as your normal user; it uses sudo where needed."; exit 1
fi
ME="$(id -un)"

# The old TouchTwin uses the same port (47830) and wants /dev/uinput's group too.
if [[ -e /etc/systemd/system/touchtwind.service || -e /etc/udev/rules.d/60-touchtwin-uinput.rules ]]; then
  echo "TouchTwin is still installed. Remove it first (it uses the same port and /dev/uinput):"
  echo "    ~/Desktop/Projects/PhoneMouse/scripts/uninstall-system.sh"
  echo "then run this again."
  exit 1
fi
if ! command -v setfacl >/dev/null; then
  echo "setfacl is missing (package 'acl'): sudo apt install acl"; exit 1
fi

echo "==> Building phonelink-touchpad"
cargo build --release --locked -p phonelink-touchpad

echo "==> Installing (sudo)"
sudo install -Dm755 target/release/phonelink-touchpad /usr/local/bin/phonelink-touchpad
sudo install -Dm644 $P/phonelink-touchpad.sysusers /etc/sysusers.d/phonelink-touchpad.conf
sudo systemd-sysusers /etc/sysusers.d/phonelink-touchpad.conf
sudo install -Dm644 $P/dev.echotwin.touchpad.policy /usr/share/polkit-1/actions/dev.echotwin.touchpad.policy
sudo install -Dm644 $P/60-phonelink-uinput.rules /etc/udev/rules.d/60-phonelink-uinput.rules
sudo install -Dm644 $P/71-touchpad-phonelink.rules /etc/udev/rules.d/71-touchpad-phonelink.rules
sed "s/@USER@/$ME/g" $P/72-phonelink-pad.rules.in |
  sudo tee /etc/udev/rules.d/72-phonelink-pad.rules >/dev/null
sudo udevadm control --reload
sudo udevadm trigger --sysname-match=uinput
sudo install -Dm644 $P/phonelink-touchpad.service /etc/systemd/system/phonelink-touchpad.service
sudo systemctl daemon-reload
sudo systemctl enable phonelink-touchpad.service
sudo systemctl restart phonelink-touchpad.service

echo "==> Settings mirror (user service)"
install -Dm644 $P/phonelink-touchpad-mirror.service "$HOME/.config/systemd/user/phonelink-touchpad-mirror.service"
systemctl --user daemon-reload
systemctl --user enable phonelink-touchpad-mirror.service
systemctl --user restart phonelink-touchpad-mirror.service

echo
echo "==> Checks"
sleep 1
ls -l /dev/uinput
if [[ "$(stat -c '%U %G %a' /dev/uinput)" == "root phonelink-touchpad 660" ]]; then
  echo "ok: /dev/uinput is root:phonelink-touchpad 0660"
else
  echo "WARNING: /dev/uinput is $(stat -c '%U %G %a' /dev/uinput), expected root phonelink-touchpad 660"
fi
if [[ -w /dev/uinput ]]; then
  echo "WARNING: you can still write /dev/uinput; another udev rule grants it. Look for 'uinput' in /etc/udev/rules.d and /usr/lib/udev/rules.d."
else
  echo "ok: your programs cannot open /dev/uinput; only the touchpad service can."
fi
if systemctl is-active --quiet phonelink-touchpad.service; then
  echo "ok: phonelink-touchpad.service is running"
else
  echo "phonelink-touchpad.service failed:"; sudo journalctl -u phonelink-touchpad -n 20 --no-pager; exit 1
fi
if command -v systemd-analyze >/dev/null; then
  systemd-analyze security phonelink-touchpad.service 2>/dev/null | tail -1 || true
fi
if command -v ufw >/dev/null && sudo ufw status | grep -q "Status: active"; then
  echo "note: ufw is active; let the phone in with: sudo ufw allow 47830/udp"
fi
echo
echo "Pair your phone:  phonelink touchpad pair   (then scan it in the app's Touchpad tab)"
