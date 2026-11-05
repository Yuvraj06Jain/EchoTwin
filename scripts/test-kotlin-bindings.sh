#!/usr/bin/env bash
# End-to-end: generated Kotlin bindings (JVM) <-> Rust core <-> real phonelinkd.
set -euo pipefail
cd "$(dirname "$0")/.."
./scripts/gen-kotlin-bindings.sh
cargo build -q -p phonelink-desktop --bins
cd core-ffi/kotlin-test && gradle test --console=plain "$@"
