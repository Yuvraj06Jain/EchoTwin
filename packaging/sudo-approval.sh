#!/bin/bash
# Approve `sudo` with your phone's fingerprint (EchoTwin). Run with sudo:
#   sudo ./packaging/sudo-approval.sh install     set it up (asks you to confirm twice)
#   sudo ./packaging/sudo-approval.sh uninstall   remove it again
#   sudo ./packaging/sudo-approval.sh status
#
# What install changes:
#   /etc/echotwin/approve.pub                     your phone's public key (root-owned)
#   /usr/local/libexec/echotwin-sudo-approve      the checker (root-owned)
#   /etc/pam.d/sudo                                one line, before the password step:
#     auth sufficient pam_exec.so quiet stdout seteuid /usr/local/libexec/echotwin-sudo-approve
#   (backup: /etc/pam.d/sudo.echotwin-backup)
# "sufficient" means: if the phone approves, sudo goes ahead; if not (denied, no answer,
# phone away, anything wrong) sudo asks for your password exactly as before.
#
# Keep a root terminal open (sudo -i) while you try it, just in case.
set -euo pipefail

PAM_FILE=${ECHOTWIN_PAM_FILE:-/etc/pam.d/sudo}
KEY_DIR=/etc/echotwin
KEY=$KEY_DIR/approve.pub
HELPER=/usr/local/libexec/echotwin-sudo-approve
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
LINE="auth sufficient pam_exec.so quiet stdout seteuid $HELPER"

# ---------------------------------------------------------------- PAM editing (tested)

pam_has_line() { grep -qxF "$LINE" "$1"; }

# Put LINE before the first "@include common-auth" (or the first auth line). Idempotent.
pam_insert() {
    local f=$1 tmp
    pam_has_line "$f" && return 0
    tmp=$(mktemp)
    awk -v line="$LINE" '
        !done && ($0 ~ /^@include[ \t]+common-auth/ || $0 ~ /^[ \t]*auth[ \t]/) { print line; done = 1 }
        { print }
        END { if (!done) print line }
    ' "$f" >"$tmp"
    cat "$tmp" >"$f" # keeps the file's owner and mode
    rm -f "$tmp"
}

pam_remove() {
    local f=$1 tmp
    tmp=$(mktemp)
    grep -vxF "$LINE" "$f" >"$tmp" || true
    cat "$tmp" >"$f"
    rm -f "$tmp"
}

# First 8 bytes of SHA-256(DER public key) as XXXX-XXXX-XXXX-XXXX (as the phone shows it).
key_fingerprint() {
    openssl pkey -pubin -in "$1" -outform DER 2>/dev/null | sha256sum | cut -c1-16 | tr a-f A-F |
        sed 's/\(....\)\(....\)\(....\)\(....\)/\1-\2-\3-\4/'
}

[ "${ECHOTWIN_LIB:-}" = 1 ] && return 0 2>/dev/null # sourced by test.sh

# ---------------------------------------------------------------- commands

need_root() {
    [ "$(id -u)" = 0 ] || { echo "Run it with sudo: sudo $0 $1" >&2; exit 1; }
}

ask() {
    local reply
    read -r -p "$1 [y/N] " reply </dev/tty || true
    [ "$reply" = y ] || [ "$reply" = Y ]
}

cmd_install() {
    need_root install
    local user=${SUDO_USER:-}
    [ -n "$user" ] && [ "$user" != root ] || { echo "Run it with sudo from your own account." >&2; exit 1; }
    local home
    home=$(getent passwd "$user" | cut -d: -f6)
    local pending="${home}/.local/share/phonelink/approve-pending.pem"
    [ -f "$pending" ] || {
        echo "No phone key yet. On the phone: EchoTwin → Settings → Approve sudo with fingerprint → Set up," >&2
        echo "while it is connected to this computer. Then run this again." >&2
        exit 1
    }
    command -v openssl >/dev/null || { echo "Install openssl first." >&2; exit 1; }
    local fp
    fp=$(key_fingerprint "$pending")
    [ -n "$fp" ] || { echo "The saved phone key is unreadable; set it up again on the phone." >&2; exit 1; }
    echo "Phone key fingerprint: $fp"
    ask "Does your phone show exactly this?" || { echo "Nothing changed."; exit 1; }

    install -d -m 755 -o root -g root "$KEY_DIR"
    openssl pkey -pubin -in "$pending" -out "$KEY.new"
    chown root:root "$KEY.new"
    chmod 644 "$KEY.new"
    mv "$KEY.new" "$KEY"
    install -D -m 755 -o root -g root "$HERE/sudo-approval/echotwin-sudo-approve" "$HELPER"
    [ -f "$PAM_FILE.echotwin-backup" ] || cp -p "$PAM_FILE" "$PAM_FILE.echotwin-backup"
    pam_insert "$PAM_FILE"

    echo
    echo "Installed. Now, in a NEW terminal, run:   sudo -k; sudo true"
    echo "Your phone should ask for your fingerprint (code shown in the terminal)."
    if ask "Did it work?"; then
        echo "Done. Your password still works whenever the phone doesn't answer."
    else
        cp -p "$PAM_FILE.echotwin-backup" "$PAM_FILE"
        echo "Put /etc/pam.d/sudo back as it was. The key and checker stay; run uninstall to remove them."
        exit 1
    fi
}

cmd_uninstall() {
    need_root uninstall
    if [ -f "$PAM_FILE" ]; then pam_remove "$PAM_FILE"; fi
    rm -f "$HELPER" "$KEY"
    rmdir "$KEY_DIR" 2>/dev/null || true
    echo "Removed. sudo asks only for your password again."
}

cmd_status() {
    if [ -f "$PAM_FILE" ] && pam_has_line "$PAM_FILE"; then echo "PAM: on ($PAM_FILE)"; else echo "PAM: off"; fi
    if [ -f "$KEY" ]; then echo "Phone key: $(key_fingerprint "$KEY")"; else echo "Phone key: none"; fi
    if [ -x "$HELPER" ]; then echo "Checker: $HELPER"; else echo "Checker: not installed"; fi
}

case "${1:-}" in
install) cmd_install ;;
uninstall) cmd_uninstall ;;
status) cmd_status ;;
*) sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
