//! DHT endpoints for h2hc-linker.
//!
//! These endpoints query the DHT directly via kitsune2 wire protocol.

use axum::extract::{Path, Query, State};
use axum::Json;
use holochain_types::prelude::{
    ActionHash, AgentPubKey, AnyDhtHash, AnyLinkableHash, EntryHash, ExternalHash,
};
use serde::Deserialize;

use crate::error::{LinkerError, LinkerResult};
use crate::service::AppState;

// For direct DHT queries
use holochain_types::link::WireLinkKey;
use holochain_types::prelude::{ActionData, ChainFilter, LimitConditions, LinkTag, LinkTypeFilter};
use holochain_zome_types::link::Link;

// For wire_ops_to_details_json
use holochain_types::entry::WireEntryOps;
use holochain_types::prelude::{
    ActionHashed, Details, EntryDetails, EntryDhtStatus, Judged, Record, RecordDetails,
    RecordEntry, SignedAction, SignedActionHashed, ValidationStatus, WireOps,
};
use holochain_types::record::WireRecordOps;

// ============================================================================
// Hash parsing helpers
// ============================================================================

fn parse_dna_hash(s: &str) -> LinkerResult<holochain_types::dna::DnaHash> {
    holochain_types::dna::DnaHash::try_from(s)
        .map_err(|_| LinkerError::RequestMalformed(format!("Invalid DNA hash: {s}")))
}

fn parse_any_dht_hash(s: &str) -> LinkerResult<AnyDhtHash> {
    if let Ok(hash) = EntryHash::try_from(s) {
        return Ok(AnyDhtHash::from(hash));
    }
    if let Ok(hash) = ActionHash::try_from(s) {
        return Ok(AnyDhtHash::from(hash));
    }
    Err(LinkerError::RequestMalformed(format!(
        "Invalid DHT hash: {s}"
    )))
}

fn parse_any_linkable_hash(s: &str) -> LinkerResult<AnyLinkableHash> {
    if let Ok(hash) = AgentPubKey::try_from(s) {
        return Ok(AnyLinkableHash::from(hash));
    }
    if let Ok(hash) = EntryHash::try_from(s) {
        return Ok(AnyLinkableHash::from(hash));
    }
    if let Ok(hash) = ActionHash::try_from(s) {
        return Ok(AnyLinkableHash::from(hash));
    }
    if let Ok(hash) = ExternalHash::try_from(s) {
        return Ok(AnyLinkableHash::from(hash));
    }
    Err(LinkerError::RequestMalformed(format!(
        "Invalid linkable hash: {s}"
    )))
}

// ============================================================================
// HTTP request/response types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct RecordPath {
    pub dna_hash: String,
    pub hash: String,
}

#[derive(Debug, Deserialize)]
pub struct LinksPath {
    pub dna_hash: String,
}

#[derive(Debug, Deserialize)]
pub struct LinksQuery {
    pub base: String,
    #[serde(rename = "type")]
    pub link_type: Option<u16>,
    pub tag: Option<String>,
    /// Optional zome index for filtering links. Required when filtering by link_type.
    /// If omitted and no link_type filter, returns links from all zomes.
    pub zome_index: Option<u8>,
}

// ============================================================================
// Endpoints
// ============================================================================

/// GET /dht/{dna_hash}/record/{hash}
///
/// Get a record by action or entry hash.
/// Queries the DHT directly via kitsune2.
#[tracing::instrument(skip(state))]
pub async fn dht_get_record(
    Path(path): Path<RecordPath>,
    State(state): State<AppState>,
) -> LinkerResult<Json<serde_json::Value>> {
    let dht_query = state
        .dht_query
        .as_ref()
        .ok_or_else(|| LinkerError::Internal("DHT queries not available".to_string()))?;

    let dna_hash = parse_dna_hash(&path.dna_hash)?;
    let hash = parse_any_dht_hash(&path.hash)?;

    let result = dht_query.get(&dna_hash, hash).await?;

    match result {
        Some(wire_ops) => {
            let json_value = wire_ops_to_record_json(&wire_ops);
            Ok(Json(json_value))
        }
        None => Ok(Json(serde_json::Value::Null)),
    }
}

/// GET /dht/{dna_hash}/details/{hash}
///
/// Get details for a hash (including updates and deletes).
/// Queries the DHT directly via kitsune2 and converts WireOps to the
/// Details format matching Holochain's get_details return type.
#[tracing::instrument(skip(state))]
pub async fn dht_get_details(
    Path(path): Path<RecordPath>,
    State(state): State<AppState>,
) -> LinkerResult<Json<serde_json::Value>> {
    let dht_query = state
        .dht_query
        .as_ref()
        .ok_or_else(|| LinkerError::Internal("DHT queries not available".to_string()))?;

    let dna_hash = parse_dna_hash(&path.dna_hash)?;
    let hash = parse_any_dht_hash(&path.hash)?;

    let result = dht_query.get(&dna_hash, hash).await?;

    match result {
        Some(wire_ops) => {
            let json_value = wire_ops_to_details_json(&wire_ops);
            Ok(Json(json_value))
        }
        None => Ok(Json(serde_json::Value::Null)),
    }
}

