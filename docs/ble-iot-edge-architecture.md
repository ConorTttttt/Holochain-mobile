# Holochain BLE Edge — IoT architecture (proposed)

Status: transport-boundary implementation committed on this branch (2026-10-30); physical BLE radio integration and hardware validation still pending.

### Implementation progress

- **Implemented:** `h2hc_protocol::transport::FrameChannel` shared by phone and linker, plus `channel_over_io` for bounded length-prefixed MessagePack over any reliable async byte stream. This is the L2CAP-facing framing layer, **not a BLE device connection**.
- **Implemented:** `ConductorLinks::serve_channel` handles the existing Hello/Welcome, requests, calls, receipts and disconnect logic independently from WebSocket.
- **Implemented:** WebSocket endpoint preserved as an adapter to `FrameChannel`.
- **Implemented:** `h2hc_p2p::stream::connector_from_stream` adapts a future authenticated BLE dialer to the existing Link supervisor and reconnect loop.
- **Tests added (not run here):** duplex in-memory stream frames, split length headers with simultaneous writes, rejection of oversized/truncated frames, complete H2HC client handshake/request/response over the framed stream.
- **Pending:** Linux BLE acceptor, Android BLE dialer/JNI bootstrap, hardware authentication/pairing, bounded BLE transfer benchmarks, GATT fragmentation fallback, and real Holochain zome calls over BLE.
- **No compilation or real-radio tests have been performed** in this environment. Run the repository's vendoring/preparation steps and `cargo test -p h2hc-protocol -p h2hc-p2p`, plus the separate linker tests, before treating this as validated.

#### Integration seam for the next patch

The Android BLE central will obtain an authenticated, ordered, reliable BLE L2CAP stream and hand it to `h2hc_p2p::stream::connector_from_stream` (which invokes `channel_over_io`). The Linux gateway BLE listener will obtain a matching accepted stream and invoke `ConductorLinks::serve_channel(channel_over_io(stream, queue, limit))`. Provisioning, key pinning, agent authorization and BLE device-advertisement handling are separate concerns; **do not** expose the new session entry point to unauthenticated peers.

## Decision

Extend the existing H2HC zero-arc conductor with an optional BLE link to a nearby gateway. A BLE-enabled H2HC linker runs on a gateway that does the Kitsune2 networking and DHT queries. Build the BLE **client-to-linker** transport independently of the existing BLE **Kitsune-to-Kitsune** transport.

This preserves the mobile power/CPU improvements, and provides a separate evolution path for microcontrollers that cannot run a Holochain conductor.

## Two classes of edge client

| Client | Device example | On device | On gateway |
|---|---|---|---|
| Conductor edge | Android phone, capable ARM Linux SBC | Holochain 0.7.1, wasmi, real agent key, source chain, zome execution, H2HC adapter, BLE | H2HC linker, durable acceptance store, network routing, DHT authority queries |
| Sensor edge | ESP32-class / low-memory embedded board (future) | Small BLE client, private device key, signed readings/commands, queue | Verifies device messages, executes app-specific logic, authors Holochain records under gateway's agent unless a separate true-agent architecture is built |

Do not call the sensor-edge client a Holochain agent merely because its messages enter the DHT. If the gateway authors an entry, it must say so explicitly and preserve the sensor signature as provenance. A true device-authored Holochain agent needs source-chain and Holochain-compatible signing rules, which are out of scope for sensor-edge v1.

The current H2HC + wasmi Android benchmark reports ~60 MB steady state, but also ~256 MB transient startup allocation. It does not fit typical microcontroller RAM budgets. BLE energy/battery metrics are **not measured**.

## Architecture

Conductor-edge phone:
- Existing prototypes/h2hc-conductor and prototypes/h2hc-p2p are retained.
- h2hc-p2p/src/link.rs already accepts a Connector that yields a FrameChannel.
- Add a BLE connector alongside h2hc-p2p/src/ws.rs, using the existing h2hc-protocol Frame and MessagePack codec.
- Keep H2HC Hello/Welcome, request/response correlation, durable receipt inbox and rejoin/reconnect logic.

Gateway:
- Refactor prototypes/h2hc-linker/src/conductor_link/mod.rs: separate the Frame-channel session loop from its Axum WebSocket binding. Leave /conductor/v1 working unchanged.
- Add a BLE server/session adapter that drives the *same* ConductorLinks session implementation.
- Keep Kitsune2 on the gateway. Use normal IP network connectivity initially; optional kitsune2_transport_iroh BLE-only support belongs to a separate later milestone.

Use blew for a shared Android/Linux BLE abstraction: prefer L2CAP credit-based channels when supported, fall back to GATT. The existing iroh-ble-transport + Kitsune2 BLE fork is useful for gateway-to-node BLE networking but not required on constrained clients. Do not force MCU clients to implement QUIC/Iroh.

Illustrative flow:

    Holochain edge conductor -- H2HC FrameChannel -- BLE -- H2HC linker -- Kitsune2 -- DHT peers
    Sensor edge firmware   -- compact signed IoT frames -- BLE -- sensor bridge -- Holochain zome

## BLE wire transport

For conductor-edge v1, the transport carries the unmodified serialized H2HC Frame. A transport shim, NOT h2hc-protocol itself, owns message boundaries.

