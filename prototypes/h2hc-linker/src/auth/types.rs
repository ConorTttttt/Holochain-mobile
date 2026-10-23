//! Core authentication data types.

use holochain_types::prelude::{AgentPubKey, DnaHash};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Capability levels for agent access control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// GET /dht/* endpoints (record, details, links)
    DhtRead,
    /// POST /dht/*/publish
    DhtWrite,
    /// GET /k2/* endpoints
    K2,
}

/// An agent that has been granted access to the gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowedAgent {
    pub agent_pubkey: AgentPubKey,
    pub capabilities: HashSet<Capability>,
    #[serde(default)]
    pub label: Option<String>,
}

/// Opaque session token -- 32 random bytes, hex-encoded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionToken(pub String);

impl SessionToken {
    /// Generate a new random session token.
    pub fn generate() -> Self {
        use rand::Rng;
        let bytes: [u8; 32] = rand::rng().random();
        Self(hex::encode(bytes))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Information about an active session.
///
/// Sessions live until the WebSocket disconnects or the agent is removed.
/// No TTL — cleanup is connection-based.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub agent_pubkey: AgentPubKey,
    pub capabilities: HashSet<Capability>,
    /// DNAs this session is authorized to access (populated via Register messages).
    pub registered_dnas: HashSet<DnaHash>,
}

impl SessionInfo {
    pub fn has_capability(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }

    pub fn has_dna(&self, dna: &DnaHash) -> bool {
        self.registered_dnas.contains(dna)
    }
}

/// Auth context injected into request extensions by middleware.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub agent_pubkey: AgentPubKey,
    pub capabilities: HashSet<Capability>,
    pub registered_dnas: HashSet<DnaHash>,
}

impl AuthContext {
    pub fn has_dna(&self, dna: &DnaHash) -> bool {
        self.registered_dnas.contains(dna)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_serde_roundtrip() {
        let caps = vec![Capability::DhtRead, Capability::DhtWrite, Capability::K2];
        for cap in &caps {
            let json = serde_json::to_string(cap).unwrap();
            let parsed: Capability = serde_json::from_str(&json).unwrap();
            assert_eq!(*cap, parsed);
        }
    }

    #[test]
    fn test_capability_snake_case_serialization() {
        assert_eq!(
            serde_json::to_string(&Capability::DhtRead).unwrap(),
            "\"dht_read\""
        );
        assert_eq!(
            serde_json::to_string(&Capability::DhtWrite).unwrap(),
            "\"dht_write\""
        );
        assert_eq!(serde_json::to_string(&Capability::K2).unwrap(), "\"k2\"");
    }

    #[test]
    fn test_capability_deserialization_from_snake_case() {
        let read: Capability = serde_json::from_str("\"dht_read\"").unwrap();
        assert_eq!(read, Capability::DhtRead);
        let write: Capability = serde_json::from_str("\"dht_write\"").unwrap();
        assert_eq!(write, Capability::DhtWrite);
        let k2: Capability = serde_json::from_str("\"k2\"").unwrap();
        assert_eq!(k2, Capability::K2);
    }

    #[test]
    fn test_session_token_generate_is_64_hex_chars() {
        let token = SessionToken::generate();
        assert_eq!(token.as_str().len(), 64);
        assert!(token.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_session_token_uniqueness() {
        let a = SessionToken::generate();
        let b = SessionToken::generate();
        assert_ne!(a, b);
    }

    #[test]
    fn test_session_info_has_capability() {
        let info = SessionInfo {
            agent_pubkey: AgentPubKey::from_raw_32(vec![0u8; 32]),
            capabilities: HashSet::from([Capability::DhtRead, Capability::K2]),
            registered_dnas: HashSet::new(),
        };
        assert!(info.has_capability(Capability::DhtRead));
        assert!(info.has_capability(Capability::K2));
        assert!(!info.has_capability(Capability::DhtWrite));
    }

    #[test]
    fn test_session_info_has_dna() {
        let dna1 = DnaHash::from_raw_32(vec![1u8; 32]);
        let dna2 = DnaHash::from_raw_32(vec![2u8; 32]);
        let dna3 = DnaHash::from_raw_32(vec![3u8; 32]);
        let info = SessionInfo {
            agent_pubkey: AgentPubKey::from_raw_32(vec![0u8; 32]),
            capabilities: HashSet::from([Capability::DhtRead]),
            registered_dnas: HashSet::from([dna1.clone(), dna2.clone()]),
        };
        assert!(info.has_dna(&dna1));
        assert!(info.has_dna(&dna2));
        assert!(!info.has_dna(&dna3));
    }
}
