//! App websocket connection for zome calls.

use crate::conductor::AdminConn;
use crate::error::{LinkerError, LinkerResult};
use holochain_client::{
    AppInfo, AppWebsocket, CellId, ClientAgentSigner, ConnectRequest, ExternIO, GrantedFunctions,
    IssueAppAuthenticationTokenPayload, WebsocketConfig, ZomeCallTarget,
};
use holochain_conductor_api::{AppStatusFilter, CellInfo};
use holochain_types::dna::DnaHash;
use holochain_types::prelude::ZomeName;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Origin header for gateway connections.
const GATEWAY_ORIGIN: &str = "h2hc-linker";

/// App websocket connection manager.
///
/// Manages connections to the conductor's app interface for making zome calls.
/// Simplified version that caches a single app connection.
#[derive(Debug, Clone)]
pub struct AppConn {
    admin_conn: AdminConn,
    admin_addr: SocketAddr,
    zome_call_timeout: Duration,
    /// Cached app connections by installed_app_id
    connections: Arc<RwLock<HashMap<String, AppWebsocket>>>,
    /// Cached app info
    app_info_cache: Arc<RwLock<Vec<AppInfo>>>,
}

impl AppConn {
    /// Create a new app connection manager.
    pub fn new(admin_conn: AdminConn, admin_addr: SocketAddr, zome_call_timeout: Duration) -> Self {
        Self {
            admin_conn,
            admin_addr,
            zome_call_timeout,
            connections: Default::default(),
            app_info_cache: Default::default(),
        }
    }

    /// Make a zome call to the dht_util zome.
    pub async fn call_dht_util(
        &self,
        dna_hash: &DnaHash,
        fn_name: &str,
        payload: ExternIO,
    ) -> LinkerResult<ExternIO> {
        self.call_zome(dna_hash, "dht_util", fn_name, payload).await
    }

    /// Make a zome call to any zome.
    pub async fn call_zome(
        &self,
        dna_hash: &DnaHash,
        zome_name: &str,
        fn_name: &str,
        payload: ExternIO,
    ) -> LinkerResult<ExternIO> {
        // Find an app with this DNA
        let (app_info, cell_id) = self.find_app_with_dna(dna_hash).await?;

        // Get or create app connection
        let app_ws = self.get_or_connect(&app_info.installed_app_id).await?;

        // Make the zome call
        let result = app_ws
            .call_zome(
                ZomeCallTarget::CellId(cell_id),
                ZomeName::from(zome_name),
                fn_name.into(),
                payload,
            )
            .await
            .map_err(LinkerError::HolochainError)?;

        Ok(result)
    }

    /// Find an app that contains the specified DNA.
    async fn find_app_with_dna(&self, dna_hash: &DnaHash) -> LinkerResult<(AppInfo, CellId)> {
        // Check cache first
        {
            let cache = self.app_info_cache.read().await;
            if let Some(result) = self.find_in_apps(&cache, dna_hash) {
                return Ok(result);
            }
        }

        // Refresh cache from conductor
        let apps = self
            .admin_conn
            .list_apps(Some(AppStatusFilter::Enabled))
            .await?;

        // Update cache
        {
            let mut cache = self.app_info_cache.write().await;
            *cache = apps.clone();
        }

        // Search again
        self.find_in_apps(&apps, dna_hash)
            .ok_or_else(|| LinkerError::NotFound(format!("No app found with DNA {dna_hash}")))
    }

    fn find_in_apps(&self, apps: &[AppInfo], dna_hash: &DnaHash) -> Option<(AppInfo, CellId)> {
        for app in apps {
            for cells in app.cell_info.values() {
                for cell in cells {
                    if let CellInfo::Provisioned(p) = cell {
                        if p.cell_id.dna_hash() == dna_hash {
                            return Some((app.clone(), p.cell_id.clone()));
                        }
                    }
                }
            }
        }
        None
    }

    async fn get_or_connect(&self, installed_app_id: &str) -> LinkerResult<AppWebsocket> {
        // Check cache
        {
            let conns = self.connections.read().await;
            if let Some(ws) = conns.get(installed_app_id) {
                return Ok(ws.clone());
            }
        }

        // Connect new
        let mut conns = self.connections.write().await;

        // Check again after lock
        if let Some(ws) = conns.get(installed_app_id) {
            return Ok(ws.clone());
        }

        let ws = self.connect_app(installed_app_id).await?;
        conns.insert(installed_app_id.to_string(), ws.clone());
        Ok(ws)
    }

    async fn connect_app(&self, installed_app_id: &str) -> LinkerResult<AppWebsocket> {
        // Get admin connection to issue token
        let admin_ws = self.admin_conn.get_websocket().await?;

        // Issue app auth token
        let token = admin_ws
            .issue_app_auth_token(IssueAppAuthenticationTokenPayload::for_installed_app_id(
                installed_app_id.to_string(),
            ))
            .await
            .map_err(LinkerError::HolochainError)?;

        // Get app port (or attach new interface)
        let app_port = self.get_app_port(&admin_ws, installed_app_id).await?;

        // Build connection request
        let request = ConnectRequest::from(SocketAddr::new(self.admin_addr.ip(), app_port))
            .try_set_header("Origin", GATEWAY_ORIGIN)
            .expect("Origin header");

        // Configure websocket
        let mut config = WebsocketConfig::CLIENT_DEFAULT;
        config.default_request_timeout = self.zome_call_timeout;

        let signer = ClientAgentSigner::default();

        // Connect
        let app_ws = AppWebsocket::connect_with_request_and_config(
            request,
            Arc::new(config),
            token.token,
            signer.clone().into(),
        )
        .await
        .map_err(|e| {
            tracing::error!(?e, "Failed to connect app websocket");
            LinkerError::UpstreamUnavailable
        })?;

        // Authorize signing for all cells
        let app_info = app_ws.cached_app_info();
        for cells in app_info.cell_info.values() {
            for cell in cells {
                if let CellInfo::Provisioned(p) = cell {
                    let creds = admin_ws
                        .authorize_signing_credentials(
                            holochain_client::AuthorizeSigningCredentialsPayload {
                                cell_id: p.cell_id.clone(),
                                functions: Some(GrantedFunctions::All),
                            },
                        )
                        .await
                        .map_err(LinkerError::HolochainError)?;
                    signer.add_credentials(p.cell_id.clone(), creds);
                }
            }
        }

        tracing::info!("Connected app websocket for {}", installed_app_id);
        Ok(app_ws)
    }

    async fn get_app_port(
        &self,
        admin_ws: &holochain_client::AdminWebsocket,
        _installed_app_id: &str,
    ) -> LinkerResult<u16> {
        // List existing interfaces
        let interfaces = admin_ws
            .list_app_interfaces()
            .await
            .map_err(LinkerError::HolochainError)?;

        // Find one that allows our origin
        for iface in &interfaces {
            if iface.allowed_origins.is_allowed(GATEWAY_ORIGIN) {
                return Ok(iface.port);
            }
        }

        // Attach new interface
        let port = admin_ws
            .attach_app_interface(
                0,
                None,
                holochain_types::websocket::AllowedOrigins::from(GATEWAY_ORIGIN.to_string()),
                None,
            )
            .await
            .map_err(LinkerError::HolochainError)?;

        tracing::info!("Attached new app interface on port {}", port);
        Ok(port)
    }
}
