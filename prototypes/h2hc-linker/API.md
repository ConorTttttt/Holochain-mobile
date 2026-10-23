# h2hc-linker API Specification

> Version: 0.1.0 | Last updated: 2026-03-06

All hashes are HoloHash base64 strings (e.g. `uhCAk...`, `uhCEk...`, `uhCkk...`).
All responses are JSON. Errors return `{ "error": "<message>", "code": <http_status> }`.

---

## Authentication

Authentication is **optional**. When `H2HC_LINKER_ADMIN_SECRET` is set, all DHT/K2 endpoints require a session token via `Authorization: Bearer <token>`. When unset, all endpoints are open.

### Capabilities

| Capability | Protects |
|------------|----------|
| `dht_read` | `GET /dht/*` endpoints |
| `dht_write` | `POST /dht/*/publish` |
| `k2` | `GET /k2/*` endpoints |

### Per-Cell (DNA) Scoping

Sessions are scoped to registered cells (agent+DNA pairs). When a client sends a `Register` message on the WebSocket for a given DNA, that DNA is added to the session's authorized set. HTTP requests to `/dht/{dna_hash}/*` endpoints are checked against this set — requests for unregistered DNAs return `403 Forbidden`.

### Session Lifecycle

Sessions have **no TTL**. They live until:
- The WebSocket connection disconnects (all sessions for that agent are revoked)
- The agent is removed via `DELETE /admin/agents` (all sessions and connections are closed)
- A session is explicitly revoked

### Session Flow

1. Admin registers an agent via `POST /admin/agents` (requires `Authorization: Bearer <admin_secret>`)
2. Agent connects via WebSocket, completes challenge-response auth (ed25519 signature)
3. Agent receives a session token (no expiry)
4. Agent sends `Register` messages for each DNA it needs to access
5. Agent uses `Authorization: Bearer <session_token>` for HTTP requests (scoped to registered DNAs)

### Error Codes

| Code | Meaning |
|------|---------|
| `401 Unauthorized` | Missing, invalid, or revoked session token |
| `403 Forbidden` | Valid token but insufficient capability or unregistered DNA |

---

## Health

### `GET /health`

No auth required.

**Response** `200`:
```json
{
  "status": "ok",
  "version": "0.1.0"
}
```

---

## DHT Endpoints

All DHT endpoints query the network directly via kitsune2 wire protocol.

### `GET /dht/{dna_hash}/record/{hash}`

Get a record by action hash or entry hash.

| Parameter | In | Type | Description |
|-----------|-----|------|-------------|
| `dna_hash` | path | string | DNA hash (base64) |
| `hash` | path | string | Action hash or entry hash (base64) |

**Auth**: `dht_read` (when auth enabled)

**Response** `200`: A flat record object, or `null` if not found.

```json
{
  "signature": "<base64>",
  "action": {
    "type": "Create",
    "author": "uhCAk...",
    "timestamp": [1234567890, 0],
    "action_seq": 5,
    "prev_action": "uhCkk...",
    "entry_type": { ... },
    "entry_hash": "uhCEk...",
    "weight": { ... }
  },
  "entry": {
    "entry_type": "App",
    "entry": "<base64 msgpack>"
  }
}
```

---

### `GET /dht/{dna_hash}/details/{hash}`

