//! Conductor-facing protocol (`/conductor/v1`).
//!
//! Serves a full Holochain conductor (typically a zero-arc phone) whose
//! `HcP2p` is the linker, using `h2hc_protocol` frames over a WebSocket. Unlike
//! the browser API, it carries Holochain's wire types unflattened, labels
//! every answer's source, takes durable custody of published ops, and keeps
//! validation receipts in an inbox until the phone acknowledges them.

pub(crate) mod admission;
pub(crate) mod store;

use crate::dht_query::{DhtQuery, PendingDhtResponses};
use crate::gateway_kitsune::GatewayKitsune;
use crate::proxy_agent::AgentSigner;
use crate::temp_op_store::TempOpStoreHandle;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{IntoResponse, Response as HttpResponse};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use h2hc_protocol::Call;
use h2hc_protocol::*;
use holochain_p2p::WireMessage;
use holochain_types::prelude::*;
use kitsune2_api::{AgentInfo, DynSpace, Url};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use store::AcceptanceStore;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const RECENT_ACTIONS: usize = 500;

type AgentKey = (DnaHash, AgentPubKey);

fn internal(e: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::new(ErrorKind::Internal, e.to_string())
}

/// One phone connection.
pub(crate) struct Conn {
    id: u64,
    tx: mpsc::Sender<Frame>,
    calls: Mutex<HashMap<u64, oneshot::Sender<Result<Reply, ProtocolError>>>>,
    next_call: AtomicU64,
}

impl Conn {
    async fn send(&self, frame: Frame) -> bool {
        self.tx.send(frame).await.is_ok()
    }

    /// Ask the phone something and wait for its reply.
    async fn call(&self, body: Call, timeout: Duration) -> Result<Reply, ProtocolError> {
        let id = self.next_call.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.calls.lock().unwrap().insert(id, tx);
        if !self.send(Frame::Call { id, body }).await {
            self.calls.lock().unwrap().remove(&id);
            return Err(ProtocolError::new(
                ErrorKind::Offline,
                "phone connection closed",
            ));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(ProtocolError::new(
                ErrorKind::Offline,
                "phone connection closed",
            )),
            Err(_) => {
                self.calls.lock().unwrap().remove(&id);
                Err(ProtocolError::new(
                    ErrorKind::Timeout,
                    "phone did not reply",
                ))
            }
        }
    }

    fn resolve(&self, id: u64, body: Result<Reply, ProtocolError>) {
        if let Some(waiter) = self.calls.lock().unwrap().remove(&id) {
            let _ = waiter.send(body);
        }
    }
}

struct Inner {
    store: Arc<AcceptanceStore>,
    gateway: GatewayKitsune,
    dht_query: DhtQuery,
    pending: PendingDhtResponses,
    temp_ops: TempOpStoreHandle,
    leave_on_disconnect: bool,
    /// The live connection serving each joined agent.
    routes: RwLock<HashMap<AgentKey, Arc<Conn>>>,
    /// Agents that have joined through this protocol since startup. Incoming
    /// traffic for them is handled here, even while their phone is offline.
    known: RwLock<HashSet<AgentKey>>,
    blocked: RwLock<HashSet<BlockTargetId>>,
    /// Actions whose ops were most recently accepted, newest last, so tests
    /// can tell when a specific write is in the linker's custody.
    recent_actions: Mutex<VecDeque<ActionHash>>,
    next_conn: AtomicU64,
}

#[derive(Clone)]
pub struct ConductorLinks {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ConductorLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConductorLinks").finish_non_exhaustive()
    }
}

