#!/usr/bin/env bash
# Build the native Wasmi/H2HC JNI library and package the Android debug APK.
# Does not touch benchmark outputs or the proven adb-shell smoke executable.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

# Keep native compilation serial on shared development machines.
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"

# Build ABIs serially; each target has its own Cargo artifact directory.
ABIS=${ANDROID_ABIS:-arm64-v8a,x86_64}
APP="$ROOT/android/turbo-kart-mobile/app"
if [ ! -d vendor/holochain-0.7.1 ] || [ ! -d vendor/wasmer-7.1.0 ]; then
  ./scripts/vendor-holochain.sh
fi
IFS=',' read -ra BUILD_ABIS <<< "$ABIS"
for abi in "${BUILD_ABIS[@]}"; do
  source "$ROOT/scripts/android-abi-env.sh" "$abi"
  cargo build --locked --release --target "$RUST_TARGET" \
    -p turbo-kart-android-native --target-dir target/wasmi
  LIB="$ROOT/target/wasmi/$RUST_TARGET/release/libturbo_kart_android_native.so"
  verify_android_elf "$LIB"
  mkdir -p "$APP/src/main/jniLibs/$ABI" "$APP/src/main/assets"
  cp "$LIB" "$APP/src/main/jniLibs/$ABI/"
  bash "$ROOT/scripts/build-turbo-kart-realtime-jni.sh" "$ABI"
done

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

# Stage the existing Three.js game into a separate landscape touch-playtest
# Activity, leaving the Holochain/Iroh diagnostic screens and DNAs unchanged.
GAME_UI="${TURBO_KART_GAME_UI_DIR:-}"
if [ -z "$GAME_UI" ]; then
  for candidate in \
    "$BASE/resources/ui" \
    "/opt/Turbo Kart Rally/resources/app.asar.unpacked/resources/ui" \
    "$BASE/../../../turbo-kart-rally-main"; do
    if [ -f "$candidate/index.html" ] && [ -d "$candidate/src" ]; then
      GAME_UI="$candidate"
      break
    fi
  done
fi
if [ -n "$GAME_UI" ]; then
  python3 "$ROOT/scripts/stage-turbo-kart-touch-game.py" \
    --source "$GAME_UI" \
    --dest "$APP/src/main/assets/game" \
    --touch-script "$ROOT/mobile/touch-controls/mobile-touch.mjs"
else
  echo "Required original game missing; set TURBO_KART_GAME_UI_DIR to resources/ui." >&2
  exit 1
fi

echo "Both 16 KiB-aligned JNI libraries and original Turbo Kart hApps staged into Android project."
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
  (cd "$ANDROID_ROOT" && ./gradlew --no-daemon --max-workers="${GRADLE_WORKERS:-1}" assembleDebug -PandroidAbis="$ABIS")
elif command -v gradle >/dev/null 2>&1; then
  echo "Using installed Gradle. AGP 8.7.3 needs Gradle 8.9 and JDK 17."
  (cd "$ANDROID_ROOT" && gradle --no-daemon --max-workers="${GRADLE_WORKERS:-1}" assembleDebug -PandroidAbis="$ABIS")
else
  echo "Native build succeeded; no Gradle executable or wrapper was found." >&2
  echo "Install Gradle 8.9 / use Android Studio, then run assembleDebug in:" >&2
  echo "  $ANDROID_ROOT" >&2
  exit 2
fi

APK="$ANDROID_ROOT/app/build/outputs/apk/debug/app-debug.apk"
# AGP 8.7 should align uncompressed native libs in the APK as well as ELF.
# Google's zipalign -P 16 check verifies the APK ZIP placement separately.
if [ -x "$SDK/build-tools/35.0.0/zipalign" ]; then
  "$SDK/build-tools/35.0.0/zipalign" -c -P 16 -v 4 "$APK"
else
  echo "Warning: SDK Build-Tools 35.0.0 zipalign unavailable; verify APK ZIP alignment separately."
fi

python3 "$ROOT/scripts/verify-turbo-kart-apk.py" "$APK" --abis "$ABIS"

echo "Debug APK:"
echo "$APK"
