//! Kitsune2 proxy for browser extension agents
//!
//! This module allows the gateway to participate in kitsune2
//! on behalf of zero-arc browser agents whose private keys live
//! in the browser extension.
//!
//! # Architecture
//!
//! The gateway runs its own kitsune2 instance that:
//! 1. Joins spaces (DNAs) on behalf of browser agents
//! 2. Receives `RemoteSignalEvt` messages via `recv_notify`
//! 3. Forwards signals to browser via WebSocket
//!
//! # Signal Flow
//!
//! ```text
//! Conductor Agent A ──send_remote_signal──► kitsune2 network
//!                                                │
//!                                                ▼
//! Gateway ◄── recv_notify (RemoteSignalEvt) ◄────┘
//!    │
//!    └── decode WireMessage
//!    └── forward to AgentProxyManager
//!    └── WebSocket to browser
//! ```

use crate::agent_proxy::AgentProxyManager;
use crate::conductor_link::ConductorLinks;
use crate::dht_query::PendingDhtResponses;
use crate::proxy_agent::{AgentSigner, ProxyAgent};
use crate::routes::websocket::ServerMessage;
use crate::wire_preflight::{BootstrapWrapperFactory, PreflightCache, WirePreflightMessage};
use base64::Engine;
use bytes::Bytes;
use holochain_p2p::WireMessage;
use holochain_types::prelude::{AgentPubKey, DnaHash, ExternIO};
use kitsune2_api::{
    AgentId, BoxFut, Config, DynKitsune, DynLocalAgent, DynSpace, DynSpaceHandler, K2Error,
    K2Result, KitsuneHandler, PublishOp, SpaceHandler, SpaceId, Url,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Convert signal payload (ExternIO) to a base64-encoded string.
fn signal_to_b64(signal: &ExternIO) -> String {
    base64::engine::general_purpose::STANDARD.encode(&signal.0)
}

/// Top-level kitsune2 handler for the gateway.
///
/// This implements `KitsuneHandler` and creates `ProxySpaceHandler`
/// instances for each space (DNA) that has registered browser agents.
#[derive(Debug)]
pub struct KitsuneProxy {
    agent_proxy: AgentProxyManager,
    /// Shared pending DHT responses for routing responses to DhtQuery requests
    pending_dht_responses: PendingDhtResponses,
    /// Cached preflight data updated by bootstrap wrapper when agents are published
    preflight_cache: PreflightCache,
    /// Handles traffic for agents served over the conductor-facing protocol.
    conductor_links: ConductorLinksSlot,
}

/// Set once the linker has built its conductor links, which need the
/// Kitsune2 instance this handler is created for.
pub(crate) type ConductorLinksSlot = Arc<std::sync::OnceLock<ConductorLinks>>;

impl KitsuneProxy {
    /// Create a new KitsuneProxy with the given agent proxy manager.
    #[cfg(test)]
    pub fn new(agent_proxy: AgentProxyManager) -> Self {
        Self {
            agent_proxy,
            pending_dht_responses: PendingDhtResponses::new(),
            preflight_cache: PreflightCache::new(),
            conductor_links: Default::default(),
        }
    }

    /// Create a new KitsuneProxy with shared pending DHT responses.
    ///
    /// Use this constructor when you need the pending responses to be shared
    /// with a DhtQuery instance for response routing.
    pub fn with_pending_responses(
        agent_proxy: AgentProxyManager,
        pending_dht_responses: PendingDhtResponses,
    ) -> Self {
        Self {
            agent_proxy,
            pending_dht_responses,
            preflight_cache: PreflightCache::new(),
            conductor_links: Default::default(),
        }
    }

    /// Route traffic for conductor-linked agents to `slot` once it is set.
    pub(crate) fn with_conductor_links(mut self, slot: ConductorLinksSlot) -> Self {
        self.conductor_links = slot;
        self
    }

    /// Get the preflight cache (for sharing with BootstrapWrapperFactory).
    pub fn preflight_cache(&self) -> &PreflightCache {
        &self.preflight_cache
    }
}

impl KitsuneHandler for KitsuneProxy {
    fn create_space(
        &self,
        space_id: SpaceId,
        _config: Option<&Config>,
    ) -> BoxFut<'_, K2Result<DynSpaceHandler>> {
        let agent_proxy = self.agent_proxy.clone();
        let pending_dht_responses = self.pending_dht_responses.clone();
        let conductor_links = self.conductor_links.clone();
        Box::pin(async move {
            info!(?space_id, "Creating proxy space handler");
            let handler: DynSpaceHandler = Arc::new(ProxySpaceHandler {
                space_id,
                agent_proxy,
                pending_dht_responses,
                conductor_links,
            });
            Ok(handler)
        })
    }

    fn new_listening_address(&self, this_url: Url) -> BoxFut<'static, ()> {
        info!(%this_url, "Gateway kitsune2 listening on new address");
        Box::pin(async move {})
    }

    fn peer_disconnect(&self, peer: Url, reason: Option<String>) {
        debug!(%peer, ?reason, "Peer disconnected from gateway");
    }

    fn preflight_gather_outgoing(&self, peer_url: Url) -> BoxFut<'_, K2Result<Bytes>> {
        Box::pin(async move {
            // Return the cached preflight (updated by BootstrapWrapper when agents publish)
            let preflight_bytes = self.preflight_cache.get();

            // Log what we're sending (decode for logging purposes)
            if let Ok(preflight) = WirePreflightMessage::decode(&preflight_bytes) {
                info!(
                    %peer_url,
                    proto_ver = preflight.compat.proto_ver,
                    agent_count = preflight.agents.len(),
                    "Sending preflight to peer"
                );
            }

            Ok(preflight_bytes)
        })
    }

    fn preflight_validate_incoming(&self, peer_url: Url, data: Bytes) -> BoxFut<'_, K2Result<()>> {
        Box::pin(async move {
            // Decode and validate the incoming preflight
            let preflight = WirePreflightMessage::decode(&data)
                .map_err(|e| K2Error::other(format!("Invalid preflight from peer: {e}")))?;

            // Check protocol version compatibility
            if preflight.compat.proto_ver != 2 {
                return Err(K2Error::other(format!(
                    "Incompatible protocol version from {}: expected 2, got {}",
                    peer_url, preflight.compat.proto_ver
                )));
            }

            info!(
                %peer_url,
                proto_ver = preflight.compat.proto_ver,
                agent_count = preflight.agents.len(),
                "Validated incoming preflight"
            );

            Ok(())
        })
    }
}

