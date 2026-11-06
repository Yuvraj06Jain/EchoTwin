#!/usr/bin/env bash
# Fail if the built APK asks for any permission outside this allow-list.
# Microphone, location, storage, Bluetooth scanning etc. are never allowed. Camera is only
# used for the pairing scan (and handed back afterwards; see MainActivity.giveUpCamera).
# SMS and contacts are asked for only when the user taps "Allow SMS access".
set -euo pipefail
APK="${1:?usage: audit-apk-permissions.sh app.apk}"
BT="$(ls -d "${ANDROID_HOME:?}"/build-tools/* | sort -V | tail -1)"

ALLOWED="
android.permission.INTERNET
android.permission.ACCESS_NETWORK_STATE
android.permission.ACCESS_WIFI_STATE
android.permission.CHANGE_WIFI_MULTICAST_STATE
android.permission.FOREGROUND_SERVICE
android.permission.FOREGROUND_SERVICE_CONNECTED_DEVICE
android.permission.RECEIVE_BOOT_COMPLETED
android.permission.REQUEST_IGNORE_BATTERY_OPTIMIZATIONS
android.permission.POST_NOTIFICATIONS
android.permission.CAMERA
android.permission.READ_SMS
android.permission.SEND_SMS
android.permission.READ_CONTACTS
android.permission.BLUETOOTH_CONNECT
android.permission.BLUETOOTH
dev.phonelink.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION
"

REQUESTED="$("$BT/aapt2" dump permissions "$APK" | grep -E "^uses-permission" | grep -oE "name='[^']+'" | cut -d"'" -f2 | sort -u)"
echo "Permissions requested by $(basename "$APK"):"
echo "$REQUESTED" | sed 's/^/  /'

bad=0
while read -r p; do
  [ -z "$p" ] && continue
  if ! grep -qxF "$p" <<< "$ALLOWED"; then
    echo "NOT ALLOWED: $p"
    bad=1
  fi
done <<< "$REQUESTED"

if [ "$bad" != 0 ]; then
  echo "The APK requests permissions outside the allow-list (scripts/audit-apk-permissions.sh)."
  exit 1
fi
echo "Permission audit passed: no microphone, location, storage or other extra access."
