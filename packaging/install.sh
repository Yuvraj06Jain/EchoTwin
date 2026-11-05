#!/usr/bin/env bash
# Install PhoneLink for the current user (no root needed).
# Works on any systemd distro: Kubuntu, Arch, Fedora, ...
#
# Uses the prebuilt binaries in ./bin when present and compatible (needs glibc >= 2.39,
# e.g. Kubuntu 24.04+ or Arch); otherwise builds from source with cargo.
set -euo pipefail
cd "$(dirname "$0")/.."

glibc_ok() {
  local have
  have="$(ldd --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+$' || echo 0.0)"
  [ "$(printf '%s\n2.39\n' "$have" | sort -V | head -1)" = "2.39" ]
}

if [ -x bin/phonelinkd ] && [ -x bin/phonelink ] && [ "$(uname -m)" = "x86_64" ] && glibc_ok; then
  echo "Using prebuilt binaries from ./bin"
  SRC=bin
else
  command -v cargo >/dev/null || {
    echo "No compatible prebuilt binaries and Rust is not installed."
    echo "Install Rust from https://rustup.rs, then run this script again."
    exit 1
  }
  echo "Building from source (takes a few minutes the first time)..."
  cargo build --release --locked -p phonelink-desktop
  SRC=target/release
fi

install -Dm755 "$SRC/phonelinkd" "$HOME/.local/bin/phonelinkd"
install -Dm755 "$SRC/phonelink"  "$HOME/.local/bin/phonelink"
install -Dm644 packaging/systemd/phonelinkd.service "$HOME/.config/systemd/user/phonelinkd.service"

systemctl --user daemon-reload
systemctl --user enable --now phonelinkd
systemctl --user restart phonelinkd   # pick up new binaries on upgrades

sleep 1
if systemctl --user is-active --quiet phonelinkd; then
  echo
  echo "PhoneLink is running. Next: run 'phonelink pair' and scan the QR code with the PhoneLink app."
else
  echo
  echo "phonelinkd did not start. See: journalctl --user -u phonelinkd -n 50"
  exit 1
fi
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo "Note: add ~/.local/bin to your PATH (log out and back in on Kubuntu).";; esac
