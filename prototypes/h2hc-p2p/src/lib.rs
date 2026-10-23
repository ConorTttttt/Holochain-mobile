//! An `HcP2p` for a zero-arc phone conductor that reaches the network
//! through an H2HC linker instead of running Kitsune2 itself.
//!
//! Each method follows `research/hcp2p-compat-matrix.md`: forwarded methods go
//! to the linker over [`h2hc_protocol`]; the rest are answered locally, return
//! an explicit unsupported error, or (diagnostics only) return empty results.
//!
//! Construct it with [`H2hcP2p::connect`], or hand [`factory`] to a
//! `ConductorBuilder` that accepts a network factory.

mod inbound;
mod peer_store;
pub mod session;
pub mod ws;

pub use session::{FrameChannel, InboundHandler, LinkerSession};

use futures::future::BoxFuture;
use h2hc_protocol::*;
use holochain_keystore::MetaLairClient;
use holochain_p2p::actor::*;
use holochain_p2p::event::{CountersigningSessionNegotiationMessage, DynHcP2pHandler};
use holochain_p2p::{
    build_chain_dht_op, build_warrant_dht_op, GetConductorStore, GetDhtStore, HolochainP2pConfig,
    HolochainP2pError, HolochainP2pResult,
};
use holochain_types::cell_config_overrides::CellConfigOverrides;
use holochain_types::network::{Kitsune2NetworkMetrics, Kitsune2NetworkMetricsRequest};
use holochain_types::prelude::*;
use inbound::Inbound;
use kitsune2_api::{AgentInfoSigned, BoxFut, DhtArc, SpaceId, StoredOp};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Extra time given to the linker beyond a request's own timeout, so the
/// linker's timeout error arrives before the phone gives up.
const LINKER_SLACK: Duration = Duration::from_secs(2);

/// Which answers the adapter passes to the conductor.
#[derive(Debug, Clone)]
pub struct SourcePolicy {
    /// Accept data from the linker's publish buffer. It carries no authority
    /// judgment, so this is off unless a deployment chooses that trust.
    pub accept_linker_buffer: bool,
    /// Accept the linker's cached copies of earlier authority answers.
    pub accept_linker_cache: bool,
}

impl Default for SourcePolicy {
    fn default() -> Self {
        Self {
            accept_linker_buffer: false,
            accept_linker_cache: true,
        }
    }
}

/// Where verified acceptance receipts go. Recovery needs them, so a real
/// phone stores them durably.
pub trait AcceptanceLog: 'static + Send + Sync + std::fmt::Debug {
    fn record(
        &self,
        receipt: AcceptanceReceipt,
        body: AcceptanceBody,
    ) -> BoxFuture<'_, Result<(), String>>;
}

/// Keeps receipts in memory. For tests and early prototypes only.
#[derive(Debug, Default)]
pub struct MemoryAcceptanceLog(pub Mutex<Vec<(AcceptanceReceipt, AcceptanceBody)>>);

impl AcceptanceLog for MemoryAcceptanceLog {
    fn record(
        &self,
        receipt: AcceptanceReceipt,
        body: AcceptanceBody,
    ) -> BoxFuture<'_, Result<(), String>> {
        self.0.lock().unwrap().push((receipt, body));
        Box::pin(async { Ok(()) })
    }
}

pub struct H2hcOptions {
    pub hello: Hello,
    /// Pin the linker's receipt-signing key. Strongly recommended.
    pub expected_linker: Option<LinkerId>,
    pub handshake_timeout: Duration,
    pub source_policy: SourcePolicy,
    pub acceptance_log: Arc<dyn AcceptanceLog>,
}

impl Default for H2hcOptions {
    fn default() -> Self {
        Self {
            hello: Hello {
                versions: vec![PROTOCOL_VERSION],
                session_token: None,
            },
            expected_linker: None,
            handshake_timeout: Duration::from_secs(10),
            source_policy: SourcePolicy::default(),
            acceptance_log: Arc::new(MemoryAcceptanceLog::default()),
        }
    }
}

