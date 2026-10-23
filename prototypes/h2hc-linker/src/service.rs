//! Service for running h2hc-linker

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::TcpListener;

use crate::agent_proxy::AgentProxyManager;
use crate::auth::AuthStore;
use crate::conductor::{AdminConn, AppConn};
use crate::conductor_link::{store::AcceptanceStore, ConductorLinks};
use crate::config::Configuration;
use crate::dht_query::{DhtQuery, PendingDhtResponses};
use crate::error::{LinkerError, LinkerResult};
use crate::gateway_kitsune::{GatewayKitsune, KitsuneProxy, KitsuneProxyBuilder};
use crate::identity::LinkerIdentity;
use crate::registration::RegistrationClient;
use crate::router::create_router;
use crate::routes::kitsune::KitsuneState;
use crate::temp_op_store::{TempOpStoreFactory, TempOpStoreHandle};

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    /// Configuration
    pub configuration: Configuration,
    /// Agent proxy manager for WebSocket connections
    pub agent_proxy: AgentProxyManager,
    /// Gateway kitsune manager (if kitsune2 enabled)
    pub gateway_kitsune: Option<GatewayKitsune>,
    /// Kitsune state for liveness endpoints
    pub kitsune_state: Arc<KitsuneState>,
    /// App connection for conductor zome calls (if conductor enabled)
    pub app_conn: Option<AppConn>,
    /// Temp op store for publishing (if kitsune2 enabled)
    pub temp_op_store: Option<TempOpStoreHandle>,
    /// DHT query handler for direct kitsune2 queries
    pub dht_query: Option<DhtQuery>,
    /// Auth store (if auth enabled via H2HC_LINKER_ADMIN_SECRET)
    pub auth_store: Option<AuthStore>,
    /// Conductors connected over `/conductor/v1`.
    pub conductor_links: Option<ConductorLinks>,
}

/// The main h2hc-linker service
pub struct LinkerService {
    addr: SocketAddr,
    app_state: AppState,
    identity: Arc<LinkerIdentity>,
}

