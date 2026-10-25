#!/usr/bin/env bash
# Cross-build the embedded Android Iroh/MoQ endpoint from the *original*
# desktop game's networking crates. Does not touch either Cargo workspace,
# installed APK data, Holochain DNAs or existing standalone probe binary.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

GAME=${TURBO_KART_REPO:-/mnt/holochain/offloaded/sf/Turbokart/turbo-kart}
if [ ! -f "$GAME/crates/mesh-iroh/Cargo.toml" ] || [ ! -f "$GAME/crates/kart-wire/Cargo.toml" ]; then
  echo "Turbo Kart original desktop source not found: $GAME" >&2
  echo "Set TURBO_KART_REPO=/path/to/turbo-kart" >&2
  exit 1
fi
GAME=$(cd "$GAME" && pwd)
echo "Reusing original Turbo Kart realtime crates from $GAME"
if command -v git >/dev/null 2>&1; then
  git -C "$GAME" rev-parse --short=12 HEAD || true
fi

NDK=${ANDROID_NDK_HOME:-$HOME/Android/Sdk/android-ndk-r27c}
API=${ANDROID_API:-26}
BIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
if [ ! -x "$BIN/aarch64-linux-android$API-clang" ]; then
  echo "Android NDK clang unavailable at $BIN" >&2
  exit 1
fi

export ANDROID_NDK_HOME="$NDK"
export PATH="$BIN:$PATH"
export CC_aarch64_linux_android="$BIN/aarch64-linux-android$API-clang"
export CXX_aarch64_linux_android="$BIN/aarch64-linux-android$API-clang++"
export AR_aarch64_linux_android="$BIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384"

# Isolate the desktop runtime's dependency graph from the patched Wasmi/Holochain
# workspace. Reuse the successful standalone probe's Android build cache.
PROJECT="$ROOT/target/turbo-kart-realtime-jni-source"
TARGET="$ROOT/target/turbo-kart-realtime-probe-build"
mkdir -p "$PROJECT/src" "$TARGET"
cp "$ROOT/prototypes/turbo-kart-realtime-jni/src/lib.rs" "$PROJECT/src/lib.rs"
cat > "$PROJECT/Cargo.toml" <<EOF
[workspace]
resolver = "2"

[package]
name = "turbo-kart-realtime-jni"
version = "0.1.0"
edition = "2021"
publish = false

[lib]
name = "turbo_kart_realtime_jni"
crate-type = ["cdylib"]

[dependencies]
anyhow = "1"
serde_json = "1"
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["rt-multi-thread", "time", "net"] }
jni = "0.21"
zeroize = "1"
holochain-moq-kart-wire = { path = "$GAME/crates/kart-wire" }
holochain-moq-mesh-iroh = { path = "$GAME/crates/mesh-iroh" }
# The same SignedPresence / binding payload / TTL definitions as desktop DNA.
holochain-moq-protocol = { path = "$GAME/crates/protocol" }
# Frozen RaceRecord types and the desktop's exact locked-roster validator.
holochain-moq-turbo-kart = { path = "$GAME/crates/turbo-kart" }
holochain-moq-turbo-kart-client = { path = "$GAME/crates/turbo-kart-client" }
EOF

echo "Building separate Android Iroh/MoQ JNI library (16 KiB aligned)..."
cargo build --release --target aarch64-linux-android \
  --lib --manifest-path "$PROJECT/Cargo.toml" \
  --target-dir "$TARGET"

LIB="$TARGET/aarch64-linux-android/release/libturbo_kart_realtime_jni.so"
if [ ! -s "$LIB" ]; then
  echo "Android Iroh/MoQ JNI library missing: $LIB" >&2
  exit 1
fi

# All ELF LOAD segments must use 16 KiB alignment for Android 15+ devices.
"$BIN/llvm-readelf" -lW "$LIB" | awk '
  $1 == "LOAD" {
    count++
    if ($NF != "0x4000") {
      print "ERROR: non-16 KiB ELF LOAD segment: " $0 > "/dev/stderr"
      failed = 1
    }
  }
  END {
    if (count == 0) {
      print "ERROR: no ELF LOAD segments found" > "/dev/stderr"
      failed = 1
    }
    exit failed
  }
'

APP="$ROOT/android/turbo-kart-mobile/app"
mkdir -p "$APP/src/main/jniLibs/arm64-v8a"
cp "$LIB" "$APP/src/main/jniLibs/arm64-v8a/libturbo_kart_realtime_jni.so"
echo "16 KiB-aligned Iroh/MoQ JNI staged into Android APK project."
echo "$APP/src/main/jniLibs/arm64-v8a/libturbo_kart_realtime_jni.so"
