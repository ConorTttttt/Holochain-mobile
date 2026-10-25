# Lightweight Holochain Nodes for Mobile v4

Working repo for the v4 plan: https://claude.ai/artifact/QzfGayyLzmZmhPsGc4zKtS. Everything here is a private local test; nothing is pushed to Holo.

| Step | Deliverable | Status |
|---|---|---|
| 1 | Physical arm64 baseline, Cranelift and wasmi | Desktop and Galaxy S23+ results in [benchmarks/](benchmarks/README.md); phone idle CPU with H2HC cut from 2.2 s/min to 12–20 ms/min; wasmi memory leak fixed, H2HC + wasmi holds about 60 MB on the phone |
| 2 | HcP2p and HcP2pHandler compatibility matrix; injectable network factory | Done: [matrix](research/hcp2p-compat-matrix.md), [factory patch](patches/holochain-0.7.1-p2p-factory.patch) |
| 3 | Durable linker acceptance, truthful status, receipt inbox | Done in the local linker fork |
| 4 | End-to-end H2HC conductor | Runs against a real local network: reads, writes, receipts, remote calls and signals both ways |
| 5 | Suspension, restart and recovery | Reconnection, linker crash and phone kill covered; command idempotency not started |
| 6 | Checkpoints and reconciliation | Not started |
| 7 | Authority-workflow reduction | Not started |

Pinned sources: Holochain 0.7.1 (HDK 0.7.1, HDI 0.8.1) for conductors, 0.7.0 for the linker, h2hc-linker `cab242d`.

## Turbo Kart Android APK milestone (25 October 2026)

The `feat/turbo-kart-integration` branch now contains a real Android
arm64 APK with in-process Holochain 0.7.1, Wasmi and H2HC, plus a
persistent Lair-backed agent protected by Android Keystore. On a Galaxy S23,
the user verified that the APK recovers **the same agent** and reopens both
original Turbo Kart hApps across force-stop/relaunch, discovers desktop
Holochain 0.7.0 lobbies, and can **join and leave a desktop public lobby
as Zippy**. The desktop Turbo Kart UI independently displayed Zippy's
arrival and departure. ELF and APK alignment checks passed, and Android
no longer displayed the former 16 KiB alignment warning.

Build via `scripts/build-turbo-kart-apk.sh`; source under
`android/turbo-kart-mobile` and
`prototypes/turbo-kart-android-native`. Detailed status and remaining
limitations: [docs/turbo-kart-android-apk.md](docs/turbo-kart-android-apk.md).
**This is multiplayer lobby control, not playable real-time racing**:
Android iroh/MoQ race sessions, graphics/physics, race UI and proper
wireless/production networking have not been implemented.

**Realtime progress:** the original desktop Iroh/MoQ transport plus
WP3 42-byte kart codec **compiled and passed on the Galaxy S23** using
`scripts/build-turbo-kart-realtime-probe.sh`. The user saw matching golden
bytes, a bound Iroh EndpointId and direct IP addresses, and `complete/ok:true`.
The updated APK's second JNI library uses Android Keystore-protected
**persistent Iroh identity**, and the user verified that **both the Iroh
EndpointId and the original Holochain agent survive app process restart**.
A subsequent build adds buttons to **Publish signed Iroh presence** and
**Verify directory Iroh bindings** using the desktop's original signature
format and existing Holochain `moq_directory` zome. This new signed-presence
feature is **in source but has not yet been built or phone-tested**.

See [docs/turbo-kart-android-realtime.md](docs/turbo-kart-android-realtime.md)
for the exact on-phone signed-presence acceptance test and next authenticated
race admission gate. Android real-time peer connectivity and MoQ races
remain unimplemented.


## What is built

| Path | What it is |
|---|---|
| [prototypes/h2hc-protocol](prototypes/h2hc-protocol/src/lib.rs) | Conductor-facing linker protocol v1: unflattened wire types, source labels, an acknowledged receipt inbox, signed acceptance receipts |
| [prototypes/h2hc-p2p](prototypes/h2hc-p2p/src/lib.rs) | `HcP2p` for a zero-arc phone, implementing the matrix, with a WebSocket transport and automatic reconnection |
| [prototypes/h2hc-linker](prototypes/h2hc-linker) | Local fork of h2hc-linker (CAL-1.0) with `/conductor/v1`: admission checks, SQLite acceptance store and inbox, sourced queries, remote calls and signals both ways |
| [prototypes/h2hc-conductor](prototypes/h2hc-conductor/src/lib.rs) | `build_phone_conductor`, and the `h2hc-phone` binary the benchmarks drive |
| [prototypes/h2hc-bench](prototypes/h2hc-bench/src/main.rs) | Starts a local network (bootstrap and relay, full-arc authorities, linker), runs `h2hc-phone` in each configuration, writes a report |
| [patches/](patches) | Holochain 0.7.1 changes: `ConductorBuilder::with_p2p_factory`; app signals and DHT store reads without `test_utils`, for in-process embedding; a scheduler that sleeps until a function is due instead of polling SQLite every 100 ms. wasmer 7.1.0: its wasmi backend frees each store when the store is dropped, instead of leaking every zome call's memory |
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

### Idle CPU

`--settle-secs` waits after the workload before idle is measured. The report then gives idle CPU for each minute, and CPU and wakeups for each thread. `--profile-secs` profiles the phone after the idle period, and `--wake-samples` times the first operations after a quiet spell. To compare another revision's phones on the same network, build them with `scripts/build-phones-at.sh` and pass `--phone-dir`:

```sh
./scripts/bench.sh --iterations 20 --wasmi-iterations 10 --settle-secs 60 --idle-secs 300 --no-scenarios
./scripts/build-phones-at.sh 711db75 target/before          # the last revision before the scheduler fix
target/release/h2hc-bench --phone-dir target/before --iterations 20 --wasmi-iterations 10 --settle-secs 60 --idle-secs 300 --no-scenarios
```

On the phone, the same comparison:

```sh
./scripts/build-phones-at.sh 711db75 target/before --android
./scripts/bench-android.sh --settle-secs 60 --idle-secs 300 --wake-samples 4
./scripts/bench-android.sh --phone-dir target/before --settle-secs 60 --idle-secs 300 --wake-samples 4
```

`network:wasm@NAME=value` gives one phone its own environment variables; this is how the scheduler experiments ran.

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
