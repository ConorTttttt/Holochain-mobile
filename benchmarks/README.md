# Benchmark results

All runs so far are on a desktop (Linux x86_64), with the "phone" as an ordinary process and the whole network on localhost: release builds, a local bootstrap and relay, 3 full-arc authorities, and the local linker fork. They show how the design behaves, not phone performance: there is no radio, no Android scheduler and no ARM CPU. Phone results are in the next section.

| Report | What it covers |
|---|---|
| [20261023-120551-local](results/20261023-120551-local.md) | All four configurations, 20 iterations, 60 s idle, scenarios |
| [20261023-121834-local](results/20261023-121834-local.md) | Scenarios only, on a fresh network |
| [20261023-122614-local](results/20261023-122614-local.md) | H2HC with both engines at 60 iterations: wasmi memory growth and idle traffic |
| [20261023-140623-local](results/20261023-140623-local.md) | Idle baseline and profile, Kitsune2 (the H2HC phone was aborted by a slow authority) |
| [20261023-141859-local](results/20261023-141859-local.md) | Idle experiments: scheduler and publish loop switched off or slowed |
| [20261023-151131-local](results/20261023-151131-local.md) | Before the scheduler fix: all four configurations, 5 minutes idle after settling |
| [20261023-153819-local](results/20261023-153819-local.md) | After the scheduler fix: the same, plus the scenarios |
| [20261023-160825-local](results/20261023-160825-local.md), [162323](results/20261023-162323-local.md) | First operations after 40 s of quiet, before and after the fix |
| [20261023-163930-local](results/20261023-163930-local.md), [164542](results/20261023-164542-local.md) | First operations after 20 s and after 40 s of quiet, after the fix |
| [20261023-170040-local](results/20261023-170040-local.md) | After the fix, phones under strace to trace the remaining idle wakeups (strace inflates CPU and wakeups) |

## Idle CPU: Holochain's scheduler polled SQLite ten times a second

The phone run below measured 2.2–2.4 s of CPU per idle minute in every configuration. Almost all of it was Holochain 0.7.1's scheduler for functions that zomes pass to `schedule()`. It is now event-driven ([patch](../patches/holochain-0.7.1-scheduler-wakeups.patch)), which cuts idle CPU on the desktop by **98% with H2HC and 86–88% with Kitsune2**. It has not yet been measured on the phone: the phone was disconnected when the fix was ready.

### What runs when the conductor is idle

The conductor's background loops in 0.7.1:

| Loop | When it runs while idle | Cost found |
|---|---|---|
| Scheduler (`start_scheduler`) | Every 100 ms, even when nothing is scheduled (the benchmark schedules nothing) | Nearly all of it |
| Publish retry (`publish_dht_ops_consumer`) | Every 60 s, backing off to 5 min, while ops lack receipts; pauses itself otherwise | None measurable |
| Validation, integration, receipts | Only when triggered by incoming work, or retrying missing dependencies | None while idle |
| SQLite pool maintenance (sqlx) | Every 30 s per database | Negligible |
| Kitsune2 (Kitsune2 configurations only) | Iroh connections and relay, gossip timeouts, peer-store pruning every 10 s | The rest of Kitsune2's idle cost |

The H2HC adapter has no timers of its own.

Each scheduler tick ran three SQL statements per running cell:
- find expired persisted schedules;
- find live schedules;
- delete live ephemeral schedules.

sqlx pings each connection before handing it out, and each statement and ping is a round trip to a SQLite worker thread. That is about 60 round trips a second, about 15,000 thread wakeups a minute, for an app that schedules nothing.

### How it was found

The phone measured 2.2–2.4 s/min; the desktop measures about 300 ms/min of the same work. Every step ran on the desktop, because the phone was disconnected:

1. **Per-thread accounting.** The new `threads` control command showed idle CPU split between tokio workers (about 210 ms/min, 9,600 wakeups/min) and SQLite worker threads (about 110 ms/min, 5,400 wakeups/min).
2. **Profile.** `perf` needs root, so `h2hc-phone` now has an in-process SIGPROF sampler (the `profile` feature), which also works over adb. In an idle phone conductor, most sampled CPU was the scheduler task and the SQLite workers serving its queries:

   | Share of samples | [H2HC](profiles/idle-h2hc-cranelift-before.svg) | [Kitsune2](profiles/idle-kitsune2-cranelift-before.svg) |
   |---|---|---|
   | Scheduler task | 33% | 25% |
   | SQLite worker threads | 36% | 31% |
   | Tokio workers between tasks | 26% | 13% |
   | Kitsune2 and iroh | – | about 10% |

   About half of all CPU time is not sampled: it is spent in futex and epoll calls inside libc, which the sampler skips. That cost comes from the wakeups themselves.