/// A network factory: given the conductor's p2p config and keystore, return
/// the `HcP2p` it should use.
pub type P2pFactory = Box<
    dyn FnOnce(HolochainP2pConfig, MetaLairClient) -> BoxFut<'static, HolochainP2pResult<DynHcP2p>>
        + Send,
>;

/// Build a [`P2pFactory`] that connects to a linker. `connect` opens the
/// byte transport and returns its frame channel.
pub fn factory<C>(connect: C, options: H2hcOptions) -> P2pFactory
where
    C: FnOnce() -> BoxFut<'static, Result<FrameChannel, ProtocolError>> + Send + 'static,
{
    Box::new(move |config, keystore| {
        Box::pin(async move {
            let chan = connect().await.map_err(HolochainP2pError::other)?;
            H2hcP2p::connect(config, keystore, chan, options).await
        })
    })
}

pub struct H2hcP2p {
    session: Arc<LinkerSession>,
    inbound: Arc<Inbound>,
    get_dht_store: GetDhtStore,
    get_conductor_store: GetConductorStore,
    request_timeout: Duration,
    source_policy: SourcePolicy,
    acceptance_log: Arc<dyn AcceptanceLog>,
}

impl std::fmt::Debug for H2hcP2p {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H2hcP2p")
            .field("session", &self.session)
            .field("source_policy", &self.source_policy)
            .finish()
    }
}

impl H2hcP2p {
    pub async fn connect(
        config: HolochainP2pConfig,
        keystore: MetaLairClient,
        chan: FrameChannel,
        options: H2hcOptions,
    ) -> HolochainP2pResult<DynHcP2p> {
        let inbound = Arc::new(Inbound::new(keystore));
        let session = LinkerSession::connect(
            chan,
            options.hello,
            options.expected_linker,
            options.handshake_timeout,
            inbound.clone(),
        )
        .await
        .map_err(HolochainP2pError::other)?;
        Ok(Arc::new(Self {
            session,
            inbound,
            get_dht_store: config.get_dht_store,
            get_conductor_store: config.get_conductor_store,
            request_timeout: config.request_timeout,
            source_policy: options.source_policy,
            acceptance_log: options.acceptance_log,
        }))
    }

    fn timeout(&self, timeout_ms: Option<u64>) -> Duration {
        timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(self.request_timeout)
            + LINKER_SLACK
    }

    async fn request(
        &self,
        op: &'static str,
        body: Request,
        timeout: Duration,
    ) -> HolochainP2pResult<Response> {
        self.session
            .request(body, timeout)
            .await
            .map_err(|e| to_p2p_error(op, e))
    }

    /// Keep only answers whose source the policy accepts.
    fn admit<T>(&self, op: &'static str, answers: Vec<Sourced<T>>) -> Vec<T> {
        let total = answers.len();
        let kept: Vec<T> = answers
            .into_iter()
            .filter(|a| match a.source {
                DataSource::Authority { .. } => true,
                DataSource::LinkerCache { .. } => self.source_policy.accept_linker_cache,
                DataSource::LinkerBuffer => self.source_policy.accept_linker_buffer,
            })
            .map(|a| a.data)
            .collect();
        if kept.len() < total {
            tracing::debug!(op, dropped = total - kept.len(), "dropped answers the source policy does not accept");
        }
        kept
    }

