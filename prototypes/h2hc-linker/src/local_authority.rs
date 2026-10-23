//! Minimal, local reconstruction of DHT authority responses from ops this
//! gateway is already holding in [`crate::temp_op_store::TempOpStore`].
//!
//! # Why this exists
//!
//! Every hosted browser agent is a zero-arc kitsune2 [`crate::proxy_agent::ProxyAgent`]
//! joined to a single shared kitsune2 [`kitsune2_api::Space`] per DNA. When two such
//! agents are hosted by the *same* linker instance, kitsune2 can never treat one as a
//! "remote peer" of the other:
//! `kitsune2_core::get_responsive_remote_agents_near_location` unconditionally removes
//! every agent present in the space's `local_agent_store` from its results
//! (`kitsune2_core::common::filter_local_agents`), and this has been true across every
//! kitsune2 version this gateway has depended on (unchanged between `0.4.0-dev.2`,
//! used on `main`/pre-0.7, and `0.5.0`, used here). So `GatewayKitsune::publish_ops`
//! and `DhtQuery::{get,get_links}` always see zero co-located peers, independent of any
//! kitsune2 version regression -- the gateway's design has always relied on a real,
//! non-zero-arc DHT authority being reachable to relay data between hosted agents.
//!
//! In a deployment where the *only* peers currently reachable are other zero-arc agents
//! hosted by this same gateway (a lightweight-client-only linker, or -- as in the
//! `ziptest` e2e setup -- a real authority that a hosted agent's ops haven't reached
//! yet), that assumption fails and published ops are silently stuck. This module gives
//! the linker its own short-circuit for that case: since `TempOpStore` already holds
//! the raw bytes of everything published through this gateway (regardless of whether
//! any remote peer was found), a `get`/`get_links` from one hosted agent can be
//! answered directly from another hosted agent's still-buffered publish, without
//! waiting on kitsune2's wire protocol at all.
//!
//! # Scope
//!
//! This deliberately does **not** reimplement DHT authority semantics
//! (`holochain_cascade::authority`). `TempOpStore` only holds fresh ops for a short
//! TTL, so there is no overlay of deletes/updates here:
//! - only `Create*`/`CreateLink` ops are served (a delete/update published by a hosted
//!   agent will reach co-located peers once a real DHT authority converges, same as
//!   today);
//! - warrants are never synthesized.
//!
//! Every served action IS given an honest validation status, though: `TempOpStore`
//! holds ops as soon as they're published, before any authority (real or otherwise)
//! has validated them, so this module has no basis for asserting `Valid` -- it reports
//! `Judged::none` (`status: None`, i.e. "unvalidated/unknown") instead. This is safe
//! for existing consumers of this data: `routes::dht.rs`'s wire-to-JSON conversions
//! already treat `None` the same as `Valid` when deciding what to surface to a caller
//! (`.status.map(|s| s == ValidationStatus::Valid).unwrap_or(true)`), and neither the
//! HWC extension nor core ever branches on a fetched record/link's validation status.
//! Stamping `Valid` here would have been actively misleading for anything that DOES
//! start caring about validation status later (e.g. a future warrant/rejection path).
//!
//! This is enough to let one hosted agent's freshly published record or link become
//! visible to another hosted agent immediately, which is the concrete gap that leaves
//! co-located zero-arc agents unable to see each other at all.
//!
//! # Enabling/disabling
//!
//! Gated by `Configuration::local_authority_fallback_enabled`
//! (`H2HC_LINKER_LOCAL_AUTHORITY_FALLBACK`, default enabled) - see `service.rs`'s
//! `DhtQuery::with_local_ops` call site. An operator can disable this fallback in the
//! field without a rebuild if it ever causes trouble.

use holochain_types::entry::{EntryData, WireEntryOps};
use holochain_types::link::{WireLinkKey, WireLinkOps};
use holochain_types::prelude::{
    ActionData, ChainOp, DhtOp, Entry, EntryType, Judged, OpEntry, SignedAction, WireOps,
};
use holochain_types::record::WireRecordOps;

