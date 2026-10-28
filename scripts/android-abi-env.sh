#!/usr/bin/env bash
# Source with one APK ABI. Target flags stay separate from host build scripts.
case "${1:-arm64-v8a}" in
  arm64-v8a) ABI=arm64-v8a; RUST_TARGET=aarch64-linux-android ;;
  x86_64) ABI=x86_64; RUST_TARGET=x86_64-linux-android ;;
  *) echo "Unsupported Android ABI: $1" >&2; return 1 ;;
esac
NDK=${ANDROID_NDK_HOME:-$HOME/Android/Sdk/android-ndk-r27c}
API=${ANDROID_API:-26}
BIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
CLANG="$BIN/$RUST_TARGET$API-clang"
if [ ! -x "$CLANG" ]; then
  echo "Android NDK linker unavailable: $CLANG" >&2; return 1
fi
export ANDROID_NDK_HOME="$NDK"
export PATH="$BIN:$PATH"
if [ "${ANDROID_SERIAL_MAKE:-0}" = 1 ]; then
  export PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/android-build-tools:$PATH"
fi
TARGET_ENV=${RUST_TARGET//-/_}
export "CC_$TARGET_ENV=$CLANG"
export "CXX_$TARGET_ENV=$CLANG++"
export "AR_$TARGET_ENV=$BIN/llvm-ar"
export "CARGO_TARGET_${TARGET_ENV^^}_LINKER=$CLANG"
export "CARGO_TARGET_${TARGET_ENV^^}_RUSTFLAGS=-C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384"
verify_android_elf() {
  local library=$1 machine
  case "$ABI" in arm64-v8a) machine=AArch64 ;; x86_64) machine='Advanced Micro Devices X86-64' ;; esac
  "$BIN/llvm-readelf" -h "$library" | grep -F "Machine:" | grep -F "$machine" >/dev/null || return 1
  "$BIN/llvm-readelf" -lW "$library" | awk '
    $1 == "LOAD" { n++; if ($NF != "0x4000") { print "Invalid 16 KiB LOAD alignment: " $0 > "/dev/stderr"; bad=1 } }
    END { if (n == 0) bad=1; exit bad }'
}
