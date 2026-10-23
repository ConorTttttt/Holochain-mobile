//! Direct DHT queries via kitsune2 wire protocol.
//!
//! This module provides direct DHT queries (get, get_links) by sending
//! wire protocol messages to DHT authorities, bypassing the conductor.
//!
//! # Architecture
//!
//! ```text
//! DhtQuery
//!    │
//!    ├── Finds peers near hash location via peer_store
//!    ├── Sends WireMessage::GetReq/GetLinksReq via space.send_notify()
//!    │
//!    ▼
//! Remote Authority
//!    │
//!    ├── Receives request, queries local DHT
//!    ├── Sends WireMessage::GetRes/GetLinksRes back
//!    │
//!    ▼
//! ProxySpaceHandler.recv_notify()
//!    │
//!    └── Routes response via shared PendingDhtResponses
//! ```

use holo_hash::{AnyDhtHash, AnyLinkableHash};
use holochain_p2p::event::GetActivityOptions;
use holochain_p2p::WireMessage;
use holochain_types::activity::AgentActivityResponse;
use holochain_types::chain::MustGetAgentActivityResponse;
use holochain_types::link::{CountLinksResponse, WireLinkKey, WireLinkOps, WireLinkQuery};
use holochain_types::prelude::{AgentPubKey, ChainFilter, DnaHash, WireOps};
use holochain_zome_types::query::{ChainQueryFilter, ChainStatus};
use kitsune2_api::{DynSpace, Url};
use kitsune2_core::get_responsive_remote_agents_near_location;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, RwLock};

use crate::temp_op_store::TempOpStoreHandle;
use tracing::{debug, info, warn};

use crate::error::{LinkerError, LinkerResult};
use crate::gateway_kitsune::GatewayKitsune;

/// Number of peers to query in parallel (same as holochain_p2p)
const PARALLEL_GET_AGENTS_COUNT: usize = 3;

/// Default timeout for DHT queries
const DEFAULT_QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Pending request tracker
type PendingResponder = oneshot::Sender<WireMessage>;

/// Shared storage for pending DHT query responses.
///
/// This is shared between DhtQuery (which registers pending requests)
/// and ProxySpaceHandler (which routes incoming responses).
#[derive(Clone, Default)]
pub struct PendingDhtResponses {
    inner: Arc<RwLock<HashMap<u64, PendingResponder>>>,
}

impl std::fmt::Debug for PendingDhtResponses {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingDhtResponses").finish()
    }
}

impl PendingDhtResponses {
    /// Create a new empty pending responses tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pending request.
    pub async fn register(&self, msg_id: u64, responder: PendingResponder) {
        let mut guard = self.inner.write().await;
        debug!(
            msg_id,
            pending_count = guard.len(),
            "Registering pending DHT request"
        );
        guard.insert(msg_id, responder);
    }

    /// Remove a pending request (for cleanup on timeout).
    pub async fn remove(&self, msg_id: u64) {
        let mut guard = self.inner.write().await;
        let existed = guard.remove(&msg_id).is_some();
        debug!(
            msg_id,
            existed,
            pending_count = guard.len(),
            "Removed pending DHT request (timeout cleanup)"
        );
    }

    /// Route a response to its pending request.
    ///
    /// Returns true if the response was routed, false if no pending request was found.
    pub async fn route_response(&self, msg: WireMessage) -> bool {
        let msg_id = match msg.get_msg_id() {
            Some(id) => id,
            None => {
                warn!("Cannot route response without msg_id");
                return false;
            }
        };

        info!(msg_id, "Attempting to route response to pending request");

        let (responder, pending_count, pending_ids) = {
            let mut guard = self.inner.write().await;
            let pending_ids: Vec<u64> = guard.keys().copied().collect();
            let r = guard.remove(&msg_id);
            (r, guard.len(), pending_ids)
        };

        info!(
            msg_id,
            pending_count,
            ?pending_ids,
            "Pending requests state before routing"
        );

        match responder {
            Some(tx) => {
                info!(
                    msg_id,
                    "Found matching pending request, sending response..."
                );
                if tx.send(msg).is_err() {
                    warn!(
                        msg_id,
                        "Pending request receiver dropped (caller cancelled)"
                    );
                }
                true
            }
            None => {
                warn!(
                    msg_id,
                    pending_count,
                    ?pending_ids,
                    "No pending request for msg_id - response dropped (possibly timed out or wrong instance)"
                );
                false
            }
        }
    }
}

/// DHT query handler for direct wire protocol queries.
///
/// This allows the gateway to query the DHT directly via kitsune2,
/// without going through the conductor.
#[derive(Clone)]
pub struct DhtQuery {
    /// Gateway kitsune manager for space access
    gateway_kitsune: GatewayKitsune,
    /// Shared pending requests (also used by ProxySpaceHandler for response routing)
    pending: PendingDhtResponses,
    /// Query timeout
    timeout: Duration,
    /// This gateway's local publish buffer, consulted as a fallback whenever kitsune2
    /// finds no remote peer (or no peer has the data) so that a `get`/`get_links` from
    /// one hosted zero-arc agent can still be answered directly from another hosted
    /// agent's still-buffered publish. A real remote response always takes priority
    /// when one is found. See `crate::local_authority` module docs for why this is
    /// needed: co-located zero-arc agents can never see each other via kitsune2's
    /// normal peer discovery. `None` when no `TempOpStore` is configured (e.g. some
    /// test constructions).
    local_ops: Option<TempOpStoreHandle>,
}

impl std::fmt::Debug for DhtQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DhtQuery")
            .field("pending", &self.pending)
            .finish()
    }
}

impl DhtQuery {
    /// Create a new DhtQuery handler with shared pending responses.
    ///
    /// The `pending` should be the same instance passed to KitsuneProxy
    /// so that response routing works correctly.
    pub fn new(gateway_kitsune: GatewayKitsune, pending: PendingDhtResponses) -> Self {
        Self {
            gateway_kitsune,
            pending,
            timeout: DEFAULT_QUERY_TIMEOUT,
            local_ops: None,
        }
    }

    /// Create a new DhtQuery handler with custom timeout.
    pub fn with_timeout(
        gateway_kitsune: GatewayKitsune,
        pending: PendingDhtResponses,
        timeout: Duration,
    ) -> Self {
        Self {
            gateway_kitsune,
            pending,
            timeout,
            local_ops: None,
        }
    }

    /// Attach this gateway's `TempOpStore` so `get`/`get_links` can serve co-located
    /// zero-arc agents' freshly published ops directly. See `crate::local_authority`.
    pub fn with_local_ops(mut self, local_ops: TempOpStoreHandle) -> Self {
        self.local_ops = Some(local_ops);
        self
    }

