#!/usr/bin/env bash
# Build PhoneLink and install it for the current user (no root needed).
# Works on any systemd distro: Kubuntu, Arch, Fedora, ...
set -euo pipefail
cd "$(dirname "$0")/.."

command -v cargo >/dev/null || { echo "Rust is not installed: https://rustup.rs"; exit 1; }
cargo build --release --locked -p phonelink-desktop

install -Dm755 target/release/phonelinkd "$HOME/.local/bin/phonelinkd"
install -Dm755 target/release/phonelink  "$HOME/.local/bin/phonelink"
install -Dm644 packaging/systemd/phonelinkd.service "$HOME/.config/systemd/user/phonelinkd.service"

systemctl --user daemon-reload
systemctl --user enable --now phonelinkd
systemctl --user restart phonelinkd   # pick up a rebuilt binary on upgrades

echo
echo "Installed. Next: run 'phonelink pair' and scan the QR code with the PhoneLink app."
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo "Note: add ~/.local/bin to your PATH (log out and back in on Kubuntu).";; esac
