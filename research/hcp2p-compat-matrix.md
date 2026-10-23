# HcP2p / HcP2pHandler compatibility matrix

Plan v4, step 2. Gives every method and callback a defined meaning for a zero-arc phone conductor whose network is an H2HC linker instead of a local Kitsune2.

## Sources checked

| Source | Version |
|---|---|
| `holochain`, `holochain_p2p`, `holochain_cascade`, `holochain_state` | 0.7.0 (cargo registry). Traits diffed against 0.7.1: `HcP2pHandler` is unchanged; `HcP2p` changes only the doc comment on `publish` |
| `h2hc-linker` | github.com/Holo-Host/h2hc-linker @ `cab242d` (2026-08-28) |
| `kitsune2_core` | 0.5.1. The linker locks 0.5.0, which I have not checked |

Line references are `crate@version/path:line` or `linker:path:line`.

## Semantics key

- **Forward**: needs the conductor-facing linker protocol.
- **Local**: answered inside the adapter. This is the correct behaviour for a zero-arc node, not a stub.
- **Error**: returns an explicit unsupported error.
- **Stub**: optional diagnostics only; an empty result is allowed.

## HcP2p: phone to network

Trait at `holochain_p2p@0.7.0/src/types/actor.rs:148`.

| Method | Caller in 0.7.0 | Semantics | Linker today | Protocol needs |
|---|---|---|---|---|
| `register_handler` | `conductor/builder.rs:289` | Local: keep the handler | n/a | — |
| `join` | `conductor.rs:1959` (cell enable) | Forward: register agent and DNA; linker joins the space as a `ProxyAgent`; phone signs the agent info | WS `Register` and `SignAgentInfo` | Stop leaving on WS disconnect (finding 4). `config_override` only carries bootstrap and relay URLs, which the adapter ignores; the phone instead refuses to sign any agent info that claims an arc |
| `leave` | `cell.rs:871` (cell disable) | Forward | WS `Unregister` | — |
| `peer_store` | `conductor.rs:869, 2794, 2870` (admin agent-info list and add, stats pruning) | Local: empty per-DNA store whose `insert` returns an error | n/a | Optional: the linker's peer view |
| `new_integrated_data` | `integrate_dht_ops_workflow.rs:101` | Local: `Ok(())`, because a zero-arc node does not gossip | n/a | — |
| `publish` | `publish_dht_ops_workflow.rs:54` | Forward. Only op hashes arrive, so the adapter reads op bytes through `HolochainP2pConfig::get_dht_store`. Return `Ok` only after the linker has durably accepted the ops; otherwise return a retryable error | `POST /dht/{dna}/publish`: checks only the signature length, keeps a 60 s in-memory buffer, can report success when only some ops reached a peer (finding 5) | Durable acceptance, signed acceptance receipt, action manifest, embedded-signature check |
| `get` | `cascade@0.7.0/src/fetch.rs:186` | Forward: `Vec<WireOps>`, one entry per responder, judgments intact | `DhtQuery::get` keeps `WireOps` (`linker:src/dht_query.rs:225`), but the REST route flattens it and renders an unknown status as `Valid` (`linker:src/routes/dht.rs:546-548`). Buffer fallback is on by default (`linker:src/config.rs:173`) | Serialize `WireOps` with responder identity and data source (authority or buffer) |
| `get_links` | `fetch.rs:213` | Forward: `Vec<WireLinkOps>` including deletes | Same as `get` (`dht_query.rs:293`) | Same as `get` |
| `count_links` | `cascade lib.rs:588` | Forward | REST `/count_links` | Responder and source |
| `get_agent_activity` | `fetch.rs:240` | Forward | REST `/agent_activity/{agent}`; internally `Option<AgentActivityResponse>`. REST shape not checked | Unflattened response |
| `get_agent_activity_multi` | `fetch.rs:276` | Forward: fan out, return each peer's response paired with its key, fail with `InsufficientResponses` | **None** | New request type |
| `must_get_agent_activity` | `fetch.rs:296` | Forward. Never answered from the publish buffer, because validation depends on it | REST `POST /must_get_agent_activity` | Unflattened response, authority-only source |
| `call_remote` | `ribosome/host_fn/call.rs:147` | Forward: original `ExternIO` and signature out, response bytes back | **None**: the linker has no `CallRemote` code at all | `CallRemoteReq`/`Res` relay; durable command ID when the response is lost |
| `send_remote_signal` | `host_fn/send_remote_signal.rs:97` | Forward | WS `SendRemoteSignal`; an unparseable signature becomes 64 zero bytes (`linker:src/gateway_kitsune.rs:811`) | Reject bad signatures |
| `send_remote_signal_direct` | `conductor.rs:2993` | Forward | **None** | `RemoteSignalDirectEvt` relay |
| `was_agent_recently_online` | `validation_receipt_workflow.rs:76` | Local: `Ok(false)`. The phone validates no foreign ops, so it has no receipts to send | n/a | — |
| `send_validation_receipts` | `validation_receipt_workflow.rs` (`sign_and_send_receipts_to_author`) | Error: a zero-arc phone is never an authority | n/a | — |
| `authority_for_hash` | `cascade lib.rs:910` (`am_i_an_authority`) | Local: always `Ok(false)`, so every read goes to the network. Returning `true` would make the cascade answer from local data only | n/a | — |
| `target_arcs` | `call_zome_workflow.rs:86`, `initialize_zomes_workflow.rs:58` | Local: `[DhtArc::Empty]` per joined agent. `SourceChain::flush` uses it only in the rebase path (`holochain_state@0.7.0/src/source_chain.rs:254`) | n/a | — |
| `publish_countersign` | `countersigning_workflow.rs:790`, `countersigning_workflow/complete.rs:150` | Error: countersigning is outside the MVP | None | Later |
| `countersigning_session_negotiation` | `witnessing_workflow.rs:78`, `countersigning_workflow.rs:776` | Error | None | Later |
| `block` | `api_cell.rs:262` (HDK `block_agent`) | Local: write to the conductor store, as the real actor does (`actor.rs:2814`). Also Forward, so the linker stops relaying for that agent | None | Per-phone block list |
| `is_blocked` | `conductor.rs:981` | Local: read the conductor store (`actor.rs:2856`) | n/a | — |
| `dump_network_metrics` | Admin API | Stub: empty map | n/a | Optional |
| `dump_network_stats` | Admin API | Stub: empty transport stats | n/a | Optional |
| `conductor_store_getter` | — | Local: return `config.get_conductor_store` | n/a | — |
| `test_kitsune`, `test_set_full_arcs` | `test_utils` builds only | Not available. Sweettest helpers that need a Kitsune2 instance cannot run against the adapter | — | — |

