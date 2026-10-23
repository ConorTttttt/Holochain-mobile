//! Kitsune2 instance builder and minimal handler for h2hc-linker
//!
//! This module creates a Kitsune2 instance for the liveness endpoints.
//! It provides a minimal `KitsuneHandler` implementation that does not
//! handle signals (that will be added in later steps when holochain_p2p
//! is integrated).

use bytes::Bytes;
use kitsune2_api::{
    BoxFut, Config, DynSpaceHandler, K2Error, K2Result, KitsuneHandler, SpaceHandler, SpaceId, Url,
};
use std::sync::Arc;
use tracing::{debug, info};

/// Minimal Kitsune handler for liveness endpoints.
///
/// This handler does not process incoming requests (zero-arc behavior).
/// It exists to satisfy the Kitsune2 handler trait requirements.
#[derive(Debug, Default)]
pub struct MinimalKitsuneHandler;

impl KitsuneHandler for MinimalKitsuneHandler {
    fn create_space(
        &self,
        space_id: SpaceId,
        _config: Option<&Config>,
    ) -> BoxFut<'_, K2Result<DynSpaceHandler>> {
        Box::pin(async move {
            info!(?space_id, "Creating minimal space handler");
            let handler: DynSpaceHandler = Arc::new(MinimalSpaceHandler { space_id });
            Ok(handler)
        })
    }

    fn new_listening_address(&self, this_url: Url) -> BoxFut<'static, ()> {
        info!(%this_url, "h2hc-linker kitsune2 listening on new address");
        Box::pin(async move {})
    }

    fn peer_disconnect(&self, peer: Url, reason: Option<String>) {
        debug!(%peer, ?reason, "Peer disconnected");
    }

    fn preflight_gather_outgoing(&self, _peer_url: Url) -> BoxFut<'_, K2Result<Bytes>> {
        Box::pin(async move {
            // Create a minimal preflight message
            // Protocol version 2 is required for compatibility with Holochain 0.6
            let mut preflight = Vec::with_capacity(64);

            // WirePreflightMessage format from holochain_p2p:
            // - compat: WireCompatInfo { proto_ver: u32 }
            // - agents: Vec<AgentInfoSigned> (empty for minimal handler)
            //
            // Using msgpack encoding to match holochain_p2p wire format
            use rmp_serde::Serializer;
            use serde::Serialize;

            let compat = WireCompatInfo { proto_ver: 2 };
            let msg = WirePreflightMessage {
                compat,
                agents: vec![],
            };

            msg.serialize(&mut Serializer::new(&mut preflight))
                .map_err(|e| K2Error::other(format!("Failed to encode preflight: {e}")))?;

            debug!("Sending preflight");
            Ok(Bytes::from(preflight))
        })
    }

    fn preflight_validate_incoming(&self, peer_url: Url, data: Bytes) -> BoxFut<'_, K2Result<()>> {
        Box::pin(async move {
            // Decode and validate the incoming preflight
            let preflight: WirePreflightMessage = rmp_serde::from_slice(&data)
                .map_err(|e| K2Error::other(format!("Invalid preflight from peer: {e}")))?;

            // Check protocol version compatibility
            if preflight.compat.proto_ver != 2 {
                return Err(K2Error::other(format!(
                    "Incompatible protocol version from {}: expected 2, got {}",
                    peer_url, preflight.compat.proto_ver
                )));
            }

            debug!(%peer_url, proto_ver = preflight.compat.proto_ver, "Validated incoming preflight");
            Ok(())
        })
    }
}

/// Wire compatibility info for preflight handshake
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct WireCompatInfo {
    proto_ver: u32,
}

/// Wire preflight message for peer handshake
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct WirePreflightMessage {
    compat: WireCompatInfo,
    #[serde(default)]
    agents: Vec<()>, // Empty for minimal handler
}

/// Minimal space handler that returns empty for all requests.
#[derive(Debug)]
struct MinimalSpaceHandler {
    #[allow(dead_code)] // Used for Debug output
    space_id: SpaceId,
}

impl SpaceHandler for MinimalSpaceHandler {
    fn recv_notify(&self, from_peer: Url, space_id: SpaceId, data: Bytes) -> K2Result<()> {
        debug!(
            %from_peer,
            ?space_id,
            data_len = data.len(),
            "Received notification (ignoring - minimal handler)"
        );
        // Zero-arc: we don't process incoming requests
        Ok(())
    }
}

/// Builder for creating a standalone Kitsune2 instance (used in tests).
///
/// Production code uses [`KitsuneProxyBuilder`](crate::gateway_kitsune::KitsuneProxyBuilder) instead.
#[cfg(test)]
pub struct KitsuneBuilder {
    bootstrap_url: Option<String>,
    relay_url: Option<String>,
}

#[cfg(test)]
impl Default for KitsuneBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl KitsuneBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            bootstrap_url: None,
            relay_url: None,
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

    /// Build the Kitsune2 instance.
    #[allow(dead_code)]
    pub async fn build(
        self,
    ) -> Result<kitsune2_api::DynKitsune, Box<dyn std::error::Error + Send + Sync>> {
        use kitsune2::default_builder;
        use kitsune2_core::factories::{
            CoreBootstrapConfig, CoreBootstrapModConfig, CoreSpaceConfig, CoreSpaceModConfig,
        };
        use kitsune2_transport_iroh::{IrohTransportConfig, IrohTransportModConfig};

        let builder = default_builder().with_default_config()?;

        // Configure bootstrap server
        if let Some(bootstrap_url) = self.bootstrap_url {
            info!(%bootstrap_url, "Configuring bootstrap server");
            builder.config.set_module_config(&CoreBootstrapModConfig {
                core_bootstrap: CoreBootstrapConfig {
                    server_url: Some(bootstrap_url),
                    backoff_max_ms: 10000,
                    ..Default::default()
                },
            })?;
        }

        // Configure core space settings (matching conductor settings)
        builder.config.set_module_config(&CoreSpaceModConfig {
            core_space: CoreSpaceConfig {
                re_sign_expire_time_ms: 10000,
                re_sign_freq_ms: 10000,
            },
        })?;

        // Configure iroh transport
        let iroh_config = IrohTransportConfig {
            relay_allow_plain_text: true,
            relay_url: self.relay_url,
            ..Default::default()
        };
        info!(?iroh_config.relay_url, "Configuring iroh transport");
        builder.config.set_module_config(&IrohTransportModConfig {
            iroh_transport: iroh_config,
        })?;

        // Build and register minimal handler
        let kitsune = builder.build().await?;
        let handler: Arc<dyn KitsuneHandler> = Arc::new(MinimalKitsuneHandler);
        kitsune.register_handler(handler).await?;

        info!("h2hc-linker kitsune2 instance created");
        Ok(kitsune)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_creation() {
        let builder = KitsuneBuilder::new()
            .with_bootstrap_url("https://bootstrap.example.com")
            .with_relay_url("https://relay.example.com");

        assert!(builder.bootstrap_url.is_some());
        assert!(builder.relay_url.is_some());
    }

    #[test]
    fn test_wire_preflight_encoding() {
        let msg = WirePreflightMessage {
            compat: WireCompatInfo { proto_ver: 2 },
            agents: vec![],
        };

        let encoded = rmp_serde::to_vec(&msg).expect("encode");
        let decoded: WirePreflightMessage = rmp_serde::from_slice(&encoded).expect("decode");

        assert_eq!(decoded.compat.proto_ver, 2);
        assert!(decoded.agents.is_empty());
    }
}
