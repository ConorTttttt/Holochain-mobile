//! Conductor connection module for h2hc-linker.
//!
//! This module provides connectivity to a Holochain conductor for the migration period.
//! It will be removed when direct kitsune2 queries are implemented (M6).

mod admin_conn;
mod app_conn;

pub use admin_conn::AdminConn;
pub use app_conn::AppConn;
