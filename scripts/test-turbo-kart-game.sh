#!/usr/bin/env bash
# Run original physics/items/WP3/interpolation assertions against APK-staged code.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
GAME="$ROOT/android/turbo-kart-mobile/app/src/main/assets/game"
TESTS=${TURBO_KART_GAME_TESTS_DIR:-/mnt/holochain/offloaded/sf/Turbokart/turbo-kart-rally-main/tests}
DEST="$ROOT/target/staged-game-tests"
mkdir -p "$DEST/tests"
for suite in netplay driving-surfaces weapons; do
  cp "$TESTS/$suite.test.mjs" "$DEST/tests/"
done
ln -sfn "$GAME/src" "$DEST/src"
export TURBO_KART_THREE="$GAME/vendor/three@0.170.0"
test -f "$TURBO_KART_THREE/build/three.module.js"
node --test --test-isolation=none "$DEST"/tests/*.test.mjs