    /// Get a record from the DHT by hash.
    ///
    /// Queries multiple peers near the hash location in parallel and returns
    /// the first non-empty response.
    pub async fn get(&self, dna_hash: &DnaHash, hash: AnyDhtHash) -> LinkerResult<Option<WireOps>> {
        let space = self
            .gateway_kitsune
            .get_or_create_space(dna_hash)
            .await
            .map_err(LinkerError::Internal)?;

        let loc = hash.get_loc();
        let agents = self.get_peers_for_location(&space, loc).await?;

        if agents.is_empty() {
            info!(dna = %dna_hash, loc, "No peers found for DHT location");
            return Ok(self.local_get_logged(&hash).await);
        }

        debug!(
            dna = %dna_hash,
            hash = %hash,
            peer_count = agents.len(),
            "Querying peers for get"
        );

        // Query peers in parallel, return first non-empty response
        let mut handles = Vec::new();
        for (agent, url) in agents.into_iter().take(PARALLEL_GET_AGENTS_COUNT) {
            let space = space.clone();
            let hash = hash.clone();
            let pending = self.pending.clone();
            let timeout = self.timeout;

            handles.push(tokio::spawn(async move {
                Self::send_get_request(space, agent, url, hash, pending, timeout).await
            }));
        }

        // Await all and return first non-empty success
        let mut outcome = PeerRaceOutcome::default();
        for handle in handles {
            match handle.await {
                Ok(Ok(Some(wire_ops))) if !is_wire_ops_empty(&wire_ops) => {
                    return Ok(Some(wire_ops));
                }
                Ok(Ok(result)) => {
                    outcome.record_ok(result);
                }
                Ok(Err(e)) => {
                    debug!(?e, "Peer query failed");
                    outcome.record_err(e);
                }
                Err(e) => {
                    debug!(?e, "Task join error");
                }
            }
        }

        // No peer returned non-empty data (see PeerRaceOutcome doc comment for
        // why this no longer depends on completion order) - try the
        // local-authority fallback before giving up.
        if let Some(local) = self.local_get_logged(&hash).await {
            return Ok(Some(local));
        }
        outcome.into_result()
    }

    /// Get links from the DHT by base hash.
    ///
    /// Queries multiple peers near the base hash location in parallel and returns
    /// the first non-empty response.
    pub async fn get_links(
        &self,
        dna_hash: &DnaHash,
        link_key: WireLinkKey,
    ) -> LinkerResult<Option<WireLinkOps>> {
        let space = self
            .gateway_kitsune
            .get_or_create_space(dna_hash)
            .await
            .map_err(LinkerError::Internal)?;

        let loc = link_key.base.get_loc();
        let agents = self.get_peers_for_location(&space, loc).await?;

        if agents.is_empty() {
            info!(dna = %dna_hash, loc, "No peers found for DHT location");
            return Ok(self.local_get_links_logged(&link_key).await);
        }

        debug!(
            dna = %dna_hash,
            base = %link_key.base,
            peer_count = agents.len(),
            "Querying peers for get_links"
        );

        // Query peers in parallel, return first non-empty response
        let mut handles = Vec::new();
        for (agent, url) in agents.into_iter().take(PARALLEL_GET_AGENTS_COUNT) {
            let space = space.clone();
            let link_key = link_key.clone();
            let pending = self.pending.clone();
            let timeout = self.timeout;

            handles.push(tokio::spawn(async move {
                Self::send_get_links_request(space, agent, url, link_key, pending, timeout).await
            }));
        }

        // Await all and return first non-empty success
        let mut outcome = PeerRaceOutcome::default();
        for handle in handles {
            match handle.await {
                Ok(Ok(Some(wire_link_ops))) if !wire_link_ops.creates.is_empty() => {
                    return Ok(Some(wire_link_ops));
                }
                Ok(Ok(result)) => {
                    outcome.record_ok(result);
                }
                Ok(Err(e)) => {
                    debug!(?e, "Peer query failed");
                    outcome.record_err(e);
                }
                Err(e) => {
                    debug!(?e, "Task join error");
                }
            }
        }

        // No peer returned non-empty data - try the local-authority fallback
        // before giving up (see PeerRaceOutcome doc comment).
        if let Some(local) = self.local_get_links_logged(&link_key).await {
            return Ok(Some(local));
        }
        outcome.into_result()
    }

    /// Count links from the DHT by base hash.
    ///
    /// Sends CountLinksReq to peers near the base hash location and returns
    /// the first response.
    pub async fn count_links(
        &self,
        dna_hash: &DnaHash,
        query: WireLinkQuery,
    ) -> LinkerResult<Option<CountLinksResponse>> {
        let space = self
            .gateway_kitsune
            .get_or_create_space(dna_hash)
            .await
            .map_err(LinkerError::Internal)?;

        let loc = query.base.get_loc();
        let agents = self.get_peers_for_location(&space, loc).await?;

        if agents.is_empty() {
            info!(dna = %dna_hash, loc, "No peers found for DHT location");
            return Ok(None);
        }

        debug!(
            dna = %dna_hash,
            base = %query.base,
            peer_count = agents.len(),
            "Querying peers for count_links"
        );

        // Query peers in parallel, return first response
        let mut handles = Vec::new();
        for (agent, url) in agents.into_iter().take(PARALLEL_GET_AGENTS_COUNT) {
            let space = space.clone();
            let query = query.clone();
            let pending = self.pending.clone();
            let timeout = self.timeout;

            handles.push(tokio::spawn(async move {
                Self::send_count_links_request(space, agent, url, query, pending, timeout).await
            }));
        }

        // Await all and return first success
        let mut outcome = PeerRaceOutcome::default();
        for handle in handles {
            match handle.await {
                Ok(Ok(Some(response))) => {
                    return Ok(Some(response));
                }
                Ok(Ok(result)) => {
                    outcome.record_ok(result);
                }
                Ok(Err(e)) => {
                    debug!(?e, "Peer count_links query failed");
                    outcome.record_err(e);
                }
                Err(e) => {
                    debug!(?e, "Task join error");
                }
            }
        }

        outcome.into_result()
    }

    /// Serve a `get` from this gateway's local publish buffer, if it holds a matching
    /// `Create*` op. See `crate::local_authority` module docs.
    async fn local_get_logged(&self, hash: &AnyDhtHash) -> Option<WireOps> {
        let local_ops = self.local_ops.as_ref()?;
        let basis = AnyLinkableHash::from(hash.clone());
        let ops = local_ops.decode_ops_at_basis(&basis).await;
        if ops.is_empty() {
            return None;
        }
        let wire_ops = crate::local_authority::wire_ops_for_get(ops)?;
        debug!(%hash, "Served get from local publish buffer (co-located agent)");
        Some(wire_ops)
    }

    /// Serve a `get_links` fallback from this gateway's local publish buffer, if it
    /// holds any matching `CreateLink` ops. Only called once a remote DHT authority
    /// query has already come back empty. See `crate::local_authority` module docs.
    async fn local_get_links_logged(&self, link_key: &WireLinkKey) -> Option<WireLinkOps> {
        let local_ops = self.local_ops.as_ref()?;
        let ops = local_ops.decode_ops_at_basis(&link_key.base).await;
        if ops.is_empty() {
            return None;
        }
        let wire_link_ops = crate::local_authority::wire_link_ops_for_links(ops, link_key);
        if wire_link_ops.creates.is_empty() {
            return None;
        }
        debug!(
            base = %link_key.base,
            "Served get_links from local publish buffer (co-located agent)"
        );
        Some(wire_link_ops)
    }

