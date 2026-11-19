#!/usr/bin/env bash
# Remove everything install-touchpad.sh set up (the touchpad's keys and paired phones
# included). EchoTwin itself (phonelinkd, the window, the widget) is left alone.
set -euo pipefail
if [[ $EUID -eq 0 ]]; then
  echo "Run this as your normal user; it uses sudo where needed."; exit 1
fi
systemctl --user disable --now phonelink-touchpad-mirror.service 2>/dev/null || true
rm -f "$HOME/.config/systemd/user/phonelink-touchpad-mirror.service"
systemctl --user daemon-reload
sudo systemctl disable --now phonelink-touchpad.service 2>/dev/null || true
sudo rm -f /etc/systemd/system/phonelink-touchpad.service \
  /etc/udev/rules.d/60-phonelink-uinput.rules /etc/udev/rules.d/71-touchpad-phonelink.rules \
  /etc/udev/rules.d/72-phonelink-pad.rules \
  /usr/share/polkit-1/actions/dev.echotwin.touchpad.policy /etc/sysusers.d/phonelink-touchpad.conf \
  /usr/local/bin/phonelink-touchpad
sudo rm -rf /var/lib/phonelink-touchpad
sudo systemctl daemon-reload
sudo udevadm control --reload && sudo udevadm trigger --sysname-match=uinput
sudo userdel phonelink-touchpad 2>/dev/null || true
echo "EchoTwin touchpad removed."
