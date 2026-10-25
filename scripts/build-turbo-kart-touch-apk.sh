#!/usr/bin/env bash
# Prepare the original Three.js game as Android assets and package a touch APK.
# Preserves the existing H2HC, Iroh/MoQ test code and original DNAs.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP="$ROOT/android/turbo-kart-mobile/app"
BASE=${TURBO_KART_DESKTOP_BASE:-/mnt/holochain/offloaded/sf/Turbokart/turbo-kart/apps/turbo-kart-desktop}
TOUCH="$ROOT/android/turbo-kart-mobile/touch-source/touch-controls.js"

# Prefer the EXACT desktop-packaged runtime, which includes vendored Three.js
# and all game modules and assets that passed desktop module validation.
UI=${TURBO_KART_UI_DIR:-"$BASE/resources/ui"}
if [ ! -f "$UI/index.html" ] || [ ! -d "$UI/src" ]; then
    echo "Turbo Kart packaged UI was not found at: $UI" >&2
    echo "The desktop game's renderer lives in its sibling turbo-kart-rally-main repo." >&2
    echo "First use its existing packaged runtime (usually desktop/resources/ui)." >&2
    echo "Otherwise set TURBO_KART_UI_DIR to your matching game folder with index.html and src/." >&2
    echo "Do not rebuild Holochain DNAs for this touch preview." >&2
    exit 1
fi

[ -f "$TOUCH" ] || { echo "Missing touch overlay: $TOUCH" >&2; exit 1; }

STAGED="$APP/src/main/assets"
mkdir -p "$STAGED/mobile"
# Only the generated touch game directory is replaced. Never overwrite the
# Android app's bundled hApps, Lair state or existing JNI libraries.
rm -rf "$STAGED/game"
mkdir -p "$STAGED/game"
# Copy only runtime assets. If an explicit source checkout is supplied, do not
# accidentally bundle its .git/, docs/, node_modules/ or developer secrets.
cp "$UI/index.html" "$STAGED/game/"
cp -R "$UI/src" "$STAGED/game/"
for name in assets vendor LICENSE NOTICE icon.png; do
  if [ -e "$UI/$name" ]; then
    cp -R "$UI/$name" "$STAGED/game/"
  fi
done
cp "$TOUCH" "$STAGED/mobile/touch-controls.js"

# Fail rather than packaging a blank WebView.
test -s "$STAGED/game/index.html"
test -s "$STAGED/game/src/main.js"
test -s "$STAGED/mobile/touch-controls.js"

echo "Staged desktop game and mobile touch layer from:"
echo "  $UI"
echo "  $TOUCH"
echo "Game entry SHA256:"
sha256sum "$STAGED/game/index.html"
echo
echo "Building APK with the preserved native H2HC/Iroh libraries..."
exec bash "$ROOT/scripts/build-turbo-kart-apk.sh" "$@"