impl LinkerService {
    /// Create a new service with the given configuration
    pub async fn new(address: IpAddr, port: u16, config: Configuration) -> LinkerResult<Self> {
        let addr = SocketAddr::new(address, port);

        // Load or generate the linker identity (persistent ed25519 keypair)
        let identity =
            Arc::new(LinkerIdentity::load(&config.identity).map_err(|e| {
                LinkerError::Internal(format!("Failed to load linker identity: {e}"))
            })?);

        // Warn if registration is configured but admin_secret is not set
        if config.registration_enabled() && config.admin_secret.is_none() {
            tracing::warn!(
                "Registration is configured but H2HC_LINKER_ADMIN_SECRET is not set. \
                 The joining service will not be able to call back to this linker's admin API."
            );
        }

        // Create agent proxy manager
        let agent_proxy = AgentProxyManager::new();

        // Create shared pending DHT responses for DhtQuery <-> ProxySpaceHandler communication
        let pending_dht_responses = PendingDhtResponses::new();

        // Create Kitsune2 instance (always enabled - bootstrap_url is required)
        let conductor_links_slot: crate::gateway_kitsune::ConductorLinksSlot = Default::default();
        let (kitsune, gateway_kitsune, temp_op_store, dht_query) = if config.kitsune_enabled() {
            tracing::info!("Initializing Kitsune2 instance with agent registration support and direct DHT queries");

            // Create TempOpStore for publishing
            let (op_store_factory, op_store_handle) = TempOpStoreFactory::create();
            op_store_factory.start_cleanup_task();

            // Create KitsuneProxy handler with shared pending responses
            let kitsune_proxy = KitsuneProxy::with_pending_responses(
                agent_proxy.clone(),
                pending_dht_responses.clone(),
            )
            .with_conductor_links(conductor_links_slot.clone());

            let mut builder =
                KitsuneProxyBuilder::new(kitsune_proxy).with_op_store(op_store_factory.into_dyn());

            builder = builder.with_bootstrap_url(&config.bootstrap_url);
            if let Some(ref relay_url) = config.relay_url {
                builder = builder.with_relay_url(relay_url);
            }

            // Configure reporting if enabled (uses the persistent identity)
            if let crate::config::ReportConfig::JsonLines {
                days_retained,
                fetched_op_interval_s,
            } = &config.report
            {
                tracing::info!(
                    days_retained,
                    fetched_op_interval_s,
                    path = %config.report_path.display(),
                    "Enabling kitsune2 reporting (JsonLines)"
                );
                let report_factory = crate::linker_report::LinkerReportFactory::create(
                    identity.signing_key().clone(),
                );
                let report_config = crate::linker_report::HcReportConfig {
                    days_retained: *days_retained,
                    path: config.report_path.clone(),
                    fetched_op_interval_s: *fetched_op_interval_s,
                };
                builder = builder.with_report(report_factory, report_config);
            }

            match builder.build().await {
                Ok(k) => {
                    tracing::info!("Kitsune2 instance created successfully");
                    let gw_kitsune = GatewayKitsune::new(k.clone(), agent_proxy.clone());
                    // Create DhtQuery with the same pending responses, plus (unless
                    // disabled via H2HC_LINKER_LOCAL_AUTHORITY_FALLBACK) this
                    // gateway's publish buffer so co-located zero-arc agents can see
                    // each other's freshly published ops (see local_authority docs).
                    let dht_query_base =
                        DhtQuery::new(gw_kitsune.clone(), pending_dht_responses.clone());
                    let dht_query = if config.local_authority_fallback_enabled {
                        dht_query_base.with_local_ops(op_store_handle.clone())
                    } else {
                        dht_query_base
                    };
                    (
                        Some(k),
                        Some(gw_kitsune),
                        Some(op_store_handle),
                        Some(dht_query),
                    )
                }
                Err(e) => {
                    tracing::error!("Failed to create Kitsune2 instance: {}", e);
                    return Err(LinkerError::Internal(format!(
                        "Failed to create Kitsune2 instance: {e}"
                    )));
                }
            }
        } else {
            unreachable!("bootstrap_url is required; from_env() enforces this")
        };

        let kitsune_state = Arc::new(KitsuneState {
            enabled: true,
            bootstrap_url: Some(config.bootstrap_url.clone()),
            relay_url: config.relay_url.clone(),
            kitsune,
        });

        // Create conductor connection if configured
        let app_conn = if let Some(conductor_addr) = config.conductor_url {
            tracing::info!("Initializing conductor connection to {}", conductor_addr);
            let admin_conn = AdminConn::new(conductor_addr);
            Some(AppConn::new(
                admin_conn,
                conductor_addr,
                config.zome_call_timeout,
            ))
        } else {
            tracing::info!("Conductor not configured (no H2HC_LINKER_CONDUCTOR_URL)");
            None
        };

        // Create auth store if auth is enabled
        let auth_store = if config.auth_enabled() {
            tracing::info!("Authentication enabled (H2HC_LINKER_ADMIN_SECRET set)");
            let session_store: std::sync::Arc<dyn crate::auth::session_store::SessionStore> =
                match &config.session_store {
                    crate::config::SessionStoreConfig::Memory => {
                        tracing::info!("Using in-memory session store");
                        std::sync::Arc::new(crate::auth::memory_store::MemorySessionStore::new())
                    }
                    crate::config::SessionStoreConfig::Sqlite { path } => {
                        tracing::info!("Using SQLite session store at {}", path.display());
                        std::sync::Arc::new(
                            crate::auth::sqlite_store::SqliteSessionStore::new(path).map_err(
                                |e| {
                                    LinkerError::Internal(format!(
                                        "Failed to open session store: {e}"
                                    ))
                                },
                            )?,
                        )
                    }
                };
            Some(AuthStore::with_store(session_store))
        } else {
            tracing::info!("Authentication disabled (no H2HC_LINKER_ADMIN_SECRET)");
            None
        };

        let conductor_links = match (&gateway_kitsune, &dht_query, &temp_op_store) {
            (Some(gateway), Some(query), Some(temp_ops)) => {
                let link_config = &config.conductor_link;
                let store = AcceptanceStore::open(
                    &link_config.data_dir,
                    identity.signing_key().clone(),
                    link_config.retention,
                )
                .map_err(|e| {
                    LinkerError::Internal(format!("Failed to open acceptance store: {e}"))
                })?;
                let links = ConductorLinks::new(
                    Arc::new(store),
                    gateway.clone(),
                    query.clone(),
                    pending_dht_responses.clone(),
                    temp_ops.clone(),
                    link_config.leave_on_disconnect,
                );
                let reloaded = links.reload_retained_ops().await.map_err(|e| {
                    LinkerError::Internal(format!("Failed to reload retained ops: {e}"))
                })?;
                tracing::info!(
                    reloaded,
                    data_dir = %link_config.data_dir.display(),
                    "Conductor links enabled at /conductor/v1"
                );
                let _ = conductor_links_slot.set(links.clone());
                Some(links)
            }
            _ => None,
        };

        let app_state = AppState {
            configuration: config,
            agent_proxy,
            gateway_kitsune,
            kitsune_state,
            app_conn,
            temp_op_store,
            dht_query,
            auth_store,
            conductor_links,
        };

        Ok(Self {
            addr,
            app_state,
            identity,
        })
    }

    /// Run the service, including registration heartbeat loop if configured.
    ///
    /// The `shutdown_rx` channel is used to coordinate graceful shutdown:
    /// when `true` is sent, both axum and the registration client will stop.
    pub async fn run(
        self,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> LinkerResult<()> {
        let router = create_router(self.app_state.clone());

        // Start registration heartbeat loop if configured
        let registration_handle =
            if let Some(ref reg_config) = self.app_state.configuration.registration {
                let admin_secret = self
                    .app_state
                    .configuration
                    .admin_secret
                    .clone()
                    .unwrap_or_default();

                let client = RegistrationClient::new(
                    self.identity.clone(),
                    reg_config.clone(),
                    admin_secret,
                    shutdown_rx.clone(),
                );

                tracing::info!(
                    joining_service = %reg_config.joining_service_url,
                    public_url = %reg_config.public_url,
                    "Starting registration heartbeat loop"
                );

                Some(tokio::spawn(async move {
                    client.run().await;
                }))
            } else {
                tracing::info!("Registration not configured (no H2HC_LINKER_JOINING_SERVICE_URL)");
                None
            };

        tracing::info!("Starting h2hc-linker on {}", self.addr);

        let listener = TcpListener::bind(self.addr)
            .await
            .map_err(|e| crate::error::LinkerError::Network(e.to_string()))?;

        // Serve with graceful shutdown
        let shutdown_signal = async move {
            let _ = shutdown_rx.wait_for(|v| *v).await;
        };

        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal)
            .await
            .map_err(|e| crate::error::LinkerError::Network(e.to_string()))?;

        // Wait for registration deregistration (with timeout)
        if let Some(handle) = registration_handle {
            match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
                Ok(Ok(())) => tracing::debug!("Registration task completed"),
                Ok(Err(e)) => tracing::warn!(error = %e, "Registration task panicked"),
                Err(_) => tracing::warn!("Registration shutdown timed out after 5s"),
            }
        }

        Ok(())
    }
}
