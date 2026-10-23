# h2hc-linker Architecture

> Holochain-to-Holochain Linker - Network edge gateway providing DHT access for lightweight clients
>
> Last updated: 2026-03-06 (reflects implementation as of M5.1)

---

## High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                            CLIENTS                                   │
│                                                                      │
│   ┌─────────────────┐    ┌─────────────────┐    ┌────────────────┐  │
│   │ Browser (holo-web-conductor) │    │  Mobile App     │    │   CLI Tool     │  │
│   │                 │    │                 │    │                │  │
│   │ Sync XHR for    │    │ HTTP/WS for     │    │ HTTP for       │  │
│   │ DHT queries     │    │ all operations  │    │ DHT queries    │  │
│   │ WS for signals  │    │                 │    │                │  │
│   └────────┬────────┘    └────────┬────────┘    └───────┬────────┘  │
│            │                      │                      │           │
└────────────┼──────────────────────┼──────────────────────┼───────────┘
             │                      │                      │
             └──────────────────────┼──────────────────────┘
                                    │
                     HTTP (sync) + WebSocket (async)
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────┐
│                          h2hc-linker                                 │
│                                                                      │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │ HTTP API Layer (axum)                                         │  │
│  │                                                                │  │
│  │ DHT Endpoints (/dht/*)                                        │  │
│  │  GET  /dht/{dna}/record/{hash}          → get record by hash  │  │
│  │  GET  /dht/{dna}/details/{hash}         → record + updates    │  │
│  │  GET  /dht/{dna}/links                  → get_links           │  │
│  │  GET  /dht/{dna}/count_links            → count_links         │  │
│  │  GET  /dht/{dna}/agent_activity/{agent} → agent activity      │  │
│  │  POST /dht/{dna}/must_get_agent_activity → filtered activity  │  │
│  │  POST /dht/{dna}/publish                → publish DhtOps      │  │
│  │                                                                │  │
│  │ Zome Call Endpoint (/api/*)      [requires conductor]         │  │
│  │  GET  /api/{dna}/{zome}/{fn}     → call zome function          │  │
│  │                                                                │  │
│  │ Kitsune Direct API (/k2/*)                                    │  │
│  │  GET  /k2/status                 → overall network status      │  │
│  │  GET  /k2/peers                  → list all known peers        │  │
│  │  GET  /k2/space/{id}/status      → space-specific status      │  │
│  │  GET  /k2/space/{id}/peers       → list peers in a space      │  │
│  │  GET  /k2/space/{id}/local-agents → list local agents         │  │
│  │  GET  /k2/transport/stats        → transport statistics        │  │
│  │                                                                │  │
│  │ WebSocket (/ws)                                               │  │
│  │  ← signal(dna, from_agent, payload)  deliver signal to client │  │
│  │  ← sign(agent, message)              request signature        │  │
│  │  → register(dna, agent)              register for signals     │  │
│  │  → sign_response(request_id, sig)    return signature         │  │
│  │  → send_remote_signal(dna, signals)  relay signal to peers    │  │
│  │                                                                │  │
│  │ Admin API (/admin/*)   [requires H2HC_LINKER_ADMIN_SECRET]    │  │
│  │  POST   /admin/agents            → add allowed agent           │  │
│  │  DELETE /admin/agents            → remove agent                │  │
│  │  GET    /admin/agents            → list allowed agents         │  │
│  │                                                                │  │
│  │ Test Endpoints                                                │  │
│  │  POST /test/signal               → inject test signal          │  │
│  │                                                                │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                    │                                 │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │ DhtQuery (Direct Wire Protocol)                               │  │
│  │                                                                │  │
│  │  Uses holochain_p2p wire types (WireMessage, WireOps)         │  │
│  │  Finds peers near hash location via peer_store                │  │
│  │  Sends GetReq/GetLinksReq via space.send_notify()             │  │
│  │  Receives GetRes/GetLinksRes via PendingDhtResponses           │  │
│  │  Parallel peer querying (3 peers, first non-empty wins)       │  │
│  │                                                                │  │
│  │  See "Why Not holochain_p2p as Semantic Layer?" below         │  │
│  │                                                                │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                    │                                 │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │ TempOpStore (Zero-Arc Publishing)                             │  │
│  │                                                                │  │
│  │  Implements kitsune2_api::OpStore                             │  │
│  │  Holds ops until authorities fetch (60s TTL)                  │  │
│  │  Enables publishing without persistent storage                │  │
│  │  Automatic cleanup every 10s                                  │  │
│  │                                                                │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                    │                                 │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │ AgentProxyManager                                             │  │
│  │                                                                │  │
│  │  Track browser agents per DNA                                 │  │
│  │  Route signals to correct WebSocket                           │  │
│  │  Manage sign request/response flow                            │  │
│  │  ProxyAgent: zero-arc, remote signing via WS                  │  │
│  │                                                                │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                    │                                 │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │ PreflightCache                                                │  │
│  │                                                                │  │
│  │  Caches AgentInfoSigned from all registered agents            │  │
│  │  Included in preflight messages to authorize with conductors  │  │
│  │  BootstrapWrapperFactory intercepts Bootstrap::put() calls    │  │
│  │                                                                │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                    │                                 │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │ Kitsune2 Core                                                 │  │
│  │                                                                │  │
│  │  KitsuneProxy handler (spaces, agent lifecycle, responses)    │  │
│  │  Spaces (one per DNA)                                         │  │
│  │  Peer discovery (bootstrap + gossip)                          │  │
│  │  Op fetch (request from authorities)                          │  │
│  │  Transport (iroh - QUIC)                                      │  │
│  │                                                                │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                      │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │ Optional: Conductor Connection  [feature: conductor-dht]      │  │
│  │                                                                │  │
│  │  AdminConn  → conductor admin websocket (list apps, cells)    │  │
│  │  AppConn    → conductor app websocket (zome calls)            │  │
│  │  Fallback DHT queries via conductor's dht_util zome           │  │
│  │  Required for /api/* zome call endpoint                       │  │
│  │  Required for /dht/*/details endpoint                         │  │
│  │                                                                │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                      │
└──────────────────────────────────┬───────────────────────────────────┘
                                   │
                                   │ Kitsune2 P2P Protocol (iroh/QUIC)
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                      HOLOCHAIN NETWORK                               │
│                                                                      │
│   ┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐ │
│   │ Conductor 1     │    │ Conductor 2     │    │ Conductor N     │ │
│   │                 │    │                 │    │                 │ │
│   │ DHT Storage     │    │ DHT Storage     │    │ DHT Storage     │ │
│   │ Full Arc        │    │ Full Arc        │    │ Full Arc        │ │
│   └─────────────────┘    └─────────────────┘    └─────────────────┘ │
│                                                                      │
│   (h2hc-linker clients are zero-arc: store nothing, fetch all)      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Why This Architecture?

### The Linker Concept

**h2hc-linker** is NOT "Holochain Lite" - it provides no:
- Source chain (no local chain storage)
- Validation (no validation workflows)
- Full node capabilities (zero-arc, no DHT storage)

Instead, it's a **network edge gateway** that links lightweight clients (browsers, mobile) to the Holochain DHT network without requiring them to run a full conductor.

### Key Design Decisions

1. **Zero-Arc Participation**: Clients don't store DHT data, they fetch everything from the network. This is intentional for lightweight deployments.

2. **Direct Wire Protocol**: DHT queries use kitsune2 wire messages (WireMessage::GetReq/GetLinksReq) sent directly to DHT authorities, without the holochain_p2p semantic layer. See [Why Not holochain_p2p?](#why-not-holochain_p2p-as-semantic-layer) below.

3. **TempOpStore for Publishing**: Since zero-arc nodes don't persist data, ops are held temporarily (60s) until DHT authorities fetch them.

4. **Remote Signing**: Keys stay with clients. Gateway requests signatures via WebSocket when Kitsune2 needs them.

5. **Dual API Design**:
   - `/dht/*` - DHT operations (get record, get links, publish)
   - `/k2/*` - Kitsune2 direct access (peers, status, network stats)
   - `/api/*` - Zome calls (requires conductor connection)

6. **Preflight Agent Info**: Gateway includes registered browser agents in preflight messages so conductors authorize the gateway's connections.

7. **Feature-Flagged Conductor Fallback**: The `conductor-dht` feature flag switches DHT queries from direct wire protocol to conductor-mediated zome calls, enabling incremental migration.

---

## Why Not holochain_p2p as Semantic Layer?

The original architecture analysis recommended using `holochain_p2p`'s `HolochainP2pDna` trait as the semantic layer for DHT queries. During implementation, the gateway instead implemented direct wire protocol queries via a custom `DhtQuery` module. Here's why:

### What was planned

Use `HolochainP2pDna::get()`, `::get_links()`, `::publish()` from the `holochain_p2p` crate. These methods handle peer selection, wire message encoding, retries, and timeouts internally.

### What was built

A `DhtQuery` module (`src/dht_query.rs`) that:
- Calls `kitsune2_core::get_responsive_remote_agents_near_location()` directly for peer selection
- Constructs `WireMessage::GetReq` / `GetLinksReq` manually (using wire types from `holochain_p2p`)
- Sends them via `space.send_notify()` on the kitsune2 space
- Routes responses through a shared `PendingDhtResponses` map back to the waiting HTTP handler

This is functionally equivalent to what `HolochainP2pDna` does internally, but without the full crate integration.

### Why the divergence

**`holochain_p2p` as a crate is designed to be embedded in a full conductor.** Using it as a library requires providing:

1. **Storage callbacks** (`GetDbPeerMeta`, `GetDbOpStore`, `GetDbConductor`) backed by SQLite databases with holochain-specific schemas
2. **An `HcP2pHandler` implementation** for incoming queries (even if zero-arc returns empty)
3. **A `MetaLairClient`** for signing (even though gateway delegates to browser)
4. **`holochain_sqlite`** and **`holochain_state`** dependencies, pulling in the conductor's database layer

For a zero-arc gateway that only needs to *send* queries and *receive* responses, this is a large amount of infrastructure to satisfy a trait interface that would mostly return empty results.

The wire protocol types (`WireMessage`, `WireOps`, `WireLinkOps`) and the peer selection function (`get_responsive_remote_agents_near_location`) were already available as lighter-weight imports. Using them directly avoids the conductor database stack while achieving the same network behavior.

### What was preserved from holochain_p2p

- **Wire message types**: `WireMessage::get_req()`, `WireMessage::get_links_req()`, `WireOps`, `WireLinkOps` are all imported from `holochain_p2p`
- **Peer selection**: `kitsune2_core::get_responsive_remote_agents_near_location()` is the same function holochain_p2p uses internally
- **Parallel querying**: 3 peers queried in parallel, first non-empty response wins (same strategy as conductor)

### Trade-offs

| Factor | holochain_p2p trait | Direct wire protocol (current) |
|--------|--------------------|---------------------------------|
| Dependency weight | Heavy (sqlite, state, keystore) | Light (wire types + core only) |
| Retry/timeout logic | Built-in | Hand-rolled (simpler) |
| Future Holochain changes | Automatic | Must track wire protocol changes |
| Incoming query handling | Required (even if noop) | Not needed |
| Code maintenance | Less code, more dependency surface | More code, less dependency surface |

### Future consideration

If `holochain_p2p` is ever refactored to separate wire protocol operations from conductor storage requirements (e.g. a `holochain_p2p_core` crate), it would make sense to revisit this decision.

---

## Data Flow Diagrams

### Get Record Flow

```
Client                      h2hc-linker                    DHT Authorities
  │                              │                              │
  │ GET /dht/{dna}/record/{hash} │                              │
  │─────────────────────────────►│                              │
  │                              │                              │
  │                              │ DhtQuery.get(hash)           │
  │                              │  peer_store → find 3 peers   │
  │                              │  near hash location          │
  │                              │                              │
  │                              │ WireMessage::GetReq           │
  │                              │ via space.send_notify()       │
  │                              │─────────────────────────────►│
  │                              │                              │
  │                              │ WireMessage::GetRes           │
  │                              │ via PendingDhtResponses       │
  │                              │◄─────────────────────────────│
  │                              │                              │
  │  JSON { record, ... }       │                              │
  │◄─────────────────────────────│                              │
  │                              │                              │
```

### Publish Flow

```
Client                      h2hc-linker                    DHT Authorities
  │                              │                              │
  │ POST /dht/{dna}/publish      │                              │
  │ { ops: [{op_data, sig}] }   │                              │
  │─────────────────────────────►│                              │
  │                              │                              │
  │                              │ Decode + store in TempOpStore│
  │                              │                              │
  │                              │ GatewayKitsune.publish_ops() │
  │                              │  find peers near basis loc   │
  │                              │  space.publish().publish_ops()│
  │                              │─────────────────────────────►│
  │                              │                              │
  │                              │ Peers fetch from TempOpStore │
  │                              │◄─────────────────────────────│
  │                              │                              │
  │  { success, queued, ... }   │                              │
  │◄─────────────────────────────│                              │
  │                              │                              │
```

### Signal Flow

```
Holochain Conductor         h2hc-linker                     Client
  │                              │                              │
  │ send_remote_signal()         │                              │
  │ via Kitsune2                 │                              │
  │─────────────────────────────►│                              │
  │                              │                              │
  │                              │ ProxySpaceHandler receives   │
  │                              │ RemoteSignalEvt              │
  │                              │                              │
  │                              │ AgentProxyManager routes     │
  │                              │ to correct WebSocket         │
  │                              │                              │
  │                              │ WS: { type: "signal", ... }  │
  │                              │─────────────────────────────►│
  │                              │                              │
```

### Remote Signing Flow

```
Kitsune2 (needs sig)        h2hc-linker                     Client
  │                              │                              │
  │ ProxyAgent.sign(message)     │                              │
  │─────────────────────────────►│                              │
  │                              │                              │
  │                              │ AgentProxyManager            │
  │                              │ request_signature()          │
  │                              │                              │
  │                              │ WS: sign_request             │
  │                              │─────────────────────────────►│
  │                              │                              │
  │                              │                              │ Sign with
  │                              │                              │ local key
  │                              │                              │
  │                              │◄─────────────────────────────│
  │                              │ WS: sign_response            │
  │                              │                              │
  │◄─────────────────────────────│ Return signature             │
  │                              │                              │
```

---

## Component Details

### DhtQuery

Direct DHT query engine (`src/dht_query.rs`):

```rust
pub struct DhtQuery {
    gateway_kitsune: Arc<GatewayKitsune>,
    pending_responses: PendingDhtResponses,  // shared with ProxySpaceHandler
}
```

- `get(dna_hash, hash)` → sends `WireMessage::GetReq`, returns `WireOps`
- `get_links(dna_hash, base, zome_index, type, tag)` → sends `WireMessage::GetLinksReq`, returns `WireLinkOps`
- Queries up to 3 peers in parallel, returns first non-empty response
- 30-second default timeout per query

Response routing: `ProxySpaceHandler.recv_notify()` receives incoming wire messages and routes `GetRes`/`GetLinksRes`/`ErrorRes` to the `PendingDhtResponses` map, which unblocks the waiting HTTP handler.

### TempOpStore

Temporary storage for ops during the publish flow:

```rust
pub struct TempOpStore {
    ops: Arc<RwLock<HashMap<OpId, StoredOpEntry>>>,
}

struct StoredOpEntry {
    op: StoredOp,
    expires_at: Instant,  // 60s TTL
}
```

- Implements `kitsune2_api::OpStore` trait
- Cleanup task runs every 10s
- Authorities fetch ops after receiving publish notification

### AgentProxyManager

Manages browser agent connections:

```rust
pub struct AgentProxyManager {
    // (dna_hash, agent_pubkey) → WebSocket sender
    connections: HashMap<(DnaHash, AgentPubKey), WsSender>,
    // request_id → oneshot channel for signature response
    pending_signatures: HashMap<String, oneshot::Sender<Signature>>,
}
```

### ProxyAgent

Represents a browser agent in Kitsune2:

```rust
pub struct ProxyAgent {
    agent_pubkey: AgentPubKey,
    dna_hash: DnaHash,
    agent_proxy_manager: Arc<AgentProxyManager>,
}

impl Signer for ProxyAgent {
    fn sign(&self, message: &[u8]) -> BoxFut<Result<Signature>> {
        // Request signature via WebSocket
        self.agent_proxy_manager.request_signature(
            self.agent_pubkey.clone(),
            message.to_vec()
        )
    }
}
```

### PreflightCache

Ensures conductors authorize connections from the gateway (`src/wire_preflight.rs`):

```rust
pub struct PreflightCache {
    agent_infos: Arc<RwLock<Vec<AgentInfoSigned>>>,
}
```

- `BootstrapWrapperFactory` wraps the real `BootstrapFactory` and intercepts `Bootstrap::put()` calls to capture agent infos
- Multiple spaces share the same `PreflightCache`
- Preflight messages include protocol version and all registered agent infos
- Modeled after `holochain_p2p::spawn::actor::BootWrap`

### KitsuneProxy

The main kitsune2 handler (`src/gateway_kitsune.rs`):

- Implements `KitsuneHandler` trait
- Creates a `ProxySpaceHandler` per space (DNA)
- `ProxySpaceHandler` handles:
  - `recv_notify()`: routes wire messages (signals → browser, DHT responses → PendingDhtResponses)
  - Agent lifecycle (join/leave space)
- `GatewayKitsune` manages spaces, agents, publishing, and the DhtQuery instance

---

## Build Modes

### Default (direct DHT queries)

```bash
nix develop --command cargo build --release
```

- `/dht/*/record` and `/dht/*/links` → direct kitsune2 wire protocol via DhtQuery
- `/dht/*/details` → requires conductor (returns 503 without one)
- `/api/*` → requires conductor
- `/k2/*` → available
- `/ws` → available
- `/dht/*/publish` → available (via TempOpStore + kitsune2 publish)

### conductor-dht mode (fallback)

```bash
nix develop --command cargo build --release --features conductor-dht
```

- `/dht/*/record` and `/dht/*/links` → routed through conductor's `dht_util` zome
- All other endpoints same as default

---

## Endpoints Not Yet Implemented

The following endpoints from the original architecture analysis are not yet implemented:

| Endpoint | Status | Notes |
|----------|--------|-------|
| `GET /hc/{dna}/entry/{hash}` | Not implemented | Use `/dht/{dna}/record/{hash}` with entry hash |
| `POST /hc/{dna}/signal` (HTTP) | Not implemented | Signals use WebSocket instead |
| `POST /hc/{dna}/call-remote` | Not implemented | Not needed by holo-web-conductor yet |

Previously unimplemented endpoints that have since been added:
- `count_links` → `GET /dht/{dna}/count_links`
- `agent-activity` → `GET /dht/{dna}/agent_activity/{agent}` and `POST /dht/{dna}/must_get_agent_activity`

---

## Related Documentation

- [holo-web-conductor](https://github.com/Holo-Host/holo-web-conductor) - Full browser extension architecture (client side)
