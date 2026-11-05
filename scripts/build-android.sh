#!/usr/bin/env bash
# Build the Android APK: Rust core for arm64 (+ x86_64 emulator), then the Kotlin app.
#
# Needs: Android SDK + NDK (Android Studio > Settings > Languages & Frameworks >
#        Android SDK > SDK Tools > "NDK (Side by side)"), JDK 17+, Rust (rustup).
set -euo pipefail
cd "$(dirname "$0")/.."

SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
  ANDROID_NDK_HOME="$(ls -d "$SDK"/ndk/* 2>/dev/null | sort -V | tail -1 || true)"
fi
[[ -d "${ANDROID_NDK_HOME:-}" ]] || { echo "Android NDK not found. Install it from Android Studio's SDK Manager or set ANDROID_NDK_HOME."; exit 1; }
export ANDROID_HOME="$SDK" ANDROID_NDK_HOME
echo "sdk.dir=$SDK" > android/local.properties

command -v cargo-ndk >/dev/null || cargo install cargo-ndk
rustup target add aarch64-linux-android x86_64-linux-android

cargo ndk -t arm64-v8a -t x86_64 --platform 29 -o android/app/src/main/jniLibs \
  build --release -p phonelink-ffi

(cd android && ./gradlew --console=plain assembleDebug)
APK=android/app/build/outputs/apk/debug/app-debug.apk
echo
echo "Built $APK"
echo "Install over USB (USB debugging on):  adb install -r $APK"
