#!/bin/bash
# Tests for the sudo approval checker and the PAM editing, without root and without PAM.
# Run: packaging/sudo-approval/test.sh
set -u
here=$(cd "$(dirname "$0")" && pwd)
helper=$here/echotwin-sudo-approve
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
ok=0
bad=0
check() { if eval "$1"; then ok=$((ok + 1)); else bad=$((bad + 1)); echo "FAIL: $2"; fi; }

# The "phone": a P-256 key; a second one for the wrong-key case.
openssl ecparam -name prime256v1 -genkey -noout -out "$work/phone.key" 2>/dev/null
openssl ec -in "$work/phone.key" -pubout -out "$work/phone.pub" 2>/dev/null
openssl ecparam -name prime256v1 -genkey -noout -out "$work/other.key" 2>/dev/null

# The fixed vector shared with core/src/message.rs the_signed_approval_message_is_fixed.
nonce=$(printf '0123456789abcdef%.0s' 1 2 3 4)
expected=$(printf 'echotwin-approve-v1\nsudo\nMARK-I\n%s\n' "$nonce" | sha256sum | cut -c1-64)
vector=$(printf 'echotwin-approve-v1\nsudo\nMARK-I\n0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n' | sha256sum | cut -c1-64)
check '[ "$expected" = "$vector" ]' "message format matches the shared vector"

# A fake `phonelink approve-relay`: MODE says how it misbehaves.
cat >"$work/relay" <<'FAKE'
#!/bin/bash
while [ $# -gt 0 ]; do case $1 in --what) what=$2; shift;; --host) host=$2; shift;; --nonce) nonce=$2; shift;; esac; shift; done
echo "$nonce" >"$WORK/last-nonce"
key=$WORK/phone.key
case $MODE in
  wrongkey) key=$WORK/other.key ;;
  othernonce) nonce=$(printf 'f%.0s' $(seq 64)) ;;
  garbage) echo "not hex; rm -rf /"; exit 0 ;;
  empty) exit 0 ;;
  slow) sleep 60 ;;
  fail) echo "denied on the phone" >&2; exit 1 ;;
esac
printf 'echotwin-approve-v1\n%s\n%s\n%s\n' "$what" "$host" "$nonce" | openssl dgst -sha256 -sign "$key" | od -An -tx1 | tr -d ' \n'
FAKE
chmod +x "$work/relay"

run() { # MODE [extra env…] → exit code of the checker
    local mode=$1; shift
    env -i PATH=/usr/bin:/bin WORK="$work" MODE="$mode" ECHOTWIN_TEST=1 PAM_SERVICE=echotwin-test \
        ECHOTWIN_KEY="$work/phone.pub" ECHOTWIN_RELAY="$work/relay" PAM_TYPE=auth PAM_USER="$(id -un)" "$@" \
        "$helper" >"$work/out" 2>&1
}

run good;        check '[ $? = 0 ]' "a correct signature approves"
code=$(grep -o 'code [0-9A-F]\{4\}' "$work/out" | cut -c6-)
first=$(cut -c1-4 "$work/last-nonce" | tr a-f A-F)
check '[ -n "$code" ] && [ "$code" = "$first" ]' "the code shown is the nonce's first 4 characters ($code vs $first)"
n1=$(cat "$work/last-nonce"); run good; n2=$(cat "$work/last-nonce")
check '[ "$n1" != "$n2" ] && [[ $n1 =~ ^[0-9a-f]{64}$ ]]' "a fresh 64-hex nonce every time"
for mode in wrongkey othernonce garbage empty fail; do
    run "$mode"; check '[ $? = 1 ]' "refused: $mode"
done
start=$(date +%s); run slow timeout_short=1; rc=$?
# The checker's own limit is 40 s; the slow fake sleeps 60, so this takes ~40 s.
check '[ $rc = 1 ] && [ $(( $(date +%s) - start )) -lt 50 ]' "refused: a relay that hangs (timeout)"
run good PAM_USER='root;id';  check '[ $? = 1 ]' "refused: odd user name"
run good PAM_USER=nosuchuser9; check '[ $? = 1 ]' "refused: unknown user"
run good PAM_TYPE=account;     check '[ $? = 1 ]' "refused: not the auth step"
run good PAM_SERVICE=sudo;     check '[ $? = 1 ]' "test switches ignored outside test mode (real key path missing here)"

# PAM editing on sample files.
ECHOTWIN_LIB=1 source "$here/../sudo-approval.sh"
cat >"$work/sudo" <<'PAM'
#%PAM-1.0
session    required   pam_env.so readenv=1 user_readenv=0
@include common-auth
@include common-account
@include common-session-noninteractive
PAM
cp "$work/sudo" "$work/orig"
pam_insert "$work/sudo"; pam_insert "$work/sudo"
check '[ $(grep -cxF "$LINE" "$work/sudo") = 1 ]' "the line is added once (idempotent)"
check '[ "$(grep -n "" "$work/sudo" | grep -F "pam_exec" | cut -d: -f1)" = 3 ]' "it comes right before @include common-auth"
pam_remove "$work/sudo"
check 'cmp -s "$work/sudo" "$work/orig"' "removing restores the file exactly"
printf '#%%PAM-1.0\nauth required pam_unix.so\n' >"$work/sudo2"
pam_insert "$work/sudo2"
check '[ "$(sed -n 2p "$work/sudo2")" = "$LINE" ]' "without common-auth it goes before the first auth line"
check '[ "$(key_fingerprint "$work/phone.pub")" != "" ] && [[ $(key_fingerprint "$work/phone.pub") =~ ^[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}$ ]]' "key fingerprint format"

echo "$ok passed, $bad failed"
[ "$bad" = 0 ]
