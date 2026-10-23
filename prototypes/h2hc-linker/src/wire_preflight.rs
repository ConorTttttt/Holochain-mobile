//! Wire format for kitsune2 preflight messages.
//!
//! This module defines the preflight message format that must match
//! `holochain_p2p::types::wire::WirePreflightMessage` for compatibility
//! with Holochain conductors.
//!
//! Also provides `PreflightCache` and bootstrap wrappers to capture
//! `AgentInfoSigned` from local agents for inclusion in preflight messages.

use bytes::{BufMut, Bytes, BytesMut};
use kitsune2_api::{
    AgentInfoSigned, BoxFut, Builder, Config, DynBootstrap, DynBootstrapFactory, DynPeerStore,
    K2Result, SpaceId,
};
use std::sync::{Arc, Mutex};
use tracing::debug;

/// Network compatibility parameters.
///
/// Must match `holochain_p2p::NetworkCompatParams` exactly.
/// The protocol version is used during preflight to ensure compatible nodes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NetworkCompatParams {
    /// The current protocol version.
    /// Must match HCP2P_PROTO_VER (currently 2) from holochain_p2p.
    pub proto_ver: u32,
}

/// Current protocol version matching holochain_p2p::HCP2P_PROTO_VER
pub const HCP2P_PROTO_VER: u32 = 2;

impl Default for NetworkCompatParams {
    fn default() -> Self {
        Self {
            proto_ver: HCP2P_PROTO_VER,
        }
    }
}

/// Preflight message exchanged during kitsune2 peer handshake.
///
/// Must match `holochain_p2p::types::wire::WirePreflightMessage` format exactly.
/// Uses `rmp_serde::encode::write_named` for serialization (msgpack with field names).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct WirePreflightMessage {
    /// Compatibility parameters for protocol version checking.
    pub compat: NetworkCompatParams,
    /// Local agent infos as base64-encoded strings.
    /// Each string is a base64-encoded `AgentInfoSigned`.
    pub agents: Vec<String>,
}

impl WirePreflightMessage {
    /// Create a new preflight message with default compat params and no agents.
    pub fn new() -> Self {
        Self {
            compat: NetworkCompatParams::default(),
            agents: vec![],
        }
    }

    /// Create a new preflight message with the given agents.
    #[cfg(test)]
    pub fn with_agents(agents: Vec<String>) -> Self {
        Self {
            compat: NetworkCompatParams::default(),
            agents,
        }
    }

    /// Encode the preflight message to bytes.
    ///
    /// Uses `rmp_serde::encode::write_named` to match holochain_p2p encoding.
    pub fn encode(&self) -> Result<bytes::Bytes, Box<dyn std::error::Error + Send + Sync>> {
        let mut b = BufMut::writer(BytesMut::new());
        rmp_serde::encode::write_named(&mut b, self)?;
        Ok(b.into_inner().freeze())
    }

    /// Decode a preflight message from bytes.
    pub fn decode(data: &[u8]) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(rmp_serde::decode::from_slice(data)?)
    }
}

impl Default for WirePreflightMessage {
    fn default() -> Self {
        Self::new()
    }
}

/// Cache for preflight data that gets updated when agents are published.
///
/// This is shared between all `BootstrapWrapper` instances (one per space/DNA)
/// and `KitsuneProxy` (which reads it in `preflight_gather_outgoing`).
///
/// The agent cache is shared across all spaces, so agents from different DNAs
/// and different browser tabs are all included in the preflight. This ensures
/// that conductors will accept messages from any of our registered agents.
#[derive(Clone, Debug)]
pub struct PreflightCache {
    /// The encoded preflight message bytes.
    preflight: Arc<Mutex<Bytes>>,
    /// The compatibility parameters (constant).
    compat: NetworkCompatParams,
    /// Cached agent infos from all spaces (shared across BootstrapWrapper instances).
    agents: Arc<Mutex<Vec<Arc<AgentInfoSigned>>>>,
}

impl Default for PreflightCache {
    fn default() -> Self {
        Self::new()
    }
}

