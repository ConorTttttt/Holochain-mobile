#!/usr/bin/env bash
# Linux shared-host build: protect the editor with a limit on this build only.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
for unit in turbo-kart-test-services.scope turbo-kart-test-authority.scope; do
  if systemctl --user is-active --quiet "$unit"; then
    echo "Stop the owned test unit before native compilation: systemctl --user stop $unit" >&2
    exit 1
  fi
done
AVAILABLE_KIB=$(awk '/MemAvailable:/ { print $2 }' /proc/meminfo)
if (( AVAILABLE_KIB < 8 * 1024 * 1024 )); then
  echo "Need 8 GiB available before a native build; stop the test emulator/desktop first." >&2
  exit 1
fi
export CARGO_BUILD_JOBS=1
export GRADLE_WORKERS=1
export ANDROID_SERIAL_MAKE=1
exec systemd-run --user --scope --unit=turbo-kart-native-build \
  -p MemoryHigh=3G -p MemoryMax=4G -p MemorySwapMax=0 -p OOMPolicy=kill \
  env CARGO_BUILD_JOBS=1 GRADLE_WORKERS=1 ANDROID_SERIAL_MAKE=1 \
  PATH="$HOME/.local/opt/gradle-8.9/bin:$PATH" \
  bash "$ROOT/scripts/build-turbo-kart-apk.sh" "$@"
