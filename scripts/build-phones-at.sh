#!/usr/bin/env bash
# Build the h2hc-phone binaries of another revision into OUT, laid out like
# target/, so that `h2hc-bench --phone-dir OUT` runs them against the same
# network as the current build. --android builds the arm64 Android phones
# instead of the desktop ones.
#
#   scripts/build-phones-at.sh 711db75 target/before
#   scripts/build-phones-at.sh 711db75 target/before --android
set -euo pipefail
[ $# -ge 2 ] || { echo "usage: $0 <git revision> <out dir> [--android]" >&2; exit 1; }
REV=$1
OUT=$2
ANDROID=${3:-}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)

WT="$ROOT/target/worktree-$REV"
git -C "$ROOT" worktree add --force --detach "$WT" "$REV"
trap 'git -C "$ROOT" worktree remove --force "$WT"' EXIT
cd "$WT"
./scripts/vendor-holochain.sh

if [ "$ANDROID" = "--android" ]; then
  ./scripts/build-android.sh
  for t in aarch64-linux-android/release wasmi/aarch64-linux-android/release; do
    mkdir -p "$OUT/$t" && cp "target/$t/h2hc-phone" "$OUT/$t/"
  done
else
  cargo build --release -p h2hc-conductor --bin h2hc-phone --features profile
  cargo build --release -p h2hc-conductor --bin h2hc-phone \
    --no-default-features --features wasmi,profile --target-dir target/wasmi
  for t in release wasmi/release; do
    mkdir -p "$OUT/$t" && cp "target/$t/h2hc-phone" "$OUT/$t/"
  done
fi
echo "phones from $REV in $OUT"
