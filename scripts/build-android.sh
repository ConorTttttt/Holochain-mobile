#!/usr/bin/env bash
# Cross-compile h2hc-phone for arm64 Android with the NDK's clang.
# Output: target/aarch64-linux-android/release/h2hc-phone
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

[ -d vendor/holochain-0.7.1 ] || ./scripts/vendor-holochain.sh
cargo build --release --target aarch64-linux-android --bin h2hc-phone