/// Per-space handler that receives notifications from kitsune2.
///
/// When a `RemoteSignalEvt` is received, it decodes the wire message
/// and forwards the signal to the appropriate browser agent via WebSocket.
/// When response messages (GetRes, GetLinksRes) are received, it routes
/// them to pending DhtQuery requests.
#[derive(Debug)]
struct ProxySpaceHandler {
    space_id: SpaceId,
    agent_proxy: AgentProxyManager,
    /// Shared pending DHT responses for routing responses to DhtQuery requests
    pending_dht_responses: PendingDhtResponses,
    conductor_links: ConductorLinksSlot,
}

impl SpaceHandler for ProxySpaceHandler {
    fn recv_notify(&self, from_peer: Url, space_id: SpaceId, data: Bytes) -> K2Result<()> {
        info!(
            %from_peer,
            ?space_id,
            data_len = data.len(),
            ">>> RECV_NOTIFY called - received notification in proxy space"
        );

        // Decode the wire messages
        match WireMessage::decode_batch(&data) {
            Ok(messages) => {
                info!(
                    message_count = messages.len(),
                    "Decoded {} wire message(s)",
                    messages.len()
                );
                for msg in messages {
                    // Log what type of message we received
                    match &msg {
                        WireMessage::GetRes { msg_id, .. } => {
                            info!(msg_id, ">>> Received GetRes response");
                        }
                        WireMessage::GetLinksRes { msg_id, .. } => {
                            info!(msg_id, ">>> Received GetLinksRes response");
                        }
                        WireMessage::CountLinksRes { msg_id, .. } => {
                            info!(msg_id, ">>> Received CountLinksRes response");
                        }
                        WireMessage::GetAgentActivityRes { msg_id, .. } => {
                            info!(msg_id, ">>> Received GetAgentActivityRes response");
                        }
                        WireMessage::MustGetAgentActivityRes { msg_id, .. } => {
                            info!(msg_id, ">>> Received MustGetAgentActivityRes response");
                        }
                        WireMessage::ErrorRes { msg_id, error } => {
                            info!(msg_id, %error, ">>> Received ErrorRes response");
                        }
                        WireMessage::RemoteSignalEvt { to_agent, .. } => {
                            info!(%to_agent, ">>> Received RemoteSignalEvt");
                        }
                        other => {
                            debug!(?other, ">>> Received other message type");
                        }
                    }
                    self.handle_wire_message(msg, &from_peer);
                }
            }
            Err(e) => {
                warn!(%e, data_len = data.len(), "Failed to decode wire messages - data may be in wrong format");
            }
        }

        Ok(())
    }
}

impl ProxySpaceHandler {
    fn handle_wire_message(&self, msg: WireMessage, from_peer: &Url) {
        if let Some(links) = self.conductor_links.get() {
            let dna_hash = DnaHash::from_k2_space(&self.space_id);
            if links.claims(&dna_hash, &msg) {
                let (links, from_peer) = (links.clone(), from_peer.clone());
                tokio::spawn(async move { links.on_incoming(dna_hash, from_peer, msg).await });
                return;
            }
        }
        match msg {
            WireMessage::RemoteSignalEvt {
                to_agent,
                zome_call_params_serialized,
                signature: _,
            } => {
                info!(
                    ?to_agent,
                    %from_peer,
                    payload_len = zome_call_params_serialized.0.len(),
                    "Received RemoteSignalEvt for browser agent"
                );

                // Convert SpaceId to DnaHash using the built-in conversion
                let dna_hash = DnaHash::from_k2_space(&self.space_id);

                // to_agent is already an AgentPubKey
                let signal_data = signal_to_b64(&zome_call_params_serialized);

                // Create the server message with string representations for JSON
                // Note: from_agent is "remote" since we don't have the sender's
                // agent key in RemoteSignalEvt (it's embedded in zome_call_params)
                let server_msg = ServerMessage::Signal {
                    dna_hash: dna_hash.to_string(),
                    to_agent: to_agent.to_string(),
                    from_agent: "remote".to_string(),
                    zome_name: "recv_remote_signal".to_string(),
                    signal: signal_data,
                };

                // Forward to the registered browser agent via AgentProxyManager
                // spawn a task since send_signal is async and recv_notify is sync
                let agent_proxy = self.agent_proxy.clone();
                tokio::spawn(async move {
                    let sent = agent_proxy
                        .send_signal(&dna_hash, &to_agent, server_msg)
                        .await;
                    if sent {
                        debug!(
                            dna = %dna_hash,
                            agent = %to_agent,
                            "Remote signal forwarded to browser agent"
                        );
                    }
                });
            }

            // DHT query response messages - route to pending DhtQuery requests
            msg @ WireMessage::GetRes { .. }
            | msg @ WireMessage::GetLinksRes { .. }
            | msg @ WireMessage::CountLinksRes { .. }
            | msg @ WireMessage::GetAgentActivityRes { .. }
            | msg @ WireMessage::MustGetAgentActivityRes { .. }
            | msg @ WireMessage::CallRemoteRes { .. }
            | msg @ WireMessage::SendValidationReceiptsRes { .. }
            | msg @ WireMessage::PingRes { .. }
            | msg @ WireMessage::ErrorRes { .. } => {
                let msg_id = msg.get_msg_id();
                info!(?msg_id, "Routing DHT response to pending request handler");
                let pending = self.pending_dht_responses.clone();
                tokio::spawn(async move {
                    let routed = pending.route_response(msg).await;
                    if routed {
                        info!(
                            ?msg_id,
                            "Successfully routed DHT response to pending request"
                        );
                    } else {
                        warn!(
                            ?msg_id,
                            "Failed to route DHT response - no matching pending request found"
                        );
                    }
                });
            }

            other => {
                debug!(
                    msg_type = ?std::mem::discriminant(&other),
                    "Ignoring non-signal wire message"
                );
            }
        }
    }
}

