#!/usr/bin/env bash
# Remove PhoneLink. Your keys and paired devices stay in ~/.local/share/phonelink
# unless you pass --purge.
set -euo pipefail
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
systemctl --user disable --now phonelinkd 2>/dev/null || true
rm -f "$HOME/.local/bin/phonelinkd" "$HOME/.local/bin/phonelink" "$HOME/.local/bin/phonelink-app" \
      "$HOME/.config/systemd/user/phonelinkd.service" "$DATA/applications/phonelink.desktop" \
      "$DATA/kio/servicemenus/phonelink-send.desktop" "$DATA/kservices5/ServiceMenus/phonelink-send.desktop"
command -v kpackagetool6 >/dev/null && kpackagetool6 -t Plasma/Applet -r dev.phonelink.widget >/dev/null 2>&1 || true
for size in 32 64 128 256 512; do rm -f "$DATA/icons/hicolor/${size}x${size}/apps/phonelink.png"; done
systemctl --user daemon-reload
if [[ "${1:-}" == "--purge" ]]; then
  rm -rf "$DATA/phonelink" "$HOME/.config/phonelink"
  echo "Removed PhoneLink and all its data."
else
  echo "Removed PhoneLink. Data kept in $DATA/phonelink (use --purge to delete)."
fi
