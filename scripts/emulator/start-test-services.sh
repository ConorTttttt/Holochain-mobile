#!/usr/bin/env bash
# Start only owned test services inside the documented, memory-limited unit.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"
if [[ $(cat /proc/self/cgroup) != *"/turbo-kart-test-services.scope" ]] ||
   [[ $(systemctl --user show turbo-kart-test-services.scope -p MemoryMax --value) != 6442450944 ]]; then
  echo "Run through the turbo-kart-test-services memory-limited scope; see combined documentation." >&2
  exit 1
fi
if systemctl --user is-active --quiet turbo-kart-native-build.scope; then
  echo "Wait for native compilation before starting test services." >&2
  exit 1
fi
SDK=${ANDROID_HOME:-$HOME/Android/Sdk}
DESKTOP=${TURBO_KART_DESKTOP_BASE:-/mnt/holochain/offloaded/sf/Turbokart/turbo-kart/apps/turbo-kart-desktop}
LINKER=${TURBO_KART_LINKER_BIN:-/mnt/holochain/TurboMobile/Holochain-mobile/prototypes/h2hc-linker/target/release/h2hc-linker}
"$ROOT/scripts/emulator/start-turbo-kart.sh" > /tmp/turbokart-emulator.log 2>&1 &
EMULATOR_PID=$!
(
  cd "$DESKTOP"
  exec env -u ELECTRON_RUN_AS_NODE -u WAYLAND_DISPLAY \
    ADMIN_PORT=40093 NODE_OPTIONS=--inspect=127.0.0.1:9333 \
    XDG_SESSION_TYPE=x11 ELECTRON_OZONE_PLATFORM_HINT=x11 \
    ./node_modules/electron/dist/electron . --profile mobilebridge \
    --game-relay-url "${TURBO_KART_GAME_RELAY_URL:-https://dev-test-bootstrap2.holochain.org/}"
) > /tmp/turbokart-desktop-x11.log 2>&1 &
DESKTOP_PID=$!
env RUST_LOG=warn,h2hc_linker=debug \
  H2HC_LINKER_CONDUCTOR_URL=127.0.0.1:40093 \
  H2HC_LINKER_BOOTSTRAP_URL=https://dev-test-bootstrap2.holochain.org/ \
  H2HC_LINKER_RELAY_URL=https://dev-test-bootstrap2.holochain.org/ \
  H2HC_LINKER_KEY_FILE="$ROOT/target/emulator-linker/key.ed25519" \
  H2HC_LINKER_DATA_DIR="$ROOT/target/emulator-linker/data" \
  "$LINKER" > /tmp/turbokart-linker-bounded.log 2>&1 &
LINKER_PID=$!
trap 'kill "$EMULATOR_PID" "$DESKTOP_PID" "$LINKER_PID" 2>/dev/null || true' EXIT
wait -n "$EMULATOR_PID" "$DESKTOP_PID" "$LINKER_PID"
