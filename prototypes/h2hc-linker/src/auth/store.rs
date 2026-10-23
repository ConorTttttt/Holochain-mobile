//! Thread-safe authentication store.
//!
//! Wraps a [`SessionStore`] backend (memory or SQLite) and adds
//! runtime-only WS connection tracking.

use super::session_store::SessionStore;
use super::types::*;
use crate::agent_proxy::WsSender;
use holochain_types::prelude::{AgentPubKey, DnaHash};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::memory_store::MemorySessionStore;

/// The shared authentication store.
///
/// Delegates agent/session persistence to a [`SessionStore`] backend.
/// WS sender tracking is always in-memory (runtime-only).
#[derive(Debug, Clone)]
pub struct AuthStore {
    store: Arc<dyn SessionStore>,
    ws_senders: Arc<RwLock<HashMap<AgentPubKey, Vec<WsSender>>>>,
}

impl Default for AuthStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthStore {
    /// Create with the default in-memory backend.
    pub fn new() -> Self {
        Self::with_store(Arc::new(MemorySessionStore::new()))
    }

    /// Create with a specific backend.
    pub fn with_store(store: Arc<dyn SessionStore>) -> Self {
        Self {
            store,
            ws_senders: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    // --- Agent management (delegated) ---

    pub async fn add_agent(&self, agent: AllowedAgent) {
        if let Err(e) = self.store.add_agent(agent).await {
            tracing::error!(error = %e, "Failed to add agent");
        }
    }

    /// Remove an agent. Revokes all sessions and closes all WS connections.
    pub async fn remove_agent(&self, agent_pubkey: &AgentPubKey) -> bool {
        match self.store.remove_agent(agent_pubkey).await {
            Ok(removed) => {
                if removed {
                    let mut ws = self.ws_senders.write().await;
                    ws.remove(agent_pubkey);
                }
                removed
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to remove agent");
                false
            }
        }
    }

    pub async fn list_agents(&self) -> Vec<AllowedAgent> {
        self.store.list_agents().await.unwrap_or_else(|e| {
            tracing::error!(error = %e, "Failed to list agents");
            Vec::new()
        })
    }

    pub async fn is_agent_allowed(&self, agent_pubkey: &AgentPubKey) -> bool {
        self.store
            .is_agent_allowed(agent_pubkey)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "Failed to check agent allowed");
                false
            })
    }

    pub async fn get_agent(&self, agent_pubkey: &AgentPubKey) -> Option<AllowedAgent> {
        self.store
            .get_agent(agent_pubkey)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "Failed to get agent");
                None
            })
    }

    // --- Session management (delegated) ---

    pub async fn create_session(&self, agent_pubkey: &AgentPubKey) -> Option<SessionToken> {
        self.store
            .create_session(agent_pubkey)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "Failed to create session");
                None
            })
    }

    pub async fn validate_session(&self, token: &str) -> Option<SessionInfo> {
        self.store
            .validate_session(token)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "Failed to validate session");
                None
            })
    }

    pub async fn revoke_session(&self, token: &str) -> bool {
        self.store.revoke_session(token).await.unwrap_or_else(|e| {
            tracing::error!(error = %e, "Failed to revoke session");
            false
        })
    }

    /// Register a DNA for all sessions belonging to an agent.
    /// Called when a client sends a Register message on the WebSocket.
    pub async fn register_dna_for_agent(&self, agent_pubkey: &AgentPubKey, dna: &DnaHash) {
        if let Err(e) = self.store.register_dna_for_agent(agent_pubkey, dna).await {
            tracing::error!(error = %e, "Failed to register DNA for agent");
        }
    }

    /// Revoke all sessions for an agent. Called on WebSocket disconnect.
    pub async fn revoke_sessions_for_agent(&self, agent_pubkey: &AgentPubKey) -> usize {
        self.store
            .revoke_sessions_for_agent(agent_pubkey)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "Failed to revoke sessions for agent");
                0
            })
    }

    /// Get total session count (for testing/monitoring).
    pub async fn session_count(&self) -> usize {
        self.store.session_count().await.unwrap_or_else(|e| {
            tracing::error!(error = %e, "Failed to get session count");
            0
        })
    }

    // --- WS connection tracking (always in-memory) ---

    /// Register a WS sender for an agent (called on successful WS auth).
    pub async fn register_ws_sender(&self, agent_pubkey: &AgentPubKey, sender: WsSender) {
        let mut ws = self.ws_senders.write().await;
        ws.entry(agent_pubkey.clone()).or_default().push(sender);
    }

    /// Unregister a WS sender for an agent (called on WS disconnect).
    /// Removes senders that are closed.
    pub async fn unregister_ws_sender(&self, agent_pubkey: &AgentPubKey, sender: &WsSender) {
        let mut ws = self.ws_senders.write().await;
        if let Some(senders) = ws.get_mut(agent_pubkey) {
            senders.retain(|s| !s.same_channel(sender));
            if senders.is_empty() {
                ws.remove(agent_pubkey);
            }
        }
    }

    /// Get count of active WS connections for an agent.
    pub async fn ws_connection_count(&self, agent_pubkey: &AgentPubKey) -> usize {
        let ws = self.ws_senders.read().await;
        ws.get(agent_pubkey).map_or(0, |v| v.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn test_agent(seed: u8) -> AgentPubKey {
        AgentPubKey::from_raw_32(vec![seed; 32])
    }

    fn test_allowed_agent(seed: u8, caps: &[Capability]) -> AllowedAgent {
        AllowedAgent {
            agent_pubkey: test_agent(seed),
            capabilities: caps.iter().copied().collect(),
            label: None,
        }
    }

    // WS sender tests (AuthStore-specific, not part of shared suite)

    #[tokio::test]
    async fn test_ws_sender_register_and_unregister() {
        let store = AuthStore::new();
        let (tx, _rx) = mpsc::channel(1);

        store.register_ws_sender(&test_agent(1), tx.clone()).await;
        assert_eq!(store.ws_connection_count(&test_agent(1)).await, 1);

        store.unregister_ws_sender(&test_agent(1), &tx).await;
        assert_eq!(store.ws_connection_count(&test_agent(1)).await, 0);
    }

    #[tokio::test]
    async fn test_remove_agent_drops_ws_senders() {
        let store = AuthStore::new();
        store
            .add_agent(test_allowed_agent(1, &[Capability::DhtRead]))
            .await;

        let (tx, _rx) = mpsc::channel(1);
        store.register_ws_sender(&test_agent(1), tx.clone()).await;
        assert_eq!(store.ws_connection_count(&test_agent(1)).await, 1);

        store.remove_agent(&test_agent(1)).await;
        assert_eq!(store.ws_connection_count(&test_agent(1)).await, 0);
    }

    #[tokio::test]
    async fn test_multiple_ws_connections_per_agent() {
        let store = AuthStore::new();
        let (tx1, _rx1) = mpsc::channel(1);
        let (tx2, _rx2) = mpsc::channel(1);

        store.register_ws_sender(&test_agent(1), tx1.clone()).await;
        store.register_ws_sender(&test_agent(1), tx2.clone()).await;
        assert_eq!(store.ws_connection_count(&test_agent(1)).await, 2);

        // Unregister one
        store.unregister_ws_sender(&test_agent(1), &tx1).await;
        assert_eq!(store.ws_connection_count(&test_agent(1)).await, 1);
    }
}
