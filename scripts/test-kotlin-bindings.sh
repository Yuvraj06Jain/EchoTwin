#!/usr/bin/env bash
# End-to-end: generated Kotlin bindings (JVM) <-> Rust core <-> real phonelinkd (and the
# real phonelink-touchpad service for the Touchpad tab's API).
set -euo pipefail
cd "$(dirname "$0")/.."
./scripts/gen-kotlin-bindings.sh
cargo build -q -p phonelink-desktop --bins
cargo build -q -p phonelink-touchpad
cd core-ffi/kotlin-test && gradle test --console=plain "$@"