/// GET /dht/{dna_hash}/links
///
/// Get links from a base hash.
/// Queries the DHT directly via kitsune2.
#[tracing::instrument(skip(state))]
pub async fn dht_get_links(
    Path(path): Path<LinksPath>,
    Query(query): Query<LinksQuery>,
    State(state): State<AppState>,
) -> LinkerResult<Json<serde_json::Value>> {
    let dht_query = state
        .dht_query
        .as_ref()
        .ok_or_else(|| LinkerError::Internal("DHT queries not available".to_string()))?;

    let dna_hash = parse_dna_hash(&path.dna_hash)?;
    let base = parse_any_linkable_hash(&query.base)?;

    let tag = if let Some(tag_str) = query.tag {
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &tag_str)
            .map_err(|_| LinkerError::RequestMalformed("Invalid tag encoding".to_string()))?;
        Some(LinkTag::new(bytes))
    } else {
        None
    };

    // Build the WireLinkKey for the query
    // - If zome_index + link_type provided: filter by specific zome and type
    // - If zome_index only: all types from that zome
    // - If neither: match all links (Types(Vec::new()))
    // - If link_type without zome_index: error (zome_index required for type filtering)
    let type_query = match (query.zome_index, query.link_type) {
        (Some(zome_idx), Some(link_type)) => {
            // Specific zome and link type
            LinkTypeFilter::single_type(zome_idx.into(), (link_type as u8).into())
        }
        (Some(zome_idx), None) => {
            // All link types from specific zome
            LinkTypeFilter::single_dep(zome_idx.into())
        }
        (None, Some(_)) => {
            // link_type without zome_index - can't filter properly
            return Err(LinkerError::RequestMalformed(
                "zome_index is required when filtering by link type".to_string(),
            ));
        }
        (None, None) => {
            // No filtering - match all links from all zomes
            LinkTypeFilter::Types(Vec::new())
        }
    };

    let link_key = WireLinkKey {
        base: base.clone(),
        type_query,
        tag: tag.clone(),
        after: None,
        before: None,
        author: None, // Don't filter by author
    };

    let result = dht_query.get_links(&dna_hash, link_key).await?;

    match result {
        Some(wire_link_ops) => {
            tracing::debug!(
                creates_count = wire_link_ops.creates.len(),
                deletes_count = wire_link_ops.deletes.len(),
                "Direct DHT get_links response"
            );
            let links = wire_link_ops_to_links(&wire_link_ops, &base, tag.as_ref());
            let json_value = serde_json::to_value(&links).unwrap_or_else(|e| {
                tracing::warn!("Failed to serialize links: {}", e);
                serde_json::json!([])
            });
            Ok(Json(json_value))
        }
        None => Ok(Json(serde_json::json!([]))),
    }
}

// ============================================================================
// Additional kitsune endpoints
// ============================================================================

/// GET /dht/{dna_hash}/count_links
///
/// Count links from a base hash.
/// Queries the DHT directly via kitsune2.
/// Returns the count of matching links as a JSON number.
#[tracing::instrument(skip(state))]
pub async fn dht_count_links(
    Path(path): Path<LinksPath>,
    Query(query): Query<LinksQuery>,
    State(state): State<AppState>,
) -> LinkerResult<Json<serde_json::Value>> {
    let dht_query = state
        .dht_query
        .as_ref()
        .ok_or_else(|| LinkerError::Internal("DHT queries not available".to_string()))?;

    let dna_hash = parse_dna_hash(&path.dna_hash)?;
    let base = parse_any_linkable_hash(&query.base)?;

    let tag_prefix = if let Some(tag_str) = query.tag {
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &tag_str)
            .map_err(|_| LinkerError::RequestMalformed("Invalid tag encoding".to_string()))?;
        Some(LinkTag::new(bytes))
    } else {
        None
    };

    let type_query = match (query.zome_index, query.link_type) {
        (Some(zome_idx), Some(link_type)) => {
            LinkTypeFilter::single_type(zome_idx.into(), (link_type as u8).into())
        }
        (Some(zome_idx), None) => LinkTypeFilter::single_dep(zome_idx.into()),
        (None, Some(_)) => {
            return Err(LinkerError::RequestMalformed(
                "zome_index is required when filtering by link type".to_string(),
            ));
        }
        (None, None) => LinkTypeFilter::Types(Vec::new()),
    };

    let query = holochain_types::link::WireLinkQuery {
        base,
        link_type: type_query,
        tag_prefix,
        before: None,
        after: None,
        author: None,
    };

    let result = dht_query.count_links(&dna_hash, query).await?;

    match result {
        Some(count_response) => {
            let json_value = serde_json::to_value(&count_response).unwrap_or_else(|e| {
                tracing::warn!("Failed to serialize count_links response: {}", e);
                serde_json::json!(0)
            });
            Ok(Json(json_value))
        }
        None => Ok(Json(serde_json::json!(0))),
    }
}

// ============================================================================
// Agent activity endpoints
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct AgentActivityPath {
    pub dna_hash: String,
    pub agent_hash: String,
}

