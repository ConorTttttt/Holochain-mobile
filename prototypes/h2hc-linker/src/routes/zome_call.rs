//! Zome call endpoint for executing zome functions.
//!
//! Provides HTTP access to zome functions via the conductor.

use crate::error::{LinkerError, LinkerResult};
use crate::service::AppState;
use axum::extract::{Path, Query, State};
use base64::{prelude::BASE64_URL_SAFE, Engine};
use holochain_types::prelude::{DnaHash, ExternIO};
use serde::Deserialize;
use tracing::info;

// ============================================================================
// Request types
// ============================================================================

/// Path parameters for zome call endpoint.
#[derive(Debug, Deserialize)]
pub struct ZomeCallPath {
    /// DNA hash (base64 encoded).
    pub dna_hash: String,
    /// Zome name.
    pub zome_name: String,
    /// Function name.
    pub fn_name: String,
}

/// Query parameters for zome call endpoint.
#[derive(Debug, Deserialize)]
pub struct ZomeCallQuery {
    /// Optional payload (base64 URL-safe encoded JSON).
    pub payload: Option<String>,
}

// ============================================================================
// Endpoint implementation
// ============================================================================

/// GET /api/{dna_hash}/{zome_name}/{fn_name}
///
/// Execute a zome function and return the result as JSON.
///
/// # Path Parameters
///
/// - `dna_hash` - The DNA hash (base64 encoded)
/// - `zome_name` - The zome name
/// - `fn_name` - The function name
///
/// # Query Parameters
///
/// - `payload` - Optional base64 URL-safe encoded JSON payload
///
/// # Response
///
/// Returns the zome function result as JSON.
#[tracing::instrument(skip(state))]
pub async fn zome_call(
    Path(path): Path<ZomeCallPath>,
    State(state): State<AppState>,
    Query(query): Query<ZomeCallQuery>,
) -> LinkerResult<String> {
    // Parse DNA hash
    let dna_hash = DnaHash::try_from(path.dna_hash.clone())
        .map_err(|_| LinkerError::RequestMalformed("Invalid DNA hash".to_string()))?;

    info!(
        dna = %dna_hash,
        zome = %path.zome_name,
        fn_name = %path.fn_name,
        has_payload = query.payload.is_some(),
        "Processing zome call"
    );

    // Get app connection
    let app_conn = state
        .app_conn
        .as_ref()
        .ok_or_else(|| LinkerError::UpstreamUnavailable)?;

    // Transcode payload from base64 encoded JSON to ExternIO
    let payload = base64_json_to_extern_io(query.payload)?;

    // Make the zome call
    let result = app_conn
        .call_zome(&dna_hash, &path.zome_name, &path.fn_name, payload)
        .await?;

    // Transcode ExternIO response to JSON
    extern_io_to_json(&result)
}

/// Transcode an optional base64 encoded JSON payload to ExternIO.
/// If no payload is passed, a unit value (null) is serialized.
fn base64_json_to_extern_io(maybe_payload: Option<String>) -> LinkerResult<ExternIO> {
    let json_value = if let Some(base64_encoded) = maybe_payload {
        let decoded = BASE64_URL_SAFE
            .decode(base64_encoded)
            .map_err(|_| LinkerError::RequestMalformed("Invalid base64 encoding".to_string()))?;
        serde_json::from_slice::<serde_json::Value>(&decoded)
            .map_err(|_| LinkerError::RequestMalformed("Invalid JSON value".to_string()))?
    } else {
        serde_json::Value::Null
    };

    ExternIO::encode(json_value)
        .map_err(|e| LinkerError::RequestMalformed(format!("Failed to serialize payload: {e}")))
}

/// Transcode an ExternIO response to a JSON string.
fn extern_io_to_json(response: &ExternIO) -> LinkerResult<String> {
    let json_value = response
        .decode::<serde_json::Value>()
        .map_err(|e| LinkerError::Internal(format!("Failed to decode response: {e}")))?;
    Ok(json_value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_json_to_extern_io_none() {
        let result = base64_json_to_extern_io(None).unwrap();
        let decoded: () = result.decode().unwrap();
        assert_eq!(decoded, ());
    }

    #[test]
    fn test_base64_json_to_extern_io_valid() {
        // Encode {"field": true} as base64 URL-safe
        let json = r#"{"field":true}"#;
        let encoded = BASE64_URL_SAFE.encode(json);

        let result = base64_json_to_extern_io(Some(encoded)).unwrap();
        let decoded: serde_json::Value = result.decode().unwrap();
        assert_eq!(decoded["field"], true);
    }

    #[test]
    fn test_base64_json_to_extern_io_invalid_base64() {
        let result = base64_json_to_extern_io(Some("not valid base64!!!".to_string()));
        assert!(matches!(result, Err(LinkerError::RequestMalformed(_))));
    }

    #[test]
    fn test_base64_json_to_extern_io_invalid_json() {
        let encoded = BASE64_URL_SAFE.encode("not json");
        let result = base64_json_to_extern_io(Some(encoded));
        assert!(matches!(result, Err(LinkerError::RequestMalformed(_))));
    }

    #[test]
    fn test_extern_io_to_json() {
        let value = serde_json::json!({"result": 42});
        let extern_io = ExternIO::encode(value).unwrap();

        let json = extern_io_to_json(&extern_io).unwrap();
        assert!(json.contains("\"result\":42"));
    }
}
