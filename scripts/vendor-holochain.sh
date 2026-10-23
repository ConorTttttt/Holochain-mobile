#!/usr/bin/env bash
# Recreate vendor/holochain-0.7.1: the published crate plus every
# patches/holochain-0.7.1-*.patch:
# - p2p-factory: ConductorBuilder::with_p2p_factory
# - embedder-access: app signals and DHT store reads without test_utils
# - scheduler-wakeups: the scheduler sleeps until a function is due instead
#   of polling the database every 100 ms
set -euo pipefail

VERSION=0.7.1
SHA256=f4bfdd6137ee65f2966cf0bbabed0ebfd71edc3a25443596c1bcad20c9a39035

ROOT=$(cd "$(dirname "$0")/.." && pwd)
DEST="$ROOT/vendor/holochain-$VERSION"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

CRATE="$TMP/holochain-$VERSION.crate"
CACHED=$(ls "${CARGO_HOME:-$HOME/.cargo}"/registry/cache/*/holochain-$VERSION.crate 2>/dev/null | head -n1 || true)
if [ -n "$CACHED" ]; then
  cp "$CACHED" "$CRATE"
else
  curl -sSfL "https://static.crates.io/crates/holochain/holochain-$VERSION.crate" -o "$CRATE"
fi
echo "$SHA256  $CRATE" | sha256sum -c --quiet -

rm -rf "$DEST"
mkdir -p "$ROOT/vendor"
tar -xzf "$CRATE" -C "$ROOT/vendor"
for p in "$ROOT"/patches/holochain-$VERSION-*.patch; do
  patch -s -d "$DEST" -p1 < "$p"
done
echo "vendored holochain $VERSION with $(ls "$ROOT"/patches/holochain-$VERSION-*.patch | wc -l) patches at $DEST"
