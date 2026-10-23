//! # Holochain-to-Holochain Linker (h2hc-linker)
//!
//! Network edge gateway providing DHT access for lightweight browser clients.
//!
//! ## What is Holochain Membrane?
//!
//! Holochain Membrane is a network edge - like a cell membrane, it provides selective
//! access between lightweight browser clients and the Holochain DHT network.
//!
//! This is NOT a "lite conductor" - it has no:
//! - Source chain (no local chain storage)
//! - Validation (no validation workflows)
//! - Full node capabilities (zero-arc, no DHT storage)
//!
//! It provides:
//! - Holochain semantic API (/hc/*): get, get_links, publish, signals
//! - Kitsune direct API (/k2/*): network status, peer info, liveness

pub(crate) mod agent_proxy;
pub mod auth;
pub mod conductor;
mod config;
pub(crate) mod dht_query;
mod error;
pub(crate) mod gateway_kitsune;
pub mod identity;
mod kitsune;
pub(crate) mod linker_report;
pub(crate) mod local_authority;
pub(crate) mod proxy_agent;
pub(crate) mod registration;
mod router;
pub(crate) mod service;
pub(crate) mod temp_op_store;
pub(crate) mod wire_preflight;

// Routes
pub mod routes;

pub use config::Configuration;
pub use error::{LinkerError, LinkerResult};
pub use router::create_router;
pub use service::LinkerService;

// Re-export common types
pub use holo_hash;
pub use holochain_types;