    /// Read the locally validated ops to publish.
    async fn load_ops(
        &self,
        dna_hash: DnaHash,
        op_hashes: &[DhtOpHash],
    ) -> HolochainP2pResult<Vec<DhtOp>> {
        let store = (self.get_dht_store)(dna_hash).await?;
        let raw: Vec<Vec<u8>> = op_hashes.iter().map(|h| h.get_raw_36().to_vec()).collect();
        let (chain_rows, warrant_rows) = futures::future::try_join(
            store.get_chain_ops_for_wire(&raw),
            store.get_warrants_for_wire(&raw),
        )
        .await
        .map_err(HolochainP2pError::other)?;
        let mut ops = Vec::with_capacity(op_hashes.len());
        for row in chain_rows {
            ops.push(build_chain_dht_op(row).map_err(HolochainP2pError::other)?);
        }
        for row in warrant_rows {
            ops.push(build_warrant_dht_op(row).map_err(HolochainP2pError::other)?);
        }
        let found: HashSet<DhtOpHash> = ops
            .iter()
            .map(|op| DhtOpHashed::from_content_sync(op.clone()).into_hash())
            .collect();
        let missing = op_hashes.iter().filter(|h| !found.contains(*h)).count();
        if missing > 0 {
            return Err(HolochainP2pError::other(format!(
                "{missing} of {} ops to publish are not in the local DHT store yet",
                op_hashes.len()
            )));
        }
        Ok(ops)
    }
}

/// Check a publish receipt is signed by the pinned linker and covers exactly
/// the DNA, author and ops that were handed over.
pub fn check_acceptance(
    receipt: &AcceptanceReceipt,
    linker: &LinkerId,
    dna_hash: &DnaHash,
    author: &AgentPubKey,
    op_hashes: &[DhtOpHash],
) -> HolochainP2pResult<AcceptanceBody> {
    let accepted = receipt.verify(linker).map_err(HolochainP2pError::other)?;
    if accepted.dna_hash != *dna_hash || accepted.author != *author {
        return Err(HolochainP2pError::other(
            "acceptance receipt is for a different DNA or author",
        ));
    }
    let covered: HashSet<&DhtOpHash> = accepted.op_hashes.iter().collect();
    if let Some(h) = op_hashes.iter().find(|h| !covered.contains(h)) {
        return Err(HolochainP2pError::other(format!(
            "acceptance receipt does not cover op {h}"
        )));
    }
    Ok(accepted)
}

fn to_p2p_error(op: &'static str, e: ProtocolError) -> HolochainP2pError {
    match e.kind {
        ErrorKind::NoPeers { loc } => HolochainP2pError::NoPeersForLocation(op.into(), loc),
        ErrorKind::InsufficientResponses { received, required } => {
            HolochainP2pError::InsufficientResponses {
                operation: op.into(),
                received: received as usize,
                required: required as usize,
            }
        }
        ErrorKind::Malformed => HolochainP2pError::InvalidRequest(format!("{op}: {}", e.message)),
        _ => HolochainP2pError::other(e),
    }
}

fn unexpected(op: &'static str, got: &Response) -> HolochainP2pError {
    HolochainP2pError::invalid_p2p_message(format!("{op}: unexpected linker response {got:?}"))
}

fn unsupported(op: &'static str) -> HolochainP2pError {
    HolochainP2pError::other(ProtocolError::unsupported(format!(
        "{op} is not supported on an H2HC phone conductor"
    )))
}

fn query_options(o: &NetworkRequestOptions) -> QueryOptions {
    QueryOptions {
        remote_agent_count: o.remote_agent_count,
        timeout_ms: o.timeout_ms,
        as_race: o.as_race,
    }
}

fn activity_options(o: &holochain_p2p::event::GetActivityOptions) -> ActivityOptions {
    ActivityOptions {
        include_valid_activity: o.include_valid_activity,
        include_rejected_activity: o.include_rejected_activity,
        include_warrants: o.include_warrants,
        include_full_records: o.include_full_records,
    }
}

impl HcP2p for H2hcP2p {
    #[cfg(feature = "test_utils")]
    fn test_kitsune(&self) -> &kitsune2_api::DynKitsune {
        unimplemented!("an H2HC phone conductor has no local Kitsune2 instance")
    }