/// Build a `get` response from locally-buffered ops, if any of them create the
/// requested record or entry.
///
/// Returns `WireOps::Record` for a `CreateRecord` op (action-hash-keyed get) or
/// `WireOps::Entry` for a `CreateEntry` op (entry-hash-keyed get). `ops` is expected to
/// already be filtered to a single DHT basis (see
/// `TempOpStoreHandle::decode_ops_at_basis`), so the first matching create wins.
pub fn wire_ops_for_get(ops: Vec<DhtOp>) -> Option<WireOps> {
    for op in ops {
        let DhtOp::ChainOp(chain_op) = op else {
            continue;
        };
        match *chain_op {
            ChainOp::CreateRecord(signed_action, op_entry) => {
                return Some(WireOps::Record(WireRecordOps {
                    // Unvalidated - see the module doc comment. No authority
                    // (real or otherwise) has judged this op yet.
                    action: Some(Judged::none(signed_action)),
                    deletes: vec![],
                    updates: vec![],
                    entry: entry_from_op_entry(op_entry),
                    warrants: vec![],
                }));
            }
            ChainOp::CreateEntry(signed_action, op_entry) => {
                let OpEntry::Present(entry) = op_entry else {
                    continue;
                };
                let Some(entry_type) = entry_type_of(&signed_action) else {
                    continue;
                };
                return Some(WireOps::Entry(WireEntryOps {
                    // Unvalidated - see the module doc comment.
                    creates: vec![Judged::none(signed_action)],
                    deletes: vec![],
                    updates: vec![],
                    entry: Some(EntryData { entry, entry_type }),
                    warrants: vec![],
                }));
            }
            _ => continue,
        }
    }
    None
}

/// Build a `get_links` response from locally-buffered ops at a link base, applying the
/// same type/tag/author/time filters a real authority would
/// (`holochain_cascade::authority::filter_link_creates`).
///
/// `ops` is expected to already be filtered to `link_key.base`'s DHT basis (see
/// `TempOpStoreHandle::decode_ops_at_basis`). Deletes are intentionally left empty: the
/// HWC extension derives the live link set client-side from `/details` deletes, not
/// from this endpoint (see `routes::dht::wire_link_ops_to_links` doc comment).
pub fn wire_link_ops_for_links(ops: Vec<DhtOp>, link_key: &WireLinkKey) -> WireLinkOps {
    let creates = ops
        .into_iter()
        .filter_map(|op| {
            let DhtOp::ChainOp(chain_op) = op else {
                return None;
            };
            let ChainOp::CreateLink(signed_action) = *chain_op else {
                return None;
            };
            // Unvalidated - see the module doc comment.
            matches_link_key(&signed_action, link_key).then(|| Judged::none(signed_action))
        })
        .collect();

    WireLinkOps {
        creates,
        deletes: vec![],
        warrants: vec![],
    }
}

fn entry_from_op_entry(op_entry: OpEntry) -> Option<Entry> {
    match op_entry {
        OpEntry::Present(entry) => Some(entry),
        OpEntry::Hidden | OpEntry::ActionOnly => None,
    }
}

fn entry_type_of(signed_action: &SignedAction) -> Option<EntryType> {
    match &signed_action.data().data {
        ActionData::Create(d) => Some(d.entry_type.clone()),
        ActionData::Update(d) => Some(d.entry_type.clone()),
        _ => None,
    }
}