impl ConductorLinks {
    pub(crate) fn new(
        store: Arc<AcceptanceStore>,
        gateway: GatewayKitsune,
        dht_query: DhtQuery,
        pending: PendingDhtResponses,
        temp_ops: TempOpStoreHandle,
        leave_on_disconnect: bool,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                gateway,
                dht_query,
                pending,
                temp_ops,
                leave_on_disconnect,
                routes: Default::default(),
                known: Default::default(),
                blocked: Default::default(),
                recent_actions: Default::default(),
                next_conn: AtomicU64::new(1),
            }),
        }
    }

    /// Make retained ops fetchable again after a restart.
    pub(crate) async fn reload_retained_ops(&self) -> anyhow::Result<usize> {
        let ops = self.inner.store.retained_ops()?;
        let n = ops.len();
        for (bytes, keep_for) in ops {
            if let Err(e) = self
                .inner
                .temp_ops
                .retain_op(Bytes::from(bytes), keep_for)
                .await
            {
                warn!(%e, "could not reload a retained op");
            }
        }
        Ok(n)
    }

    fn route(&self, key: &AgentKey) -> Option<Arc<Conn>> {
        self.inner.routes.read().unwrap().get(key).cloned()
    }

    /// Whether incoming traffic for this agent belongs to a conductor link.
    pub(crate) fn serves(&self, dna_hash: &DnaHash, agent: &AgentPubKey) -> bool {
        self.inner
            .known
            .read()
            .unwrap()
            .contains(&(dna_hash.clone(), agent.clone()))
    }

    /// Kitsune2 needs this agent's info signed; ask the phone serving it.
    pub(crate) async fn sign_agent_info(
        &self,
        agent: &AgentPubKey,
        agent_info: &AgentInfo,
    ) -> Result<Bytes, ProtocolError> {
        let key = (DnaHash::from_k2_space(&agent_info.space), agent.clone());
        let conn = self
            .route(&key)
            .ok_or_else(|| ProtocolError::new(ErrorKind::Offline, "phone not connected"))?;
        match conn
            .call(
                Call::SignAgentInfo {
                    agent_info: agent_info.clone(),
                },
                CALL_TIMEOUT,
            )
            .await?
        {
            Reply::AgentInfoSignature(sig) => Ok(Bytes::copy_from_slice(&sig.0)),
            other => Err(ProtocolError::malformed(format!(
                "expected a signature, got {other:?}"
            ))),
        }
    }

    // ---- connection lifecycle ----

    async fn serve(self, socket: WebSocket) {
        let (mut sink, mut stream) = socket.split();
        let (tx, mut rx) = mpsc::channel::<Frame>(256);
        tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                match codec::encode(&frame) {
                    Ok(bytes) => {
                        if sink.send(Message::Binary(bytes.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => warn!(?err, "dropping frame that cannot be encoded"),
                }
            }
        });

        let hello = tokio::time::timeout(HANDSHAKE_TIMEOUT, next_frame(&mut stream)).await;
        let Ok(Some(Frame::Hello(hello))) = hello else {
            debug!("conductor connection closed before Hello");
            return;
        };
        if !hello.versions.contains(&PROTOCOL_VERSION) {
            let _ = tx
                .send(Frame::Response {
                    id: 0,
                    body: Err(ProtocolError::new(
                        ErrorKind::VersionMismatch,
                        format!("linker speaks version {PROTOCOL_VERSION}"),
                    )),
                })
                .await;
            return;
        }
        let conn = Arc::new(Conn {
            id: self.inner.next_conn.fetch_add(1, Ordering::Relaxed),
            tx: tx.clone(),
            calls: Mutex::new(HashMap::new()),
            next_call: AtomicU64::new(1),
        });
        let _ = tx
            .send(Frame::Welcome(Welcome {
                version: PROTOCOL_VERSION,
                linker: self.inner.store.linker(),
            }))
            .await;
        info!(conn = conn.id, "conductor connected");

        while let Some(frame) = next_frame(&mut stream).await {
            match frame {
                Frame::Request { id, body } => {
                    let (links, conn) = (self.clone(), conn.clone());
                    tokio::spawn(async move {
                        let body = links.handle_request(&conn, body).await;
                        conn.send(Frame::Response { id, body }).await;
                    });
                }
                Frame::Reply { id, body } => conn.resolve(id, body),
                Frame::Ack { seq } => {
                    let (links, conn) = (self.clone(), conn.clone());
                    tokio::spawn(async move { links.ack(&conn, seq).await });
                }
                other => debug!(?other, "unexpected frame from conductor"),
            }
        }
        self.disconnected(&conn).await;
    }

    async fn disconnected(&self, conn: &Arc<Conn>) {
        let gone: Vec<AgentKey> = {
            let mut routes = self.inner.routes.write().unwrap();
            let gone: Vec<AgentKey> = routes
                .iter()
                .filter(|(_, c)| c.id == conn.id)
                .map(|(k, _)| k.clone())
                .collect();
            for key in &gone {
                routes.remove(key);
            }
            gone
        };
        for (_, waiter) in conn.calls.lock().unwrap().drain() {
            let _ = waiter.send(Err(ProtocolError::new(
                ErrorKind::Offline,
                "phone disconnected",
            )));
        }
        info!(
            conn = conn.id,
            agents = gone.len(),
            "conductor disconnected"
        );
        if self.inner.leave_on_disconnect {
            for (dna, agent) in gone {
                self.inner.gateway.agent_leave(&dna, &agent).await;
                self.inner.known.write().unwrap().remove(&(dna, agent));
            }
        }
    }

    async fn ack(&self, conn: &Arc<Conn>, seq: u64) {
        // Only the connection serving an item's agent may remove it.
        match self.inner.store.inbox_owner(seq) {
            Ok(Some(key)) if self.route(&key).is_some_and(|c| c.id == conn.id) => {
                if let Err(err) = self.inner.store.inbox_ack(seq) {
                    warn!(?err, seq, "could not remove acknowledged inbox item");
                }
            }
            Ok(_) => debug!(
                seq,
                "ignoring ack for an item this connection does not serve"
            ),
            Err(err) => warn!(?err, seq, "inbox lookup failed"),
        }
    }

    async fn deliver_inbox(&self, key: &AgentKey) {
        let Some(conn) = self.route(key) else { return };
        match self.inner.store.inbox_for(&key.0, &key.1) {
            Ok(items) => {
                for (seq, item) in items {
                    conn.send(Frame::Deliver { seq, item }).await;
                }
            }
            Err(err) => warn!(?err, "could not read inbox"),
        }
    }

    // ---- requests from the phone ----

    async fn handle_request(
        &self,
        conn: &Arc<Conn>,
        body: Request,
    ) -> Result<Response, ProtocolError> {
        let q = &self.inner.dht_query;
        match body {
            Request::Join { dna_hash, agent } => self.join(conn, dna_hash, agent).await,
            Request::Leave { dna_hash, agent } => {
                self.inner.gateway.agent_leave(&dna_hash, &agent).await;
                let key = (dna_hash, agent);
                self.inner.routes.write().unwrap().remove(&key);
                self.inner.known.write().unwrap().remove(&key);
                Ok(Response::Done)
            }
            Request::Publish(req) => self.publish(req).await,
            Request::Get {
                dna_hash,
                hash,
                options,
            } => q
                .sourced_get(&dna_hash, hash, &options)
                .await
                .map(Response::Get),
            Request::GetLinks {
                dna_hash,
                key,
                options,
            } => q
                .sourced_get_links(&dna_hash, key, &options)
                .await
                .map(Response::GetLinks),
            Request::CountLinks {
                dna_hash,
                query,
                options,
            } => q
                .sourced_count_links(&dna_hash, query, &options)
                .await
                .map(Response::CountLinks),
            Request::GetAgentActivity {
                dna_hash,
                agent,
                filter,
                activity,
                options,
            } => {
                let timeout = options
                    .timeout_ms
                    .map(Duration::from_millis)
                    .unwrap_or(q.default_timeout());
                q.sourced_agent_activity(
                    &dna_hash,
                    agent,
                    filter,
                    &activity,
                    options.remote_agent_count as usize,
                    options.as_race,
                    timeout,
                )
                .await
                .map(Response::AgentActivity)
            }
            Request::GetAgentActivityMulti {
                dna_hash,
                agent,
                filter,
                activity,
                target_peer_count,
                required_responses,
                timeout_ms,
            } => {
                let timeout = timeout_ms
                    .map(Duration::from_millis)
                    .unwrap_or(q.default_timeout());
                let answers = q
                    .sourced_agent_activity(
                        &dna_hash,
                        agent,
                        filter,
                        &activity,
                        target_peer_count as usize,
                        false,
                        timeout,
                    )
                    .await?;
                if answers.len() < required_responses as usize {
                    return Err(ProtocolError::new(
                        ErrorKind::InsufficientResponses {
                            received: answers.len() as u32,
                            required: required_responses as u32,
                        },
                        "too few authorities answered",
                    ));
                }
                Ok(Response::AgentActivity(answers))
            }
            Request::MustGetAgentActivity {
                dna_hash,
                author,
                filter,
                options,
            } => q
                .sourced_must_get_agent_activity(&dna_hash, author, filter, &options)
                .await
                .map(Response::MustGetAgentActivity),
            Request::CallRemote {
                dna_hash,
                to_agent,
                zome_call_params_serialized,
                signature,
            } => self
                .call_remote(dna_hash, to_agent, zome_call_params_serialized, signature)
                .await
                .map(Response::CallRemote),
            Request::SendRemoteSignal { dna_hash, signals } => {
                for s in signals {
                    let msg = WireMessage::remote_signal_evt(
                        s.to_agent.clone(),
                        s.zome_call_params_serialized,
                        s.signature,
                    );
                    self.notify_agent(&dna_hash, &s.to_agent, msg).await;
                }
                Ok(Response::Done)
            }
            Request::SendRemoteSignalDirect {
                dna_hash,
                to_agents,
                signal,
                from_agent,
                signature,
            } => {
                for to in to_agents {
                    let msg = WireMessage::remote_signal_direct_evt(
                        to.clone(),
                        signal.clone(),
                        from_agent.clone(),
                        signature.clone(),
                    );
                    self.notify_agent(&dna_hash, &to, msg).await;
                }
                Ok(Response::Done)
            }
            Request::Block(block) => {
                self.inner.blocked.write().unwrap().insert(block.target);
                Ok(Response::Done)
            }
        }
    }

    async fn join(
        &self,
        conn: &Arc<Conn>,
        dna_hash: DnaHash,
        agent: AgentPubKey,
    ) -> Result<Response, ProtocolError> {
        let key = (dna_hash.clone(), agent.clone());
        self.inner
            .routes
            .write()
            .unwrap()
            .insert(key.clone(), conn.clone());
        self.inner.known.write().unwrap().insert(key.clone());
        // An agent rejoining after a disconnect is still joined in Kitsune2;
        // its signer looks up the new connection, so nothing else changes.
        self.inner
            .gateway
            .agent_join_with_signer(&dna_hash, &agent, AgentSigner::Conductor(self.clone()))
            .await
            .map_err(internal)?;
        let links = self.clone();
        tokio::spawn(async move { links.deliver_inbox(&key).await });
        Ok(Response::Done)
    }

    async fn publish(&self, req: PublishRequest) -> Result<Response, ProtocolError> {
        if !self.serves(&req.dna_hash, &req.author) {
            return Err(ProtocolError::new(
                ErrorKind::NotJoined,
                "publishing agent has not joined through this linker",
            ));
        }
        let admitted = admission::admit(&req)?;
        let store = self.inner.store.clone();
        let (dna, author) = (req.dna_hash.clone(), req.author.clone());
        let (receipt, admitted) = tokio::task::spawn_blocking(move || {
            store
                .accept(&dna, &author, &admitted)
                .map(|r| (r, admitted))
        })
        .await
        .map_err(internal)??;

        // Committed: make the ops fetchable, then tell authorities.
        let mut op_ids = Vec::with_capacity(admitted.len());
        for op in admitted {
            match self
                .inner
                .temp_ops
                .retain_op(Bytes::from(op.bytes), self.inner.store.retention())
                .await
            {
                Ok(id) => op_ids.push(id),
                Err(e) => warn!(%e, "accepted op could not be made fetchable"),
            }
        }
        let (gateway, dna, loc) = (
            self.inner.gateway.clone(),
            req.dna_hash.clone(),
            req.basis.get_loc(),
        );
        tokio::spawn(async move {
            match gateway
                .publish_ops_requesting_receipts(&dna, op_ids, loc)
                .await
            {
                Ok(0) => debug!("no authorities reachable yet; the conductor will republish"),
                Ok(n) => debug!(peers = n, "published accepted ops"),
                Err(e) => warn!(%e, "publishing accepted ops failed"),
            }
        });
        {
            let mut recent = self.inner.recent_actions.lock().unwrap();
            for op in &req.ops {
                if let DhtOp::ChainOp(op) = op {
                    let action = ActionHash::with_data_sync(op.signed_action().data());
                    if !recent.contains(&action) {
                        recent.push_back(action);
                    }
                }
            }
            while recent.len() > RECENT_ACTIONS {
                recent.pop_front();
            }
        }
        Ok(Response::PublishAccepted(receipt))
    }

    async fn peer_url(&self, space: &DynSpace, agent: &AgentPubKey) -> Option<Url> {
        space
            .peer_store()
            .get(agent.to_k2_agent())
            .await
            .ok()
            .flatten()
            .and_then(|info| info.url.clone())
    }

    async fn send_wire(
        &self,
        space: &DynSpace,
        to: Url,
        msg: &WireMessage,
    ) -> Result<(), ProtocolError> {
        let bytes = WireMessage::encode_batch(&[msg]).map_err(internal)?;
        space.send_notify(to, bytes).await.map_err(internal)
    }

    /// Deliver a fire-and-forget message to an agent: directly if it is
    /// another phone on this linker, otherwise over Kitsune2.
    async fn notify_agent(&self, dna_hash: &DnaHash, to_agent: &AgentPubKey, msg: WireMessage) {
        if self.serves(dna_hash, to_agent) {
            self.on_incoming_local(dna_hash.clone(), msg).await;
            return;
        }
        let Ok(space) = self.inner.gateway.get_or_create_space(dna_hash).await else {
            return;
        };
        match self.peer_url(&space, to_agent).await {
            Some(url) => {
                if let Err(err) = self.send_wire(&space, url, &msg).await {
                    debug!(?err, "signal not sent");
                }
            }
            None => debug!(%to_agent, "signal target not in peer store"),
        }
    }

    async fn call_remote(
        &self,
        dna_hash: DnaHash,
        to_agent: AgentPubKey,
        params: ExternIO,
        signature: Signature,
    ) -> Result<SerializedBytes, ProtocolError> {
        if let Some(conn) = self.route(&(dna_hash.clone(), to_agent.clone())) {
            // Both agents are phones on this linker.
            return match conn
                .call(
                    Call::CallRemote {
                        dna_hash,
                        to_agent,
                        zome_call_params_serialized: params,
                        signature,
                    },
                    CALL_TIMEOUT,
                )
                .await?
            {
                Reply::CallRemote(bytes) => Ok(bytes),
                other => Err(ProtocolError::malformed(format!(
                    "unexpected reply {other:?}"
                ))),
            };
        }
        let space = self
            .inner
            .gateway
            .get_or_create_space(&dna_hash)
            .await
            .map_err(internal)?;
        let url = self.peer_url(&space, &to_agent).await.ok_or_else(|| {
            ProtocolError::new(
                ErrorKind::NoPeers {
                    loc: to_agent.get_loc(),
                },
                "target agent unknown",
            )
        })?;
        let (msg_id, req) = WireMessage::call_remote_req(to_agent, params, signature);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.register(msg_id, tx).await;
        if let Err(err) = self.send_wire(&space, url, &req).await {
            self.inner.pending.remove(msg_id).await;
            return Err(err);
        }
        match tokio::time::timeout(self.inner.dht_query.default_timeout(), rx).await {
            Ok(Ok(WireMessage::CallRemoteRes { response, .. })) => Ok(response),
            Ok(Ok(WireMessage::ErrorRes { error, .. })) => {
                Err(ProtocolError::new(ErrorKind::Remote, error))
            }
            Ok(Ok(other)) => Err(ProtocolError::malformed(format!(
                "unexpected response {other:?}"
            ))),
            Ok(Err(_)) => Err(internal("response channel dropped")),
            Err(_) => {
                self.inner.pending.remove(msg_id).await;
                Err(ProtocolError::new(
                    ErrorKind::Timeout,
                    "remote call timed out",
                ))
            }
        }
    }

    // ---- traffic from the network for phone agents ----

    /// Whether the gateway should hand this message to conductor links.
    pub(crate) fn claims(&self, dna_hash: &DnaHash, msg: &WireMessage) -> bool {
        let to_agent = match msg {
            WireMessage::PingReq { .. } => return true,
            WireMessage::CallRemoteReq { to_agent, .. }
            | WireMessage::RemoteSignalEvt { to_agent, .. }
            | WireMessage::RemoteSignalDirectEvt { to_agent, .. }
            | WireMessage::SendValidationReceiptsReq { to_agent, .. }
            | WireMessage::GetReq { to_agent, .. }
            | WireMessage::GetLinksReq { to_agent, .. }
            | WireMessage::CountLinksReq { to_agent, .. }
            | WireMessage::GetAgentActivityReq { to_agent, .. }
            | WireMessage::MustGetAgentActivityReq { to_agent, .. }
            | WireMessage::CountersigningSessionNegotiationEvt { to_agent, .. } => to_agent,
            _ => return false,
        };
        self.serves(dna_hash, to_agent)
    }

    /// Handle a message from another node for one of this linker's phones,
    /// answering `from_peer` where the protocol expects an answer.
    pub(crate) async fn on_incoming(&self, dna_hash: DnaHash, from_peer: Url, msg: WireMessage) {
        let Ok(space) = self.inner.gateway.get_or_create_space(&dna_hash).await else {
            return;
        };
        let answer = match msg {
            WireMessage::PingReq { msg_id } => Some(WireMessage::ping_res(msg_id)),
            WireMessage::CallRemoteReq {
                msg_id,
                to_agent,
                zome_call_params_serialized,
                signature,
            } => Some(
                match self.route(&(dna_hash.clone(), to_agent.clone())) {
                    None => Err("agent's phone is offline".to_string()),
                    Some(conn) => match conn
                        .call(
                            Call::CallRemote {
                                dna_hash: dna_hash.clone(),
                                to_agent,
                                zome_call_params_serialized,
                                signature,
                            },
                            CALL_TIMEOUT,
                        )
                        .await
                    {
                        Ok(Reply::CallRemote(bytes)) => Ok(bytes),
                        Ok(other) => Err(format!("unexpected reply {other:?}")),
                        Err(e) => Err(e.to_string()),
                    },
                }
                .map(|bytes| WireMessage::call_remote_res(msg_id, bytes))
                .unwrap_or_else(|error| WireMessage::ErrorRes { msg_id, error }),
            ),
            WireMessage::SendValidationReceiptsReq {
                msg_id,
                to_agent,
                receipts,
            } => {
                let item = InboxItem::ValidationReceipts {
                    dna_hash: dna_hash.clone(),
                    to_agent: to_agent.clone(),
                    receipts,
                    received_at: Timestamp::now(),
                };
                let store = self.inner.store.clone();
                let (dna, agent, stored) = (dna_hash.clone(), to_agent.clone(), item.clone());
                let pushed =
                    tokio::task::spawn_blocking(move || store.inbox_push(&dna, &agent, &stored))
                        .await
                        .map_err(internal)
                        .and_then(|r| r);
                Some(match pushed {
                    Ok(seq) => {
                        // Stored: the validator can stop retrying.
                        if let Some(conn) = self.route(&(dna_hash.clone(), to_agent)) {
                            conn.send(Frame::Deliver { seq, item }).await;
                        }
                        WireMessage::SendValidationReceiptsRes { msg_id }
                    }
                    Err(err) => WireMessage::ErrorRes {
                        msg_id,
                        error: err.to_string(),
                    },
                })
            }
            WireMessage::GetReq { msg_id, .. }
            | WireMessage::GetLinksReq { msg_id, .. }
            | WireMessage::CountLinksReq { msg_id, .. }
            | WireMessage::GetAgentActivityReq { msg_id, .. }
            | WireMessage::MustGetAgentActivityReq { msg_id, .. } => Some(WireMessage::ErrorRes {
                msg_id,
                error: "zero-arc agent holds no DHT data".into(),
            }),
            other => {
                self.on_incoming_local(dna_hash.clone(), other).await;
                None
            }
        };
        if let Some(answer) = answer {
            if let Err(err) = self.send_wire(&space, from_peer, &answer).await {
                debug!(?err, "could not answer peer");
            }
        }
    }

    /// Deliver fire-and-forget messages to a phone, if it is connected.
    async fn on_incoming_local(&self, dna_hash: DnaHash, msg: WireMessage) {
        let notification = match msg {
            WireMessage::RemoteSignalEvt {
                to_agent,
                zome_call_params_serialized,
                signature,
            } => Notification::RemoteSignal {
                dna_hash,
                to_agent,
                zome_call_params_serialized,
                signature,
            },
            WireMessage::RemoteSignalDirectEvt {
                to_agent,
                signal,
                from_agent,
                signature,
            } => Notification::RemoteSignalDirect {
                dna_hash,
                to_agent,
                signal,
                from_agent,
                signature,
            },
            other => {
                debug!(?other, "dropping message a zero-arc phone does not handle");
                return;
            }
        };
        let to_agent = match &notification {
            Notification::RemoteSignal { to_agent, .. }
            | Notification::RemoteSignalDirect { to_agent, .. } => to_agent.clone(),
        };
        let dna = match &notification {
            Notification::RemoteSignal { dna_hash, .. }
            | Notification::RemoteSignalDirect { dna_hash, .. } => dna_hash.clone(),
        };
        if let Some(conn) = self.route(&(dna, to_agent)) {
            conn.send(Frame::Notify(notification)).await;
        }
    }

    pub(crate) fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "connected_agents": self.inner.routes.read().unwrap().len(),
            "known_agents": self.inner.known.read().unwrap().len(),
            "acceptances": self.inner.store.acceptance_count().unwrap_or(0),
            "inbox": self.inner.store.inbox_len().unwrap_or(0),
            "recent_actions": self
                .inner
                .recent_actions
                .lock()
                .unwrap()
                .iter()
                .map(|h| h.to_string())
                .collect::<Vec<_>>(),
        })
    }
}

async fn next_frame(stream: &mut futures::stream::SplitStream<WebSocket>) -> Option<Frame> {
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Binary(bytes)) => match codec::decode(&bytes) {
                Ok(frame) => return Some(frame),
                Err(err) => warn!(?err, "undecodable frame from conductor"),
            },
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
    None
}

/// `GET /conductor/v1`
pub async fn conductor_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<crate::service::AppState>,
) -> HttpResponse {
    match state.conductor_links {
        Some(links) => ws
            .max_message_size(codec::MAX_FRAME_BYTES)
            .on_upgrade(move |socket| links.serve(socket)),
        None => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "conductor links are not enabled",
        )
            .into_response(),
    }
}

/// `GET /conductor/status`
pub async fn conductor_status_handler(
    State(state): State<crate::service::AppState>,
) -> axum::Json<serde_json::Value> {
    axum::Json(
        state
            .conductor_links
            .map(|l| l.status())
            .unwrap_or(serde_json::Value::Null),
    )
}
