#!/usr/bin/env bash
# Fails if the Android app calls Bluetooth APIs that need BLUETOOTH_SCAN, which EchoTwin
# deliberately doesn't have (the manifest removes it). Such a call throws SecurityException
# at run time: cancelDiscovery() did exactly that and broke the Bluetooth backup.
set -euo pipefail
cd "$(dirname "$0")/.."
hits=$(git grep -nE 'cancelDiscovery\(|[Aa]dapter\??\.startDiscovery\(|isDiscovering|[Bb]luetoothLeScanner|\.startScan\(' \
    -- android/app/src/main/java ':!android/app/src/main/java/dev/phonelink/core/' || true)
if [ -n "$hits" ]; then
    echo "These need the BLUETOOTH_SCAN permission, which EchoTwin doesn't have:" >&2
    echo "$hits" >&2
    exit 1
fi
echo "Android source check: ok"
