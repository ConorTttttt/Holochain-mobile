# Lightweight Holochain Nodes for Mobile v4

Working repo for the v4 plan: https://claude.ai/artifact/QzfGayyLzmZmhPsGc4zKtS. Everything here is a private local test; nothing is pushed to Holo.

| Step | Deliverable | Status |
|---|---|---|
| 1 | Physical arm64 baseline, Cranelift and wasmi | Desktop results in [benchmarks/](benchmarks/README.md); arm64 run waiting for the Android tools and a phone |
| 2 | HcP2p and HcP2pHandler compatibility matrix; injectable network factory | Done: [matrix](research/hcp2p-compat-matrix.md), [factory patch](patches/holochain-0.7.1-p2p-factory.patch) |
| 3 | Durable linker acceptance, truthful status, receipt inbox | Done in the local linker fork |
| 4 | End-to-end H2HC conductor | Runs against a real local network: reads, writes, receipts, remote calls and signals both ways |
| 5 | Suspension, restart and recovery | Reconnection, linker crash and phone kill covered; command idempotency not started |
| 6 | Checkpoints and reconciliation | Not started |
| 7 | Authority-workflow reduction | Not started |

Pinned sources: Holochain 0.7.1 (HDK 0.7.1, HDI 0.8.1) for conductors, 0.7.0 for the linker, h2hc-linker `cab242d`.

## What is built

| Path | What it is |
|---|---|
| [prototypes/h2hc-protocol](prototypes/h2hc-protocol/src/lib.rs) | Conductor-facing linker protocol v1: unflattened wire types, source labels, an acknowledged receipt inbox, signed acceptance receipts |
| [prototypes/h2hc-p2p](prototypes/h2hc-p2p/src/lib.rs) | `HcP2p` for a zero-arc phone, implementing the matrix, with a WebSocket transport and automatic reconnection |
| [prototypes/h2hc-linker](prototypes/h2hc-linker) | Local fork of h2hc-linker (CAL-1.0) with `/conductor/v1`: admission checks, SQLite acceptance store and inbox, sourced queries, remote calls and signals both ways |
| [prototypes/h2hc-conductor](prototypes/h2hc-conductor/src/lib.rs) | `build_phone_conductor`, and the `h2hc-phone` binary the benchmarks drive |
| [prototypes/h2hc-bench](prototypes/h2hc-bench/src/main.rs) | Starts a local network (bootstrap and relay, full-arc authorities, linker), runs `h2hc-phone` in each configuration, writes a report |
| [patches/](patches) | Holochain 0.7.1 changes: `ConductorBuilder::with_p2p_factory`, and app signals and DHT store reads without `test_utils` for in-process embedding |
| [tests/fixtures/notes](tests/fixtures/notes) | Test-only DNA with deliberately restrictive validation, links, remote calls and signals |

## Run the benchmarks

On this machine (the phone is an ordinary process):

```sh
./scripts/bench.sh                                   # all four configurations and the scenarios
./scripts/bench.sh --configs h2hc:cranelift --iterations 5 --idle-secs 10 --no-scenarios
```

On an Android phone over USB (needs `~/Android/Sdk/platform-tools` and `~/Android/Sdk/android-ndk-r27c`):

```sh
./scripts/bench-android.sh
```

Reports are written to `benchmarks/results/<time>-<target>.md` and `.json`; process logs go to `target/bench-runs/<time>/`.

Configurations are `network:wasm` pairs: `h2hc` (through the linker, no Kitsune2 on the phone) or `kitsune2` (a zero-arc Kitsune2 node), with `cranelift` or `wasmi`. Each run measures:

- startup, install, first zome call, and time until the phone can read authority data;
- p50 and p95 latency of local writes, reads, link queries, remote calls and signals in both directions, publish-to-visible and publish-to-receipts;
- the phone process's memory, threads, idle CPU, idle linker traffic and data on disk.

Scenarios (H2HC with Cranelift):

- an invalid note is rejected on the phone;
- the phone is killed after publishing, and receipts wait in the linker until it restarts;
- the linker crashes and restarts, keeping its acceptance log, and the phone reconnects and publishes what it wrote during the outage.

`--long` adds a phone sleep longer than the 20-minute agent-info lifetime.

## Tests

```sh
./scripts/vendor-holochain.sh && ./scripts/build-fixtures.sh
cargo test --workspace
(cd prototypes/h2hc-linker && cargo test)
```

Building zomes with this machine's Rust 1.99 outside Holonix needs two wasm flags, set in `tests/fixtures/notes/.cargo/config.toml`.

## Not built yet

- Command idempotency for retried zome calls (step 5).
- Durable phone-side storage of acceptance receipts.
- Checkpoints, reconciliation and authority-workflow reduction (steps 6 and 7).
- An installable Android app, needed to test Android's background limits and measure battery properly.