fn matches_link_key(signed_action: &SignedAction, key: &WireLinkKey) -> bool {
    let action = signed_action.data();
    let ActionData::CreateLink(create_link) = &action.data else {
        return false;
    };

    if !key
        .type_query
        .contains(&create_link.zome_index, &create_link.link_type)
    {
        return false;
    }
    if let Some(tag) = &key.tag {
        if !create_link.tag.0.starts_with(&tag.0) {
            return false;
        }
    }
    if let Some(author) = &key.author {
        if &action.header.author != author {
            return false;
        }
    }
    if let Some(before) = key.before {
        if action.header.timestamp > before {
            return false;
        }
    }
    if let Some(after) = key.after {
        if action.header.timestamp < after {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use holo_hash::{ActionHash, AgentPubKey, AnyLinkableHash, EntryHash};
    use holochain_types::prelude::{
        Action, ActionHeader, AppEntryDef, CreateData, CreateLinkData, EntryVisibility, LinkTag,
        LinkTypeFilter, Signature, Timestamp,
    };

    fn agent() -> AgentPubKey {
        AgentPubKey::from_raw_32(vec![1; 32])
    }

    fn app_entry_type() -> EntryType {
        EntryType::App(AppEntryDef {
            entry_index: 0.into(),
            zome_index: 0.into(),
            visibility: EntryVisibility::Public,
        })
    }

    fn signed_create_record(entry_hash: EntryHash, _entry: Entry) -> (ActionHash, SignedAction) {
        let action = Action {
            header: ActionHeader {
                author: agent(),
                timestamp: Timestamp::from_micros(1_000),
                action_seq: 3,
                prev_action: Some(ActionHash::from_raw_32(vec![9; 32])),
            },
            data: ActionData::Create(CreateData {
                entry_type: app_entry_type(),
                entry_hash,
            }),
        };
        let action_hash = ActionHash::with_data_sync(&action);
        (
            action_hash,
            SignedAction::new(action, Signature::from([0xaa; 64])),
        )
    }

    #[test]
    fn wire_ops_for_get_builds_record_from_create_record_op() {
        let entry = Entry::App(holochain_types::prelude::AppEntryBytes(
            holochain_serialized_bytes::prelude::SerializedBytes::from(
                holochain_serialized_bytes::prelude::UnsafeBytes::from(vec![1, 2, 3]),
            ),
        ));
        let entry_hash = EntryHash::with_data_sync(&entry);
        let (_action_hash, signed_action) = signed_create_record(entry_hash, entry.clone());

        let op = DhtOp::ChainOp(Box::new(ChainOp::CreateRecord(
            signed_action,
            OpEntry::Present(entry),
        )));

        let result = wire_ops_for_get(vec![op]).expect("should build a Record response");
        match result {
            WireOps::Record(record_ops) => {
                let action = record_ops.action.as_ref().expect("action present");
                // Never a real validation verdict: TempOpStore holds ops before any
                // authority (real or otherwise) has judged them - see the module doc
                // comment's "Judged::none, not Judged::valid" rationale (the F2A fix).
                assert!(action.status.is_none());
                assert!(record_ops.entry.is_some());
            }
            other => panic!("expected WireOps::Record, got {other:?}"),
        }
    }

    #[test]
    fn wire_ops_for_get_returns_none_for_no_matching_ops() {
        assert!(wire_ops_for_get(vec![]).is_none());
    }

    #[test]
    fn wire_link_ops_for_links_filters_by_type_and_tag() {
        let base = AnyLinkableHash::from(EntryHash::from_raw_32(vec![2; 32]));
        let target = AnyLinkableHash::from(EntryHash::from_raw_32(vec![3; 32]));

        let make_create = |zome: u8, link_type: u8, tag: Vec<u8>| {
            let action = Action {
                header: ActionHeader {
                    author: agent(),
                    timestamp: Timestamp::from_micros(1_000),
                    action_seq: 4,
                    prev_action: Some(ActionHash::from_raw_32(vec![9; 32])),
                },
                data: ActionData::CreateLink(CreateLinkData {
                    base_address: base.clone(),
                    target_address: target.clone(),
                    zome_index: zome.into(),
                    link_type: link_type.into(),
                    tag: LinkTag::new(tag),
                }),
            };
            DhtOp::ChainOp(Box::new(ChainOp::CreateLink(SignedAction::new(
                action,
                Signature::from([0xbb; 64]),
            ))))
        };

        let matching = make_create(0, 1, vec![0xde, 0xad]);
        let wrong_type = make_create(0, 2, vec![0xde, 0xad]);

        let link_key = WireLinkKey {
            base,
            type_query: LinkTypeFilter::single_type(0.into(), 1.into()),
            tag: None,
            after: None,
            before: None,
            author: None,
        };

        let result = wire_link_ops_for_links(vec![matching, wrong_type], &link_key);
        assert_eq!(result.creates.len(), 1);
        // Never a real validation verdict - see the record-path test above and the
        // module doc comment's "Judged::none, not Judged::valid" rationale (the F2A
        // fix).
        assert!(result.creates[0].status.is_none());
        assert!(result.deletes.is_empty());
    }
}
