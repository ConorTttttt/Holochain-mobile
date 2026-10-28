#!/usr/bin/env bash
# Optional normal DHT authority for a fresh two-peer development network.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
if systemctl --user is-active --quiet turbo-kart-native-build.scope; then
  echo "Wait for native compilation before starting a test conductor." >&2
  exit 1
fi
AVAILABLE_KIB=$(awk '/MemAvailable:/ { print $2 }' /proc/meminfo)
if (( AVAILABLE_KIB < 4 * 1024 * 1024 )); then
  echo "Need 4 GiB available before starting the capped 1 GiB authority." >&2
  exit 1
fi
cd "$ROOT"
exec systemd-run --user --scope --unit=turbo-kart-test-authority \
  -p MemoryHigh=1G -p MemoryMax=1G -p MemorySwapMax=0 -p OOMPolicy=kill \
  python3 "$ROOT/scripts/emulator/test-authority.py"
