//! Admin websocket connection with auto-reconnection.

use crate::error::{LinkerError, LinkerResult};
use holochain_client::{AdminWebsocket, AppInfo, ConductorApiError};
use holochain_conductor_api::AppStatusFilter;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Admin websocket connection with auto-reconnection.
#[derive(Debug, Clone)]
pub struct AdminConn {
    socket_addr: SocketAddr,
    handle: Arc<RwLock<Option<AdminWebsocket>>>,
}

impl AdminConn {
    /// Create a new admin connection.
    pub fn new(socket_addr: SocketAddr) -> Self {
        Self {
            socket_addr,
            handle: Default::default(),
        }
    }

    /// List all installed apps.
    pub async fn list_apps(
        &self,
        status_filter: Option<AppStatusFilter>,
    ) -> LinkerResult<Vec<AppInfo>> {
        for _ in 0..2 {
            let admin_ws = self.get_or_connect().await?;

            match admin_ws.list_apps(status_filter.clone()).await {
                Ok(apps) => return Ok(apps),
                Err(ConductorApiError::WebsocketError(e)) => {
                    tracing::warn!(?e, "Admin websocket error, reconnecting");
                    *self.handle.write().await = None;
                    continue;
                }
                Err(e) => return Err(LinkerError::HolochainError(e)),
            }
        }

        Err(LinkerError::UpstreamUnavailable)
    }

    /// Get the current websocket connection (connecting if needed).
    pub async fn get_websocket(&self) -> LinkerResult<AdminWebsocket> {
        self.get_or_connect().await
    }

    async fn get_or_connect(&self) -> LinkerResult<AdminWebsocket> {
        {
            let lock = self.handle.read().await;
            if let Some(ws) = lock.as_ref() {
                return Ok(ws.clone());
            }
        }

        let mut lock = self.handle.write().await;

        // Check again after acquiring write lock
        if let Some(ws) = lock.as_ref() {
            return Ok(ws.clone());
        }

        match AdminWebsocket::connect(self.socket_addr, None).await {
            Ok(ws) => {
                tracing::info!("Connected to conductor admin at {}", self.socket_addr);
                *lock = Some(ws.clone());
                Ok(ws)
            }
            Err(e) => {
                tracing::error!(?e, "Failed to connect to conductor admin");
                Err(LinkerError::UpstreamUnavailable)
            }
        }
    }
}