3. **Controlled experiments** ([141859](results/20261023-141859-local.md)). A temporary build had environment switches for the scheduler interval and the publish loop. Each variant ran 4 minutes idle after 60 s of settling, on one network:

   | H2HC + Cranelift | Idle CPU | Thread wakeups |
   |---|---|---|
   | Unchanged | 296 ms/min | 15,306/min |
   | Scheduler off | 4 ms/min | 68/min |
   | Scheduler every 1 s instead of 100 ms | 30 ms/min | 1,628/min |
   | Publish loop off | 283 ms/min (no change) | 15,518/min |
   | Scheduler and publish loop off | 4 ms/min | 53/min |
   | Kitsune2 + Cranelift, scheduler off | 36 ms/min | 868/min |

   Idle cost scales with the scheduler's tick rate, and nothing else in the conductor is significant.

### The fix

[`holochain-0.7.1-scheduler-wakeups.patch`](../patches/holochain-0.7.1-scheduler-wakeups.patch) changes when the scheduler ticks, not what a tick does:

- **After each tick, it sleeps until the next function is due.** One query per running cell finds the earliest due time, `MIN(start_at)`.
- **With nothing scheduled, it never wakes on a timer.**
- **It wakes early on the events that can create a due function:**
  - a zome call or `init` writes a `schedule()` call;
  - cells start, so persisted schedules from an earlier run are picked up.
- **Ticks stay at least 100 ms apart, as before.** A function that re-arms with a zero delay runs as often as it did, not in a busy loop.
- **Each sleep is capped at 60 s.** Schedules are wall-clock times, but tokio's clock stops while a phone is suspended. The cap bounds how late a function runs after a suspend or a clock change.
- **If the due-time query fails, it falls back to polling.** The scheduler checks again after 100 ms, as it used to.

Scheduled-function support is unchanged for apps. The HDK already tells developers not to rely on the loop's frequency. The new `scheduled_functions_run` test in `h2hc-conductor` checks three things:
- `schedule()` starts functions promptly;
- an ephemeral function re-arms every 300 ms and no faster;
- a persisted crontab keeps firing.

The test fails if the wake after `schedule()` is removed.

### Before and after (desktop)

Same network and workload: 20 iterations (10 for wasmi), then 60 s settling, then 5 minutes idle. Before: [151131](results/20261023-151131-local.md), run with the pre-fix phones via `--phone-dir`. After: [153819](results/20261023-153819-local.md).

| Configuration | Idle CPU before | Idle CPU after | Wakeups before | Wakeups after |
|---|---|---|---|---|
| H2HC + Cranelift | 325 ms/min | **5 ms/min** | 15,028/min | 64/min |
| H2HC + wasmi | 323 ms/min | **5 ms/min** | 14,983/min | 57/min |
| Kitsune2 + Cranelift | 358 ms/min | **42 ms/min** | 16,837/min | 819/min |
| Kitsune2 + wasmi | 325 ms/min | **47 ms/min** | 16,192/min | 1,333/min |

Per-minute samples ranged as follows:

| | Before | After |
|---|---|---|
| H2HC | 315–328 ms | 3–7 ms |
| Kitsune2 | 292–395 ms | 37–83 ms |

The SQLite worker threads are now gone while idle: their connections close after Holochain's 30 s pool idle timeout.

**Everything else measured the same.** Local writes, reads, link queries, remote calls and signals both ways, publish-to-visible and validation receipts have the same p50 on both networks before and after. All three scenarios pass after the fix: rejection on the phone, receipts waiting while the phone is killed, and the linker crash.

