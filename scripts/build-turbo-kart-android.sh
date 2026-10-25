#!/usr/bin/env bash
# Cross-compile the Turbo Kart H2HC/Wasmi smoke harness for Android ARM64.
# Does not rebuild the existing h2hc-phone benchmarks or alter any hApps.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

NDK=${ANDROID_NDK_HOME:-$HOME/Android/Sdk/android-ndk-r27c}
API=${ANDROID_API:-26}
BIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
[ -x "$BIN/aarch64-linux-android$API-clang" ] || {
  echo "NDK not found at $NDK (set ANDROID_NDK_HOME)" >&2
  exit 1
}

export ANDROID_NDK_HOME="$NDK"
export PATH="$BIN:$PATH"
export CC_aarch64_linux_android="$BIN/aarch64-linux-android$API-clang"
export CXX_aarch64_linux_android="$BIN/aarch64-linux-android$API-clang++"
export AR_aarch64_linux_android="$BIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"

[ -d vendor/holochain-0.7.1 ] && [ -d vendor/wasmer-7.1.0 ] || ./scripts/vendor-holochain.sh
cargo build --release --target aarch64-linux-android \
  -p h2hc-conductor --bin turbo-kart-smoke \
  --no-default-features --features wasmi --target-dir target/wasmi

echo "Android ARM64 smoke harness:"
echo "$ROOT/target/wasmi/aarch64-linux-android/release/turbo-kart-smoke"
