//! Error types for h2hc-linker

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use holochain_client::ConductorApiError;
use serde::Serialize;

/// Result type for h2hc-linker operations
pub type LinkerResult<T> = Result<T, LinkerError>;

/// Error type for h2hc-linker operations
#[derive(Debug, thiserror::Error)]
pub enum LinkerError {
    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// Network error
    #[error("Network error: {0}")]
    Network(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Not found
    #[error("Not found: {0}")]
    NotFound(String),

    /// Invalid request
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),

    /// Conductor/Holochain error
    #[error("Holochain error: {0}")]
    HolochainError(#[from] ConductorApiError),

    /// Upstream (conductor) unavailable
    #[error("Upstream unavailable")]
    UpstreamUnavailable,

    /// Authentication failed
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    /// Forbidden (authenticated but insufficient permissions)
    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// Malformed request
    #[error("Request malformed: {0}")]
    RequestMalformed(String),
}

/// JSON error response
#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: u16,
}

impl IntoResponse for LinkerError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            LinkerError::Config(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            LinkerError::Network(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
            LinkerError::Serialization(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            LinkerError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            LinkerError::InvalidRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            LinkerError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            LinkerError::HolochainError(e) => {
                (StatusCode::BAD_GATEWAY, format!("Holochain error: {e}"))
            }
            LinkerError::UpstreamUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Upstream unavailable".to_string(),
            ),
            LinkerError::AuthenticationFailed(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            LinkerError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            LinkerError::RequestMalformed(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
        };

        let body = serde_json::to_string(&ErrorResponse {
            error: message,
            code: status.as_u16(),
        })
        .unwrap_or_else(|_| r#"{"error":"serialization failed","code":500}"#.to_string());

        (status, body).into_response()
    }
}