**One trade-off to watch on the phone.** Before, the scheduler's queries kept the pooled SQLite connections open. Now they close after 30 s of quiet, so the first operation afterwards reopens one. On the desktop this cost was lost in noise: after 40 s of quiet, create-note p50 was 16.8 ms before the fix, and 28.8 ms and 16.7 ms in two runs after it ([160825](results/20261023-160825-local.md), [162323](results/20261023-162323-local.md), [164542](results/20261023-164542-local.md)). `--wake-samples` measures it; it should be measured on the phone, where reopening costs more. If it matters, keeping one connection per pool (`min_connections(1)` in `holochain_data`) would remove it without polling.

### What is left

A syscall trace of the fixed phones over 3 idle minutes ([170040](results/20261023-170040-local.md)) accounts for what remains. strace inflates that run's CPU and wakeup figures; the trace ran under `--phone-dir` with a wrapper that starts the phone under strace.

**H2HC, about 5 ms/min on the desktop.** In order of frequency:
- **Lair keystore pings, every 5 s.** Holochain's keystore client checks its connection to the in-process Lair server. This is desktop-only so far: the phone runs use the in-memory keystore. A real Android app with Lair would pay it.
- **The benchmark's own once-a-minute `stats` and `threads` commands.**
- **The publish retry loop.** It queries the DHT database every 1–5 minutes while some op still lacks a full set of receipts, and pauses when none do.
- **SQLite pool maintenance**, every 30 s per database.
- **Late validation receipts from the linker.** These are written as they arrive.

**Kitsune2, about 42–47 ms/min.** Most of its tokio wakeups are short QUIC and iroh timers: epoll waits of 1–25 ms, keepalives and relay traffic. The rest is gossip timeouts and Kitsune2's peer-store pruning queries every 10 s. This is Kitsune2's networking, outside the conductor, and was left as it is.

**On the phone**, the desktop cost multiplied by about 7 (2.2–2.4 s/min against about 0.3 s/min before the fix). By that ratio, H2HC would idle at about 35 ms/min, under the 0.2 s/min target, and Kitsune2 at about 0.3 s/min, over it. These are estimates until the phone is measured again.

## On a phone: Samsung Galaxy S23+ (SM-S916B), Android 16

[20261023-133555-android](results/20261023-133555-android.md) ran all four configurations: 20 iterations for Cranelift and 10 for wasmi.

How it was run:
- `h2hc-phone` ran over `adb shell` with Holochain's in-memory keystore, because Android does not let the shell user bind Lair's Unix socket. Startup therefore leaves out Lair's passphrase hashing.
- The phone reached the network on this PC through the USB cable (`adb reverse`), not over a radio.
- The restart scenarios were skipped.

| | H2HC + Cranelift | H2HC + wasmi | Kitsune2 + Cranelift | Kitsune2 + wasmi |
|---|---|---|---|---|
| Network ready after enable | 2.06 s | 0.37 s | 1.65 s | 2.26 s |
| Local write, p50 | 144 ms | 117 ms | 144 ms | 105 ms |
| Read an authority's note, p50 | 34 ms | 29 ms | 22 ms | 21 ms |
| Remote call, either direction, p50 | 29–33 ms | 28 ms | 32–36 ms | 25–28 ms |
| Validation receipts, p50 | 0.80 s | 0.17 s | 9.8 s | 9.7 s |
| RSS ready, then after workload | 144, then 145 MB | 96, then 575 MB | 156, then 152 MB | 122, then 598 MB |
| Idle CPU | 2.2 s/min | 2.4 s/min | 2.4 s/min | 2.3 s/min |

What the phone run shows:

- **Idle CPU is about 2.2–2.4 s of CPU per minute** in every configuration, roughly 4% of a core all the time, with or without Kitsune2. This was the conductor's own background work, not networking: the scheduler polling described above, before the fix.
- **The wasmi leak reproduces on arm64:** 96 MB to 575 MB after 10 iterations. Cranelift holds steady at about 145 MB, far below the desktop's 430 MB.
- **Writes cost about 100–145 ms on the phone**, against 16 ms on the desktop. This is local commit and validation, the same with either network.
- **H2HC still gets receipts in under a second**, against about 10 s for Kitsune2.
- **Over this USB link, H2HC reads and remote calls are about the same as Kitsune2.**

## What the numbers say (desktop)

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

**Idle cost is the conductor, not the network layer.** Over a clean idle minute, before the scheduler fix:

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