    /// Find peers near a given DHT location.
    async fn get_peers_for_location(
        &self,
        space: &DynSpace,
        loc: u32,
    ) -> LinkerResult<Vec<(AgentPubKey, Url)>> {
        // First log what we have in peer store
        match space.peer_store().get_all().await {
            Ok(all_peers) => {
                info!(
                    peer_count = all_peers.len(),
                    loc, "Peer store contents for DHT query"
                );
                for peer in &all_peers {
                    debug!(
                        agent = ?peer.agent,
                        url = ?peer.url,
                        arc = ?peer.storage_arc,
                        is_tombstone = peer.is_tombstone,
                        contains_loc = peer.storage_arc.contains(loc),
                        "Peer in store"
                    );
                }
            }
            Err(e) => {
                warn!(?e, "Failed to get all peers for debug");
            }
        }

        // Also log local agents
        match space.local_agent_store().get_all().await {
            Ok(local_agents) => {
                info!(
                    local_agent_count = local_agents.len(),
                    "Local agents in space"
                );
            }
            Err(e) => {
                warn!(?e, "Failed to get local agents for debug");
            }
        }

        let agents = get_responsive_remote_agents_near_location(
            space.peer_store().clone(),
            space.local_agent_store().clone(),
            space.peer_meta_store().clone(),
            loc,
            1024,
        )
        .await
        .map_err(|e| LinkerError::Internal(format!("Failed to get peers: {e}")))?;

        info!(
            responsive_count = agents.len(),
            loc, "Found responsive remote agents"
        );

        Ok(agents
            .into_iter()
            .filter_map(|a| {
                if a.url.is_none() || a.is_tombstone {
                    debug!(agent = ?a.agent, "Skipping agent (no url or tombstone)");
                    return None;
                }
                if !a.storage_arc.contains(loc) {
                    debug!(agent = ?a.agent, arc = ?a.storage_arc, loc, "Skipping agent (arc doesn't contain loc)");
                    return None;
                }
                Some((
                    AgentPubKey::from_k2_agent(&a.agent),
                    a.url.as_ref().unwrap().clone(),
                ))
            })
            .collect())
    }

