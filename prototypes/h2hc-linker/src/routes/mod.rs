//! HTTP routes for h2hc-linker
//!
//! ## API Structure
//!
//! - `/api/*` - Zome call endpoint
//! - `/dht/*` - DHT access endpoints (get record, get links, publish)
//! - `/k2/*` - Kitsune direct API (network status, peers, liveness)
//! - `/ws` - WebSocket for browser extension connections
//! - `/test/*` - Test endpoints for development
//! - `/health` - Health check endpoint

pub mod dht;
pub mod health;
pub mod kitsune;
pub mod publish;
pub mod test_signal;
pub mod websocket;
pub mod zome_call;

pub use dht::{
    dht_count_links, dht_get_agent_activity, dht_get_details, dht_get_links, dht_get_record,
    dht_must_get_agent_activity,
};
pub use health::health_check;
pub use kitsune::kitsune_routes;
pub use publish::dht_publish;
pub use test_signal::test_signal;
pub use zome_call::zome_call;
