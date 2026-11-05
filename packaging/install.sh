#!/usr/bin/env bash
# Install PhoneLink for the current user (no root needed for PhoneLink itself).
# Works on any systemd distro: Kubuntu, Arch, Fedora, ...
#
#  * phonelinkd   background service (systemd user unit), keeps the phone connected
#  * phonelink    command-line tool
#  * PhoneLink    the desktop window (menu entry + icon); needs WebKitGTK 4.1
#
# Uses the prebuilt binaries in ./bin when present and compatible (x86_64, glibc >= 2.39,
# e.g. Kubuntu 24.04+ or Arch); otherwise builds from source with cargo.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="$HOME/.local/bin"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"

glibc_ok() {
  local have
  have="$(ldd --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+$' || echo 0.0)"
  [ "$(printf '%s\n2.39\n' "$have" | sort -V | head -1)" = "2.39" ]
}
prebuilt_ok() { [ -x "bin/$1" ] && [ "$(uname -m)" = "x86_64" ] && glibc_ok; }
# Can the prebuilt window actually load? (Checks every library it links, WebKitGTK included.)
app_libs_ok() { ldd bin/phonelink-app >/dev/null 2>&1 && ! ldd bin/phonelink-app 2>/dev/null | grep -q 'not found'; }
webkit_dev() { pkg-config --exists webkit2gtk-4.1 2>/dev/null; }
webkit_hint() {
  echo "    Kubuntu/Ubuntu:  sudo apt install $1"
  echo "    Arch:            sudo pacman -S webkit2gtk-4.1"
  echo "    Fedora:          sudo dnf install webkit2gtk4.1$2"
}

# ---------------------------------------------------------------- service + CLI
if prebuilt_ok phonelinkd && prebuilt_ok phonelink; then
  echo "Using prebuilt service binaries from ./bin"
  SRC=bin
else
  command -v cargo >/dev/null || {
    echo "No compatible prebuilt binaries and Rust is not installed."
    echo "Install Rust from https://rustup.rs, then run this script again."
    exit 1
  }
  echo "Building the service from source (takes a few minutes the first time)..."
  cargo build --release --locked -p phonelink-desktop
  SRC=target/release
fi
install -Dm755 "$SRC/phonelinkd" "$BIN/phonelinkd"
install -Dm755 "$SRC/phonelink"  "$BIN/phonelink"
install -Dm644 packaging/systemd/phonelinkd.service "$HOME/.config/systemd/user/phonelinkd.service"

# ---------------------------------------------------------------- desktop window
APP_OK=0
if prebuilt_ok phonelink-app; then
  if app_libs_ok; then
    install -Dm755 bin/phonelink-app "$BIN/phonelink-app"
    APP_OK=1
  else
    echo
    echo "The PhoneLink window needs WebKitGTK 4.1. Install it, then run this script again:"
    webkit_hint libwebkit2gtk-4.1-0 ""
  fi
elif command -v cargo >/dev/null && webkit_dev; then
  echo "Building the PhoneLink window from source..."
  cargo build --release --locked -p phonelink-app
  install -Dm755 target/release/phonelink-app "$BIN/phonelink-app"
  APP_OK=1
else
  echo
  echo "Skipping the PhoneLink window: building it needs Rust and the WebKitGTK 4.1 headers:"
  webkit_hint "libwebkit2gtk-4.1-dev build-essential libssl-dev librsvg2-dev" "-devel"
fi

if [ "$APP_OK" = 1 ]; then
  for size in 32 64 128 256 512; do
    install -Dm644 "desktop/app/icons/${size}x${size}.png" "$DATA/icons/hicolor/${size}x${size}/apps/phonelink.png"
  done
  mkdir -p "$DATA/applications"
  sed "s|@BINDIR@|$BIN|" packaging/desktop/phonelink.desktop.in > "$DATA/applications/phonelink.desktop"
  command -v update-desktop-database >/dev/null && update-desktop-database -q "$DATA/applications" || true
  command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$DATA/icons/hicolor" 2>/dev/null || true
fi

# ---------------------------------------------------------------- start
systemctl --user daemon-reload
systemctl --user enable --now phonelinkd
systemctl --user restart phonelinkd   # pick up new binaries on upgrades

sleep 1
if ! systemctl --user is-active --quiet phonelinkd; then
  echo
  echo "phonelinkd did not start. See: journalctl --user -u phonelinkd -n 50"
  exit 1
fi
echo
echo "PhoneLink is running."
if [ "$APP_OK" = 1 ]; then
  echo "Open \"PhoneLink\" from your application menu to pair your phone."
else
  echo "Pair your phone from a terminal with: phonelink pair"
fi
case ":$PATH:" in *":$BIN:"*) ;; *) echo "Note: add ~/.local/bin to your PATH (log out and back in on Kubuntu).";; esac