    /// Send a get request to a specific peer.
    async fn send_get_request(
        space: DynSpace,
        to_agent: AgentPubKey,
        to_url: Url,
        hash: AnyDhtHash,
        pending: PendingDhtResponses,
        timeout: Duration,
    ) -> LinkerResult<Option<WireOps>> {
        let (msg_id, req) = WireMessage::get_req(to_agent.clone(), hash.clone());

        info!(
            msg_id,
            %to_url,
            %to_agent,
            %hash,
            "Preparing GetReq message"
        );

        let encoded = WireMessage::encode_batch(&[&req])
            .map_err(|e| LinkerError::Internal(format!("Failed to encode request: {e}")))?;

        info!(
            msg_id,
            encoded_len = encoded.len(),
            "Encoded GetReq message"
        );

        // Register pending request
        let (tx, rx) = oneshot::channel();
        pending.register(msg_id, tx).await;

        info!(
            msg_id,
            "Registered pending request, sending via send_notify..."
        );

        // Set up timeout to clean up pending request
        let pending_cleanup = pending.clone();
        let timeout_msg_id = msg_id;
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            warn!(
                msg_id = timeout_msg_id,
                "Timeout cleanup triggered for pending request"
            );
            pending_cleanup.remove(timeout_msg_id).await;
        });

        // Send request - this is the key step
        let send_start = std::time::Instant::now();
        match space.send_notify(to_url.clone(), encoded).await {
            Ok(()) => {
                info!(
                    msg_id,
                    %to_url,
                    %to_agent,
                    send_elapsed_ms = send_start.elapsed().as_millis(),
                    "send_notify completed successfully, waiting for response..."
                );
            }
            Err(e) => {
                warn!(
                    msg_id,
                    %to_url,
                    %to_agent,
                    error = %e,
                    "send_notify FAILED"
                );
                return Err(LinkerError::Internal(format!(
                    "Failed to send request: {e}"
                )));
            }
        }

        // Wait for response
        info!(
            msg_id,
            timeout_secs = timeout.as_secs(),
            "Waiting for response..."
        );
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(WireMessage::GetRes { response, .. })) => {
                info!(msg_id, "Got GetRes response");
                Ok(Some(response))
            }
            Ok(Ok(WireMessage::ErrorRes { error, .. })) => {
                warn!(msg_id, %error, "Got ErrorRes response");
                Err(LinkerError::Internal(format!("Remote error: {error}")))
            }
            Ok(Ok(other)) => {
                warn!(msg_id, ?other, "Got unexpected response type");
                Err(LinkerError::Internal(format!(
                    "Unexpected response: {other:?}"
                )))
            }
            Ok(Err(_)) => {
                warn!(msg_id, "Response channel closed (receiver dropped)");
                Err(LinkerError::Internal("Response channel closed".to_string()))
            }
            Err(_) => {
                warn!(
                    msg_id,
                    timeout_secs = timeout.as_secs(),
                    "Request TIMED OUT - no response received"
                );
                Err(LinkerError::Internal("Request timed out".to_string()))
            }
        }
    }

    /// Send a get_links request to a specific peer.
    async fn send_get_links_request(
        space: DynSpace,
        to_agent: AgentPubKey,
        to_url: Url,
        link_key: WireLinkKey,
        pending: PendingDhtResponses,
        timeout: Duration,
    ) -> LinkerResult<Option<WireLinkOps>> {
        use holochain_p2p::event::GetLinksOptions;

        let (msg_id, req) =
            WireMessage::get_links_req(to_agent.clone(), link_key.clone(), GetLinksOptions {});

        info!(
            msg_id,
            %to_url,
            %to_agent,
            base = %link_key.base,
            "Preparing GetLinksReq message"
        );

        let encoded = WireMessage::encode_batch(&[&req])
            .map_err(|e| LinkerError::Internal(format!("Failed to encode request: {e}")))?;

        info!(
            msg_id,
            encoded_len = encoded.len(),
            "Encoded GetLinksReq message"
        );

        // Register pending request
        let (tx, rx) = oneshot::channel();
        pending.register(msg_id, tx).await;

        info!(
            msg_id,
            "Registered pending request, sending via send_notify..."
        );

        // Set up timeout to clean up pending request
        let pending_cleanup = pending.clone();
        let timeout_msg_id = msg_id;
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            warn!(
                msg_id = timeout_msg_id,
                "Timeout cleanup triggered for pending get_links request"
            );
            pending_cleanup.remove(timeout_msg_id).await;
        });

        // Send request - this is the key step
        let send_start = std::time::Instant::now();
        match space.send_notify(to_url.clone(), encoded).await {
            Ok(()) => {
                info!(
                    msg_id,
                    %to_url,
                    %to_agent,
                    send_elapsed_ms = send_start.elapsed().as_millis(),
                    "send_notify completed successfully, waiting for response..."
                );
            }
            Err(e) => {
                warn!(
                    msg_id,
                    %to_url,
                    %to_agent,
                    error = %e,
                    "send_notify FAILED"
                );
                return Err(LinkerError::Internal(format!(
                    "Failed to send request: {e}"
                )));
            }
        }

        // Wait for response
        info!(
            msg_id,
            timeout_secs = timeout.as_secs(),
            "Waiting for get_links response..."
        );
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(WireMessage::GetLinksRes { response, .. })) => {
                info!(
                    msg_id,
                    creates_count = response.creates.len(),
                    deletes_count = response.deletes.len(),
                    "Got GetLinksRes response"
                );
                Ok(Some(response))
            }
            Ok(Ok(WireMessage::ErrorRes { error, .. })) => {
                warn!(msg_id, %error, "Got ErrorRes response");
                Err(LinkerError::Internal(format!("Remote error: {error}")))
            }
            Ok(Ok(other)) => {
                warn!(msg_id, ?other, "Got unexpected response type");
                Err(LinkerError::Internal(format!(
                    "Unexpected response: {other:?}"
                )))
            }
            Ok(Err(_)) => {
                warn!(msg_id, "Response channel closed (receiver dropped)");
                Err(LinkerError::Internal("Response channel closed".to_string()))
            }
            Err(_) => {
                warn!(
                    msg_id,
                    timeout_secs = timeout.as_secs(),
                    "GetLinks request TIMED OUT - no response received"
                );
                Err(LinkerError::Internal("Request timed out".to_string()))
            }
        }
    }

    /// Get agent activity from the DHT.
    ///
    /// Queries multiple peers near the agent's DHT location in parallel and returns
    /// the first non-empty response.
    pub async fn get_agent_activity(
        &self,
        dna_hash: &DnaHash,
        agent: AgentPubKey,
        query: ChainQueryFilter,
        options: GetActivityOptions,
    ) -> LinkerResult<Option<AgentActivityResponse>> {
        let space = self
            .gateway_kitsune
            .get_or_create_space(dna_hash)
            .await
            .map_err(LinkerError::Internal)?;

        let loc = agent.get_loc();
        let agents = self.get_peers_for_location(&space, loc).await?;

        if agents.is_empty() {
            info!(dna = %dna_hash, loc, "No peers found for agent activity query");
            return Ok(None);
        }

        debug!(
            dna = %dna_hash,
            %agent,
            peer_count = agents.len(),
            "Querying peers for get_agent_activity"
        );

        let mut handles = Vec::new();
        for (to_agent, url) in agents.into_iter().take(PARALLEL_GET_AGENTS_COUNT) {
            let space = space.clone();
            let agent = agent.clone();
            let query = query.clone();
            let options = options.clone();
            let pending = self.pending.clone();
            let timeout = self.timeout;

            handles.push(tokio::spawn(async move {
                Self::send_get_agent_activity_request(
                    space, to_agent, url, agent, query, options, pending, timeout,
                )
                .await
            }));
        }

        let mut outcome = PeerRaceOutcome::default();
        for handle in handles {
            match handle.await {
                Ok(Ok(Some(response))) if response.status != ChainStatus::Empty => {
                    return Ok(Some(response));
                }
                Ok(Ok(result)) => {
                    outcome.record_ok(result);
                }
                Ok(Err(e)) => {
                    debug!(?e, "Peer agent activity query failed");
                    outcome.record_err(e);
                }
                Err(e) => {
                    debug!(?e, "Task join error");
                }
            }
        }

        outcome.into_result()
    }

    /// Get agent activity with must-get semantics from the DHT.
    ///
    /// Queries multiple peers near the agent's DHT location in parallel and returns
    /// the first Activity response.
    pub async fn must_get_agent_activity(
        &self,
        dna_hash: &DnaHash,
        agent: AgentPubKey,
        filter: ChainFilter,
    ) -> LinkerResult<Option<MustGetAgentActivityResponse>> {
        let space = self
            .gateway_kitsune
            .get_or_create_space(dna_hash)
            .await
            .map_err(LinkerError::Internal)?;

        let loc = agent.get_loc();
        let agents = self.get_peers_for_location(&space, loc).await?;

        if agents.is_empty() {
            info!(dna = %dna_hash, loc, "No peers found for must_get_agent_activity query");
            return Ok(None);
        }

        debug!(
            dna = %dna_hash,
            %agent,
            peer_count = agents.len(),
            "Querying peers for must_get_agent_activity"
        );

        let mut handles = Vec::new();
        for (to_agent, url) in agents.into_iter().take(PARALLEL_GET_AGENTS_COUNT) {
            let space = space.clone();
            let agent = agent.clone();
            let filter = filter.clone();
            let pending = self.pending.clone();
            let timeout = self.timeout;

            handles.push(tokio::spawn(async move {
                Self::send_must_get_agent_activity_request(
                    space, to_agent, url, agent, filter, pending, timeout,
                )
                .await
            }));
        }

        let mut outcome = PeerRaceOutcome::default();
        for handle in handles {
            match handle.await {
                Ok(Ok(Some(response))) => {
                    if matches!(&response, MustGetAgentActivityResponse::Activity { .. }) {
                        return Ok(Some(response));
                    }
                    outcome.record_ok(Some(response));
                }
                Ok(Ok(result)) => {
                    outcome.record_ok(result);
                }
                Ok(Err(e)) => {
                    debug!(?e, "Peer must_get_agent_activity query failed");
                    outcome.record_err(e);
                }
                Err(e) => {
                    debug!(?e, "Task join error");
                }
            }
        }

        outcome.into_result()
    }

    /// Send a get_agent_activity request to a specific peer.
    #[allow(clippy::too_many_arguments)]
    async fn send_get_agent_activity_request(
        space: DynSpace,
        to_agent: AgentPubKey,
        to_url: Url,
        agent: AgentPubKey,
        query: ChainQueryFilter,
        options: GetActivityOptions,
        pending: PendingDhtResponses,
        timeout: Duration,
    ) -> LinkerResult<Option<AgentActivityResponse>> {
        let (msg_id, req) =
            WireMessage::get_agent_activity_req(to_agent.clone(), agent.clone(), query, options);

        debug!(
            msg_id,
            %to_url,
            %to_agent,
            %agent,
            "Preparing GetAgentActivityReq message"
        );

        let encoded = WireMessage::encode_batch(&[&req])
            .map_err(|e| LinkerError::Internal(format!("Failed to encode request: {e}")))?;

        let (tx, rx) = oneshot::channel();
        pending.register(msg_id, tx).await;

        let pending_cleanup = pending.clone();
        let timeout_msg_id = msg_id;
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            pending_cleanup.remove(timeout_msg_id).await;
        });

        match space.send_notify(to_url.clone(), encoded).await {
            Ok(()) => {
                debug!(msg_id, %to_url, "GetAgentActivityReq send_notify completed");
            }
            Err(e) => {
                warn!(msg_id, %to_url, error = %e, "GetAgentActivityReq send_notify FAILED");
                return Err(LinkerError::Internal(format!(
                    "Failed to send request: {e}"
                )));
            }
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(WireMessage::GetAgentActivityRes { response, .. })) => {
                debug!(msg_id, "Got GetAgentActivityRes response");
                Ok(Some(response))
            }
            Ok(Ok(WireMessage::ErrorRes { error, .. })) => {
                warn!(msg_id, %error, "Got ErrorRes for get_agent_activity");
                Err(LinkerError::Internal(format!("Remote error: {error}")))
            }
            Ok(Ok(other)) => {
                warn!(
                    msg_id,
                    ?other,
                    "Got unexpected response type for get_agent_activity"
                );
                Err(LinkerError::Internal(format!(
                    "Unexpected response: {other:?}"
                )))
            }
            Ok(Err(_)) => Err(LinkerError::Internal("Response channel closed".to_string())),
            Err(_) => {
                warn!(
                    msg_id,
                    timeout_secs = timeout.as_secs(),
                    "GetAgentActivity request TIMED OUT"
                );
                Err(LinkerError::Internal("Request timed out".to_string()))
            }
        }
    }

    /// Send a must_get_agent_activity request to a specific peer.
    async fn send_must_get_agent_activity_request(
        space: DynSpace,
        to_agent: AgentPubKey,
        to_url: Url,
        agent: AgentPubKey,
        filter: ChainFilter,
        pending: PendingDhtResponses,
        timeout: Duration,
    ) -> LinkerResult<Option<MustGetAgentActivityResponse>> {
        let (msg_id, req) =
            WireMessage::must_get_agent_activity_req(to_agent.clone(), agent.clone(), filter);

        debug!(
            msg_id,
            %to_url,
            %to_agent,
            %agent,
            "Preparing MustGetAgentActivityReq message"
        );

        let encoded = WireMessage::encode_batch(&[&req])
            .map_err(|e| LinkerError::Internal(format!("Failed to encode request: {e}")))?;

        let (tx, rx) = oneshot::channel();
        pending.register(msg_id, tx).await;

        let pending_cleanup = pending.clone();
        let timeout_msg_id = msg_id;
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            pending_cleanup.remove(timeout_msg_id).await;
        });

        match space.send_notify(to_url.clone(), encoded).await {
            Ok(()) => {
                debug!(msg_id, %to_url, "MustGetAgentActivityReq send_notify completed");
            }
            Err(e) => {
                warn!(msg_id, %to_url, error = %e, "MustGetAgentActivityReq send_notify FAILED");
                return Err(LinkerError::Internal(format!(
                    "Failed to send request: {e}"
                )));
            }
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(WireMessage::MustGetAgentActivityRes { response, .. })) => {
                debug!(msg_id, "Got MustGetAgentActivityRes response");
                Ok(Some(response))
            }
            Ok(Ok(WireMessage::ErrorRes { error, .. })) => {
                warn!(msg_id, %error, "Got ErrorRes for must_get_agent_activity");
                Err(LinkerError::Internal(format!("Remote error: {error}")))
            }
            Ok(Ok(other)) => {
                warn!(
                    msg_id,
                    ?other,
                    "Got unexpected response type for must_get_agent_activity"
                );
                Err(LinkerError::Internal(format!(
                    "Unexpected response: {other:?}"
                )))
            }
            Ok(Err(_)) => Err(LinkerError::Internal("Response channel closed".to_string())),
            Err(_) => {
                warn!(
                    msg_id,
                    timeout_secs = timeout.as_secs(),
                    "MustGetAgentActivity request TIMED OUT"
                );
                Err(LinkerError::Internal("Request timed out".to_string()))
            }
        }
    }

    /// Send a count_links request to a specific peer.
    async fn send_count_links_request(
        space: DynSpace,
        to_agent: AgentPubKey,
        to_url: Url,
        query: WireLinkQuery,
        pending: PendingDhtResponses,
        timeout: Duration,
    ) -> LinkerResult<Option<CountLinksResponse>> {
        let (msg_id, req) = WireMessage::count_links_req(to_agent.clone(), query.clone());

        info!(
            msg_id,
            %to_url,
            %to_agent,
            base = %query.base,
            "Preparing CountLinksReq message"
        );

        let encoded = WireMessage::encode_batch(&[&req])
            .map_err(|e| LinkerError::Internal(format!("Failed to encode request: {e}")))?;

        // Register pending request
        let (tx, rx) = oneshot::channel();
        pending.register(msg_id, tx).await;

        // Set up timeout to clean up pending request
        let pending_cleanup = pending.clone();
        let timeout_msg_id = msg_id;
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            pending_cleanup.remove(timeout_msg_id).await;
        });

        // Send request
        let send_start = std::time::Instant::now();
        match space.send_notify(to_url.clone(), encoded).await {
            Ok(()) => {
                info!(
                    msg_id,
                    %to_url,
                    send_elapsed_ms = send_start.elapsed().as_millis(),
                    "CountLinksReq send_notify completed"
                );
            }
            Err(e) => {
                warn!(msg_id, %to_url, error = %e, "CountLinksReq send_notify FAILED");
                return Err(LinkerError::Internal(format!(
                    "Failed to send request: {e}"
                )));
            }
        }

        // Wait for response
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(WireMessage::CountLinksRes { response, .. })) => {
                info!(msg_id, "Got CountLinksRes response");
                Ok(Some(response))
            }
            Ok(Ok(WireMessage::ErrorRes { error, .. })) => {
                warn!(msg_id, %error, "Got ErrorRes for count_links");
                Err(LinkerError::Internal(format!("Remote error: {error}")))
            }
            Ok(Ok(other)) => {
                warn!(
                    msg_id,
                    ?other,
                    "Got unexpected response type for count_links"
                );
                Err(LinkerError::Internal(format!(
                    "Unexpected response: {other:?}"
                )))
            }
            Ok(Err(_)) => {
                warn!(msg_id, "Response channel closed (receiver dropped)");
                Err(LinkerError::Internal("Response channel closed".to_string()))
            }
            Err(_) => {
                warn!(
                    msg_id,
                    timeout_secs = timeout.as_secs(),
                    "CountLinks request TIMED OUT"
                );
                Err(LinkerError::Internal("Request timed out".to_string()))
            }
        }
    }
}