/// Builder for creating a gateway kitsune2 instance.
///
/// # Example
///
/// ```ignore
/// let proxy = KitsuneProxy::new(agent_proxy_manager);
/// let (op_store_factory, op_store_handle) = TempOpStoreFactory::create();
/// let kitsune = KitsuneProxyBuilder::new(proxy)
///     .with_bootstrap_url("https://bootstrap.example.com")
///     .with_relay_url("https://relay.example.com")
///     .with_op_store(op_store_factory)
///     .build()
///     .await?;
/// ```
pub struct KitsuneProxyBuilder {
    handler: Arc<KitsuneProxy>,
    bootstrap_url: Option<String>,
    relay_url: Option<String>,
    op_store_factory: Option<kitsune2_api::DynOpStoreFactory>,
    report_factory: Option<kitsune2_api::DynReportFactory>,
    report_config: Option<crate::linker_report::HcReportConfig>,
}

impl KitsuneProxyBuilder {
    /// Create a new builder with the given handler.
    pub fn new(handler: KitsuneProxy) -> Self {
        Self {
            handler: Arc::new(handler),
            bootstrap_url: None,
            relay_url: None,
            op_store_factory: None,
            report_factory: None,
            report_config: None,
        }
    }

    /// Set the bootstrap server URL.
    pub fn with_bootstrap_url(mut self, url: impl Into<String>) -> Self {
        self.bootstrap_url = Some(url.into());
        self
    }

    /// Set the relay server URL (for iroh transport).
    pub fn with_relay_url(mut self, url: impl Into<String>) -> Self {
        self.relay_url = Some(url.into());
        self
    }

    /// Set a custom OpStoreFactory (for TempOpStore support).
    ///
    /// If not set, the default MemOpStoreFactory will be used.
    pub fn with_op_store(mut self, factory: kitsune2_api::DynOpStoreFactory) -> Self {
        self.op_store_factory = Some(factory);
        self
    }

    /// Set a custom ReportFactory for kitsune2 usage reporting.
    ///
    /// If not set, the default (no-op) report factory will be used.
    /// The `config` provides the report-specific settings (path, retention, interval).
    pub fn with_report(
        mut self,
        factory: kitsune2_api::DynReportFactory,
        config: crate::linker_report::HcReportConfig,
    ) -> Self {
        self.report_factory = Some(factory);
        self.report_config = Some(config);
        self
    }

    /// Build the kitsune2 instance.
    pub async fn build(self) -> Result<DynKitsune, Box<dyn std::error::Error + Send + Sync>> {
        use kitsune2::default_builder;
        use kitsune2_core::factories::{
            CoreBootstrapConfig, CoreBootstrapModConfig, CoreSpaceConfig, CoreSpaceModConfig,
        };
        use kitsune2_transport_iroh::{IrohTransportConfig, IrohTransportModConfig};

        let mut builder = default_builder();

        // Replace op_store factory if a custom one was provided
        if let Some(op_store_factory) = self.op_store_factory {
            builder.op_store = op_store_factory;
        }

        // Replace report factory if a custom one was provided
        if let Some(report_factory) = self.report_factory {
            builder.report = report_factory;
        }

        // Wrap the bootstrap factory to capture AgentInfoSigned for preflight.
        // This ensures that when agents join and publish their info, the preflight
        // cache is updated so conductors will accept our messages.
        let preflight_cache = self.handler.preflight_cache().clone();
        builder.bootstrap = Arc::new(BootstrapWrapperFactory::new(
            preflight_cache,
            builder.bootstrap,
        ));

        let builder = builder.with_default_config()?;

        // Configure bootstrap server
        if let Some(bootstrap_url) = self.bootstrap_url {
            builder.config.set_module_config(&CoreBootstrapModConfig {
                core_bootstrap: CoreBootstrapConfig {
                    server_url: Some(bootstrap_url),
                    backoff_max_ms: 10000,
                    ..Default::default()
                },
            })?;
        }

        // Configure report module if a custom config was provided
        if let Some(hc_report) = self.report_config {
            use crate::linker_report::HcReportModConfig;
            builder
                .config
                .set_module_config(&HcReportModConfig { hc_report })?;
        }

        // Configure core space settings (matching conductor settings)
        builder.config.set_module_config(&CoreSpaceModConfig {
            core_space: CoreSpaceConfig {
                re_sign_expire_time_ms: 10000,
                re_sign_freq_ms: 10000,
            },
        })?;

        // Configure iroh transport
        builder.config.set_module_config(&IrohTransportModConfig {
            iroh_transport: IrohTransportConfig {
                relay_allow_plain_text: true,
                relay_url: self.relay_url,
                ..Default::default()
            },
        })?;

        // Build and register handler
        let kitsune = builder.build().await?;
        kitsune.register_handler(self.handler).await?;

        info!("Gateway kitsune2 instance created");
        Ok(kitsune)
    }
}

/// Gateway kitsune2 manager that handles space and agent lifecycle.
///
/// This wraps a `DynKitsune` instance and provides methods to:
/// - Join browser agents to spaces when they register
/// - Leave agents from spaces when they disconnect
/// - Track active spaces and agents
///
/// # Example
///
/// ```ignore
/// let gateway_kitsune = GatewayKitsune::new(kitsune, agent_proxy);
///
/// // When browser agent registers via WebSocket
/// gateway_kitsune.agent_join(&dna_hash, &agent_pubkey).await?;
///
/// // When browser agent disconnects
/// gateway_kitsune.agent_leave(&dna_hash, &agent_pubkey).await;
/// ```
type AgentMap = Arc<RwLock<HashMap<(DnaHash, AgentPubKey), Arc<ProxyAgent>>>>;

#[derive(Clone)]
pub struct GatewayKitsune {
    kitsune: DynKitsune,
    /// Agent proxy manager for remote signing.
    agent_proxy: AgentProxyManager,
    /// Active spaces by DNA hash.
    spaces: Arc<RwLock<HashMap<DnaHash, DynSpace>>>,
    /// Registered agents by (DnaHash, AgentPubKey).
    /// Value is the ProxyAgent for potential future use.
    agents: AgentMap,
}

impl std::fmt::Debug for GatewayKitsune {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayKitsune").finish_non_exhaustive()
    }
}