- Versioned session handshake plus gateway/device identity pinning.
- L2CAP: u32 length prefix, followed by complete MessagePack H2HC frame; reject lengths over a configurable cap before allocation.
- GATT fallback: a bounded fragment header containing session ID, frame ID, fragment index/count or byte offset, total frame length, and payload. Reassemble with per-peer limits and expiration, and explicit flow control. Do not assume GATT writes are stream-oriented.
- Maximum H2HC frame in current code: 16 MiB (prototypes/h2hc-protocol/src/codec.rs). This is NOT a reasonable default IoT BLE transfer. Set a tighter BLE-profile budget during testing, then support large records by bounded streaming/chunking rather than unbounded buffering.
- Backpressure: bounded send/receive queues, one writer per BLE connection, cancellation on disconnect. No silent message dropping for requests or durable receipts.
- BLE link dropout must close the FrameChannel so existing Link supervisory reconnect logic runs.
- Prefer short connection windows and buffered telemetry over permanent scanning on battery devices.

Sensor-edge v1 should NOT reuse the full Holochain/MessagePack Frame schema (it depends on Holochain wire types). Give it a small separate versioned envelope: device ID, monotonic sequence, timestamp/expiry (where available), message type, payload, signature and an acknowledgement/last-committed cursor. Limit commands by registered capabilities and use persistent anti-replay state.

## Identity, trust and durability

- Provision/pin the gateway endpoint public key by QR, NFC or deliberate physical pairing. A BLE advertisement alone is not trust.
- Give each conductor-edge device its own persistent Holochain key and source chain; never ship the DangerTestKeystore from adb-shell benchmarking in production.
- Authenticate the *client* at the gateway as well as the gateway at the client. The current ConductorLinks Hello processing checks protocol version but does not enforce Hello.session_token. Add explicit client authentication/authorization bound to the Holochain agent and DNA before shipping.
- Preserve H2HC durable acceptance receipts and source-labelled DHT answers. Pin expected linker key; acceptance by linker is not final network integration.
- Linker may retain offline ops, but after long sleep re-signing/agent-info expiry and re-publish behaviour need hardware validation.
- Implement idempotency keys for retried mutations. A dropped BLE response must not turn a successful remote command into a duplicate.
- For sensor-edge data, a gateway's Holochain signature identifies the gateway author; sensor signatures are additional verified provenance, not interchangeable authorship.
- Review the AGPL-3.0 obligations of blew / iroh-ble-transport in distributed firmware and apps.

## Initial hardware scenario

Hardware: Android Galaxy S23+ (available mobile benchmark target) + Ubuntu Linux gateway with BLE adapter; optionally a second normal Holochain authority on LAN. All first-hop phone-to-gateway transfers must use BLE; disable phone Wi-Fi and mobile data during radio validation. The gateway may use IP to reach the Holochain network.

1. Provision a pinned gateway identity out-of-band, establish authenticated BLE session.
2. Exchange H2HC Hello/Welcome and Join over BLE.
3. Create 100 small notes, verify signed action, linker acceptance receipts and eventual remote DHT retrieval.
4. Read/query existing remote notes, call a remote zome, and receive a remote signal.
5. Force BLE disconnect/reconnect and gateway restart. Ensure no lost acknowledgements or duplicated commands.
6. Measure idle CPU, peak/steady RSS, radio-on time, bytes per operation, BLE reconnection latency, app restart behaviour and battery consumption, comparing with the existing H2HC WebSocket baseline.
7. Re-run with L2CAP disabled to exercise GATT fallback and with phones/background restrictions applied.
8. After proof: introduce a compact sensor firmware client with signed telemetry, idempotency and wake/send/sleep duty cycling.

No pass is claimed until there is evidence of BLE packets, no client IP route, and matching Holochain data/receipt traces.

## Milestones and implementation order

1. Refactor linker to accept an abstract FrameChannel/FrameStream, preserving the existing WebSocket tests.
2. Add common length-delimited framing and bidirectional cancellation/backpressure tests over a simulated duplex stream.
3. Implement BLE central/peripheral session adapters using blew, Android JNI/Kotlin bootstrap and Linux BlueZ backend.
4. Plumb an explicit transport selection into h2hc-phone (ws | ble), retain local reconnect and pinned-linker checks.
5. Run Android ↔ Ubuntu hardware tests above; file results in benchmarks/.
6. Only after those tests, define the compact embedded sensor protocol and build an MCU reference client.
7. Optionally integrate the existing kitsune2 BLE fork for BLE-only gateway-to-gateway peering.

## Known non-goals for the first prototype

- No claim that an ESP32 runs a Holochain conductor.
- No claim of Holochain network availability beyond BLE range without a gateway.
- No automatic peer admission from untrusted BLE scans.
- No transparent substitution of a sensor-device signature for a Holochain agent signature.
- No promise of a fixed battery life until measured on hardware.
- No claim that the BLE demo's pending Windows–Ubuntu two-conductor test has passed.

## Source locations

- H2HC connector: prototypes/h2hc-p2p/src/link.rs, ws.rs, session.rs.
- H2HC linker: prototypes/h2hc-linker/src/conductor_link/mod.rs.
- H2HC protocol: prototypes/h2hc-protocol/src/lib.rs and codec.rs.
- Mobile benchmarks: benchmarks/README.md.
- BLE experimental transport: ConorTttttt/holochain-ble-demo and ConorTttttt/iroh-ble-transport.
- Platform BLE abstraction: ConorTttttt/blew, docs/android-without-tauri.md.