/// Tracks what peers actually reported while racing `get`/`get_links` queries,
/// so the decision of what to do once none of them returned non-empty data is
/// independent of task completion order.
///
/// Before this existed, `get`/`get_links` tracked only a single `last_result`
/// slot that each `Ok(Ok(_))` unconditionally overwrote: whichever peer
/// happened to finish last decided both whether the local-authority fallback
/// ran at all, and (if it did and came up empty too) what got returned. Two
/// peers racing with, say, one `Ok(Some(empty))` and one `Ok(None)` could
/// therefore produce different outcomes on different runs. This struct
/// collapses all `Ok` completions into two flags (order doesn't matter for a
/// boolean OR) plus at most one representative `Some(_)` value, and keeps a
/// peer error only for the case where NO peer ever returned `Ok`.
#[derive(Debug)]
struct PeerRaceOutcome<T> {
    /// An `Ok(Some(_))` from some peer. Only ever holds an EMPTY value here:
    /// callers return immediately the moment a peer produces non-empty data,
    /// before this struct is even consulted.
    empty_some: Option<T>,
    /// Did any peer return `Ok(None)`?
    saw_ok_none: bool,
    /// The first peer error seen. Only surfaced if no peer ever returned `Ok`.
    first_err: Option<LinkerError>,
}

