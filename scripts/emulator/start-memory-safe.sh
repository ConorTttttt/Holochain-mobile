#!/usr/bin/env bash
# Linux desktop test stack, bounded as one group and kept separate from builds.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
if systemctl --user is-active --quiet turbo-kart-native-build.scope; then
  echo "Wait for the native build to finish before starting test applications." >&2
  exit 1
fi
AVAILABLE_KIB=$(awk '/MemAvailable:/ { print $2 }' /proc/meminfo)
if (( AVAILABLE_KIB < 12 * 1024 * 1024 )); then
  echo "Need 12 GiB available before starting the bounded emulator/desktop stack." >&2
  exit 1
fi
cd "$ROOT"
exec systemd-run --user --scope --unit=turbo-kart-test-services \
  -p MemoryHigh=6G -p MemoryMax=6G -p MemorySwapMax=0 -p OOMPolicy=kill \
  bash "$ROOT/scripts/emulator/start-test-services.sh"
