#!/usr/bin/env bash
# Recreate vendor/holochain-0.7.1: the published crate plus
# patches/holochain-0.7.1-p2p-factory.patch, which adds
# ConductorBuilder::with_p2p_factory.
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
patch -s -d "$DEST" -p1 < "$ROOT/patches/holochain-$VERSION-p2p-factory.patch"
echo "vendored holochain $VERSION with the p2p factory patch at $DEST"
