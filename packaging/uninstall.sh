#!/usr/bin/env bash
# Remove the PhoneLink programs. Your keys and paired devices stay in
# ~/.local/share/phonelink unless you pass --purge.
set -euo pipefail
systemctl --user disable --now phonelinkd 2>/dev/null || true
rm -f "$HOME/.local/bin/phonelinkd" "$HOME/.local/bin/phonelink" "$HOME/.config/systemd/user/phonelinkd.service"
systemctl --user daemon-reload
if [[ "${1:-}" == "--purge" ]]; then
  rm -rf "$HOME/.local/share/phonelink" "$HOME/.config/phonelink"
  echo "Removed PhoneLink and all its data."
else
  echo "Removed PhoneLink. Data kept in ~/.local/share/phonelink (use --purge to delete)."
fi