impl GatewayKitsune {
    /// Create a new gateway kitsune manager.
    ///
    /// The `agent_proxy` is used for remote signing when kitsune2 needs
    /// to sign agent info on behalf of browser agents.
    pub fn new(kitsune: DynKitsune, agent_proxy: AgentProxyManager) -> Self {
        Self {
            kitsune,
            agent_proxy,
            spaces: Arc::new(RwLock::new(HashMap::new())),
            agents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get or create a space for a DNA.
    pub async fn get_or_create_space(&self, dna_hash: &DnaHash) -> Result<DynSpace, String> {
        // Check if space exists
        {
            let spaces = self.spaces.read().await;
            if let Some(space) = spaces.get(dna_hash) {
                return Ok(space.clone());
            }
        }

        // Create new space using built-in conversion
        let space_id = dna_hash.to_k2_space();

        let space = self
            .kitsune
            .space(space_id, None)
            .await
            .map_err(|e| format!("Failed to create space: {e}"))?;

        // Store space
        {
            let mut spaces = self.spaces.write().await;
            spaces.insert(dna_hash.clone(), space.clone());
        }

        info!(dna = %dna_hash, "Created kitsune2 space for DNA");
        Ok(space)
    }

    /// Join a browser agent to a DNA's kitsune2 space.
    ///
    /// This creates a ProxyAgent and registers it with the space,
    /// enabling the gateway to receive signals on behalf of this agent.
    ///
    /// # Arguments
    ///
    /// * `dna_hash` - The DNA hash (proper Holochain type)
    /// * `agent_pubkey` - The agent public key (proper Holochain type)
    pub async fn agent_join(
        &self,
        dna_hash: &DnaHash,
        agent_pubkey: &AgentPubKey,
    ) -> Result<(), String> {
        self.agent_join_with_signer(
            dna_hash,
            agent_pubkey,
            AgentSigner::Browser(self.agent_proxy.clone()),
        )
        .await
    }

    /// Join an agent whose agent infos are signed by `signer`.
    pub(crate) async fn agent_join_with_signer(
        &self,
        dna_hash: &DnaHash,
        agent_pubkey: &AgentPubKey,
        signer: AgentSigner,
    ) -> Result<(), String> {
        let key = (dna_hash.clone(), agent_pubkey.clone());

        // Check if already registered
        {
            let agents = self.agents.read().await;
            if agents.contains_key(&key) {
                debug!(
                    dna = %dna_hash,
                    agent = %agent_pubkey,
                    "Agent already joined to space"
                );
                return Ok(());
            }
        }

        // Get or create space
        let space = self.get_or_create_space(dna_hash).await?;

        // Create proxy agent that asks `signer` to sign its agent infos
        let proxy_agent = Arc::new(ProxyAgent::with_signer(agent_pubkey.clone(), signer));

        // Join space
        space
            .local_agent_join(proxy_agent.clone() as DynLocalAgent)
            .await
            .map_err(|e| format!("Failed to join agent to space: {e}"))?;

        // Store agent
        {
            let mut agents = self.agents.write().await;
            agents.insert(key, proxy_agent);
        }

        info!(
            dna = %dna_hash,
            agent = %agent_pubkey,
            "Browser agent joined kitsune2 space"
        );
        Ok(())
    }

    /// Remove a browser agent from a DNA's kitsune2 space.
    ///
    /// This publishes a tombstone agent info to the network and removes
    /// the agent from the local tracking.
    ///
    /// # Arguments
    ///
    /// * `dna_hash` - The DNA hash (proper Holochain type)
    /// * `agent_pubkey` - The agent public key (proper Holochain type)
    pub async fn agent_leave(&self, dna_hash: &DnaHash, agent_pubkey: &AgentPubKey) {
        let key = (dna_hash.clone(), agent_pubkey.clone());

        // Remove from tracking
        let proxy_agent = {
            let mut agents = self.agents.write().await;
            agents.remove(&key)
        };

        if proxy_agent.is_none() {
            debug!(
                dna = %dna_hash,
                agent = %agent_pubkey,
                "Agent not found in space (already left?)"
            );
            return;
        }

        // Get space (if it exists)
        let space = {
            let spaces = self.spaces.read().await;
            spaces.get(dna_hash).cloned()
        };

        if let Some(space) = space {
            // Create AgentId from 32-byte key
            let agent_id = AgentId::from(Bytes::copy_from_slice(agent_pubkey.get_raw_32()));

            // Leave space (publishes tombstone)
            space.local_agent_leave(agent_id).await;

            info!(
                dna = %dna_hash,
                agent = %agent_pubkey,
                "Browser agent left kitsune2 space"
            );
        }

        // Check if space has no more agents, and clean up if so
        self.maybe_cleanup_space(dna_hash).await;
    }

    /// Remove a space if it has no more registered agents.
    async fn maybe_cleanup_space(&self, dna_hash: &DnaHash) {
        let has_agents = {
            let agents = self.agents.read().await;
            agents.keys().any(|(dna, _)| dna == dna_hash)
        };

        if !has_agents {
            let mut spaces = self.spaces.write().await;
            if spaces.remove(dna_hash).is_some() {
                info!(dna = %dna_hash, "Removed empty kitsune2 space");
            }
        }
    }

    /// Leave all agents from all spaces.
    ///
    /// Called during gateway shutdown to publish tombstones for all agents.
    pub async fn shutdown(&self) {
        let agents: Vec<_> = {
            let agents = self.agents.read().await;
            agents.keys().cloned().collect()
        };

        for (dna_hash, agent_pubkey) in agents {
            self.agent_leave(&dna_hash, &agent_pubkey).await;
        }

        info!("Gateway kitsune2 shutdown complete");
    }

    /// Get the number of registered agents.
    pub async fn agent_count(&self) -> usize {
        self.agents.read().await.len()
    }

    /// Get the number of active spaces.
    pub async fn space_count(&self) -> usize {
        self.spaces.read().await.len()
    }

    /// Get the total number of peers known to kitsune2 across all spaces.
    pub async fn peer_count(&self) -> usize {
        let spaces = self.spaces.read().await;
        let mut total = 0usize;
        for space in spaces.values() {
            match space.peer_store().get_all().await {
                Ok(peers) => total += peers.len(),
                Err(e) => tracing::warn!("Failed to query peer store: {e}"),
            }
        }
        total
    }

    /// Check if an agent is registered in a space.
    ///
    /// # Arguments
    ///
    /// * `dna_hash` - The DNA hash (proper Holochain type)
    /// * `agent_pubkey` - The agent public key (proper Holochain type)
    pub async fn is_agent_joined(&self, dna_hash: &DnaHash, agent_pubkey: &AgentPubKey) -> bool {
        let key = (dna_hash.clone(), agent_pubkey.clone());
        self.agents.read().await.contains_key(&key)
    }

    /// Send remote signals to target agents.
    ///
    /// For each signal:
    /// 1. Check if target is a registered browser agent (deliver directly via WebSocket)
    /// 2. Otherwise, look up target agent's URL from peer_store
    /// 3. Create WireMessage::remote_signal_evt
    /// 4. Send via space.send_notify()
    ///
    /// Returns (success_count, fail_count).
    pub async fn send_remote_signals(
        &self,
        dna_hash: &DnaHash,
        signals: Vec<crate::routes::websocket::SignedRemoteSignalInput>,
    ) -> (usize, usize) {
        use holochain_types::prelude::Signature;
        use kitsune2_api::AgentId;

        let mut success_count = 0;
        let mut fail_count = 0;

        for signal in signals {
            // Parse target agent from 39-byte HoloHash
            let target_agent = match AgentPubKey::try_from_raw_39(signal.target_agent.clone()) {
                Ok(a) => a,
                Err(e) => {
                    warn!(?e, "Invalid target agent in send_remote_signal");
                    fail_count += 1;
                    continue;
                }
            };

            // First, check if target is a registered browser agent
            // If so, deliver directly via WebSocket (much faster than kitsune2)
            if self
                .agent_proxy
                .is_registered(dna_hash, &target_agent)
                .await
            {
                // Create signal payload for browser delivery
                let signal_data = signal_to_b64(&ExternIO(signal.zome_call_params.clone()));
                let server_msg = ServerMessage::Signal {
                    dna_hash: dna_hash.to_string(),
                    to_agent: target_agent.to_string(),
                    from_agent: "remote".to_string(),
                    zome_name: "recv_remote_signal".to_string(),
                    signal: signal_data,
                };

                if self
                    .agent_proxy
                    .send_signal(dna_hash, &target_agent, server_msg)
                    .await
                {
                    debug!(%target_agent, "Delivered signal to browser agent via WebSocket");
                    success_count += 1;
                } else {
                    warn!(%target_agent, "Failed to deliver signal to registered browser agent");
                    fail_count += 1;
                }
                continue;
            }

            // Target is not a browser agent - try kitsune2 peer store
            let space = match self.get_or_create_space(dna_hash).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(%e, "Failed to get space for remote signal");
                    fail_count += 1;
                    continue;
                }
            };

            // Look up peer URL from peer store
            let agent_id = AgentId::from(Bytes::copy_from_slice(target_agent.get_raw_32()));
            let to_url = match space.peer_store().get(agent_id).await {
                Ok(Some(info)) if info.url.is_some() => info.url.clone().unwrap(),
                _ => {
                    debug!(%target_agent, "Target agent not found in peer store or browser registrations");
                    fail_count += 1;
                    continue;
                }
            };

            // Create wire message
            let extern_io = ExternIO(signal.zome_call_params);
            let Ok(signature) = Signature::try_from(signal.signature.as_slice()) else {
                warn!(%target_agent, "Dropping signal with an invalid signature");
                fail_count += 1;
                continue;
            };
            let wire_msg =
                WireMessage::remote_signal_evt(target_agent.clone(), extern_io, signature);

            // Encode and send
            let encoded = match WireMessage::encode_batch(&[&wire_msg]) {
                Ok(e) => e,
                Err(e) => {
                    warn!(?e, "Failed to encode wire message");
                    fail_count += 1;
                    continue;
                }
            };

            if let Err(e) = space.send_notify(to_url.clone(), encoded).await {
                warn!(?e, %to_url, "Failed to send remote signal");
                fail_count += 1;
            } else {
                debug!(%target_agent, %to_url, "Sent remote signal via kitsune2");
                success_count += 1;
            }
        }

        (success_count, fail_count)
    }

