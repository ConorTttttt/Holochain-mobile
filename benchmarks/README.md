# Benchmark results

All runs so far are on a desktop (Linux x86_64), with the "phone" as an ordinary process and the whole network on localhost: release builds, a local bootstrap and relay, 3 full-arc authorities, and the local linker fork. They show how the design behaves, not phone performance: there is no radio, no Android scheduler and no ARM CPU. The arm64 runs (plan step 1) need `scripts/bench-android.sh` and a phone.

| Report | What it covers |
|---|---|
| [20261023-120551-local](results/20261023-120551-local.md) | All four configurations, 20 iterations, 60 s idle, scenarios |
| [20261023-121834-local](results/20261023-121834-local.md) | Scenarios only, on a fresh network |
| [20261023-122614-local](results/20261023-122614-local.md) | H2HC with both engines at 60 iterations: wasmi memory growth and idle traffic |

## What the numbers say

**Latency through the linker matches direct Kitsune2.** p50 on localhost:

| Operation | H2HC | Zero-arc Kitsune2 | Two full nodes |
|---|---|---|---|
| Remote call, either direction | 6.1–6.5 ms | 5.7–6.9 ms | 5.0 ms |
| Signal round trip, either direction | 7.5–8.6 ms | 7.6–8.2 ms | 6.2 ms |
| Read an authority's note (first fetch) | 3.7–3.8 ms | 4.0–4.5 ms | – |
| Link query | 5.5–7.6 ms | 7.4–9.0 ms | – |

These are localhost figures; on a phone the radio dominates.

**H2HC reaches the network much sooner.** It needs no peer discovery:

| Configuration | Time to network-ready |
|---|---|
| H2HC + Cranelift | 0.84 s |
| H2HC + wasmi | 0.21 s |
| Zero-arc Kitsune2 + Cranelift | 7.6 s |
| Zero-arc Kitsune2 + wasmi | 2.1 s |

**Validation receipts arrive much sooner through the linker:** about 0.15–0.19 s (p50), against about 10 s for zero-arc Kitsune2. The 10-second Kitsune2 delay was consistent but is not yet explained.

**Publishing to authorities is slower through the linker:** 33–59 ms (p50) from write to visible, against 5 ms for zero-arc Kitsune2. The linker commits each acceptance to SQLite with full sync before replying, so durability has a measurable cost.

**Idle cost is the conductor, not the network layer.** Over a clean idle minute:

| | H2HC | Zero-arc Kitsune2 |
|---|---|---|
| CPU | 271–280 ms/min | 285–304 ms/min |
| Linker traffic | 4–7 KB/min, nearly all late receipt deliveries | not measured |

An idle minute straight after a heavy workload measures higher, because receipts and republishes are still settling.

**Memory is about the same with or without Kitsune2:** about 420–450 MB RSS with Cranelift on desktop. Dropping Kitsune2 from the phone does not cut memory here; the conductor, the WASM engine and the databases dominate.

**wasmi starts far faster but leaks.** Compared with Cranelift:
- install 134 ms vs 582 ms;
- first call 152 ms vs 785 ms;
- 129 MB RSS at ready vs 423 MB;
- 11 MB on disk vs 51 MB;
- less than half the CPU over the same workload.

But RSS grows with every zome call: 971 MB after 20 iterations and 2.4 GB after 60, and it is not released while idle. As shipped in Holochain 0.7.1, wasmi is not usable on a phone. A plausible cause, not verified: the process-wide shared wasmi engine in `holochain_wasmer_host` 0.0.103 keeps per-instance state.

## Scenarios

- **An invalid note is rejected by the phone's own validation** before anything is published. Passes.
- **Receipts wait in the linker while the phone is dead.** Passes in [121834](results/20261023-121834-local.md):
  - the phone is killed as soon as the linker accepts its note;
  - an authority still reads the note from the linker, 2 ms later;
  - receipts wait in the linker's inbox and reach the phone 103 ms after it restarts.

  The full run's attempt failed on timing; the scenario now records diagnostics for that case.
- **Linker crash.** Every acceptance survives, and the phone reconnects in about 0.2–0.3 s. A note written during the outage takes **59–60 s** to publish after reconnecting, because the conductor's republish timer, not the reconnect, decides when failed publishes are retried.

## Found while building

These are behaviours of Holochain 0.7.1 or the upstream linker that the prototype had to handle:

- The 0.7.1 publish workflow passes `HcP2p::publish` an untyped (`ExternalHash`) basis.
- Authorities only send validation receipts for ops published with bit 0 of the Kitsune2 publish metadata set. The upstream linker never set it, so linker publishes never got receipts.
- Without `TCP_NODELAY`, Nagle's algorithm with delayed ACKs added about 40 ms to some round trips over the phone–linker WebSocket.
- Holochain 0.7.1 cannot run wasmi when Cranelift is also compiled in: it reads wasmer's metering global and panics. Each engine needs its own build.
- An app embedding the conductor in-process needs `subscribe_to_app_signals`, which 0.7.1 only compiles with `test_utils` (`patches/holochain-0.7.1-embedder-access.patch`).
- Lair's in-process keystore puts a Unix socket inside the data directory, so long data paths fail (`SUN_LEN`, about 108 bytes).
