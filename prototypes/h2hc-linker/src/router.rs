//! Router configuration for h2hc-linker

use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, post},
    Router,
};
use tower_http::cors::{Any, CorsLayer};

use crate::auth::admin::{add_agent, list_agents, remove_agent};
use crate::auth::middleware::{
    require_admin_secret, require_dht_read, require_dht_write, require_dna_scope, require_k2,
};
use crate::routes::{
    dht_count_links, dht_get_agent_activity, dht_get_details, dht_get_links, dht_get_record,
    dht_must_get_agent_activity, dht_publish, health_check, kitsune_routes, test_signal,
    websocket::ws_handler, zome_call,
};
use crate::service::AppState;

/// DHT route definitions shared between open and authenticated routers.
fn dht_routes() -> Router<AppState> {
    Router::new()
        .route("/dht/{dna_hash}/record/{hash}", get(dht_get_record))
        .route("/dht/{dna_hash}/details/{hash}", get(dht_get_details))
        .route("/dht/{dna_hash}/links", get(dht_get_links))
        .route("/dht/{dna_hash}/count_links", get(dht_count_links))
        .route(
            "/dht/{dna_hash}/agent_activity/{agent_hash}",
            get(dht_get_agent_activity),
        )
        .route(
            "/dht/{dna_hash}/must_get_agent_activity",
            post(dht_must_get_agent_activity),
        )
}

/// Create the main router for h2hc-linker
pub fn create_router(app_state: AppState) -> Router {
    // CORS configuration - allow all origins for development
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let body_limit = app_state.configuration.payload_limit_bytes;

    if app_state.auth_store.is_some() {
        create_authenticated_router(app_state, cors, body_limit)
    } else {
        create_open_router(app_state, cors, body_limit)
    }
}

/// Router with no auth (current behavior when H2HC_LINKER_ADMIN_SECRET is not set).
fn create_open_router(app_state: AppState, cors: CorsLayer, body_limit: usize) -> Router {
    Router::new()
        // Health check
        .route("/health", get(health_check))
        // WebSocket for browser extension connections
        .route("/ws", get(ws_handler))
        // Test endpoint for signal forwarding (development only)
        .route("/test/signal", post(test_signal))
        // DHT read endpoints
        .merge(dht_routes())
        // DHT publish endpoint (via kitsune2)
        .route("/dht/{dna_hash}/publish", post(dht_publish))
        // Zome call endpoint (via conductor)
        .route("/api/{dna_hash}/{zome_name}/{fn_name}", get(zome_call))
        // Kitsune direct API
        .nest(
            "/k2",
            kitsune_routes().with_state(app_state.kitsune_state.clone()),
        )
        .with_state(app_state)
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(cors)
}

/// Router with auth middleware (when H2HC_LINKER_ADMIN_SECRET is set).
fn create_authenticated_router(app_state: AppState, cors: CorsLayer, body_limit: usize) -> Router {
    // DHT read routes (capability check, then DNA scope check)
    let dht_read_routes = dht_routes()
        .route_layer(axum::middleware::from_fn_with_state(
            app_state.clone(),
            require_dna_scope,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            app_state.clone(),
            require_dht_read,
        ));

    // DHT write routes (capability check, then DNA scope check)
    let dht_write_routes = Router::new()
        .route("/dht/{dna_hash}/publish", post(dht_publish))
        .route_layer(axum::middleware::from_fn_with_state(
            app_state.clone(),
            require_dna_scope,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            app_state.clone(),
            require_dht_write,
        ));

    // K2 routes
    let k2_routes = kitsune_routes()
        .with_state(app_state.kitsune_state.clone())
        .route_layer(axum::middleware::from_fn_with_state(
            app_state.clone(),
            require_k2,
        ));

    // Admin routes
    let admin_routes = Router::new()
        .route("/agents", post(add_agent))
        .route("/agents", delete(remove_agent))
        .route("/agents", get(list_agents))
        .route_layer(axum::middleware::from_fn_with_state(
            app_state.clone(),
            require_admin_secret,
        ));

    Router::new()
        // Always open
        .route("/health", get(health_check))
        .route("/ws", get(ws_handler))
        .route("/test/signal", post(test_signal))
        // Protected routes
        .merge(dht_read_routes)
        .merge(dht_write_routes)
        .nest("/k2", k2_routes)
        .nest("/admin", admin_routes)
        // Zome call (deprecated, no auth)
        .route("/api/{dna_hash}/{zome_name}/{fn_name}", get(zome_call))
        .with_state(app_state)
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(cors)
}