impl PreflightCache {
    /// Create a new preflight cache with default compat params.
    pub fn new() -> Self {
        // Initialize with empty agents
        let initial = WirePreflightMessage::new()
            .encode()
            .expect("encoding empty preflight should never fail");
        Self {
            preflight: Arc::new(Mutex::new(initial)),
            compat: NetworkCompatParams::default(),
            agents: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Get the current preflight bytes.
    pub fn get(&self) -> Bytes {
        self.preflight.lock().unwrap().clone()
    }

    /// Add or update an agent info and regenerate the preflight.
    ///
    /// This is called by BootstrapWrapper::put() when kitsune2 publishes
    /// agent info for any of our registered agents (from any space/DNA).
    pub fn add_agent(&self, info: Arc<AgentInfoSigned>) {
        let encoded_agents = {
            let mut agents = self.agents.lock().unwrap();

            // Remove expired infos and previous infos for the same agent+space
            let now = kitsune2_api::Timestamp::now();
            agents.retain(|cached| {
                if cached.expires_at < now {
                    return false;
                }
                if cached.agent == info.agent && cached.space == info.space {
                    return false;
                }
                true
            });

            // Add the new info
            agents.push(info);

            // Encode all agents to strings
            let mut encoded = Vec::new();
            for agent in agents.iter() {
                if let Ok(s) = agent.encode() {
                    encoded.push(s);
                }
            }

            encoded
        };

        debug!(
            agent_count = encoded_agents.len(),
            "Updated preflight cache with agent infos"
        );

        // Re-encode the preflight
        if let Ok(preflight_bytes) = (WirePreflightMessage {
            compat: self.compat.clone(),
            agents: encoded_agents,
        })
        .encode()
        {
            *self.preflight.lock().unwrap() = preflight_bytes;
        }
    }

    /// Get the number of cached agents (for debugging/testing).
    #[allow(dead_code)]
    pub fn agent_count(&self) -> usize {
        self.agents.lock().unwrap().len()
    }
}

/// Wrapper around a Bootstrap module that captures `AgentInfoSigned` for preflight.
///
/// This is modeled after `holochain_p2p::spawn::actor::BootWrap`.
/// When kitsune2 calls `put()` with a signed agent info, we:
/// 1. Add to the shared PreflightCache (which handles dedup and encoding)
/// 2. Forward to the original bootstrap module
///
/// Multiple BootstrapWrapper instances (one per space/DNA) share the same
/// PreflightCache, so agents from all spaces are included in the preflight.
#[derive(Debug)]
pub struct BootstrapWrapper {
    /// The shared preflight cache (contains agents from all spaces).
    cache: PreflightCache,
    /// The original bootstrap module.
    orig: DynBootstrap,
}

impl kitsune2_api::Bootstrap for BootstrapWrapper {
    fn put(&self, info: Arc<AgentInfoSigned>) {
        // Add to shared cache (handles dedup, expiry, and preflight encoding)
        self.cache.add_agent(info.clone());

        // Forward to original bootstrap
        self.orig.put(info);
    }
}

/// Factory that wraps a BootstrapFactory to capture agent infos for preflight.
#[derive(Debug)]
pub struct BootstrapWrapperFactory {
    /// The preflight cache shared with KitsuneProxy.
    cache: PreflightCache,
    /// The original bootstrap factory.
    orig: DynBootstrapFactory,
}

impl BootstrapWrapperFactory {
    /// Create a new factory that wraps the original and updates the given cache.
    pub fn new(cache: PreflightCache, orig: DynBootstrapFactory) -> Self {
        Self { cache, orig }
    }
}

impl kitsune2_api::BootstrapFactory for BootstrapWrapperFactory {
    fn default_config(&self, config: &mut Config) -> K2Result<()> {
        self.orig.default_config(config)
    }

    fn validate_config(&self, config: &Config) -> K2Result<()> {
        self.orig.validate_config(config)
    }

    fn create(
        &self,
        builder: Arc<Builder>,
        peer_store: DynPeerStore,
        space: SpaceId,
    ) -> BoxFut<'static, K2Result<DynBootstrap>> {
        let cache = self.cache.clone();
        let orig_fut = self.orig.create(builder, peer_store, space);
        Box::pin(async move {
            let orig = orig_fut.await?;
            // All BootstrapWrapper instances share the same PreflightCache,
            // so agents from all spaces (DNAs) are included in the preflight.
            let wrapped: DynBootstrap = Arc::new(BootstrapWrapper { cache, orig });
            Ok(wrapped)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_compat_params_default() {
        let params = NetworkCompatParams::default();
        assert_eq!(params.proto_ver, 2);
    }

    #[test]
    fn test_preflight_encode_decode_roundtrip() {
        let msg = WirePreflightMessage::new();
        let encoded = msg.encode().expect("encode");
        let decoded = WirePreflightMessage::decode(&encoded).expect("decode");

        assert_eq!(decoded.compat.proto_ver, 2);
        assert!(decoded.agents.is_empty());
    }

    #[test]
    fn test_preflight_with_agents() {
        let msg =
            WirePreflightMessage::with_agents(vec!["agent1".to_string(), "agent2".to_string()]);
        let encoded = msg.encode().expect("encode");
        let decoded = WirePreflightMessage::decode(&encoded).expect("decode");

        assert_eq!(decoded.agents.len(), 2);
        assert_eq!(decoded.agents[0], "agent1");
        assert_eq!(decoded.agents[1], "agent2");
    }
}