## HcP2pHandler: network to phone

Trait at `holochain_p2p@0.7.0/src/types/event.rs:65`. The conductor implements it. The question for each callback is whether the linker delivers the wire message to the phone.

| Callback | Wire message | Needed on a zero-arc phone | Linker today | Semantics |
|---|---|---|---|---|
| `handle_call_remote` | `CallRemoteReq`, and also `RemoteSignalEvt` (`actor.rs:1397-1407`) | Yes | `CallRemoteReq` hits the catch-all and is dropped (`gateway_kitsune.rs:314`). `RemoteSignalEvt` is forwarded with the signature discarded (`:245`) and `from_agent: "remote"` (`:266`) | Forward the original `ExternIO` and signature so the phone runs it as a verified call. Relay `CallRemoteRes`, or answer `ErrorRes` at once when the phone is offline |
| `handle_remote_signal_direct` | `RemoteSignalDirectEvt` | Yes, if apps use direct signals | Dropped | Forward with `from_agent` and signature |
| `handle_validation_receipts_received` | `SendValidationReceiptsReq` (`actor.rs:1351`) | Yes | Dropped with no `SendValidationReceiptsRes`, so the validator's request times out (finding 3) | Durable inbox. Answer `SendValidationReceiptsRes` only after commit; the phone acknowledges delivery |
| `handle_get`, `handle_get_links`, `handle_count_links`, `handle_get_agent_activity`, `handle_must_get_agent_activity` | `*Req` | No: zero-arc agents are not selected as authorities | Dropped, so a misrouted requester waits for a timeout | Linker answers `ErrorRes` immediately |
| `handle_publish` | Publish and fetch | No | Not delivered | Not delivered |
| `handle_publish_countersign`, `handle_countersigning_session_negotiation` | `PublishCountersignEvt`, `CountersigningSessionNegotiationEvt` | Only with countersigning | Dropped | Unsupported in MVP; log it |
| (none) | `PingReq` | — | Not answered | Linker answers `PingRes` |