// Manual `Default` (rather than `#[derive(Default)]`) because the derive
// macro would add a spurious `T: Default` bound - none of the fields
// actually need one; `T` only ever appears inside an `Option`.
impl<T> Default for PeerRaceOutcome<T> {
    fn default() -> Self {
        Self {
            empty_some: None,
            saw_ok_none: false,
            first_err: None,
        }
    }
}

impl<T> PeerRaceOutcome<T> {
    fn record_ok(&mut self, value: Option<T>) {
        match value {
            Some(v) => {
                self.empty_some.get_or_insert(v);
            }
            None => self.saw_ok_none = true,
        }
    }

    fn record_err(&mut self, err: LinkerError) {
        self.first_err.get_or_insert(err);
    }

    /// The deterministic result once no peer returned non-empty data AND the
    /// local-authority fallback (tried by the caller in between) also came up
    /// empty or wasn't available: prefer an explicit peer-reported `Some`
    /// (even if empty) over a bare `None`, and only surface a peer error if
    /// literally no peer returned `Ok` at all.
    fn into_result(self) -> LinkerResult<Option<T>> {
        if let Some(v) = self.empty_some {
            return Ok(Some(v));
        }
        if self.saw_ok_none {
            return Ok(None);
        }
        match self.first_err {
            Some(e) => Err(e),
            None => Ok(None),
        }
    }
}

