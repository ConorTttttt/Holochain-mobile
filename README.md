# Lightweight Holochain Nodes for Mobile v4

Working repo for the v4 plan: https://claude.ai/artifact/QzfGayyLzmZmhPsGc4zKtS

| Step | Deliverable | Status |
|---|---|---|
| 1 | Physical arm64 baseline, Cranelift and wasmi | Blocked: needs an arm64 phone over adb |
| 2 | HcP2p and HcP2pHandler compatibility matrix; injectable network factory | Done: [matrix](research/hcp2p-compat-matrix.md), [factory patch](patches/holochain-0.7.1-p2p-factory.patch) |
| 3 | Durable linker acceptance, truthful status, receipt inbox | Protocol and phone side done; linker side not started |
| 4 | End-to-end H2HC conductor | Phone conductor runs end to end against a mock linker; real linker not started |
| 5 | Suspension, restart and recovery | Not started |
| 6 | Checkpoints and reconciliation | Not started |
| 7 | Authority-workflow reduction | Not started |

Pinned sources: Holochain 0.7.1 (HDK 0.7.1, HDI 0.8.1), h2hc-linker `cab242d` (github.com/Holo-Host/h2hc-linker).

## What is built

| Path | What it is |
|---|---|
| [prototypes/h2hc-protocol](prototypes/h2hc-protocol/src/lib.rs) | Conductor-facing linker protocol v1. Carries Holochain wire types unflattened, labels each answer's source (authority, linker buffer, linker cache), delivers validation receipts through an acknowledged inbox, and defines signed acceptance receipts |
| [prototypes/h2hc-p2p](prototypes/h2hc-p2p/src/lib.rs) | `HcP2p` for a zero-arc phone, implementing every row of the matrix. Drops buffer-sourced data by default, signs agent infos only for joined, zero-arc agents with a lifetime of at most 21 minutes, and returns `Ok` from `publish` only after verifying a receipt from the pinned linker |
| [prototypes/h2hc-conductor](prototypes/h2hc-conductor/src/lib.rs) | `build_phone_conductor`: a Holochain conductor that runs no Kitsune2 |
| [patches/holochain-0.7.1-p2p-factory.patch](patches/holochain-0.7.1-p2p-factory.patch) | Adds `ConductorBuilder::with_p2p_factory`; the upstream change to propose |
| [tests/fixtures/notes](tests/fixtures/notes) | Test-only DNA with deliberately restrictive validation |

## Build and test

```sh
./scripts/vendor-holochain.sh   # published holochain 0.7.1 plus the factory patch, into vendor/
./scripts/build-fixtures.sh     # fixture zomes to wasm, packed into notes.happ
cargo test --workspace
```

The end-to-end test (`prototypes/h2hc-conductor/tests/phone_conductor.rs`) installs the fixture app on a conductor whose only network is an in-memory linker. It then checks:

- the conductor joins and signs a zero-arc agent info;
- a note is authored and its ops are published, each with a verified acceptance receipt;
- an invalid note is rejected on the phone;
- the author reads its note back;
- a validation receipt delivered through the linker inbox is acknowledged after the conductor stores it.

The fixture build sets two wasm flags in `tests/fixtures/notes/.cargo/config.toml`, because this machine's Rust 1.99 is newer than Holonix's:

- `getrandom_backend="custom"`
- `--import-undefined`, which is needed for Holochain host functions.

## Not built yet

- The linker side of the protocol: a conductor-facing endpoint in h2hc-linker with a durable acceptance store, admission checks and the receipt inbox.
- A real byte transport. Sessions run over an in-memory frame channel; a WebSocket transport comes with the linker work.
- Reconnection, and durable phone-side storage of acceptance receipts (step 5).
