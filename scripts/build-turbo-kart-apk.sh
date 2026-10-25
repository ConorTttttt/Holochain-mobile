#!/usr/bin/env bash
# Build the native Wasmi/H2HC JNI library and package the Android debug APK.
# Does not touch benchmark outputs or the proven adb-shell smoke executable.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

NDK=${ANDROID_NDK_HOME:-$HOME/Android/Sdk/android-ndk-r27c}
API=${ANDROID_API:-26}
BIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
if [ ! -x "$BIN/aarch64-linux-android$API-clang" ]; then
  echo "Android NDK unavailable at $NDK (set ANDROID_NDK_HOME)" >&2
  exit 1
fi

export ANDROID_NDK_HOME="$NDK"
export PATH="$BIN:$PATH"
export CC_aarch64_linux_android="$BIN/aarch64-linux-android$API-clang"
export CXX_aarch64_linux_android="$BIN/aarch64-linux-android$API-clang++"
export AR_aarch64_linux_android="$BIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"

if [ ! -d vendor/holochain-0.7.1 ] || [ ! -d vendor/wasmer-7.1.0 ]; then
  ./scripts/vendor-holochain.sh
fi

cargo build --release --target aarch64-linux-android \
  -p turbo-kart-android-native \
  --target-dir target/wasmi

APP="$ROOT/android/turbo-kart-mobile/app"
mkdir -p "$APP/src/main/jniLibs/arm64-v8a" "$APP/src/main/assets"

cp target/wasmi/aarch64-linux-android/release/libturbo_kart_android_native.so \
  "$APP/src/main/jniLibs/arm64-v8a/libturbo_kart_android_native.so"

BASE=${TURBO_KART_DESKTOP_BASE:-/mnt/holochain/offloaded/sf/Turbokart/turbo-kart/apps/turbo-kart-desktop}
for file in \
  "$BASE/.happs/gaming_platform.happ" \
  "$BASE/.happs/turbo_kart.happ" \
  "$BASE/expected-dna-hashes.json"; do
  if [ ! -s "$file" ]; then
    echo "Missing original desktop bundle: $file" >&2
    exit 1
  fi
  cp "$file" "$APP/src/main/assets/"
done

echo "JNI arm64 library and original Turbo Kart hApps staged into Android project."
echo "Android project: $ROOT/android/turbo-kart-mobile"

if [ "${1:-}" = "--prepare-only" ]; then
  echo "Skipping Gradle as requested."
  exit 0
fi

ANDROID_ROOT="$ROOT/android/turbo-kart-mobile"
if [ -x "$ANDROID_ROOT/gradlew" ]; then
  (cd "$ANDROID_ROOT" && ./gradlew assembleDebug)
elif command -v gradle >/dev/null 2>&1; then
  echo "Using installed Gradle. AGP 8.7.3 needs Gradle 8.9 and JDK 17."
  (cd "$ANDROID_ROOT" && gradle assembleDebug)
else
  echo "Native build succeeded; no Gradle executable or wrapper was found." >&2
  echo "Install Gradle 8.9 / use Android Studio, then run assembleDebug in:" >&2
  echo "  $ANDROID_ROOT" >&2
  exit 2
fi

echo "Debug APK:"
echo "$ANDROID_ROOT/app/build/outputs/apk/debug/app-debug.apk"