    /// Publish ops to DHT authorities near the given basis location.
    ///
    /// This finds peers whose arc covers the basis location and sends
    /// the op IDs to them. Those peers will then fetch the actual op data
    /// from our TempOpStore.
    ///
    /// This never finds *co-located* zero-arc agents (other browser agents hosted by
    /// this same gateway in the same space): `kitsune2_core::get_responsive_remote_agents_near_location`
    /// unconditionally excludes every agent present in the space's `local_agent_store`
    /// from its results, so two agents joined to the same linker instance can never
    /// see each other as "remote peers" here, independent of kitsune2 version. The op
    /// data is stored in `TempOpStore` regardless (by the `/publish` route, before this
    /// is called); `DhtQuery::get`/`get_links` query remote peers via kitsune2 first,
    /// and only fall back to checking that buffer directly once kitsune2 finds no peer
    /// (or no peer returns data) -- see `crate::local_authority` module docs for how
    /// co-located agents actually see each other's published ops.
    ///
    /// # Arguments
    ///
    /// * `dna_hash` - The DNA hash (proper Holochain type)
    /// * `op_ids` - The op IDs to publish
    /// * `basis_loc` - The DHT location to find authorities for (from OpBasis)
    pub async fn publish_ops(
        &self,
        dna_hash: &DnaHash,
        op_ids: Vec<kitsune2_api::OpId>,
        basis_loc: u32,
    ) -> Result<usize, String> {
        use kitsune2_core::get_responsive_remote_agents_near_location;

        // Get or create the space
        let space = self.get_or_create_space(dna_hash).await?;

        // Debug: Get all peers in the peer store
        let all_peers = space
            .peer_store()
            .get_all()
            .await
            .map_err(|e| format!("Failed to get all peers: {e}"))?;
        debug!(
            dna = %dna_hash,
            peer_count = all_peers.len(),
            "Peer store contents before publish"
        );
        for peer in &all_peers {
            debug!(
                agent = ?peer.agent,
                arc = ?peer.storage_arc,
                url = ?peer.url,
                is_tombstone = peer.is_tombstone,
                "Peer in store"
            );
        }

        // Debug: Get local agents
        let local_agents = space
            .local_agent_store()
            .get_all()
            .await
            .map_err(|e| format!("Failed to get local agents: {e}"))?;
        debug!(
            dna = %dna_hash,
            local_agent_count = local_agents.len(),
            "Local agents in space"
        );

        // Find peers near the basis location
        let agents = get_responsive_remote_agents_near_location(
            space.peer_store().clone(),
            space.local_agent_store().clone(),
            space.peer_meta_store().clone(),
            basis_loc,
            usize::MAX, // No limit on number of peers
        )
        .await
        .map_err(|e| format!("Failed to find peers: {e}"))?;

        debug!(
            dna = %dna_hash,
            basis_loc,
            found_agents = agents.len(),
            "Found responsive remote agents near location"
        );

        // Collect unique URLs (filter out tombstones)
        let urls: std::collections::HashSet<Url> = agents
            .into_iter()
            .filter_map(|info| {
                if info.is_tombstone {
                    debug!(agent = ?info.agent, "Skipping tombstone agent");
                    return None;
                }
                if info.url.is_none() {
                    debug!(agent = ?info.agent, "Skipping agent with no URL");
                    return None;
                }
                info.url.clone()
            })
            .collect();

        if urls.is_empty() {
            debug!(
                dna = %dna_hash,
                basis_loc,
                "No peers found to publish to"
            );
            return Ok(0);
        }

        info!(
            dna = %dna_hash,
            op_count = op_ids.len(),
            peer_count = urls.len(),
            basis_loc,
            "Publishing ops to peers"
        );

        // Publish to each peer
        let publish_ops: Vec<PublishOp> = op_ids
            .iter()
            .cloned()
            .map(|op_id| PublishOp {
                op_id,
                metadata: None,
            })
            .collect();
        let mut success_count = 0;
        for url in urls {
            match space
                .publish()
                .publish_ops(publish_ops.clone(), url.clone())
                .await
            {
                Ok(()) => {
                    success_count += 1;
                    debug!(%url, "Published ops to peer");
                }
                Err(e) => {
                    warn!(%url, %e, "Failed to publish ops to peer");
                }
            }
        }

        Ok(success_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holochain_types::prelude::{Signature, WireOps};

    // Helper to create a test DNA hash (uses from_raw_32 for valid checksums)
    fn test_dna(id: u8) -> DnaHash {
        DnaHash::from_raw_32(vec![id; 32])
    }

    // Helper to create a test agent (uses from_raw_32 for valid checksums)
    fn test_agent(id: u8) -> AgentPubKey {
        AgentPubKey::from_raw_32(vec![id; 32])
    }

    fn test_signature() -> Signature {
        let bytes: [u8; 64] = [0xaa; 64];
        Signature::from(bytes)
    }

    #[test]
    fn test_kitsune_proxy_creation() {
        let agent_proxy = AgentProxyManager::new();
        let proxy = KitsuneProxy::new(agent_proxy);
        assert!(format!("{proxy:?}").contains("KitsuneProxy"));
    }

    #[test]
    fn test_space_handler_creation() {
        let agent_proxy = AgentProxyManager::new();
        let dna = test_dna(1);
        let handler = ProxySpaceHandler {
            space_id: dna.to_k2_space(),
            agent_proxy,
            pending_dht_responses: PendingDhtResponses::new(),
            conductor_links: Default::default(),
        };
        assert!(format!("{handler:?}").contains("ProxySpaceHandler"));
    }

    #[test]
    fn test_decode_remote_signal_evt() {
        // Create a RemoteSignalEvt wire message
        let to_agent = test_agent(0xdb);
        let zome_call_params = ExternIO::encode(b"test signal payload").unwrap();
        let signature = test_signature();

        let wire_msg = WireMessage::remote_signal_evt(
            to_agent.clone(),
            zome_call_params.clone(),
            signature.clone(),
        );

        // Encode as batch using the proper holochain encoding (write_named)
        let batch: Vec<&WireMessage> = vec![&wire_msg];
        let encoded = WireMessage::encode_batch(&batch).expect("encode");

        // Decode using WireMessage::decode_batch
        let decoded = WireMessage::decode_batch(&encoded).expect("decode");
        assert_eq!(decoded.len(), 1);

        match &decoded[0] {
            WireMessage::RemoteSignalEvt {
                to_agent: decoded_agent,
                zome_call_params_serialized,
                signature: decoded_sig,
            } => {
                assert_eq!(decoded_agent.get_raw_36(), to_agent.get_raw_36());
                assert_eq!(zome_call_params_serialized.0, zome_call_params.0);
                assert_eq!(decoded_sig.0.len(), 64);
            }
            other => panic!("Expected RemoteSignalEvt, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_space_handler_recv_notify() {
        // Create handler with proper typed DNA
        let agent_proxy = AgentProxyManager::new();
        let dna = test_dna(1);
        let handler = ProxySpaceHandler {
            space_id: dna.to_k2_space(),
            agent_proxy,
            pending_dht_responses: PendingDhtResponses::new(),
            conductor_links: Default::default(),
        };

        // Create a RemoteSignalEvt
        let to_agent = test_agent(0xdb);
        let zome_call_params = ExternIO::encode(b"hello from conductor").unwrap();
        let signature = test_signature();

        let wire_msg = WireMessage::remote_signal_evt(to_agent, zome_call_params, signature);
        let batch: Vec<&WireMessage> = vec![&wire_msg];
        let encoded = WireMessage::encode_batch(&batch).expect("encode");

        // Call recv_notify
        let from_peer = Url::from_str("ws://localhost:5000").unwrap();
        let space_id = dna.to_k2_space();

        let result = handler.recv_notify(from_peer, space_id, encoded);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_signal_forwarding_to_registered_agent() {
        use tokio::sync::mpsc;

        // Create test data - use DnaHash reconstructed from SpaceId for consistency
        let base_dna = test_dna(1);
        let space_id = base_dna.to_k2_space();
        // The handler will reconstruct DnaHash from SpaceId using from_k2_space
        // so we need to register with that same DnaHash
        let dna = DnaHash::from_k2_space(&space_id);
        let to_agent = test_agent(0xdb);

        // Create handler with agent proxy
        let agent_proxy = AgentProxyManager::new();
        let (tx, mut rx) = mpsc::channel(32);

        // Register the agent using the DnaHash that matches what handler will use
        agent_proxy
            .register(dna.clone(), to_agent.clone(), tx)
            .await;

        // Create handler
        let handler = ProxySpaceHandler {
            space_id: space_id.clone(),
            agent_proxy: agent_proxy.clone(),
            pending_dht_responses: PendingDhtResponses::new(),
            conductor_links: Default::default(),
        };

        // Create a RemoteSignalEvt
        let zome_call_params = ExternIO::encode(b"test signal data").unwrap();
        let signature = test_signature();

        let wire_msg =
            WireMessage::remote_signal_evt(to_agent.clone(), zome_call_params.clone(), signature);
        let batch: Vec<&WireMessage> = vec![&wire_msg];
        let encoded = WireMessage::encode_batch(&batch).expect("encode");

        // Call recv_notify
        let from_peer = Url::from_str("ws://localhost:5000").unwrap();

        let result = handler.recv_notify(from_peer, space_id.clone(), encoded);
        assert!(result.is_ok());

        // Give the spawned task time to run
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Verify the signal was forwarded
        let expected_to_agent = to_agent.to_string();
        let received = rx.try_recv().expect("Expected to receive forwarded signal");
        match received {
            crate::routes::websocket::ServerMessage::Signal {
                dna_hash,
                to_agent: received_to_agent,
                from_agent,
                zome_name,
                signal,
            } => {
                // dna_hash in the message should be the HoloHash string representation
                assert_eq!(dna_hash, dna.to_string());
                assert_eq!(received_to_agent, expected_to_agent);
                assert_eq!(from_agent, "remote");
                assert_eq!(zome_name, "recv_remote_signal");
                // Signal should be base64-encoded version of the zome_call_params
                let expected_signal =
                    base64::engine::general_purpose::STANDARD.encode(&zome_call_params.0);
                assert_eq!(signal, expected_signal);
            }
            other => panic!("Expected Signal message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_signal_not_forwarded_to_unregistered_agent() {
        // Create handler with agent proxy (no agents registered)
        let agent_proxy = AgentProxyManager::new();
        let dna = test_dna(1);

        // Create handler
        let handler = ProxySpaceHandler {
            space_id: dna.to_k2_space(),
            agent_proxy: agent_proxy.clone(),
            pending_dht_responses: PendingDhtResponses::new(),
            conductor_links: Default::default(),
        };

        // Create a RemoteSignalEvt for an unregistered agent
        let to_agent = test_agent(0xdb);
        let zome_call_params = ExternIO::encode(b"signal for nobody").unwrap();
        let signature = test_signature();

        let wire_msg = WireMessage::remote_signal_evt(to_agent, zome_call_params, signature);
        let batch: Vec<&WireMessage> = vec![&wire_msg];
        let encoded = WireMessage::encode_batch(&batch).expect("encode");

        // Call recv_notify - should succeed (doesn't fail on unregistered agent)
        let from_peer = Url::from_str("ws://localhost:5000").unwrap();
        let space_id = dna.to_k2_space();

        let result = handler.recv_notify(from_peer, space_id, encoded);
        assert!(result.is_ok());

        // Verify no crash - the signal is just dropped for unregistered agents
        assert_eq!(agent_proxy.registration_count().await, 0);
    }

    #[tokio::test]
    async fn test_recv_notify_routes_get_res_to_pending() {
        use holochain_types::record::WireRecordOps;
        use tokio::sync::oneshot;

        let agent_proxy = AgentProxyManager::new();
        let pending = PendingDhtResponses::new();
        let dna = test_dna(1);

        let handler = ProxySpaceHandler {
            space_id: dna.to_k2_space(),
            agent_proxy,
            pending_dht_responses: pending.clone(),
            conductor_links: Default::default(),
        };

        // Register a pending request with a known msg_id
        let msg_id = 42;
        let (tx, rx) = oneshot::channel();
        pending.register(msg_id, tx).await;

        // Create a GetRes with the matching msg_id
        let response = WireOps::Record(WireRecordOps {
            action: None,
            deletes: vec![],
            updates: vec![],
            entry: None,
            warrants: vec![],
        });
        let wire_msg = WireMessage::get_res(msg_id, response);
        let encoded = WireMessage::encode_batch(&[&wire_msg]).expect("encode");

        // Deliver via recv_notify
        let from_peer = Url::from_str("ws://localhost:5000").unwrap();
        let space_id = dna.to_k2_space();
        let result = handler.recv_notify(from_peer, space_id, encoded);
        assert!(result.is_ok());

        // Give spawned routing task time to run
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // The pending request should have received the response
        let received = rx.await.expect("pending request should receive response");
        match received {
            WireMessage::GetRes { msg_id: id, .. } => assert_eq!(id, msg_id),
            other => panic!("Expected GetRes, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_recv_notify_routes_get_links_res_to_pending() {
        use holochain_types::link::WireLinkOps;
        use tokio::sync::oneshot;

        let agent_proxy = AgentProxyManager::new();
        let pending = PendingDhtResponses::new();
        let dna = test_dna(1);

        let handler = ProxySpaceHandler {
            space_id: dna.to_k2_space(),
            agent_proxy,
            pending_dht_responses: pending.clone(),
            conductor_links: Default::default(),
        };

        let msg_id = 223;
        let (tx, rx) = oneshot::channel();
        pending.register(msg_id, tx).await;

        let response = WireLinkOps {
            creates: vec![],
            deletes: vec![],
            warrants: vec![],
        };
        let wire_msg = WireMessage::get_links_res(msg_id, response);
        let encoded = WireMessage::encode_batch(&[&wire_msg]).expect("encode");

        let from_peer = Url::from_str("ws://localhost:5000").unwrap();
        let result = handler.recv_notify(from_peer, dna.to_k2_space(), encoded);
        assert!(result.is_ok());

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let received = rx.await.expect("pending request should receive response");
        match received {
            WireMessage::GetLinksRes { msg_id: id, .. } => assert_eq!(id, msg_id),
            other => panic!("Expected GetLinksRes, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_recv_notify_routes_error_res_to_pending() {
        use tokio::sync::oneshot;

        let agent_proxy = AgentProxyManager::new();
        let pending = PendingDhtResponses::new();
        let dna = test_dna(1);

        let handler = ProxySpaceHandler {
            space_id: dna.to_k2_space(),
            agent_proxy,
            pending_dht_responses: pending.clone(),
            conductor_links: Default::default(),
        };

        let msg_id = 55;
        let (tx, rx) = oneshot::channel();
        pending.register(msg_id, tx).await;

        let wire_msg = WireMessage::ErrorRes {
            msg_id,
            error: "remote DHT error".to_string(),
        };
        let encoded = WireMessage::encode_batch(&[&wire_msg]).expect("encode");

        let from_peer = Url::from_str("ws://localhost:5000").unwrap();
        let result = handler.recv_notify(from_peer, dna.to_k2_space(), encoded);
        assert!(result.is_ok());

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let received = rx.await.expect("pending request should receive response");
        match received {
            WireMessage::ErrorRes { msg_id: id, error } => {
                assert_eq!(id, msg_id);
                assert_eq!(error, "remote DHT error");
            }
            other => panic!("Expected ErrorRes, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_recv_notify_unmatched_response_does_not_panic() {
        let agent_proxy = AgentProxyManager::new();
        let pending = PendingDhtResponses::new();
        let dna = test_dna(1);

        let handler = ProxySpaceHandler {
            space_id: dna.to_k2_space(),
            agent_proxy,
            pending_dht_responses: pending,
            conductor_links: Default::default(),
        };

        // Send a GetRes with no matching pending request
        use holochain_types::record::WireRecordOps;
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
        let encoded = WireMessage::encode_batch(&[&wire_msg]).expect("encode");

        let from_peer = Url::from_str("ws://localhost:5000").unwrap();
        let result = handler.recv_notify(from_peer, dna.to_k2_space(), encoded);
        assert!(result.is_ok());

        // Give spawned task time to complete without panicking
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    #[test]
    fn test_wire_message_encode_decode_get_req() {
        let to_agent = test_agent(0xab);
        let hash = holo_hash::AnyDhtHash::from(holo_hash::ActionHash::from_raw_32(vec![0x11; 32]));
        let (_msg_id, req) = WireMessage::get_req(to_agent.clone(), hash.clone());

        let encoded = WireMessage::encode_batch(&[&req]).expect("encode");
        let decoded = WireMessage::decode_batch(&encoded).expect("decode");
        assert_eq!(decoded.len(), 1);

        match &decoded[0] {
            WireMessage::GetReq {
                to_agent: decoded_agent,
                dht_hash,
                ..
            } => {
                assert_eq!(decoded_agent.get_raw_36(), to_agent.get_raw_36());
                assert_eq!(dht_hash, &hash);
            }
            other => panic!("Expected GetReq, got {other:?}"),
        }
    }

    #[test]
    fn test_wire_message_encode_decode_get_res() {
        use holochain_types::record::WireRecordOps;

        let msg_id = 42;
        let response = WireOps::Record(WireRecordOps {
            action: None,
            deletes: vec![],
            updates: vec![],
            entry: None,
            warrants: vec![],
        });
        let wire_msg = WireMessage::get_res(msg_id, response);

        let encoded = WireMessage::encode_batch(&[&wire_msg]).expect("encode");
        let decoded = WireMessage::decode_batch(&encoded).expect("decode");
        assert_eq!(decoded.len(), 1);

        match &decoded[0] {
            WireMessage::GetRes {
                msg_id: id,
                response,
            } => {
                assert_eq!(*id, msg_id);
                match response {
                    WireOps::Record(ops) => {
                        assert!(ops.action.is_none());
                        assert!(ops.deletes.is_empty());
                    }
                    other => panic!("Expected Record WireOps, got {other:?}"),
                }
            }
            other => panic!("Expected GetRes, got {other:?}"),
        }
    }

    #[test]
    fn test_wire_message_encode_decode_get_links_req() {
        use holochain_p2p::event::GetLinksOptions;
        use holochain_types::link::WireLinkKey;
        use holochain_types::prelude::LinkTypeFilter;

        let to_agent = test_agent(0xcc);
        let base =
            holo_hash::AnyLinkableHash::from(holo_hash::EntryHash::from_raw_32(vec![0x22; 32]));
        let link_key = WireLinkKey {
            base,
            type_query: LinkTypeFilter::Types(vec![]),
            tag: None,
            after: None,
            before: None,
            author: None,
        };
        let (_msg_id, req) =
            WireMessage::get_links_req(to_agent.clone(), link_key.clone(), GetLinksOptions {});

        let encoded = WireMessage::encode_batch(&[&req]).expect("encode");
        let decoded = WireMessage::decode_batch(&encoded).expect("decode");
        assert_eq!(decoded.len(), 1);

        match &decoded[0] {
            WireMessage::GetLinksReq {
                to_agent: decoded_agent,
                link_key: decoded_key,
                ..
            } => {
                assert_eq!(decoded_agent.get_raw_36(), to_agent.get_raw_36());
                assert_eq!(decoded_key.base, link_key.base);
            }
            other => panic!("Expected GetLinksReq, got {other:?}"),
        }
    }

    #[test]
    fn test_wire_message_encode_decode_get_links_res() {
        use holochain_types::link::WireLinkOps;

        let msg_id = 99;
        let response = WireLinkOps {
            creates: vec![],
            deletes: vec![],
            warrants: vec![],
        };
        let wire_msg = WireMessage::get_links_res(msg_id, response);

        let encoded = WireMessage::encode_batch(&[&wire_msg]).expect("encode");
        let decoded = WireMessage::decode_batch(&encoded).expect("decode");
        assert_eq!(decoded.len(), 1);

        match &decoded[0] {
            WireMessage::GetLinksRes {
                msg_id: id,
                response,
            } => {
                assert_eq!(*id, msg_id);
                assert!(response.creates.is_empty());
                assert!(response.deletes.is_empty());
            }
            other => panic!("Expected GetLinksRes, got {other:?}"),
        }
    }

    #[test]
    fn test_wire_message_encode_decode_error_res() {
        let msg_id = 77;
        let wire_msg = WireMessage::ErrorRes {
            msg_id,
            error: "something went wrong".to_string(),
        };

        let encoded = WireMessage::encode_batch(&[&wire_msg]).expect("encode");
        let decoded = WireMessage::decode_batch(&encoded).expect("decode");
        assert_eq!(decoded.len(), 1);

        match &decoded[0] {
            WireMessage::ErrorRes { msg_id: id, error } => {
                assert_eq!(*id, msg_id);
                assert_eq!(error, "something went wrong");
            }
            other => panic!("Expected ErrorRes, got {other:?}"),
        }
    }
}
