#!/usr/bin/env bash
# Build the test-only fixture DNA zomes to wasm. Tests read them from
# tests/fixtures/notes/target/wasm32-unknown-unknown/release/ and packs them
# into tests/fixtures/notes/workdir/notes.happ.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT/tests/fixtures/notes"
cargo build --release --target wasm32-unknown-unknown

hc dna pack workdir
hc app pack workdir
