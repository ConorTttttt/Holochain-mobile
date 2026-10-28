#!/usr/bin/env bash
# Cross-build the embedded Android Iroh/MoQ endpoint from the *original*
# desktop game's networking crates. Does not touch either Cargo workspace,
# installed APK data, Holochain DNAs or existing standalone probe binary.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"

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

source "$ROOT/scripts/android-abi-env.sh" "${1:-arm64-v8a}"

# Isolate the desktop runtime's dependency graph from the patched Wasmi/Holochain
# workspace. Reuse the successful standalone probe's Android build cache.
PROJECT="$ROOT/target/turbo-kart-realtime-jni-source"
TARGET="$ROOT/target/turbo-kart-realtime-probe-build"
mkdir -p "$PROJECT/src" "$TARGET"
cp "$ROOT/prototypes/turbo-kart-realtime-jni/src/lib.rs" "$PROJECT/src/lib.rs"
cp "$ROOT/prototypes/turbo-kart-realtime-jni/src/race_session.rs" "$PROJECT/src/race_session.rs"
cp "$ROOT/prototypes/turbo-kart-realtime-jni/src/kart_stream.rs" "$PROJECT/src/kart_stream.rs"
cp "$ROOT/prototypes/turbo-kart-realtime-jni/src/game_bridge.rs" "$PROJECT/src/game_bridge.rs"
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
sha2 = "0.10"
tokio = { version = "1", features = ["rt-multi-thread", "time", "net", "sync"] }
jni = "0.21"
zeroize = "1"
holochain-moq-kart-wire = { path = "$GAME/crates/kart-wire" }
holochain-moq-mesh = { path = "$GAME/crates/mesh" }
holochain-moq-mesh-iroh = { path = "$GAME/crates/mesh-iroh" }
# The same SignedPresence / binding payload / TTL definitions as desktop DNA.
holochain-moq-protocol = { path = "$GAME/crates/protocol" }
# Frozen RaceRecord types and the desktop's exact locked-roster validator.
holochain-moq-turbo-kart = { path = "$GAME/crates/turbo-kart" }
holochain-moq-turbo-kart-client = { path = "$GAME/crates/turbo-kart-client" }
EOF

# Pin the isolated graph independently of the Wasmi workspace lockfile.
cp "$ROOT/prototypes/turbo-kart-realtime-jni/Cargo.lock" "$PROJECT/Cargo.lock"
echo "Building separate Android Iroh/MoQ JNI library (16 KiB aligned)..."
cargo build --locked --release --target "$RUST_TARGET" \
  --lib --manifest-path "$PROJECT/Cargo.toml" \
  --target-dir "$TARGET"

LIB="$TARGET/$RUST_TARGET/release/libturbo_kart_realtime_jni.so"
if [ ! -s "$LIB" ]; then
  echo "Android Iroh/MoQ JNI library missing: $LIB" >&2
  exit 1
fi

verify_android_elf "$LIB"

APP="$ROOT/android/turbo-kart-mobile/app"
mkdir -p "$APP/src/main/jniLibs/$ABI"
cp "$LIB" "$APP/src/main/jniLibs/$ABI/libturbo_kart_realtime_jni.so"
echo "16 KiB-aligned Iroh/MoQ JNI staged into Android APK project."
echo "$APP/src/main/jniLibs/$ABI/libturbo_kart_realtime_jni.so"
