//! Delivers linker-initiated frames to the conductor's `HcP2pHandler`.

use crate::session::InboundHandler;
use futures::future::BoxFuture;
use h2hc_protocol::*;
use h2hc_protocol::Call;
use holochain_keystore::MetaLairClient;
use holochain_p2p::event::DynHcP2pHandler;
use holochain_types::prelude::*;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// Agent infos created further than this from the phone's clock are refused.
const MAX_CLOCK_SKEW_MS: i64 = 5 * 60 * 1000;

pub(crate) struct Inbound {
    handler: OnceLock<DynHcP2pHandler>,
    keystore: MetaLairClient,
    /// Agents the conductor has joined through this adapter, by DNA.
    joined: Mutex<HashSet<(DnaHash, AgentPubKey)>>,
}

impl Inbound {
    pub(crate) fn new(keystore: MetaLairClient) -> Self {
        Self {
            handler: OnceLock::new(),
            keystore,
            joined: Mutex::new(HashSet::new()),
        }
    }

    pub(crate) fn set_handler(&self, handler: DynHcP2pHandler) -> bool {
        self.handler.set(handler).is_ok()
    }

    pub(crate) fn add_joined(&self, dna: DnaHash, agent: AgentPubKey) {
        self.joined.lock().unwrap().insert((dna, agent));
    }

    pub(crate) fn remove_joined(&self, dna: &DnaHash, agent: &AgentPubKey) {
        self.joined
            .lock()
            .unwrap()
            .remove(&(dna.clone(), agent.clone()));
    }

    pub(crate) fn joined_count(&self, dna: &DnaHash) -> usize {
        self.joined
            .lock()
            .unwrap()
            .iter()
            .filter(|(d, _)| d == dna)
            .count()
    }

    fn handler(&self) -> Result<&DynHcP2pHandler, ProtocolError> {
        self.handler.get().ok_or_else(|| {
            ProtocolError::new(ErrorKind::Retryable { retry_after_ms: Some(1_000) }, "conductor handler not registered yet")
        })
    }

    /// Refuse anything that would let the linker hold a stronger credential
    /// than "this agent is reachable through me for the next 20 minutes".
    fn check_agent_info(&self, info: &kitsune2_api::AgentInfo) -> Result<AgentPubKey, ProtocolError> {
        let refuse = |m: String| ProtocolError::new(ErrorKind::Rejected, m);
        let agent = AgentPubKey::from_k2_agent(&info.agent);
        let dna = DnaHash::from_k2_space(&info.space);
        if !self
            .joined
            .lock()
            .unwrap()
            .contains(&(dna.clone(), agent.clone()))
        {
            return Err(ProtocolError::new(
                ErrorKind::NotJoined,
                format!("{agent} has not joined {dna} through this linker"),
            ));
        }
        if info.storage_arc != kitsune2_api::DhtArc::Empty {
            return Err(refuse(format!(
                "agent info claims storage arc {:?}; a phone is zero-arc",
                info.storage_arc
            )));
        }
        if !info.is_tombstone && info.url.is_none() {
            return Err(refuse("live agent info without a URL".into()));
        }
        let created = info.created_at.as_micros();
        let lifetime_ms = (info.expires_at.as_micros() - created) / 1000;
        if lifetime_ms <= 0 || lifetime_ms > MAX_AGENT_INFO_LIFETIME_MS as i64 {
            return Err(refuse(format!(
                "agent info lifetime {lifetime_ms} ms is outside (0, {MAX_AGENT_INFO_LIFETIME_MS}]"
            )));
        }
        let skew_ms = (kitsune2_api::Timestamp::now().as_micros() - created).abs() / 1000;
        if skew_ms > MAX_CLOCK_SKEW_MS {
            return Err(refuse(format!("agent info created {skew_ms} ms from now")));
        }
        Ok(agent)
    }
}

impl InboundHandler for Inbound {
    fn call(&self, body: Call) -> BoxFuture<'_, Result<Reply, ProtocolError>> {
        Box::pin(async move {
            match body {
                Call::CallRemote {
                    dna_hash,
                    to_agent,
                    zome_call_params_serialized,
                    signature,
                } => self
                    .handler()?
                    .handle_call_remote(dna_hash, to_agent, zome_call_params_serialized, signature)
                    .await
                    .map(Reply::CallRemote)
                    .map_err(|e| ProtocolError::new(ErrorKind::Remote, e.to_string())),
                Call::SignAgentInfo { agent_info } => {
                    let agent = self.check_agent_info(&agent_info)?;
                    let message = serde_json::to_string(&agent_info)
                        .map_err(|e| ProtocolError::malformed(e.to_string()))?;
                    let signature = self
                        .keystore
                        .sign(agent, message.into_bytes().into())
                        .await
                        .map_err(|e| ProtocolError::new(ErrorKind::Internal, e.to_string()))?;
                    Ok(Reply::AgentInfoSignature(signature))
                }
            }
        })
    }

    fn notify(&self, body: Notification) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let Ok(handler) = self.handler() else {
                tracing::debug!("dropping notification before handler registration");
                return;
            };
            let result = match body {
                // holochain_p2p delivers remote signals as zome calls so the
                // conductor checks the signature and capability.
                Notification::RemoteSignal {
                    dna_hash,
                    to_agent,
                    zome_call_params_serialized,
                    signature,
                } => handler
                    .handle_call_remote(dna_hash, to_agent, zome_call_params_serialized, signature)
                    .await
                    .map(|_| ()),
                Notification::RemoteSignalDirect {
                    dna_hash,
                    to_agent,
                    signal,
                    from_agent,
                    signature,
                } => {
                    handler
                        .handle_remote_signal_direct(dna_hash, to_agent, signal, from_agent, signature)
                        .await
                }
            };
            if let Err(err) = result {
                tracing::debug!(?err, "remote signal not delivered");
            }
        })
    }

    fn deliver(&self, item: InboxItem) -> BoxFuture<'_, Result<(), ProtocolError>> {
        Box::pin(async move {
            match item {
                InboxItem::ValidationReceipts {
                    dna_hash,
                    to_agent,
                    receipts,
                    ..
                } => self
                    .handler()?
                    .handle_validation_receipts_received(dna_hash, to_agent, receipts)
                    .await
                    .map_err(|e| ProtocolError::new(ErrorKind::Internal, e.to_string())),
            }
        })
    }
}
