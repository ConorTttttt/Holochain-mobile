//! Authentication and authorization layer for h2hc-linker.
//!
//! Gated on `H2HC_LINKER_ADMIN_SECRET` -- when absent, all endpoints remain open.

pub mod admin;
pub mod middleware;
pub mod session_store;
pub mod types;

// The test macro must be defined before the modules that use it.
#[cfg(test)]
#[macro_use]
mod session_store_tests;

pub mod memory_store;
pub mod sqlite_store;
pub mod store;

pub use session_store::{SessionStoreError, SessionStoreResult};
pub use store::AuthStore;
pub use types::{AllowedAgent, AuthContext, Capability, SessionInfo, SessionToken};
