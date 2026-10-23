# h2hc-linker

> Holochain-to-Holochain Linker - Network edge gateway providing DHT access for lightweight clients

## Quick Context (READ FIRST)

**Current Step**: See [STEPS/index.md](./STEPS/index.md) for status registry

**What This Is**: A network edge gateway that allows lightweight clients (browsers, mobile) to interact with the Holochain DHT without running a full conductor. It provides selective access between clients and the DHT network.

**What This Is NOT**: This is not "Holochain Lite" - there is no source chain, no validation, no full node capabilities. Zero-arc only.

---

## Critical Rules

### Strong Typing (MANDATORY)

- **Never use plain `String`** for typed values (hashes, URLs, identifiers) - use proper newtypes
- **Reuse types from holochain crates** - check these before defining new structs:
  - `holo_hash`: `AgentPubKey`, `ActionHash`, `EntryHash`, `DnaHash`
  - `holochain_types`: `DhtOp`, `Action`, `Entry`, `Record`, `SignedAction`
  - `kitsune2_api`: `AgentInfoSigned`, `SpaceId`, `StoredOp`, `OpId`
  - `holochain_conductor_api`: API types, app info types
  - `holochain_p2p`: `WireMessage`, `WireOps`, `WireLinkOps` (wire types only, not the HolochainP2pDna trait)
- **Do not recreate structs** that already exist in dependencies - find and use them
- Use `#[serde(transparent)]` newtypes for domain-specific wrappers

### Reference Sources (Priority Order)

1. **Kitsune2 patterns**: `../holochain/crates/holochain_p2p/src/` - Holochain's kitsune2 integration
2. **Holochain types**: `../holochain/crates/holochain_types/src/`
3. **Hash types**: `../holochain/crates/holo_hash/src/`

**Avoid web searches** for implementation details - local repos have authoritative code.

---

## Architecture Overview

See [ARCHITECTURE.md](./ARCHITECTURE.md) for the full diagram.

```
┌─────────────────────────────────────────────────────────────────────┐
│                         h2hc-linker                                  │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ DHT Endpoints (/dht/*)                                       │   │
│  │  GET  /dht/{dna}/record/{hash}          → get record          │   │
│  │  GET  /dht/{dna}/details/{hash}         → record + updates    │   │
│  │  GET  /dht/{dna}/links                  → get_links           │   │
│  │  GET  /dht/{dna}/count_links            → count_links         │   │
│  │  GET  /dht/{dna}/agent_activity/{agent} → agent activity      │   │
│  │  POST /dht/{dna}/must_get_agent_activity                      │   │
│  │  POST /dht/{dna}/publish                → publish DhtOps      │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ Kitsune Direct API (/k2/*)                                   │   │
│  │  GET  /k2/status                 → overall network status     │   │
│  │  GET  /k2/peers                  → list all known peers       │   │
│  │  GET  /k2/space/{id}/status      → space-specific status     │   │
│  │  GET  /k2/space/{id}/peers       → peers in a space          │   │
│  │  GET  /k2/space/{id}/local-agents → local agents             │   │
│  │  GET  /k2/transport/stats        → transport statistics       │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ DhtQuery (direct wire protocol) + kitsune2 (network layer)   │   │
│  └──────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────┬──────────────────────────────────┘
                                   │ Kitsune2 P2P (iroh/QUIC)
                                   ▼
                  ┌────────────────────────────────┐
                  │  DHT Authorities / Full Nodes  │
                  └────────────────────────────────┘
```

---

## Development Guidelines

### Build Environment (MANDATORY)

**Always use `nix develop` for all cargo and npm run commands.** The project has native dependencies (libdatachannel, etc.) that require the nix environment.

```bash
# Correct - always prefix with nix develop
nix develop --command cargo build
nix develop --command cargo test
nix develop --command cargo run

# Or enter the shell first
nix develop
cargo build && cargo test
```

### Before Coding

1. Check if the type/struct already exists in holochain crates
2. Reference `../holochain/crates/holochain_p2p/` for kitsune2 patterns
3. Write tests before implementation

### Serialization

- Use `holochain_serialized_bytes` for msgpack serialization
- Hash types are 39 bytes (32 core + 3 type prefix + 4 location)

### Commit Hygiene

- No claude co-authored messages
- Run `cargo fmt` and `cargo clippy` before commits

### Communication Style

- No emotional tags or punctuation
- Code-related information only

---

## Testing and Integration

### Unit Tests

```bash
nix develop --command cargo build --release
nix develop --command cargo test
```

### Manual Smoke Test

```bash
nix develop --command cargo run -- --port 8090 &
curl http://localhost:8090/health
curl http://localhost:8090/k2/status
```

### E2E Testing

Full integration testing uses the [holo-web-conductor](https://github.com/Holo-Host/holo-web-conductor) browser extension with the ziptest hApp. See that repo's e2e test setup scripts for details.

Verify:
- Gateway starts and responds to `/health`
- Conductor connects successfully
- Basic operations work (create entry, get entry, get links)
- Remote signals work between browser and conductor

---

## Key Dependencies

```toml
holo_hash = "0.6.1-rc.0"              # Hash types
holochain_types = "0.6.1-rc.0"        # Core Holochain types
holochain_p2p = "0.6.1-rc.0"          # P2P/Kitsune semantic layer
kitsune2_api = "0.4.0-dev.2"          # Kitsune2 API types
kitsune2_core = "0.4.0-dev.2"         # Kitsune2 implementations
kitsune2_transport_iroh = "0.4.0-dev.2"  # Iroh transport (replaces tx5/webrtc)
```

---

## Type Lookup Reference

| Need | Crate | Type |
|------|-------|------|
| Agent key | `holo_hash` | `AgentPubKey` |
| Entry hash | `holo_hash` | `EntryHash` |
| Action hash | `holo_hash` | `ActionHash` |
| DHT operation | `holochain_types` | `DhtOp` |
| Signed action | `holochain_types` | `SignedAction` |
| Agent info | `kitsune2_api` | `AgentInfoSigned` |
| Space ID | `kitsune2_api` | `SpaceId` |
| Op storage | `kitsune2_api` | `StoredOp` |
| Wire messages | `holochain_p2p` | `WireMessage`, `WireOps` |

---

## Kitsune2 Reference Files

For implementing kitsune2 integration, study these in `../holochain/crates/holochain_p2p/src/`:

- `spawn/actor.rs` - Kitsune2 lifecycle management
- `op_store.rs` - Op storage interface
- `local_agent.rs` - Agent management
- `types/wire.rs` - Wire protocol types

---

## Documentation Structure

| File | Purpose |
|------|---------|
| `CLAUDE.md` | This file - core rules and quick context |
| `ARCHITECTURE.md` | System architecture diagram |

---
