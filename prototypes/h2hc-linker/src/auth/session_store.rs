//! Trait defining the persistable session/agent store operations.

use async_trait::async_trait;
use holochain_types::prelude::{AgentPubKey, DnaHash};

use super::types::{AllowedAgent, SessionInfo, SessionToken};

/// Errors from session store operations.
#[derive(Debug, thiserror::Error)]
pub enum SessionStoreError {
    #[error("database error: {0}")]
    Database(String),
}

pub type SessionStoreResult<T> = Result<T, SessionStoreError>;

/// Backend-agnostic store for allowed agents and sessions.
///
/// Implementations must be `Send + Sync` (shared across async tasks).
/// WS sender tracking is intentionally excluded — it is a runtime concern
/// handled by `AuthStore` directly.
#[async_trait]
pub trait SessionStore: Send + Sync + std::fmt::Debug {
    // --- Agent management ---

    async fn add_agent(&self, agent: AllowedAgent) -> SessionStoreResult<()>;

    /// Remove an agent and all its sessions. Returns `true` if the agent existed.
    async fn remove_agent(&self, agent_pubkey: &AgentPubKey) -> SessionStoreResult<bool>;

    async fn list_agents(&self) -> SessionStoreResult<Vec<AllowedAgent>>;

    async fn is_agent_allowed(&self, agent_pubkey: &AgentPubKey) -> SessionStoreResult<bool>;

    async fn get_agent(
        &self,
        agent_pubkey: &AgentPubKey,
    ) -> SessionStoreResult<Option<AllowedAgent>>;

    // --- Session management ---

    /// Create a session for an allowed agent. Returns `None` if agent is not in the allowlist.
    async fn create_session(
        &self,
        agent_pubkey: &AgentPubKey,
    ) -> SessionStoreResult<Option<SessionToken>>;

    /// Look up a session by token.
    async fn validate_session(&self, token: &str) -> SessionStoreResult<Option<SessionInfo>>;

    /// Revoke a single session. Returns `true` if it existed.
    async fn revoke_session(&self, token: &str) -> SessionStoreResult<bool>;

    /// Register a DNA for all sessions belonging to an agent.
    async fn register_dna_for_agent(
        &self,
        agent_pubkey: &AgentPubKey,
        dna: &DnaHash,
    ) -> SessionStoreResult<()>;

    /// Revoke all sessions for an agent. Returns the number removed.
    async fn revoke_sessions_for_agent(
        &self,
        agent_pubkey: &AgentPubKey,
    ) -> SessionStoreResult<usize>;

    /// Total active session count.
    async fn session_count(&self) -> SessionStoreResult<usize>;
}
