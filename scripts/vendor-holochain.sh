#!/usr/bin/env bash
# Recreate vendor/: published crates plus every patches/<crate>-<version>-*.patch.
#
# holochain 0.7.1:
# - p2p-factory: ConductorBuilder::with_p2p_factory
# - embedder-access: app signals and DHT store reads without test_utils
# - scheduler-wakeups: the scheduler sleeps until a function is due instead
#   of polling the database every 100 ms
#
# wasmer 7.1.0:
# - wasmi-store-ownership: the wasmi backend deletes the C API objects it
#   creates in a store with the store, so a dropped store frees its instances
#   and linear memories
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)

vendor() {
  local name=$1 version=$2 sha256=$3
  local dest="$ROOT/vendor/$name-$version"
  local tmp
  tmp=$(mktemp -d)
  local crate="$tmp/$name-$version.crate"
  local cached
  cached=$(ls "${CARGO_HOME:-$HOME/.cargo}"/registry/cache/*/"$name-$version.crate" 2>/dev/null | head -n1 || true)
  if [ -n "$cached" ]; then
    cp "$cached" "$crate"
  else
    curl -sSfL "https://static.crates.io/crates/$name/$name-$version.crate" -o "$crate"
  fi
  echo "$sha256  $crate" | sha256sum -c --quiet -

  rm -rf "$dest"
  mkdir -p "$ROOT/vendor"
  tar -xzf "$crate" -C "$ROOT/vendor"
  rm -rf "$tmp"
  local n=0
  for p in "$ROOT"/patches/"$name-$version"-*.patch; do
    [ -e "$p" ] || continue
    patch -s -d "$dest" -p1 < "$p"
    n=$((n + 1))
  done
  echo "vendored $name $version with $n patches at $dest"
}

vendor holochain 0.7.1 f4bfdd6137ee65f2966cf0bbabed0ebfd71edc3a25443596c1bcad20c9a39035
vendor wasmer 7.1.0 57bf3ce47ae9ef62e35c0b23e89b72250548d6d960d0832a5638b05a701769a8