/// Check if WireOps is empty (no data found)
fn is_wire_ops_empty(ops: &WireOps) -> bool {
    match ops {
        WireOps::Entry(entry_ops) => {
            entry_ops.creates.is_empty()
                && entry_ops.deletes.is_empty()
                && entry_ops.updates.is_empty()
                && entry_ops.entry.is_none()
        }
        WireOps::Record(record_ops) => {
            record_ops.action.is_none()
                && record_ops.deletes.is_empty()
                && record_ops.updates.is_empty()
                && record_ops.entry.is_none()
        }
        WireOps::Warrant(_) => false, // Warrants are never "empty"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holochain_types::entry::WireEntryOps;
    use holochain_types::link::{CountLinksResponse, WireLinkOps};
    use holochain_types::record::WireRecordOps;

    #[test]
    fn test_pending_dht_responses() {
        let pending = PendingDhtResponses::new();
        assert!(format!("{pending:?}").contains("PendingDhtResponses"));
    }

    #[test]
    fn test_is_wire_ops_empty() {
        let empty_entry = WireOps::Entry(WireEntryOps {
            creates: vec![],
            deletes: vec![],
            updates: vec![],
            entry: None,
            warrants: vec![],
        });
        assert!(is_wire_ops_empty(&empty_entry));

        let empty_record = WireOps::Record(WireRecordOps {
            action: None,
            deletes: vec![],
            updates: vec![],
            entry: None,
            warrants: vec![],
        });
        assert!(is_wire_ops_empty(&empty_record));
    }

    // ========================================================================
    // PeerRaceOutcome: Finding 3 - deterministic fallback-activation decision.
    //
    // These pin the decision helper directly (a real multi-peer race would
    // need a live multi-agent kitsune2/websocket setup this harness doesn't
    // have - see the co-located agent tests further down for the closest
    // equivalent, which only exercises the zero-remote-peers path).
    // ========================================================================

    fn empty_record() -> WireOps {
        WireOps::Record(WireRecordOps {
            action: None,
            deletes: vec![],
            updates: vec![],
            entry: None,
            warrants: vec![],
        })
    }

    #[test]
    fn peer_race_outcome_some_empty_then_none_prefers_the_some() {
        let mut outcome: PeerRaceOutcome<WireOps> = PeerRaceOutcome::default();
        outcome.record_ok(Some(empty_record()));
        outcome.record_ok(None);
        assert!(matches!(outcome.into_result(), Ok(Some(_))));
    }

    #[test]
    fn peer_race_outcome_none_then_some_empty_is_order_independent() {
        // Same two completions as above, opposite arrival order - must reach
        // the identical outcome (this is exactly the scenario the pre-fix
        // `last_result` overwrite made order-dependent).
        let mut outcome: PeerRaceOutcome<WireOps> = PeerRaceOutcome::default();
        outcome.record_ok(None);
        outcome.record_ok(Some(empty_record()));
        assert!(matches!(outcome.into_result(), Ok(Some(_))));
    }

    #[test]
    fn peer_race_outcome_all_none_returns_ok_none() {
        let mut outcome: PeerRaceOutcome<WireOps> = PeerRaceOutcome::default();
        outcome.record_ok(None);
        outcome.record_ok(None);
        assert!(matches!(outcome.into_result(), Ok(None)));
    }

    #[test]
    fn peer_race_outcome_surfaces_error_only_when_no_peer_returned_ok() {
        let mut outcome: PeerRaceOutcome<WireOps> = PeerRaceOutcome::default();
        outcome.record_err(LinkerError::Network("peer unreachable".to_string()));
        assert!(outcome.into_result().is_err());
    }

    #[test]
    fn peer_race_outcome_an_ok_suppresses_a_previously_recorded_error() {
        // A real peer answer (even an empty one) is always preferred over
        // surfacing another peer's transient error.
        let mut outcome: PeerRaceOutcome<WireOps> = PeerRaceOutcome::default();
        outcome.record_err(LinkerError::Network("peer unreachable".to_string()));
        outcome.record_ok(None);
        assert!(matches!(outcome.into_result(), Ok(None)));
    }

    #[test]
    fn peer_race_outcome_error_then_ok_and_ok_then_error_agree() {
        // Order independence for the Ok-vs-Err interaction specifically:
        // both arrival orders of the same two completions must agree.
        let mut a = PeerRaceOutcome::default();
        a.record_err(LinkerError::Network("x".to_string()));
        a.record_ok(Some(empty_record()));

        let mut b = PeerRaceOutcome::default();
        b.record_ok(Some(empty_record()));
        b.record_err(LinkerError::Network("x".to_string()));

        assert!(matches!(a.into_result(), Ok(Some(_))));
        assert!(matches!(b.into_result(), Ok(Some(_))));
    }

    #[tokio::test]
    async fn test_route_get_res_to_pending_request() {
        let pending = PendingDhtResponses::new();
        let (tx, rx) = oneshot::channel();

        let msg_id = 42;
        pending.register(msg_id, tx).await;

        let response = WireOps::Record(WireRecordOps {
            action: None,
            deletes: vec![],
            updates: vec![],
            entry: None,
            warrants: vec![],
        });
        let wire_msg = WireMessage::get_res(msg_id, response);

        let routed = pending.route_response(wire_msg).await;
        assert!(routed);

        let received = rx.await.expect("channel should deliver response");
        match received {
            WireMessage::GetRes {
                msg_id: id,
                response: _,
            } => assert_eq!(id, msg_id),
            other => panic!("Expected GetRes, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_route_get_links_res_to_pending_request() {
        let pending = PendingDhtResponses::new();
        let (tx, rx) = oneshot::channel();

        let msg_id = 99;
        pending.register(msg_id, tx).await;

        let response = WireLinkOps {
            creates: vec![],
            deletes: vec![],
            warrants: vec![],
        };
        let wire_msg = WireMessage::get_links_res(msg_id, response);

        let routed = pending.route_response(wire_msg).await;
        assert!(routed);

        let received = rx.await.expect("channel should deliver response");
        match received {
            WireMessage::GetLinksRes {
                msg_id: id,
                response,
            } => {
                assert_eq!(id, msg_id);
                assert!(response.creates.is_empty());
            }
            other => panic!("Expected GetLinksRes, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_route_count_links_res_to_pending_request() {
        let pending = PendingDhtResponses::new();
        let (tx, rx) = oneshot::channel();

        let msg_id = 200;
        pending.register(msg_id, tx).await;

        let response = CountLinksResponse::new(vec![]);
        let wire_msg = WireMessage::count_links_res(msg_id, response);

        let routed = pending.route_response(wire_msg).await;
        assert!(routed);

        let received = rx.await.expect("channel should deliver response");
        match received {
            WireMessage::CountLinksRes { msg_id: id, .. } => assert_eq!(id, msg_id),
            other => panic!("Expected CountLinksRes, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_route_error_res_to_pending_request() {
        let pending = PendingDhtResponses::new();
        let (tx, rx) = oneshot::channel();

        let msg_id = 55;
        pending.register(msg_id, tx).await;

        let wire_msg = WireMessage::ErrorRes {
            msg_id,
            error: "test error".to_string(),
        };

        let routed = pending.route_response(wire_msg).await;
        assert!(routed);

        let received = rx.await.expect("channel should deliver response");
        match received {
            WireMessage::ErrorRes { msg_id: id, error } => {
                assert_eq!(id, msg_id);
                assert_eq!(error, "test error");
            }
            other => panic!("Expected ErrorRes, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_route_response_no_matching_pending() {
        let pending = PendingDhtResponses::new();

        let wire_msg = WireMessage::get_res(
            999,
            WireOps::Record(WireRecordOps {
                action: None,
                deletes: vec![],
                updates: vec![],
                entry: None,
                warrants: vec![],
            }),
        );

        let routed = pending.route_response(wire_msg).await;
        assert!(!routed);
    }

    #[tokio::test]
    async fn test_remove_cleans_up_pending_request() {
        let pending = PendingDhtResponses::new();
        let (tx, _rx) = oneshot::channel();

        let msg_id = 77;
        pending.register(msg_id, tx).await;

        // Simulate timeout cleanup
        pending.remove(msg_id).await;

        // Now routing should fail — no pending request
        let wire_msg = WireMessage::get_res(
            msg_id,
            WireOps::Record(WireRecordOps {
                action: None,
                deletes: vec![],
                updates: vec![],
                entry: None,
                warrants: vec![],
            }),
        );
        let routed = pending.route_response(wire_msg).await;
        assert!(!routed);
    }

    #[tokio::test]
    async fn test_route_response_after_receiver_dropped() {
        let pending = PendingDhtResponses::new();
        let (tx, rx) = oneshot::channel();

        let msg_id = 88;
        pending.register(msg_id, tx).await;

        // Drop the receiver before the response arrives
        drop(rx);

        let wire_msg = WireMessage::get_res(
            msg_id,
            WireOps::Record(WireRecordOps {
                action: None,
                deletes: vec![],
                updates: vec![],
                entry: None,
                warrants: vec![],
            }),
        );

        // route_response should still return true (it found the pending entry)
        // but the send will fail silently (logged as warning)
        let routed = pending.route_response(wire_msg).await;
        assert!(routed);
    }

    #[tokio::test]
    async fn test_multiple_pending_requests_independent() {
        let pending = PendingDhtResponses::new();

        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();

        pending.register(10, tx1).await;
        pending.register(20, tx2).await;

        // Route response only to msg_id=20
        let wire_msg = WireMessage::get_links_res(
            20,
            WireLinkOps {
                creates: vec![],
                deletes: vec![],
                warrants: vec![],
            },
        );
        let routed = pending.route_response(wire_msg).await;
        assert!(routed);

        // msg_id=20 should have received response
        let received = rx2.await.expect("rx2 should get response");
        assert!(matches!(
            received,
            WireMessage::GetLinksRes { msg_id: 20, .. }
        ));

        // msg_id=10 should still be pending (not consumed)
        // Route its response now
        let wire_msg2 = WireMessage::get_res(
            10,
            WireOps::Record(WireRecordOps {
                action: None,
                deletes: vec![],
                updates: vec![],
                entry: None,
                warrants: vec![],
            }),
        );
        let routed2 = pending.route_response(wire_msg2).await;
        assert!(routed2);

        let received2 = rx1.await.expect("rx1 should get response");
        assert!(matches!(received2, WireMessage::GetRes { msg_id: 10, .. }));
    }

    // ========================================================================
    // Co-located zero-arc agent tests
    //
    // Reproduces and fixes the scenario from the holo-web-conductor e2e report
    // (task 19/19b): two zero-arc browser agents hosted by the *same* linker
    // instance, joined to the *same* DNA space, can never see each other as
    // "remote peers" via kitsune2 -- `kitsune2_core::get_responsive_remote_agents_near_location`
    // unconditionally filters out anything present in the space's
    // `local_agent_store` (see `crate::local_authority` module docs). These tests
    // build a real (in-memory, no network I/O) kitsune2 space via
    // `kitsune2_core::default_test_builder` to exercise the actual
    // `GatewayKitsune`/`DhtQuery` code paths, not just the `local_authority` helpers
    // in isolation.
    // ========================================================================

    use crate::agent_proxy::AgentProxyManager;
    use crate::temp_op_store::TempOpStoreFactory;
    use holo_hash::{ActionHash, EntryHash};
    use holochain_types::prelude::{
        Action, ActionData, ActionHeader, AppEntryBytes, AppEntryDef, ChainOp, CreateData,
        CreateLinkData, Entry, EntryVisibility, LinkTag, LinkTypeFilter, OpEntry, Signature,
        Timestamp,
    };
    use holochain_types::prelude::{DhtOp, SignedAction};

    async fn test_gateway_kitsune() -> GatewayKitsune {
        let builder = kitsune2_core::default_test_builder()
            .with_default_config()
            .expect("default test config");
        let kitsune = builder
            .build()
            .await
            .expect("build in-memory kitsune instance");
        let handler = crate::gateway_kitsune::KitsuneProxy::new(AgentProxyManager::new());
        kitsune
            .register_handler(std::sync::Arc::new(handler))
            .await
            .expect("register kitsune handler");
        GatewayKitsune::new(kitsune, AgentProxyManager::new())
    }

    fn test_entry() -> Entry {
        let bytes = holochain_serialized_bytes::prelude::UnsafeBytes::from(vec![1, 2, 3]);
        Entry::App(AppEntryBytes(
            holochain_serialized_bytes::prelude::SerializedBytes::from(bytes),
        ))
    }

    fn test_entry_type() -> holochain_types::prelude::EntryType {
        holochain_types::prelude::EntryType::App(AppEntryDef {
            entry_index: 0.into(),
            zome_index: 0.into(),
            visibility: EntryVisibility::Public,
        })
    }

    fn signed_create_record_op(author: AgentPubKey) -> (ActionHash, DhtOp) {
        let entry = test_entry();
        let entry_hash = EntryHash::with_data_sync(&entry);
        let action = Action {
            header: ActionHeader {
                author,
                timestamp: Timestamp::from_micros(1_000),
                action_seq: 3,
                prev_action: Some(ActionHash::from_raw_32(vec![9; 32])),
            },
            data: ActionData::Create(CreateData {
                entry_type: test_entry_type(),
                entry_hash,
            }),
        };
        let action_hash = ActionHash::with_data_sync(&action);
        let signed_action = SignedAction::new(action, Signature::from([0xaa; 64]));
        let op = DhtOp::ChainOp(Box::new(ChainOp::CreateRecord(
            signed_action,
            OpEntry::Present(entry),
        )));
        (action_hash, op)
    }

    #[tokio::test]
    async fn kitsune2_never_finds_colocated_agents_as_remote_peers() {
        // Confirms the root cause directly against a real kitsune2 space: two zero-arc
        // agents joined to the same linker/space are always mutually invisible via
        // `GatewayKitsune::publish_ops`'s remote-peer lookup, regardless of kitsune2
        // version -- this is not a version regression to "fix" in kitsune2 itself.
        let gateway_kitsune = test_gateway_kitsune().await;
        let dna_hash = holochain_types::prelude::DnaHash::from_raw_32(vec![7; 32]);
        let agent_a = AgentPubKey::from_raw_32(vec![1; 32]);
        let agent_b = AgentPubKey::from_raw_32(vec![2; 32]);

        gateway_kitsune
            .agent_join(&dna_hash, &agent_a)
            .await
            .unwrap();
        gateway_kitsune
            .agent_join(&dna_hash, &agent_b)
            .await
            .unwrap();

        let space = gateway_kitsune
            .get_or_create_space(&dna_hash)
            .await
            .unwrap();
        let local_agents = space.local_agent_store().get_all().await.unwrap();
        assert_eq!(
            local_agents.len(),
            2,
            "both browser agents should be local to this one shared space"
        );

        let (_action_hash, op) = signed_create_record_op(agent_a.clone());
        let op_bytes = holochain_serialized_bytes::prelude::encode(&op).unwrap();
        let (_factory, op_store_handle) = TempOpStoreFactory::create();
        let op_id = op_store_handle
            .store_op(bytes::Bytes::from(op_bytes))
            .await
            .unwrap();
        let basis_loc = op.dht_basis().get_loc();

        let published = gateway_kitsune
            .publish_ops(&dna_hash, vec![op_id], basis_loc)
            .await
            .unwrap();
        assert_eq!(
            published, 0,
            "kitsune2 must find zero remote peers for two co-located zero-arc agents"
        );
    }

    #[tokio::test]
    async fn get_serves_colocated_agents_published_record_from_local_buffer() {
        let gateway_kitsune = test_gateway_kitsune().await;
        let dna_hash = holochain_types::prelude::DnaHash::from_raw_32(vec![7; 32]);
        let agent_a = AgentPubKey::from_raw_32(vec![1; 32]);
        let agent_b = AgentPubKey::from_raw_32(vec![2; 32]);
        gateway_kitsune
            .agent_join(&dna_hash, &agent_a)
            .await
            .unwrap();
        gateway_kitsune
            .agent_join(&dna_hash, &agent_b)
            .await
            .unwrap();

        // Agent A publishes a record. This is stored in TempOpStore regardless of
        // whether any remote peer is found (see routes::publish::dht_publish), and
        // (per the previous test) publish_ops finds zero remote peers here.
        let (action_hash, op) = signed_create_record_op(agent_a);
        let op_bytes = holochain_serialized_bytes::prelude::encode(&op).unwrap();
        let (_factory, op_store_handle) = TempOpStoreFactory::create();
        op_store_handle
            .store_op(bytes::Bytes::from(op_bytes))
            .await
            .unwrap();

        // Agent B -- co-located, same linker, same space -- can still see it via a
        // direct `get`, served from the local publish buffer.
        let dht_query = DhtQuery::new(gateway_kitsune, PendingDhtResponses::new())
            .with_local_ops(op_store_handle);

        let result = dht_query
            .get(&dna_hash, AnyDhtHash::from(action_hash))
            .await
            .unwrap();
        let wire_ops = result.expect("agent B should see agent A's published record");
        match wire_ops {
            WireOps::Record(record_ops) => {
                assert!(record_ops.action.is_some());
                assert!(record_ops.entry.is_some());
            }
            other => panic!("expected WireOps::Record, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_links_serves_colocated_agents_published_link_from_local_buffer() {
        let gateway_kitsune = test_gateway_kitsune().await;
        let dna_hash = holochain_types::prelude::DnaHash::from_raw_32(vec![7; 32]);
        let agent_a = AgentPubKey::from_raw_32(vec![1; 32]);
        let agent_b = AgentPubKey::from_raw_32(vec![2; 32]);
        gateway_kitsune
            .agent_join(&dna_hash, &agent_a)
            .await
            .unwrap();
        gateway_kitsune
            .agent_join(&dna_hash, &agent_b)
            .await
            .unwrap();

        let base = holo_hash::AnyLinkableHash::from(EntryHash::from_raw_32(vec![3; 32]));
        let target = holo_hash::AnyLinkableHash::from(EntryHash::from_raw_32(vec![4; 32]));
        let action = Action {
            header: ActionHeader {
                author: agent_a,
                timestamp: Timestamp::from_micros(1_000),
                action_seq: 4,
                prev_action: Some(ActionHash::from_raw_32(vec![9; 32])),
            },
            data: ActionData::CreateLink(CreateLinkData {
                base_address: base.clone(),
                target_address: target,
                zome_index: 0.into(),
                link_type: 1.into(),
                tag: LinkTag::new(vec![0xde, 0xad]),
            }),
        };
        let signed_action = SignedAction::new(action, Signature::from([0xbb; 64]));
        let op = DhtOp::ChainOp(Box::new(ChainOp::CreateLink(signed_action)));
        let op_bytes = holochain_serialized_bytes::prelude::encode(&op).unwrap();
        let (_factory, op_store_handle) = TempOpStoreFactory::create();
        op_store_handle
            .store_op(bytes::Bytes::from(op_bytes))
            .await
            .unwrap();

        let dht_query = DhtQuery::new(gateway_kitsune, PendingDhtResponses::new())
            .with_local_ops(op_store_handle);

        let link_key = WireLinkKey {
            base,
            type_query: LinkTypeFilter::single_type(0.into(), 1.into()),
            tag: None,
            after: None,
            before: None,
            author: None,
        };
        let result = dht_query.get_links(&dna_hash, link_key).await.unwrap();
        let wire_link_ops = result.expect("agent B should see agent A's published link");
        assert_eq!(wire_link_ops.creates.len(), 1);
    }
}
