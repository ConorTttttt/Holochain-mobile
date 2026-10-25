#!/usr/bin/env bash
# Cross-build a standalone Android canary with the desktop game's ORIGINAL
# Iroh/MoQ transport crate and KartState codec. No APK, DNA or lobby changes.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

GAME=${TURBO_KART_REPO:-/mnt/holochain/offloaded/sf/Turbokart/turbo-kart}
if [ ! -f "$GAME/crates/mesh-iroh/Cargo.toml" ] || [ ! -f "$GAME/crates/kart-wire/Cargo.toml" ]; then
  echo "Turbo Kart desktop sources unavailable at $GAME" >&2
  echo "Set TURBO_KART_REPO to the checked-out turbo-kart repository." >&2
  exit 1
fi
GAME=$(cd "$GAME" && pwd)
echo "Using original desktop crates from: $GAME"
if command -v git >/dev/null 2>&1; then
  git -C "$GAME" rev-parse --short=12 HEAD || true
fi

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

# NDK r27 requires explicit 16 KiB ELF LOAD segment alignment.
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384"

# Cargo manifest lives only in the ignored target/ directory. Point straight
# to the unmodified desktop crates, preserving their original wire semantics.
# [workspace] isolates this probe from the Holochain-mobile workspace.
PROBE="$ROOT/target/turbo-kart-realtime-probe-source"
export CARGO_TARGET_DIR="$ROOT/target/turbo-kart-realtime-probe-build"
mkdir -p "$PROBE/src" "$CARGO_TARGET_DIR"
cp "$ROOT/prototypes/turbo-kart-realtime-probe/src/main.rs" "$PROBE/src/main.rs"
cat > "$PROBE/Cargo.toml" <<EOF
[workspace]
resolver = "2"

[package]
name = "turbo-kart-realtime-probe"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
anyhow = "1"
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time"] }
holochain-moq-kart-wire = { path = "$GAME/crates/kart-wire" }
holochain-moq-mesh-iroh = { path = "$GAME/crates/mesh-iroh" }
EOF

echo "Compiling desktop Iroh/MoQ + exact WP3 codec for Android ARM64..."
cargo build \
  --release \
  --target aarch64-linux-android \
  --manifest-path "$PROBE/Cargo.toml"

OUTPUT="$CARGO_TARGET_DIR/aarch64-linux-android/release/turbo-kart-realtime-probe"
if [ ! -s "$OUTPUT" ]; then
  echo "Missing Android realtime probe executable: $OUTPUT" >&2
  exit 1
fi
# Independently inspect all LOAD segments before pushing to the device.
"$BIN/llvm-readelf" -lW "$OUTPUT" | awk '
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
echo "Verified 16 KiB ELF LOAD alignment."
echo "Android realtime probe:"
echo "$OUTPUT"