Get details for a hash, including updates and deletes (matches Holochain's `get_details` return format).

| Parameter | In | Type | Description |
|-----------|-----|------|-------------|
| `dna_hash` | path | string | DNA hash (base64) |
| `hash` | path | string | Action hash or entry hash (base64) |

**Auth**: `dht_read`

**Response** `200`: Returns either `Details::Record` or `Details::Entry` depending on the hash type, or `null` if not found.

For an **action hash** (record details):
```json
{
  "type": "Record",
  "content": {
    "record": { ... },
    "validation_status": "Valid",
    "updates": [ ... ],
    "deletes": [ ... ]
  }
}
```

For an **entry hash** (entry details):
```json
{
  "type": "Entry",
  "content": {
    "entry": "<base64 msgpack>",
    "entry_type": "App",
    "actions": [ ... ],
    "updates": [ ... ],
    "deletes": [ ... ],
    "entry_dht_status": "Live"
  }
}
```

---

### `GET /dht/{dna_hash}/links`

Get links from a base hash.

| Parameter | In | Type | Required | Description |
|-----------|-----|------|----------|-------------|
| `dna_hash` | path | string | yes | DNA hash (base64) |
| `base` | query | string | yes | Base hash — agent pubkey, entry hash, action hash, or external hash (base64) |
| `zome_index` | query | u8 | no | Zome index to filter by. Required when `type` is provided. |
| `type` | query | u16 | no | Link type index to filter by. Requires `zome_index`. |
| `tag` | query | string | no | Link tag prefix filter (base64 encoded) |

**Auth**: `dht_read`

**Response** `200`: Array of link objects.
```json
[
  {
    "author": "uhCAk...",
    "target": "uhCEk...",
    "timestamp": [1234567890, 0],
    "zome_index": 0,
    "link_type": 1,
    "tag": "<base64>",
    "create_link_hash": "uhCkk..."
  }
]
```

Returns `[]` if no links found.

**Errors**:
- `400` if `type` provided without `zome_index`

---

### `GET /dht/{dna_hash}/count_links`

Count links from a base hash (same query parameters as `GET /dht/{dna_hash}/links`).

| Parameter | In | Type | Required | Description |
|-----------|-----|------|----------|-------------|
| `dna_hash` | path | string | yes | DNA hash (base64) |
| `base` | query | string | yes | Base hash (base64) |
| `zome_index` | query | u8 | no | Zome index filter |
| `type` | query | u16 | no | Link type filter (requires `zome_index`) |
| `tag` | query | string | no | Tag prefix filter (base64 encoded) |

**Auth**: `dht_read`

**Response** `200`:
```json
{
  "count": 42
}
```

---

### `GET /dht/{dna_hash}/agent_activity/{agent_hash}`

Get agent activity (chain status and optionally full action list).

| Parameter | In | Type | Required | Description |
|-----------|-----|------|----------|-------------|
| `dna_hash` | path | string | yes | DNA hash (base64) |
| `agent_hash` | path | string | yes | Agent pubkey (base64) |
| `request` | query | string | no | `"status"` for status only, `"full"` (default) for full activity |

**Auth**: `dht_read`

**Response** `200`: Agent activity response (Holochain `AgentActivityResponse` format), or `null` if not found.

---

### `POST /dht/{dna_hash}/must_get_agent_activity`

Get agent activity filtered by chain position.

| Parameter | In | Type | Description |
|-----------|-----|------|-------------|
| `dna_hash` | path | string | DNA hash (base64) |

**Request body**:
```json
{
  "agent": "uhCAk...",
  "chain_top": "uhCkk...",
  "include_cached_entries": false
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `agent` | string | yes | Agent pubkey (base64) |
| `chain_top` | string | yes | Action hash to start from (base64) |
| `include_cached_entries` | bool | no | Include cached entries (default: `false`) |

**Auth**: `dht_read`

**Response** `200`: Filtered agent activity, or `null` if not found.

---

### `POST /dht/{dna_hash}/publish`

Publish signed DhtOps to the DHT network.

| Parameter | In | Type | Description |
|-----------|-----|------|-------------|
| `dna_hash` | path | string | DNA hash (base64) |

**Auth**: `dht_write`

**Request body**:
```json
{
  "ops": [
    {
      "op_data": "<base64 msgpack-encoded DhtOp>",
      "signature": "<base64 64-byte Ed25519 signature>"
    }
  ]
}
```

**Response** `200`:
```json
{
  "success": true,
  "queued": 3,
  "failed": 0,
  "published": 3,
  "results": [
    { "success": true },
    { "success": true },
    { "success": false, "error": "reason" }
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `success` | bool | `true` if no storage failures AND at least some ops reached peers |
| `queued` | number | Ops successfully stored in TempOpStore |
| `failed` | number | Ops that failed to store |
| `published` | number | Ops published to at least one DHT peer |
| `results` | array | Per-op result in request order |

---

## Zome Call Endpoint

Unlike `/dht/*` endpoints which query the network directly via kitsune2 wire protocol, zome calls are proxied through a Holochain conductor (configured via `H2HC_LINKER_CONDUCTOR_URL`). Without a conductor running, this endpoint returns `503`.

### `GET /api/{dna_hash}/{zome_name}/{fn_name}`

Call a zome function via the conductor.

| Parameter | In | Type | Required | Description |
|-----------|-----|------|----------|-------------|
| `dna_hash` | path | string | yes | DNA hash (base64) |
| `zome_name` | path | string | yes | Zome name |
| `fn_name` | path | string | yes | Function name |
| `payload` | query | string | no | Base64 URL-safe encoded JSON payload |

**Auth**: None (not protected even when auth enabled)

**Response** `200`: JSON-encoded zome function return value.

**Errors**:
- `503` if conductor not connected

---

## Kitsune2 Network API (`/k2`)

### `GET /k2/status`

Overall network status.

**Auth**: `k2`

**Response** `200`:
```json
{
  "connected": true,
  "bootstrap_url": "http://127.0.0.1:4422",
  "relay_url": "https://relay.example.com",
  "total_peers": 5,
  "active_spaces": 2,
  "full_arc_peers": 3,
  "blocked_peers": 0,
  "ready": true
}
```

| Field | Type | Description |
|-------|------|-------------|
| `connected` | bool | Kitsune2 is enabled and has a running instance |
| `total_peers` | number | Known peers across all spaces |
| `active_spaces` | number | Number of spaces (DNAs) joined |
| `full_arc_peers` | number | Peers with full DHT arcs (conductors) |
| `blocked_peers` | number | Peers currently blocking our messages |
| `ready` | bool | Has full-arc peers and none blocking |

---

### `GET /k2/peers`

List all known peers across all spaces.

**Auth**: `k2`

**Response** `200`: Array of peer info objects.
```json
[
  {
    "agent_id": "<base64>",
    "space_id": "<base64>",
    "created_at": 1709000000000000,
    "expires_at": 1709003600000000,
    "is_tombstone": false,
    "url": "iroh://...",
    "storage_arc": {
      "arc_type": "full"
    }
  }
]
```

`storage_arc.arc_type` is one of: `"empty"`, `"full"`, or `"arc"` (with `start` and `length` fields).

---

### `GET /k2/space/{space_id}/status`

Status for a specific space (DNA).

| Parameter | In | Type | Description |
|-----------|-----|------|-------------|
| `space_id` | path | string | Space ID (base64 URL-safe no padding) |

**Auth**: `k2`

**Response** `200`:
```json
{
  "space_id": "<base64>",
  "local_agents": 2,
  "peer_count": 5
}
```

**Errors**: `404` if space not found.

---

### `GET /k2/space/{space_id}/peers`

List peers in a specific space.

**Auth**: `k2`

**Response** `200`: Array of peer info objects (same format as `/k2/peers`).

**Errors**: `404` if space not found.

---

### `GET /k2/space/{space_id}/local-agents`

List local agents registered in a space.

**Auth**: `k2`

**Response** `200`: Array of agent IDs (base64 strings).
```json
["<agent_id_base64>", "<agent_id_base64>"]
```

**Errors**: `404` if space not found.

---

### `GET /k2/transport/stats`

Transport-level network statistics (kitsune2 `ApiTransportStats`).

**Auth**: `k2`

**Response** `200`:
```json
{
  "transport_stats": {
    "backend": "iroh",
    "peer_urls": ["iroh://..."],
    "connections": [...]
  },
  "blocked_message_counts": {}
}
```

---

## Admin API

Only available when `H2HC_LINKER_ADMIN_SECRET` is set. All admin endpoints require `Authorization: Bearer <admin_secret>`.

### `POST /admin/agents`

Add or update an allowed agent.

**Request body**:
```json
{
  "agent_pubkey": "uhCAk...",
  "capabilities": ["dht_read", "dht_write", "k2"],
  "label": "My Browser Agent"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `agent_pubkey` | string | yes | HoloHash base64 agent pubkey |
| `capabilities` | array | yes | Array of capability strings: `"dht_read"`, `"dht_write"`, `"k2"` |
| `label` | string | no | Human-readable label |

**Response**: `204 No Content`

---

### `DELETE /admin/agents`

Remove an agent. Revokes all sessions and closes WebSocket connections.

**Request body**:
```json
{
  "agent_pubkey": "uhCAk..."
}
```

**Response**: `204 No Content` or `404 Not Found`

---

### `GET /admin/agents`

List all allowed agents.

**Response** `200`:
```json
{
  "agents": [
    {
      "agent_pubkey": "uhCAk...",
      "capabilities": ["dht_read", "dht_write"],
      "label": "My Agent"
    }
  ]
}
```

---

## WebSocket (`/ws`)

The WebSocket endpoint handles authentication, agent registration, signal delivery, and remote signing. All messages are JSON with a `type` field discriminator. The connection must authenticate before any other operations.

### Authentication

Client must authenticate before registering agents or sending signals.

**When auth is disabled** (no `H2HC_LINKER_ADMIN_SECRET`): sends `auth`, immediately receives `auth_ok` with empty token.

**When auth is enabled**: challenge-response flow using ed25519 signatures.

Client sends:
```json
{ "type": "auth", "agent_pubkey": "uhCAk..." }
```

Server responds with a challenge:
```json
{ "type": "auth_challenge", "challenge": "<hex 32 bytes>" }
```

Client signs the challenge and responds:
```json
{ "type": "auth_challenge_response", "signature": "<base64 64-byte ed25519 sig>" }
```

Server verifies and responds:
```json
{ "type": "auth_ok", "session_token": "<hex 64 chars>" }
```
The `session_token` is used for HTTP `Authorization: Bearer <token>` on subsequent requests. The session has no TTL — it lives until the WebSocket disconnects. After auth, the client must send `Register` messages for each DNA it wants to access; HTTP requests are scoped to registered DNAs.

On failure:
```json
{ "type": "auth_error", "message": "reason" }
```

### Agent Registration

Register an agent for a DNA to receive signals and join the kitsune2 space.

Client sends:
```json
{ "type": "register", "dna_hash": "uhC0k...", "agent_pubkey": "uhCAk..." }
```

Server confirms:
```json
{ "type": "registered", "dna_hash": "uhC0k...", "agent_pubkey": "uhCAk..." }
```

To unregister:
```json
{ "type": "unregister", "dna_hash": "uhC0k...", "agent_pubkey": "uhCAk..." }
```

Server confirms:
```json
{ "type": "unregistered", "dna_hash": "uhC0k...", "agent_pubkey": "uhCAk..." }
```

### Remote Signals

**Receiving signals** (from another agent via kitsune2):
```json
{
  "type": "signal",
  "dna_hash": "uhC0k...",
  "to_agent": "uhCAk...",
  "from_agent": "uhCAk...",
  "zome_name": "profiles",
  "signal": "<base64 msgpack payload>"
}
```

**Sending signals** (fire-and-forget, no response):
```json
{
  "type": "send_remote_signal",
  "dna_hash": "uhC0k...",
  "signals": [
    {
      "target_agent": [<byte array>],
      "zome_call_params": [<byte array>],
      "signature": [<64 byte array>]
    }
  ]
}
```

### Agent Info Signing (Transparent Signing Protocol)

When kitsune2 needs to publish agent info, the server requests a signature from the browser. The agent info fields are sent in structured form so the browser can validate what it's signing.

Server sends:
```json
{
  "type": "sign_agent_info",
  "request_id": "<id>",
  "agent_pubkey": "uhCAk...",
  "agent_info": {
    "agent": "<base64>",
    "space": "<base64>",
    "createdAt": "1731690797907204",
    "expiresAt": "1731762797907204",
    "isTombstone": false,
    "url": "iroh://...",
    "storageArc": [0, 4294967295]
  }
}
```

Client signs and responds:
```json
{
  "type": "sign_response",
  "request_id": "<id>",
  "signature": "<base64 64-byte ed25519 sig>"
}
```

Or on failure:
```json
{
  "type": "sign_response",
  "request_id": "<id>",
  "signature": null,
  "error": "reason"
}
```

### Heartbeat

Client sends:
```json
{ "type": "ping" }
```

Server responds:
```json
{ "type": "pong" }
```

The server also sends WebSocket-level pings. Connections are dropped after heartbeat timeout (default 40s with no response) or idle timeout (default 5 minutes).

### Errors

Server can send errors at any point:
```json
{ "type": "error", "message": "description" }
```

---

## Test Endpoints

### `POST /test/signal`

Inject a test signal (development only, no auth).

---

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `H2HC_LINKER_BOOTSTRAP_URL` | yes | Kitsune2 bootstrap server URL |
| `H2HC_LINKER_RELAY_URL` | no | Iroh relay server URL |
| `H2HC_LINKER_CONDUCTOR_URL` | no | Conductor address for zome call proxying |
| `H2HC_LINKER_ADMIN_SECRET` | no | Enables auth layer when set |
| `H2HC_LINKER_PAYLOAD_LIMIT_BYTES` | no | Max request payload size (default: 10MB) |
| `H2HC_LINKER_ZOME_CALL_TIMEOUT_MS` | no | Zome call timeout (default: 10000ms) |
| `H2HC_LINKER_REPORT` | no | Reporting mode: `"json_lines"` or `"none"` |
| `H2HC_LINKER_REPORT_PATH` | no | Directory for report files (default: `/tmp/h2hc-linker-reports`) |
| `H2HC_LINKER_REPORT_DAYS_RETAINED` | no | Report file retention in days (default: 5) |
| `H2HC_LINKER_REPORT_INTERVAL_S` | no | Fetched-op report interval in seconds (default: 60) |

---

## Error Responses

All errors follow the same format:

```json
{
  "error": "descriptive message",
  "code": 400
}
```

| HTTP Status | Condition |
|-------------|-----------|
| 400 | Malformed request, invalid hash, bad parameters |
| 401 | Missing or invalid authentication |
| 403 | Authenticated but insufficient capabilities |
| 404 | Resource not found (space, agent) |
| 500 | Internal server error |
| 502 | Conductor/network error |
| 503 | Conductor not connected (for zome call endpoint) |
