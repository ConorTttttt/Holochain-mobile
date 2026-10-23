#!/usr/bin/env bash
# Build everything in release mode and run the benchmark. Arguments go to
# h2hc-bench, e.g. --configs h2hc:cranelift --iterations 5 --idle-secs 10.
# Reports land in benchmarks/results/.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

[ -d vendor/holochain-0.7.1 ] || ./scripts/vendor-holochain.sh
[ -f tests/fixtures/notes/workdir/notes.happ ] || ./scripts/build-fixtures.sh
cargo build --release --bin h2hc-phone --bin h2hc-bench
(cd prototypes/h2hc-linker && cargo build --release)

exec target/release/h2hc-bench "$@"