    fn peer_store(
        &self,
        _dna_hash: DnaHash,
    ) -> BoxFut<'_, HolochainP2pResult<kitsune2_api::DynPeerStore>> {
        Box::pin(async { Ok(Arc::new(peer_store::EmptyPeerStore) as kitsune2_api::DynPeerStore) })
    }

    fn register_handler(&self, handler: DynHcP2pHandler) -> BoxFut<'_, HolochainP2pResult<()>> {
        Box::pin(async move {
            if self.inbound.set_handler(handler) {
                Ok(())
            } else {
                Err(HolochainP2pError::other("handler already registered"))
            }
        })
    }

    fn join(
        &self,
        dna_hash: DnaHash,
        agent_pub_key: AgentPubKey,
        _maybe_agent_info: Option<AgentInfoSigned>,
        config_override: Option<CellConfigOverrides>,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        Box::pin(async move {
            if config_override.is_some() {
                tracing::info!("ignoring bootstrap and relay overrides; the linker owns networking");
            }
            // Record first: the linker asks for an agent-info signature while
            // the join is still in flight.
            self.inbound.add_joined(dna_hash.clone(), agent_pub_key.clone());
            let result = self
                .request(
                    "join",
                    Request::Join {
                        dna_hash: dna_hash.clone(),
                        agent: agent_pub_key.clone(),
                    },
                    self.timeout(None),
                )
                .await;
            match result {
                Ok(Response::Done) => Ok(()),
                other => {
                    self.inbound.remove_joined(&dna_hash, &agent_pub_key);
                    match other {
                        Ok(got) => Err(unexpected("join", &got)),
                        Err(e) => Err(e),
                    }
                }
            }
        })
    }

    fn leave(
        &self,
        dna_hash: DnaHash,
        agent_pub_key: AgentPubKey,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        Box::pin(async move {
            // Stay joined until the linker answers, so it can still get the
            // leave tombstone signed.
            let result = self
                .request(
                    "leave",
                    Request::Leave {
                        dna_hash: dna_hash.clone(),
                        agent: agent_pub_key.clone(),
                    },
                    self.timeout(None),
                )
                .await;
            self.inbound.remove_joined(&dna_hash, &agent_pub_key);
            match result? {
                Response::Done => Ok(()),
                got => Err(unexpected("leave", &got)),
            }
        })
    }

    fn new_integrated_data(
        &self,
        _space_id: SpaceId,
        _ops: Vec<StoredOp>,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        // A zero-arc node does not gossip.
        Box::pin(async { Ok(()) })
    }

    fn call_remote(
        &self,
        dna_hash: DnaHash,
        to_agent: AgentPubKey,
        zome_call_params_serialized: ExternIO,
        signature: Signature,
        _zome_call_origin: Option<(ZomeName, FunctionName)>,
    ) -> BoxFut<'_, HolochainP2pResult<SerializedBytes>> {
        Box::pin(async move {
            let body = Request::CallRemote {
                dna_hash,
                to_agent,
                zome_call_params_serialized,
                signature,
            };
            match self.request("call_remote", body, self.timeout(None)).await? {
                Response::CallRemote(bytes) => Ok(bytes),
                got => Err(unexpected("call_remote", &got)),
            }
        })
    }

    fn send_remote_signal(
        &self,
        dna_hash: DnaHash,
        to_agent_list: Vec<(AgentPubKey, ExternIO, Signature)>,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        Box::pin(async move {
            let signals = to_agent_list
                .into_iter()
                .map(|(to_agent, zome_call_params_serialized, signature)| SignedRemoteSignal {
                    to_agent,
                    zome_call_params_serialized,
                    signature,
                })
                .collect();
            let body = Request::SendRemoteSignal { dna_hash, signals };
            match self.request("send_remote_signal", body, self.timeout(None)).await? {
                Response::Done => Ok(()),
                got => Err(unexpected("send_remote_signal", &got)),
            }
        })
    }

    fn send_remote_signal_direct(
        &self,
        dna_hash: DnaHash,
        agents: Vec<AgentPubKey>,
        signal: Vec<u8>,
        from_agent: AgentPubKey,
        signature: Signature,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        Box::pin(async move {
            let body = Request::SendRemoteSignalDirect {
                dna_hash,
                to_agents: agents,
                signal,
                from_agent,
                signature,
            };
            match self
                .request("send_remote_signal_direct", body, self.timeout(None))
                .await?
            {
                Response::Done => Ok(()),
                got => Err(unexpected("send_remote_signal_direct", &got)),
            }
        })
    }

    /// `Ok` means the linker has durably taken custody of every op and signed
    /// for it. Delivery to authorities is evidenced later by validation
    /// receipts, which the conductor keeps waiting for and republishing.
    fn publish(
        &self,
        dna_hash: DnaHash,
        basis_hash: OpBasis,
        source: AgentPubKey,
        op_hash_list: Vec<DhtOpHash>,
        timeout_ms: Option<u64>,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        Box::pin(async move {
            if op_hash_list.is_empty() {
                return Ok(());
            }
            let ops = self.load_ops(dna_hash.clone(), &op_hash_list).await?;
            let body = Request::Publish(PublishRequest {
                dna_hash: dna_hash.clone(),
                author: source.clone(),
                basis: basis_hash,
                ops,
            });
            let receipt = match self.request("publish", body, self.timeout(timeout_ms)).await? {
                Response::PublishAccepted(receipt) => receipt,
                got => return Err(unexpected("publish", &got)),
            };
            let accepted = check_acceptance(
                &receipt,
                &self.session.linker(),
                &dna_hash,
                &source,
                &op_hash_list,
            )?;
            self.acceptance_log
                .record(receipt, accepted)
                .await
                .map_err(HolochainP2pError::other)
        })
    }

    fn publish_countersign(
        &self,
        _dna_hash: DnaHash,
        _basis_hash: OpBasis,
        _op: ChainOp,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        Box::pin(async { Err(unsupported("publish_countersign")) })
    }

    fn get(
        &self,
        dna_hash: DnaHash,
        dht_hash: AnyDhtHash,
        options: NetworkRequestOptions,
        _zome_call_origin: Option<(ZomeName, FunctionName)>,
    ) -> BoxFut<'_, HolochainP2pResult<Vec<WireOps>>> {
        Box::pin(async move {
            let body = Request::Get {
                dna_hash,
                hash: dht_hash,
                options: query_options(&options),
            };
            match self.request("get", body, self.timeout(options.timeout_ms)).await? {
                Response::Get(answers) => Ok(self.admit("get", answers)),
                got => Err(unexpected("get", &got)),
            }
        })
    }

    fn get_links(
        &self,
        dna_hash: DnaHash,
        link_key: WireLinkKey,
        options: GetLinksRequestOptions,
        _zome_call_origin: Option<(ZomeName, FunctionName)>,
    ) -> BoxFut<'_, HolochainP2pResult<Vec<WireLinkOps>>> {
        Box::pin(async move {
            let net = &options.network_req_options;
            let body = Request::GetLinks {
                dna_hash,
                key: link_key,
                options: query_options(net),
            };
            match self.request("get_links", body, self.timeout(net.timeout_ms)).await? {
                Response::GetLinks(answers) => Ok(self.admit("get_links", answers)),
                got => Err(unexpected("get_links", &got)),
            }
        })
    }

    fn count_links(
        &self,
        dna_hash: DnaHash,
        query: WireLinkQuery,
        options: NetworkRequestOptions,
        _zome_call_origin: Option<(ZomeName, FunctionName)>,
    ) -> BoxFut<'_, HolochainP2pResult<CountLinksResponse>> {
        Box::pin(async move {
            let loc = query.base.get_loc();
            let body = Request::CountLinks {
                dna_hash,
                query,
                options: query_options(&options),
            };
            match self.request("count_links", body, self.timeout(options.timeout_ms)).await? {
                Response::CountLinks(answers) => self
                    .admit("count_links", answers)
                    .into_iter()
                    .next()
                    .ok_or_else(|| HolochainP2pError::NoPeersForLocation("count_links".into(), loc)),
                got => Err(unexpected("count_links", &got)),
            }
        })
    }

    fn get_agent_activity(
        &self,
        dna_hash: DnaHash,
        agent: AgentPubKey,
        query: ChainQueryFilter,
        options: GetActivityOptions,
        _zome_call_origin: Option<(ZomeName, FunctionName)>,
    ) -> BoxFut<'_, HolochainP2pResult<Vec<AgentActivityResponse>>> {
        Box::pin(async move {
            let net = &options.network_req_options;
            let body = Request::GetAgentActivity {
                dna_hash,
                agent,
                filter: query,
                activity: activity_options(&(&options).into()),
                options: query_options(net),
            };
            match self
                .request("get_agent_activity", body, self.timeout(net.timeout_ms))
                .await?
            {
                Response::AgentActivity(answers) => Ok(self.admit("get_agent_activity", answers)),
                got => Err(unexpected("get_agent_activity", &got)),
            }
        })
    }

    /// Only authority answers count, one per distinct responder: a cached or
    /// buffered copy is not an independent response.
    fn get_agent_activity_multi(
        &self,
        dna_hash: DnaHash,
        agent: AgentPubKey,
        query: ChainQueryFilter,
        options: GetActivityMultiOptions,
    ) -> BoxFut<'_, HolochainP2pResult<Vec<(AgentPubKey, AgentActivityResponse)>>> {
        Box::pin(async move {
            const OP: &str = "get_agent_activity_multi";
            if options.required_responses == 0
                || options.required_responses > options.target_peer_count
            {
                return Err(HolochainP2pError::InvalidRequest(format!(
                    "{OP}: required_responses {} must be between 1 and target_peer_count {}",
                    options.required_responses, options.target_peer_count
                )));
            }
            let body = Request::GetAgentActivityMulti {
                dna_hash,
                agent,
                filter: query,
                activity: activity_options(&options.remote_options),
                target_peer_count: options.target_peer_count,
                required_responses: options.required_responses,
                timeout_ms: options.timeout_ms,
            };
            let answers = match self.request(OP, body, self.timeout(options.timeout_ms)).await? {
                Response::AgentActivity(answers) => answers,
                got => return Err(unexpected(OP, &got)),
            };
            let mut seen = HashSet::new();
            let out: Vec<(AgentPubKey, AgentActivityResponse)> = answers
                .into_iter()
                .filter_map(|a| match a.source {
                    DataSource::Authority { responder } if seen.insert(responder.clone()) => {
                        Some((responder, a.data))
                    }
                    _ => None,
                })
                .collect();
            if out.len() < options.required_responses as usize {
                return Err(HolochainP2pError::InsufficientResponses {
                    operation: OP.into(),
                    received: out.len(),
                    required: options.required_responses as usize,
                });
            }
            Ok(out)
        })
    }

    /// Validation depends on this answer, so only authority answers count,
    /// whatever the source policy says.
    fn must_get_agent_activity(
        &self,
        dna_hash: DnaHash,
        author: AgentPubKey,
        filter: ChainFilter,
        options: NetworkRequestOptions,
        _zome_call_origin: Option<(ZomeName, FunctionName)>,
    ) -> BoxFut<'_, HolochainP2pResult<Vec<MustGetAgentActivityResponse>>> {
        Box::pin(async move {
            let body = Request::MustGetAgentActivity {
                dna_hash,
                author,
                filter,
                options: query_options(&options),
            };
            match self
                .request("must_get_agent_activity", body, self.timeout(options.timeout_ms))
                .await?
            {
                Response::MustGetAgentActivity(answers) => Ok(answers
                    .into_iter()
                    .filter(|a| matches!(a.source, DataSource::Authority { .. }))
                    .map(|a| a.data)
                    .collect()),
                got => Err(unexpected("must_get_agent_activity", &got)),
            }
        })
    }

    /// Only the validation-receipt workflow asks, and a zero-arc phone has no
    /// foreign ops to send receipts for.
    fn was_agent_recently_online(
        &self,
        _dna_hash: DnaHash,
        _agent: AgentPubKey,
    ) -> BoxFut<'_, HolochainP2pResult<bool>> {
        Box::pin(async { Ok(false) })
    }

    fn send_validation_receipts(
        &self,
        _dna_hash: DnaHash,
        _to_agent: AgentPubKey,
        _receipts: ValidationReceiptBundle,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        Box::pin(async { Err(unsupported("send_validation_receipts")) })
    }

    /// Always false, so every cascade read goes to the network.
    fn authority_for_hash(
        &self,
        _dna_hash: DnaHash,
        _basis: OpBasis,
    ) -> BoxFut<'_, HolochainP2pResult<bool>> {
        Box::pin(async { Ok(false) })
    }

    fn countersigning_session_negotiation(
        &self,
        _dna_hash: DnaHash,
        _agents: Vec<AgentPubKey>,
        _message: CountersigningSessionNegotiationMessage,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        Box::pin(async { Err(unsupported("countersigning_session_negotiation")) })
    }

    fn dump_network_metrics(
        &self,
        _request: Kitsune2NetworkMetricsRequest,
    ) -> BoxFut<'_, HolochainP2pResult<HashMap<DnaHash, Kitsune2NetworkMetrics>>> {
        Box::pin(async { Ok(HashMap::new()) })
    }

    fn dump_network_stats(&self) -> BoxFut<'_, HolochainP2pResult<kitsune2_api::ApiTransportStats>> {
        Box::pin(async {
            Ok(kitsune2_api::ApiTransportStats {
                transport_stats: kitsune2_api::TransportStats {
                    backend: "h2hc-linker".into(),
                    peer_urls: Vec::new(),
                    connections: Vec::new(),
                },
                blocked_message_counts: HashMap::new(),
            })
        })
    }

    fn target_arcs(&self, dna_hash: DnaHash) -> BoxFut<'_, HolochainP2pResult<Vec<DhtArc>>> {
        Box::pin(async move { Ok(vec![DhtArc::Empty; self.inbound.joined_count(&dna_hash)]) })
    }

    /// The local block is authoritative; telling the linker only saves it
    /// relaying traffic the conductor will refuse anyway.
    fn block(&self, block: Block) -> BoxFut<'_, HolochainP2pResult<()>> {
        Box::pin(async move {
            let store = (self.get_conductor_store)().await;
            store.block(block.clone()).await.map_err(|err| {
                HolochainP2pError::other(format!("Could not write block to database: {err}"))
            })?;
            let body = Request::Block(BlockRequest {
                target: block.target().clone().into(),
                reason: block.target().clone().into(),
                start: block.start(),
                end: block.end(),
            });
            if let Err(err) = self.request("block", body, self.timeout(None)).await {
                tracing::warn!(?err, "linker not told about block; blocking locally only");
            }
            Ok(())
        })
    }

    fn is_blocked(&self, target: BlockTargetId) -> BoxFut<'_, HolochainP2pResult<bool>> {
        Box::pin(async move {
            let store = (self.get_conductor_store)().await;
            store
                .as_read()
                .is_blocked(target, Timestamp::now())
                .await
                .map_err(|err| {
                    HolochainP2pError::other(format!("Could not read block from database: {err}"))
                })
        })
    }

    fn conductor_store_getter(&self) -> GetConductorStore {
        self.get_conductor_store.clone()
    }
}
