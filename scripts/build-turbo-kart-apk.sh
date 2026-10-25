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

# NDK r27c defaults to 4 KiB ELF LOAD alignment. Android 15+ can use
# 16 KiB pages; link Rust's JNI .so with both 16 KiB page-size settings.
# Target-specific flags avoid changing the Linux build or CLI benchmarks.
# A changed rustflags fingerprint can require Cargo to rebuild target deps.
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384"
echo "Building 16 KiB-aligned Android JNI library (NDK r27 compatibility)."

if [ ! -d vendor/holochain-0.7.1 ] || [ ! -d vendor/wasmer-7.1.0 ]; then
  ./scripts/vendor-holochain.sh
fi

cargo build --release --target aarch64-linux-android \
  -p turbo-kart-android-native \
  --target-dir target/wasmi

LIB="$ROOT/target/wasmi/aarch64-linux-android/release/libturbo_kart_android_native.so"
# Fail before APK packaging if a linker/toolchain change regresses alignment.
# NDK llvm-readelf's "LOAD" segment Align column should be 0x4000 (16 KiB).
if ! "$BIN/llvm-readelf" -lW "$LIB" | awk '
  $1 == "LOAD" {
    n++
    if ($NF != "0x4000") {
      print "ERROR: Android ELF LOAD segment is not 16 KiB aligned: " $0 > "/dev/stderr"
      bad = 1
    }
  }
  END {
    if (n == 0) {
      print "ERROR: no ELF LOAD segments found" > "/dev/stderr"
      bad = 1
    }
    exit bad
  }
'; then
  echo "Refusing to package a JNI library with incorrect ELF alignment." >&2
  exit 1
fi
echo "Verified: all JNI library ELF LOAD segments use 16 KiB alignment."

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

# Gradle does not automatically discover a manually installed Android SDK.
# Keep local.properties machine-specific (it is gitignored), and preserve any
# existing Android Studio configuration instead of overwriting it.
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
if [ ! -d "$SDK/platforms/android-35" ]; then
  echo "Android SDK Platform 35 not found under $SDK" >&2
  echo "Install platforms;android-35 or set ANDROID_HOME to the SDK root." >&2
  exit 1
fi
export ANDROID_HOME="$SDK"
export ANDROID_SDK_ROOT="$SDK"
if [ ! -e "$ANDROID_ROOT/local.properties" ]; then
  printf 'sdk.dir=%s\n' "$SDK" > "$ANDROID_ROOT/local.properties"
  echo "Created local Android SDK configuration: $ANDROID_ROOT/local.properties"
fi

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
