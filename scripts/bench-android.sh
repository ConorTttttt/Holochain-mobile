#!/usr/bin/env bash
# Benchmark on a USB-connected arm64 Android phone. The local network runs
# on this machine; the phone reaches it through adb reverse. Arguments go
# to h2hc-bench, e.g. --serial <id> --configs h2hc:wasmi.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

export PATH="${ANDROID_HOME:-$HOME/Android/Sdk}/platform-tools:$PATH"
command -v adb >/dev/null || { echo "adb not found; unpack platform-tools into ~/Android/Sdk" >&2; exit 1; }
[ "$(adb devices | grep -c 'device$')" -ge 1 ] || { echo "no phone in 'adb devices'; check USB debugging" >&2; exit 1; }

./scripts/build-android.sh
[ -f tests/fixtures/notes/workdir/notes.happ ] || ./scripts/build-fixtures.sh
cargo build --release --bin h2hc-bench
(cd prototypes/h2hc-linker && cargo build --release)

exec target/release/h2hc-bench --target android "$@"
