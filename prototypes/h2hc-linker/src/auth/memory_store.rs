//! In-memory implementation of [`SessionStore`].

use async_trait::async_trait;
use holochain_types::prelude::{AgentPubKey, DnaHash};
use std::collections::HashMap;
use tokio::sync::RwLock;

use super::session_store::{SessionStore, SessionStoreResult};
use super::types::{AllowedAgent, SessionInfo, SessionToken};

/// Stores agents and sessions in memory (no persistence across restarts).
#[derive(Debug)]
pub struct MemorySessionStore {
    inner: RwLock<MemoryStoreInner>,
}

#[derive(Debug, Default)]
struct MemoryStoreInner {
    allowed_agents: HashMap<AgentPubKey, AllowedAgent>,
    sessions: HashMap<String, SessionInfo>,
}

impl Default for MemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemorySessionStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(MemoryStoreInner::default()),
        }
    }
}

#[async_trait]
impl SessionStore for MemorySessionStore {
    async fn add_agent(&self, agent: AllowedAgent) -> SessionStoreResult<()> {
        let mut inner = self.inner.write().await;
        inner
            .allowed_agents
            .insert(agent.agent_pubkey.clone(), agent);
        Ok(())
    }

    async fn remove_agent(&self, agent_pubkey: &AgentPubKey) -> SessionStoreResult<bool> {
        let mut inner = self.inner.write().await;
        let removed = inner.allowed_agents.remove(agent_pubkey).is_some();
        if removed {
            inner
                .sessions
                .retain(|_, s| &s.agent_pubkey != agent_pubkey);
        }
        Ok(removed)
    }

    async fn list_agents(&self) -> SessionStoreResult<Vec<AllowedAgent>> {
        let inner = self.inner.read().await;
        Ok(inner.allowed_agents.values().cloned().collect())
    }

    async fn is_agent_allowed(&self, agent_pubkey: &AgentPubKey) -> SessionStoreResult<bool> {
        let inner = self.inner.read().await;
        Ok(inner.allowed_agents.contains_key(agent_pubkey))
    }

    async fn get_agent(
        &self,
        agent_pubkey: &AgentPubKey,
    ) -> SessionStoreResult<Option<AllowedAgent>> {
        let inner = self.inner.read().await;
        Ok(inner.allowed_agents.get(agent_pubkey).cloned())
    }

    async fn create_session(
        &self,
        agent_pubkey: &AgentPubKey,
    ) -> SessionStoreResult<Option<SessionToken>> {
        let inner = self.inner.read().await;
        let Some(allowed) = inner.allowed_agents.get(agent_pubkey) else {
            return Ok(None);
        };
        let capabilities = allowed.capabilities.clone();
        drop(inner);

        let token = SessionToken::generate();
        let info = SessionInfo {
            agent_pubkey: agent_pubkey.clone(),
            capabilities,
            registered_dnas: std::collections::HashSet::new(),
        };

        let mut inner = self.inner.write().await;
        inner.sessions.insert(token.0.clone(), info);
        Ok(Some(token))
    }

    async fn validate_session(&self, token: &str) -> SessionStoreResult<Option<SessionInfo>> {
        let inner = self.inner.read().await;
        Ok(inner.sessions.get(token).cloned())
    }

    async fn revoke_session(&self, token: &str) -> SessionStoreResult<bool> {
        let mut inner = self.inner.write().await;
        Ok(inner.sessions.remove(token).is_some())
    }

    async fn register_dna_for_agent(
        &self,
        agent_pubkey: &AgentPubKey,
        dna: &DnaHash,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.write().await;
        for session in inner.sessions.values_mut() {
            if &session.agent_pubkey == agent_pubkey {
                session.registered_dnas.insert(dna.clone());
            }
        }
        Ok(())
    }

    async fn revoke_sessions_for_agent(
        &self,
        agent_pubkey: &AgentPubKey,
    ) -> SessionStoreResult<usize> {
        let mut inner = self.inner.write().await;
        let before = inner.sessions.len();
        inner
            .sessions
            .retain(|_, s| &s.agent_pubkey != agent_pubkey);
        Ok(before - inner.sessions.len())
    }

    async fn session_count(&self) -> SessionStoreResult<usize> {
        let inner = self.inner.read().await;
        Ok(inner.sessions.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    session_store_test_suite!(MemorySessionStore::new());
}