## Injectable network factory

- `ConductorBuilder::build` and `::test` call `holochain_p2p::spawn_holochain_p2p` directly (`holochain@0.7.0/src/conductor/conductor/builder.rs:259, 499`). 0.7.1 is the same.
- Everything after that point already uses a trait object: the conductor stores `DynHcP2p = Arc<dyn HcP2p>` (`conductor.rs:176`) and registers itself as the handler (`builder.rs:289`).
- So the fork point is one call. Proposed upstream change: `ConductorBuilder::with_p2p_factory(f)`, where `f: FnOnce(HolochainP2pConfig, MetaLairClient) -> BoxFut<'static, HolochainP2pResult<DynHcP2p>>` and the default is `spawn_holochain_p2p`.
- The factory must receive the whole `HolochainP2pConfig`. The adapter needs `get_dht_store` to read op bytes for `publish`, `get_conductor_store` for `block` and `is_blocked`, and `request_timeout`.

## Findings that change v4

1. **Outgoing calls are missing too.** v4's protocol table covers remote calls *to* the phone. The linker also has no outgoing `call_remote` and no `send_remote_signal_direct` in either direction.
2. **Incoming signals lose their signature.** In `holochain_p2p`, `RemoteSignalEvt` is delivered through `handle_call_remote` with its signature, so the receiving conductor verifies it like any zome call. The linker discards the signature and sets `from_agent` to `"remote"`, so the phone cannot run `recv_remote_signal` as a verified call. This is the inbound half of the zero-signature finding in v4.
3. **The receipt-drop claim is confirmed, with a condition.** The validator clears the "receipts required" flag, and stops retrying until a new publish sets it again, only when the author is not "recently online" (`validation_receipt_workflow.rs:76-88`). "Recently online" means the author's agent info is in the peer store with a URL and the peer is not marked unresponsive (`actor.rs:2505-2534`). While the author still looks online, the linker's silence causes timeouts and repeated retries, not loss.
4. **"Linker stays routable while the phone sleeps" conflicts with current behaviour.**
   - The linker calls `agent_leave` for every registration when the WebSocket closes (`linker:src/routes/websocket.rs:297-305`).
   - Agent info is signed with the agent's own key, through remote signing on the phone. Kitsune2 gives it a fixed 20-minute lifetime (`kitsune2_core@0.5.1/src/factories/core_space.rs:539-540`).
   - So even if leave-on-disconnect is removed, a sleeping phone stays routable for at most about 20 minutes after its last signature.
   - Receipts sent during longer sleeps cannot be collected by the linker alone. Republish on wake is the main mechanism, not a supplement.
   - Removing this limit needs a Kitsune2 change, such as a longer lifetime or delegated signing. It belongs on the list to agree with Holo.
5. **Publish success is weaker than it looks.** `success = failed == 0 && (queued == 0 || published > 0)` (`linker:src/routes/publish.rs:209`). It is true if any basis group reached a peer, and "published" means at least one peer was notified, not that it fetched or integrated the op. The buffer is in memory with a 60 s TTL (`linker:src/temp_op_store.rs:51`). The embedded signature check is still a TODO (`publish.rs:293`).
6. **`get` and `get_links` are cheaper to fix than v4 implies.** Flattening happens only in the REST routes; `DhtQuery` still holds `WireOps` and `WireLinkOps`. The remaining gaps are multi-responder results and source labelling: queries race to the first non-empty response, and the no-peers path answers from the buffer.
7. **The 0.7.1 `publish` contract matches v4.** Its doc now reads "Success indicates admission for publication, not remote delivery", with `NoPeersForLocation` when there is nowhere to send (`holochain_p2p@0.7.1/src/types/actor.rs:276`). This fits "acceptance is a storage commitment".

## Not verified

- Whether an incoming republish of an already-integrated op sets "receipts required" again at the authority. A code comment says it does; I did not trace the incoming-op path.
- The agent-info lifetime in `kitsune2_core` 0.5.0, which the linker locks.
- The JSON shapes of the REST `agent_activity` and `must_get_agent_activity` routes.
- Whether the tombstone that `agent_leave` publishes after a disconnect gets signed at all, since signing needs the phone, which has just gone.
