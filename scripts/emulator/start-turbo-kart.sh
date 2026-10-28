#!/usr/bin/env bash
# Start only the dedicated AVD. Never fall back to CPU software emulation.
set -euo pipefail
SDK=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}
NAME=TurboKart_API35
"$SDK/emulator/emulator" -accel-check
if ! "$SDK/emulator/emulator" -list-avds | grep -Fx "$NAME" >/dev/null; then
  echo "Missing $NAME; see docs/turbo-kart-android-combined.md for official SDK/AVD creation." >&2
  exit 1
fi
ARGS=(-avd "$NAME" -accel on -gpu "${TURBO_KART_EMULATOR_GPU:-auto}" -no-snapshot)
if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then ARGS+=(-no-window); fi
exec "$SDK/emulator/emulator" "${ARGS[@]}" "$@"