#[derive(Debug, Deserialize)]
pub struct AgentActivityQuery {
    /// "status" or "full" (default: "full")
    pub request: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MustGetAgentActivityBody {
    pub agent: String,
    pub chain_top: String,
    #[serde(default)]
    pub include_cached_entries: bool,
}

fn parse_agent_pubkey(s: &str) -> LinkerResult<AgentPubKey> {
    AgentPubKey::try_from(s)
        .map_err(|_| LinkerError::RequestMalformed(format!("Invalid agent pubkey: {s}")))
}

fn parse_action_hash(s: &str) -> LinkerResult<ActionHash> {
    ActionHash::try_from(s)
        .map_err(|_| LinkerError::RequestMalformed(format!("Invalid action hash: {s}")))
}

/// GET /dht/{dna_hash}/agent_activity/{agent_hash}
///
/// Get agent activity (chain status and action hashes) for an agent.
/// Queries the DHT directly via kitsune2.
#[tracing::instrument(skip(state))]
pub async fn dht_get_agent_activity(
    Path(path): Path<AgentActivityPath>,
    Query(query): Query<AgentActivityQuery>,
    State(state): State<AppState>,
) -> LinkerResult<Json<serde_json::Value>> {
    let dht_query = state
        .dht_query
        .as_ref()
        .ok_or_else(|| LinkerError::Internal("DHT queries not available".to_string()))?;

    let dna_hash = parse_dna_hash(&path.dna_hash)?;
    let agent = parse_agent_pubkey(&path.agent_hash)?;

    let is_full = query.request.as_deref() != Some("status");
    let options = holochain_p2p::event::GetActivityOptions {
        include_valid_activity: is_full,
        include_rejected_activity: is_full,
        include_warrants: true,
        include_full_records: false,
    };

    let chain_query = holochain_zome_types::query::ChainQueryFilter::new();

    let result = dht_query
        .get_agent_activity(&dna_hash, agent, chain_query, options)
        .await?;

    match result {
        Some(response) => {
            let json_value = serde_json::to_value(&response).unwrap_or(serde_json::Value::Null);
            Ok(Json(json_value))
        }
        None => Ok(Json(serde_json::Value::Null)),
    }
}

/// POST /dht/{dna_hash}/must_get_agent_activity
///
/// Get agent activity with must-get semantics (chain filter walk).
/// Queries the DHT directly via kitsune2.
#[tracing::instrument(skip(state))]
pub async fn dht_must_get_agent_activity(
    Path(path): Path<LinksPath>,
    State(state): State<AppState>,
    Json(body): Json<MustGetAgentActivityBody>,
) -> LinkerResult<Json<serde_json::Value>> {
    let dht_query = state
        .dht_query
        .as_ref()
        .ok_or_else(|| LinkerError::Internal("DHT queries not available".to_string()))?;

    let dna_hash = parse_dna_hash(&path.dna_hash)?;
    let agent = parse_agent_pubkey(&body.agent)?;
    let chain_top = parse_action_hash(&body.chain_top)?;

    let filter = ChainFilter {
        chain_top,
        limit_conditions: LimitConditions::ToGenesis,
        include_cached_entries: body.include_cached_entries,
    };

    let result = dht_query
        .must_get_agent_activity(&dna_hash, agent, filter)
        .await?;

    match result {
        Some(response) => {
            let json_value = serde_json::to_value(&response).unwrap_or(serde_json::Value::Null);
            Ok(Json(json_value))
        }
        None => Ok(Json(serde_json::Value::Null)),
    }
}

// ============================================================================
// Wire type to JSON conversion (for direct DHT mode)
// ============================================================================

/// Convert a `Judged<SignedAction>` from a wire response into a `SignedActionHashed`.
///
/// In 0.7 wire responses carry the full signed `Action` directly (rather than
/// condensed per-variant wire structs), so producing the hashed+signed form is
/// just a hash computation away — no field-by-field reconstruction needed.
fn judged_signed_action_to_hashed(judged: &Judged<SignedAction>) -> SignedActionHashed {
    let action = judged.data.data().clone();
    let signature = judged.data.signature().clone();
    let action_hashed = ActionHashed::from_content_sync(action);
    SignedActionHashed::with_presigned(action_hashed, signature)
}

/// Convert WireOps to a flat Record JSON matching Holochain's get return type.
///
/// WireOps are the condensed wire protocol format. This function reconstructs the full
/// Record (signed_action + entry) that Holochain returns from get calls.
///
/// Both WireOps::Record (returned by action-keyed gets) and WireOps::Entry (returned
/// by entry-keyed gets) carry enough information to reconstruct a Record. Holochain's
/// `get(EntryHash)` selects a creating action for the entry and returns a Record built
/// from it; we mirror that selection logic locally for the Entry variant.
///
/// Returns null for the Warrant variant, or if the variant carries no usable record.
fn wire_ops_to_record_json(ops: &WireOps) -> serde_json::Value {
    match ops {
        WireOps::Record(record_ops) => wire_record_ops_to_record(record_ops),
        WireOps::Entry(entry_ops) => wire_entry_ops_to_record(entry_ops),
        WireOps::Warrant(_) => serde_json::Value::Null,
    }
}

/// Convert WireRecordOps to a flat Record JSON.
///
/// Builds SignedActionHashed + entry from the wire format's signed action,
/// producing the same JSON shape as Holochain's Record type:
/// { "signed_action": { "hashed": { "content": ..., "hash": ... }, "signature": ... }, "entry": ... }
fn wire_record_ops_to_record(record_ops: &WireRecordOps) -> serde_json::Value {
    let judged_signed_action = match &record_ops.action {
        Some(a) => a,
        None => return serde_json::Value::Null,
    };

    let signed_action_hashed = judged_signed_action_to_hashed(judged_signed_action);
    let vis = signed_action_hashed.action().entry_visibility();
    let record_entry = RecordEntry::new(vis, record_ops.entry.clone());
    let record = Record::new(signed_action_hashed, record_entry);

    serde_json::to_value(&record).unwrap_or(serde_json::Value::Null)
}

/// Convert WireEntryOps to a flat Record JSON.
///
/// Holochain's `get(EntryHash)` returns a Record for an entry by selecting one of
/// its creating actions (Create or Update); peers reply with WireOps::Entry rather
/// than WireOps::Record because the request key is an entry hash. We pick the first
/// valid create whose action has not been tombstoned by an entry-delete. If every
/// create has been deleted (or none are valid), we return null — matching what
/// Holochain returns for a fully-deleted entry on a default `get`.
fn wire_entry_ops_to_record(entry_ops: &WireEntryOps) -> serde_json::Value {
    let entry_data = match &entry_ops.entry {
        Some(ed) => ed,
        None => return serde_json::Value::Null,
    };

    // Set of action hashes that have been tombstoned. Each delete action in
    // entry_ops.deletes carries the original Delete action data, whose
    // `deletes_address` is the action hash of the create being deleted.
    let deleted_action_hashes: std::collections::HashSet<ActionHash> = entry_ops
        .deletes
        .iter()
        .filter_map(|d| match &d.data.data().data {
            ActionData::Delete(delete) => Some(delete.deletes_address.clone()),
            _ => None,
        })
        .collect();

    for judged_create in &entry_ops.creates {
        let is_valid = judged_create
            .status
            .map(|s| s == ValidationStatus::Valid)
            .unwrap_or(true);
        if !is_valid {
            continue;
        }

        let signed_action_hashed = judged_signed_action_to_hashed(judged_create);
        if deleted_action_hashes.contains(signed_action_hashed.action_address()) {
            continue;
        }
        let record = Record::new(
            signed_action_hashed,
            RecordEntry::Present(entry_data.entry.clone()),
        );
        return serde_json::to_value(&record).unwrap_or(serde_json::Value::Null);
    }

    serde_json::Value::Null
}

/// Convert WireOps to the Details JSON format matching Holochain's get_details return type.
///
/// WireOps are the condensed wire protocol format. This function reconstructs the full
/// Details structure (adjacently tagged enum with {type, content}) that Holochain returns
/// from get_details calls.
fn wire_ops_to_details_json(ops: &WireOps) -> serde_json::Value {
    match ops {
        WireOps::Record(record_ops) => wire_record_ops_to_details(record_ops),
        WireOps::Entry(entry_ops) => wire_entry_ops_to_details(entry_ops),
        WireOps::Warrant(_) => serde_json::Value::Null,
    }
}

/// Convert WireRecordOps to Details::Record JSON.
///
/// Builds the full Record (signed action + entry) and metadata (deletes, updates)
/// from the wire format's signed actions.
fn wire_record_ops_to_details(record_ops: &WireRecordOps) -> serde_json::Value {
    let judged_signed_action = match &record_ops.action {
        Some(a) => a,
        None => return serde_json::Value::Null,
    };

    let signed_action_hashed = judged_signed_action_to_hashed(judged_signed_action);
    let vis = signed_action_hashed.action().entry_visibility();
    let record_entry = RecordEntry::new(vis, record_ops.entry.clone());
    let record = Record::new(signed_action_hashed, record_entry);

    let validation_status = judged_signed_action
        .status
        .unwrap_or(ValidationStatus::Valid);

    let deletes: Vec<SignedActionHashed> = record_ops
        .deletes
        .iter()
        .map(judged_signed_action_to_hashed)
        .collect();

    let updates: Vec<SignedActionHashed> = record_ops
        .updates
        .iter()
        .map(judged_signed_action_to_hashed)
        .collect();

    let record_details = RecordDetails {
        record,
        validation_status,
        deletes,
        updates,
    };

    let details = Details::Record(record_details);
    serde_json::to_value(&details).unwrap_or(serde_json::Value::Null)
}

/// Convert WireEntryOps to Details::Entry JSON.
///
/// Builds the full Entry details including all create actions, deletes, and updates
/// from the wire format's signed actions. Entry data is shared across all creates.
fn wire_entry_ops_to_details(entry_ops: &WireEntryOps) -> serde_json::Value {
    let entry_data = match &entry_ops.entry {
        Some(ed) => ed,
        None => return serde_json::Value::Null,
    };

    let entry = entry_data.entry.clone();

    // Convert creates to SignedActionHashed, separating valid from rejected
    let mut actions = Vec::new();
    let mut rejected_actions = Vec::new();

    for judged_create in &entry_ops.creates {
        let signed_action_hashed = judged_signed_action_to_hashed(judged_create);

        let is_valid = judged_create
            .status
            .map(|s| s == ValidationStatus::Valid)
            .unwrap_or(true);

        if is_valid {
            actions.push(signed_action_hashed);
        } else {
            rejected_actions.push(signed_action_hashed);
        }
    }

    let deletes: Vec<SignedActionHashed> = entry_ops
        .deletes
        .iter()
        .map(judged_signed_action_to_hashed)
        .collect();

    let updates: Vec<SignedActionHashed> = entry_ops
        .updates
        .iter()
        .map(judged_signed_action_to_hashed)
        .collect();

    // Entry is Live if it has non-rejected creates and not all are deleted
    let entry_dht_status = if !actions.is_empty() && deletes.len() < actions.len() {
        EntryDhtStatus::Live
    } else if !actions.is_empty() {
        EntryDhtStatus::Dead
    } else {
        EntryDhtStatus::Live // Default to Live if no status info
    };

    let entry_details = EntryDetails {
        entry,
        actions,
        rejected_actions,
        deletes,
        updates,
        entry_dht_status,
    };

    let details = Details::Entry(entry_details);
    serde_json::to_value(&details).unwrap_or(serde_json::Value::Null)
}

/// Convert WireLinkOps to Vec<Link> with properly computed ActionHash.
///
/// This converts the wire protocol format to the same format that the conductor
/// returns, ensuring consistency.
///
/// # Tombstone-pair filtering happens in the HWC extension, not here.
///
/// `WireLinkOps.deletes` is deliberately ignored when building the response. The
/// HWC extension keeps a `{create, deleteHashes[]}` map keyed by base in
/// `packages/core/src/network/cache.ts` (see `mergeLinkDetailIntoCache` and
/// `addDeleteToLinkDetailsCache`) and derives the live link set on the client
/// side via `details.filter(d => d.deleteHashes.length === 0)` (covered by
/// `cascade-link-details.test.ts`). If we filtered here the extension would
/// have no way to learn about remote deletes via this endpoint, breaking that
/// contract. The full create+delete pairing is exposed via
/// `/dht/{dna}/details/{base}`, which `wire_entry_ops_to_details` populates.
fn wire_link_ops_to_links(
    ops: &holochain_types::link::WireLinkOps,
    base: &AnyLinkableHash,
    // In 0.6 the wire format could omit the tag as a size optimization, so a
    // fallback to the query's tag was needed. In 0.7 `creates` carries the
    // full signed `Action`, whose `CreateLinkData::tag` is always present, so
    // this fallback is no longer applicable. Kept for API stability with the
    // call site.
    _query_tag: Option<&LinkTag>,
) -> Vec<Link> {
    // Pass-through: deletes deliberately not applied — see function docstring.
    ops.creates
        .iter()
        .filter_map(|judged_create| {
            let action = judged_create.data.data();
            let ActionData::CreateLink(create_link) = &action.data else {
                return None;
            };

            let action_hash = ActionHash::with_data_sync(action);

            tracing::debug!(
                target = %create_link.target_address,
                computed_hash = %action_hash,
                "Converted wire CreateLink action to Link"
            );

            Some(Link {
                author: action.header.author.clone(),
                base: base.clone(),
                target: create_link.target_address.clone(),
                timestamp: action.header.timestamp,
                zome_index: create_link.zome_index,
                link_type: create_link.link_type,
                tag: create_link.tag.clone(),
                create_link_hash: action_hash,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use holochain_types::entry::{EntryData, WireEntryOps};
    use holochain_types::prelude::*;
    use holochain_types::record::WireRecordOps;
    use holochain_zome_types::judged::Judged;

    use super::{
        wire_entry_ops_to_details, wire_link_ops_to_links, wire_ops_to_details_json,
        wire_ops_to_record_json, wire_record_ops_to_details, wire_record_ops_to_record,
    };
    use holo_hash::HashableContentExtSync;

    fn test_agent() -> AgentPubKey {
        AgentPubKey::from_raw_32(vec![1; 32])
    }

    fn test_entry_hash() -> EntryHash {
        EntryHash::from_raw_32(vec![2; 32])
    }

    fn test_action_hash() -> ActionHash {
        ActionHash::from_raw_32(vec![3; 32])
    }

    fn test_prev_action() -> ActionHash {
        ActionHash::from_raw_32(vec![4; 32])
    }

    fn test_signature() -> Signature {
        Signature::from([0xaa; 64])
    }

    fn test_entry_type() -> EntryType {
        EntryType::App(AppEntryDef {
            entry_index: 0.into(),
            zome_index: 0.into(),
            visibility: EntryVisibility::Public,
        })
    }

    fn test_entry() -> Entry {
        let entry_bytes = UnsafeBytes::from(vec![1u8, 2, 3]);
        Entry::App(AppEntryBytes(SerializedBytes::from(entry_bytes)))
    }

    fn test_create_action() -> Action {
        Action {
            header: ActionHeader {
                author: test_agent(),
                timestamp: Timestamp::from_micros(1_000_000),
                action_seq: 5,
                prev_action: Some(test_prev_action()),
            },
            data: ActionData::Create(CreateData {
                entry_type: test_entry_type(),
                entry_hash: test_entry_hash(),
            }),
        }
    }

    /// A `Create` action for `test_entry()`, with `entry_hash` computed from
    /// the entry's real content.
    ///
    /// In 0.6 the wire format (`WireCreate`) omitted the entry hash — it was
    /// reconstructed from the served entry payload. In 0.7 wire responses
    /// carry the full signed `Action` (entry hash included) directly, so
    /// tests that need a create matching `test_entry()` just build it with
    /// the real hash up front instead of reconstructing it after the fact.
    fn test_create_action_for_entry() -> Action {
        Action {
            header: ActionHeader {
                author: test_agent(),
                timestamp: Timestamp::from_micros(1_000_000),
                action_seq: 5,
                prev_action: Some(test_prev_action()),
            },
            data: ActionData::Create(CreateData {
                entry_type: test_entry_type(),
                entry_hash: EntryHash::with_data_sync(&test_entry()),
            }),
        }
    }

    fn test_signed_action_for_entry() -> SignedAction {
        SignedAction::new(test_create_action_for_entry(), test_signature())
    }

    fn test_signed_create_for_entry() -> Judged<SignedAction> {
        Judged::valid(test_signed_action_for_entry())
    }

    fn test_delete_action() -> Action {
        Action {
            header: ActionHeader {
                author: test_agent(),
                timestamp: Timestamp::from_micros(2_000_000),
                action_seq: 6,
                prev_action: Some(test_action_hash()),
            },
            data: ActionData::Delete(DeleteData {
                deletes_address: test_action_hash(),
                deletes_entry_address: test_entry_hash(),
            }),
        }
    }

    fn test_signed_delete() -> Judged<SignedAction> {
        Judged::valid(SignedAction::new(
            test_delete_action(),
            Signature::from([0xbb; 64]),
        ))
    }

    fn test_update_action() -> Action {
        Action {
            header: ActionHeader {
                author: test_agent(),
                timestamp: Timestamp::from_micros(2_000_000),
                action_seq: 6,
                prev_action: Some(test_action_hash()),
            },
            data: ActionData::Update(UpdateData {
                original_action_address: test_action_hash(),
                original_entry_address: test_entry_hash(),
                entry_type: test_entry_type(),
                entry_hash: EntryHash::from_raw_32(vec![5; 32]),
            }),
        }
    }

    fn test_signed_update() -> Judged<SignedAction> {
        Judged::valid(SignedAction::new(
            test_update_action(),
            Signature::from([0xcc; 64]),
        ))
    }

    // ========================================================================
    // wire_record_ops_to_details tests
    // ========================================================================

    #[test]
    fn test_wire_record_ops_to_details_basic() {
        let action = test_create_action();
        let signed_action = SignedAction::new(action, test_signature());

        let record_ops = WireRecordOps {
            action: Some(Judged::valid(signed_action)),
            deletes: vec![],
            updates: vec![],
            entry: Some(test_entry()),
            warrants: vec![],
        };

        let result = wire_record_ops_to_details(&record_ops);

        let obj = result.as_object().expect("should be object");
        assert_eq!(obj["type"], "Record");

        let content = &obj["content"];
        assert!(content["record"].is_object());
        assert_eq!(content["validation_status"], "Valid");
        assert!(content["deletes"].as_array().unwrap().is_empty());
        assert!(content["updates"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_wire_record_ops_to_details_no_action_returns_null() {
        let record_ops = WireRecordOps {
            action: None,
            deletes: vec![],
            updates: vec![],
            entry: None,
            warrants: vec![],
        };

        let result = wire_record_ops_to_details(&record_ops);
        assert!(result.is_null());
    }

    #[test]
    fn test_wire_record_ops_to_details_with_deletes() {
        let action = test_create_action();
        let signed_action = SignedAction::new(action, test_signature());

        let record_ops = WireRecordOps {
            action: Some(Judged::valid(signed_action)),
            deletes: vec![test_signed_delete()],
            updates: vec![],
            entry: Some(test_entry()),
            warrants: vec![],
        };

        let result = wire_record_ops_to_details(&record_ops);

        let content = &result["content"];
        let deletes = content["deletes"].as_array().unwrap();
        assert_eq!(deletes.len(), 1);
    }

    #[test]
    fn test_wire_record_ops_to_details_with_updates() {
        let action = test_create_action();
        let signed_action = SignedAction::new(action, test_signature());

        let record_ops = WireRecordOps {
            action: Some(Judged::valid(signed_action)),
            deletes: vec![],
            updates: vec![test_signed_update()],
            entry: Some(test_entry()),
            warrants: vec![],
        };

        let result = wire_record_ops_to_details(&record_ops);

        let content = &result["content"];
        let updates = content["updates"].as_array().unwrap();
        assert_eq!(updates.len(), 1);
    }

    #[test]
    fn test_wire_record_ops_to_details_no_entry() {
        // Record with action but no entry (e.g., Delete action)
        let action = test_create_action();
        let signed_action = SignedAction::new(action, test_signature());

        let record_ops = WireRecordOps {
            action: Some(Judged::valid(signed_action)),
            deletes: vec![],
            updates: vec![],
            entry: None,
            warrants: vec![],
        };

        let result = wire_record_ops_to_details(&record_ops);

        // Should still produce valid Details::Record with null entry in the record
        let obj = result.as_object().expect("should be object");
        assert_eq!(obj["type"], "Record");
    }

    // ========================================================================
    // wire_record_ops_to_record tests (flat Record for get endpoint)
    // ========================================================================

    #[test]
    fn test_wire_record_ops_to_record_basic() {
        let action = test_create_action();
        let signed_action = SignedAction::new(action, test_signature());

        let record_ops = WireRecordOps {
            action: Some(Judged::valid(signed_action)),
            deletes: vec![],
            updates: vec![],
            entry: Some(test_entry()),
            warrants: vec![],
        };

        let result = wire_record_ops_to_record(&record_ops);

        // Should produce a flat Record with signed_action at top level
        let obj = result.as_object().expect("should be object");
        assert!(
            obj.contains_key("signed_action"),
            "Record JSON must have signed_action field, got keys: {:?}",
            obj.keys().collect::<Vec<_>>()
        );
        assert!(
            obj.contains_key("entry"),
            "Record JSON must have entry field"
        );

        // signed_action should have hashed.content and signature
        let sa = &obj["signed_action"];
        assert!(sa["hashed"]["content"].is_object());
        assert!(sa["hashed"]["hash"].is_array());
        assert!(sa["signature"].is_array());
    }

    #[test]
    fn test_wire_record_ops_to_record_no_action_returns_null() {
        let record_ops = WireRecordOps {
            action: None,
            deletes: vec![],
            updates: vec![],
            entry: None,
            warrants: vec![],
        };

        let result = wire_record_ops_to_record(&record_ops);
        assert!(result.is_null());
    }

    /// The action hash of `test_create_action_for_entry()` — needed when
    /// wiring up a matching delete in delete-filtering tests.
    fn expected_create_action_hash() -> ActionHash {
        test_create_action_for_entry().to_hash()
    }

    fn test_signed_delete_targeting_create() -> Judged<SignedAction> {
        let target = expected_create_action_hash();
        let real_entry_hash = EntryHash::with_data_sync(&test_entry());
        let action = Action {
            header: ActionHeader {
                author: test_agent(),
                timestamp: Timestamp::from_micros(2_000_000),
                action_seq: 6,
                prev_action: Some(test_action_hash()),
            },
            data: ActionData::Delete(DeleteData {
                deletes_address: target,
                deletes_entry_address: real_entry_hash,
            }),
        };
        Judged::valid(SignedAction::new(action, Signature::from([0xbb; 64])))
    }

    #[test]
    fn test_wire_ops_to_record_json_entry_variant_reconstructs_record() {
        let entry_data = EntryData {
            entry: test_entry(),
            entry_type: test_entry_type(),
        };
        let entry_ops = WireEntryOps {
            creates: vec![test_signed_create_for_entry()],
            deletes: vec![],
            updates: vec![],
            entry: Some(entry_data),
            warrants: vec![],
        };

        let result = wire_ops_to_record_json(&WireOps::Entry(entry_ops));
        let obj = result
            .as_object()
            .expect("entry-keyed get should reconstruct a Record JSON object");
        assert!(
            obj.contains_key("signed_action"),
            "Record JSON must have signed_action field, got keys: {:?}",
            obj.keys().collect::<Vec<_>>()
        );
        assert!(
            obj.contains_key("entry"),
            "Record JSON must have entry field"
        );

        // The signed_action must carry both the hashed action and a signature.
        let sa = &obj["signed_action"];
        assert!(sa["hashed"]["content"].is_object());
        assert!(sa["hashed"]["hash"].is_array());
        assert!(sa["signature"].is_array());
    }

    #[test]
    fn test_wire_ops_to_record_json_entry_variant_no_entry_returns_null() {
        // No entry data => nothing to reconstruct.
        let entry_ops = WireEntryOps {
            creates: vec![test_signed_create_for_entry()],
            deletes: vec![],
            updates: vec![],
            entry: None,
            warrants: vec![],
        };
        let result = wire_ops_to_record_json(&WireOps::Entry(entry_ops));
        assert!(
            result.is_null(),
            "Entry variant with no entry payload must return null"
        );
    }

    #[test]
    fn test_wire_ops_to_record_json_entry_variant_all_creates_deleted_returns_null() {
        // The single create has been tombstoned by an entry-delete pointing
        // at its action hash; treat the entry as not-found, matching
        // holochain's `get` semantics for a fully-deleted entry.
        let entry_data = EntryData {
            entry: test_entry(),
            entry_type: test_entry_type(),
        };
        let entry_ops = WireEntryOps {
            creates: vec![test_signed_create_for_entry()],
            deletes: vec![test_signed_delete_targeting_create()],
            updates: vec![],
            entry: Some(entry_data),
            warrants: vec![],
        };
        let result = wire_ops_to_record_json(&WireOps::Entry(entry_ops));
        assert!(
            result.is_null(),
            "fully-deleted entry must return null, got: {result}"
        );
    }

    #[test]
    fn test_wire_ops_to_record_json_entry_variant_skips_invalid_creates() {
        // Only a rejected create exists — the response should be null
        // since holochain treats rejected creates as not-yet-valid sources
        // for the entry.
        let entry_data = EntryData {
            entry: test_entry(),
            entry_type: test_entry_type(),
        };
        let entry_ops = WireEntryOps {
            creates: vec![Judged::new(
                test_signed_action_for_entry(),
                ValidationStatus::Rejected,
            )],
            deletes: vec![],
            updates: vec![],
            entry: Some(entry_data),
            warrants: vec![],
        };
        let result = wire_ops_to_record_json(&WireOps::Entry(entry_ops));
        assert!(
            result.is_null(),
            "rejected-only creates must return null, got: {result}"
        );
    }

    #[test]
    fn test_wire_ops_to_record_json_record_variant() {
        let action = test_create_action();
        let signed_action = SignedAction::new(action, test_signature());

        let record_ops = WireRecordOps {
            action: Some(Judged::valid(signed_action)),
            deletes: vec![],
            updates: vec![],
            entry: Some(test_entry()),
            warrants: vec![],
        };

        let result = wire_ops_to_record_json(&WireOps::Record(record_ops));
        let obj = result.as_object().expect("should be object");
        assert!(obj.contains_key("signed_action"));
    }

    // ========================================================================
    // wire_entry_ops_to_details tests
    // ========================================================================

    #[test]
    fn test_wire_entry_ops_to_details_basic() {
        let entry_data = EntryData {
            entry: test_entry(),
            entry_type: test_entry_type(),
        };

        let entry_ops = WireEntryOps {
            creates: vec![test_signed_create_for_entry()],
            deletes: vec![],
            updates: vec![],
            entry: Some(entry_data),
            warrants: vec![],
        };

        let result = wire_entry_ops_to_details(&entry_ops);

        let obj = result.as_object().expect("should be object");
        assert_eq!(obj["type"], "Entry");

        let content = &obj["content"];
        assert!(content["entry"].is_object());
        assert_eq!(content["actions"].as_array().unwrap().len(), 1);
        assert!(content["rejected_actions"].as_array().unwrap().is_empty());
        assert!(content["deletes"].as_array().unwrap().is_empty());
        assert!(content["updates"].as_array().unwrap().is_empty());
        assert_eq!(content["entry_dht_status"], "Live");
    }

    #[test]
    fn test_wire_entry_ops_to_details_no_entry_returns_null() {
        let entry_ops = WireEntryOps {
            creates: vec![],
            deletes: vec![],
            updates: vec![],
            entry: None,
            warrants: vec![],
        };

        let result = wire_entry_ops_to_details(&entry_ops);
        assert!(result.is_null());
    }

    #[test]
    fn test_wire_entry_ops_to_details_dead_status() {
        let entry_data = EntryData {
            entry: test_entry(),
            entry_type: test_entry_type(),
        };

        // One create + one delete → Dead (deletes >= actions)
        let entry_ops = WireEntryOps {
            creates: vec![test_signed_create_for_entry()],
            deletes: vec![test_signed_delete()],
            updates: vec![],
            entry: Some(entry_data),
            warrants: vec![],
        };

        let result = wire_entry_ops_to_details(&entry_ops);

        let content = &result["content"];
        assert_eq!(content["entry_dht_status"], "Dead");
    }

    #[test]
    fn test_wire_entry_ops_to_details_with_rejected_creates() {
        let rejected_create_action = Action {
            header: ActionHeader {
                author: AgentPubKey::from_raw_32(vec![9; 32]),
                timestamp: Timestamp::from_micros(2_000_000),
                action_seq: 3,
                prev_action: Some(ActionHash::from_raw_32(vec![8; 32])),
            },
            data: ActionData::Create(CreateData {
                entry_type: test_entry_type(),
                entry_hash: EntryHash::with_data_sync(&test_entry()),
            }),
        };

        let entry_data = EntryData {
            entry: test_entry(),
            entry_type: test_entry_type(),
        };

        let entry_ops = WireEntryOps {
            creates: vec![
                test_signed_create_for_entry(),
                Judged::new(
                    SignedAction::new(rejected_create_action, Signature::from([0xcc; 64])),
                    ValidationStatus::Rejected,
                ),
            ],
            deletes: vec![],
            updates: vec![],
            entry: Some(entry_data),
            warrants: vec![],
        };

        let result = wire_entry_ops_to_details(&entry_ops);

        let content = &result["content"];
        assert_eq!(content["actions"].as_array().unwrap().len(), 1);
        assert_eq!(content["rejected_actions"].as_array().unwrap().len(), 1);
        assert_eq!(content["entry_dht_status"], "Live");
    }

    #[test]
    fn test_wire_entry_ops_to_details_with_update_create() {
        // An entry-creating Update action (entry created via an update rather
        // than a create).
        let update_action = Action {
            header: ActionHeader {
                author: test_agent(),
                timestamp: Timestamp::from_micros(1_000_000),
                action_seq: 5,
                prev_action: Some(test_prev_action()),
            },
            data: ActionData::Update(UpdateData {
                original_action_address: ActionHash::from_raw_32(vec![8; 32]),
                original_entry_address: EntryHash::from_raw_32(vec![7; 32]),
                entry_type: test_entry_type(),
                entry_hash: EntryHash::with_data_sync(&test_entry()),
            }),
        };

        let entry_data = EntryData {
            entry: test_entry(),
            entry_type: test_entry_type(),
        };

        let entry_ops = WireEntryOps {
            creates: vec![Judged::valid(SignedAction::new(
                update_action,
                test_signature(),
            ))],
            deletes: vec![],
            updates: vec![],
            entry: Some(entry_data),
            warrants: vec![],
        };

        let result = wire_entry_ops_to_details(&entry_ops);

        let obj = result.as_object().expect("should be object");
        assert_eq!(obj["type"], "Entry");

        let content = &obj["content"];
        assert_eq!(content["actions"].as_array().unwrap().len(), 1);
        assert_eq!(content["entry_dht_status"], "Live");
    }

    #[test]
    fn test_wire_entry_ops_to_details_with_deletes_and_updates() {
        let entry_data = EntryData {
            entry: test_entry(),
            entry_type: test_entry_type(),
        };

        // Two creates, one delete (still Live), one update
        let create2_action = Action {
            header: ActionHeader {
                author: AgentPubKey::from_raw_32(vec![6; 32]),
                timestamp: Timestamp::from_micros(1_500_000),
                action_seq: 3,
                prev_action: Some(ActionHash::from_raw_32(vec![7; 32])),
            },
            data: ActionData::Create(CreateData {
                entry_type: test_entry_type(),
                entry_hash: EntryHash::with_data_sync(&test_entry()),
            }),
        };

        let entry_ops = WireEntryOps {
            creates: vec![
                test_signed_create_for_entry(),
                Judged::valid(SignedAction::new(
                    create2_action,
                    Signature::from([0xdd; 64]),
                )),
            ],
            deletes: vec![test_signed_delete()],
            updates: vec![test_signed_update()],
            entry: Some(entry_data),
            warrants: vec![],
        };

        let result = wire_entry_ops_to_details(&entry_ops);

        let content = &result["content"];
        assert_eq!(content["actions"].as_array().unwrap().len(), 2);
        assert_eq!(content["deletes"].as_array().unwrap().len(), 1);
        assert_eq!(content["updates"].as_array().unwrap().len(), 1);
        // 2 creates, 1 delete → still Live (deletes < actions)
        assert_eq!(content["entry_dht_status"], "Live");
    }

    // ========================================================================
    // wire_ops_to_details_json dispatcher tests
    // ========================================================================

    #[test]
    fn test_wire_ops_to_details_json_record_variant() {
        let action = test_create_action();
        let signed_action = SignedAction::new(action, test_signature());

        let record_ops = WireRecordOps {
            action: Some(Judged::valid(signed_action)),
            deletes: vec![],
            updates: vec![],
            entry: Some(test_entry()),
            warrants: vec![],
        };

        let result = wire_ops_to_details_json(&WireOps::Record(record_ops));
        assert_eq!(result["type"], "Record");
    }

    #[test]
    fn test_wire_ops_to_details_json_entry_variant() {
        let entry_data = EntryData {
            entry: test_entry(),
            entry_type: test_entry_type(),
        };

        let entry_ops = WireEntryOps {
            creates: vec![test_signed_create_for_entry()],
            deletes: vec![],
            updates: vec![],
            entry: Some(entry_data),
            warrants: vec![],
        };

        let result = wire_ops_to_details_json(&WireOps::Entry(entry_ops));
        assert_eq!(result["type"], "Entry");
    }

    // ========================================================================
    // wire_link_ops_to_links tests
    // ========================================================================

    fn test_create_link_action(fill: u8, tag_bytes: Vec<u8>, base: AnyLinkableHash) -> Action {
        Action {
            header: ActionHeader {
                author: test_agent(),
                timestamp: Timestamp::from_micros(1_000_000 + fill as i64),
                action_seq: 5 + fill as u32,
                prev_action: Some(ActionHash::from_raw_32(vec![10 + fill; 32])),
            },
            data: ActionData::CreateLink(CreateLinkData {
                base_address: base,
                target_address: AnyLinkableHash::from(EntryHash::from_raw_32(vec![20 + fill; 32])),
                zome_index: 0.into(),
                link_type: 0.into(),
                tag: LinkTag::new(tag_bytes),
            }),
        }
    }

    fn test_signed_create_link(
        fill: u8,
        tag_bytes: Vec<u8>,
        base: AnyLinkableHash,
    ) -> Judged<SignedAction> {
        Judged::valid(SignedAction::new(
            test_create_link_action(fill, tag_bytes, base),
            Signature::from([0xaa; 64]),
        ))
    }

    #[test]
    fn test_wire_link_ops_to_links_basic() {
        let base = AnyLinkableHash::from(EntryHash::from_raw_32(vec![1; 32]));
        let create = test_signed_create_link(0, vec![0xde, 0xad], base.clone());

        let ops = WireLinkOps {
            creates: vec![create.clone()],
            deletes: vec![],
            warrants: vec![],
        };

        let links = wire_link_ops_to_links(&ops, &base, None);
        assert_eq!(links.len(), 1);

        let ActionData::CreateLink(create_link) = &create.data.data().data else {
            panic!("expected a CreateLink action");
        };
        assert_eq!(links[0].target, create_link.target_address);
        assert_eq!(links[0].tag, create_link.tag);
    }

    /// Tombstone-paired create-links must be passed through unchanged. The HWC
    /// extension's link-details cache (packages/core/src/network/cache.ts)
    /// owns the create-vs-delete pairing and derives the live set on the
    /// client side. Filtering here would break that contract — so
    /// wire_link_ops_to_links must NOT drop creates whose action hash
    /// appears in `deletes[].link_add_address`.
    #[test]
    fn test_wire_link_ops_to_links_passes_through_creates_with_matching_deletes() {
        let base = AnyLinkableHash::from(EntryHash::from_raw_32(vec![1; 32]));
        let create_kept = test_signed_create_link(0, vec![0xaa], base.clone());
        let create_tombstoned = test_signed_create_link(1, vec![0xbb], base.clone());
        let tombstone_target = create_tombstoned.data.data().to_hash();

        let delete_action = Action {
            header: ActionHeader {
                author: test_agent(),
                timestamp: Timestamp::from_micros(2_000_000),
                action_seq: 10,
                prev_action: Some(test_action_hash()),
            },
            data: ActionData::DeleteLink(DeleteLinkData {
                base_address: base.clone(),
                link_add_address: tombstone_target,
            }),
        };
        let delete = Judged::valid(SignedAction::new(
            delete_action,
            Signature::from([0xcc; 64]),
        ));

        let ops = WireLinkOps {
            creates: vec![create_kept, create_tombstoned],
            deletes: vec![delete],
            warrants: vec![],
        };

        let links = wire_link_ops_to_links(&ops, &base, None);
        assert_eq!(
            links.len(),
            2,
            "linker must pass both creates through; the extension's link-details cache filters deletes"
        );
    }
}
