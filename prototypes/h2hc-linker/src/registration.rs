//! Joining-service registration client.
//!
//! When configured, the linker periodically sends signed heartbeats to a
//! joining service so it appears in the dynamic linker pool. On graceful
//! shutdown, a best-effort deregistration request is sent.
//!
//! All registration is opt-in: if `H2HC_LINKER_JOINING_SERVICE_URL` is not
//! set, this module is a no-op.

use base64::Engine;
use ed25519_dalek::Signer;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::config::RegistrationConfig;
use crate::identity::LinkerIdentity;

/// HTTP client for joining-service registration.
pub struct RegistrationClient {
    identity: Arc<LinkerIdentity>,
    config: RegistrationConfig,
    admin_secret: String,
    http: reqwest::Client,
    /// True after a successful heartbeat. Reset to false on any heartbeat
    /// failure so the next attempt re-sends `admin_secret` and the invite,
    /// which lets us recover if the joining service has lost our entry
    /// (TTL expiry during a long outage, server restart, operator delete).
    is_registered: bool,
    shutdown: tokio::sync::watch::Receiver<bool>,
}

/// Response from the joining service heartbeat endpoint.
#[derive(Debug, serde::Deserialize)]
struct HeartbeatResponse {
    registered: bool,
    ttl_seconds: u64,
    #[serde(default)]
    heartbeat_interval_seconds: Option<u64>,
}

/// Body for the deregistration DELETE request.
#[derive(Debug, serde::Serialize)]
struct DeregisterBody {
    timestamp: String,
    signature: String,
}

impl RegistrationClient {
    /// Create a new registration client.
    pub fn new(
        identity: Arc<LinkerIdentity>,
        config: RegistrationConfig,
        admin_secret: String,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build reqwest client");

        Self {
            identity,
            config,
            admin_secret,
            http,
            is_registered: false,
            shutdown,
        }
    }

    /// Run the heartbeat loop. Blocks until shutdown is signalled.
    ///
    /// Both the heartbeat call and the inter-heartbeat sleep race against
    /// the shutdown signal, so a hung heartbeat (waiting on reqwest's 30s
    /// timeout, for example) does not delay shutdown.
    pub async fn run(mut self) {
        // Clone the shutdown receiver so we can borrow it independently of
        // `&mut self.send_heartbeat()` in tokio::select! arms.
        let mut shutdown = self.shutdown.clone();
        let mut interval_secs = self.config.initial_heartbeat_interval_secs;

        loop {
            let was_registered = self.is_registered;

            // Race the heartbeat against shutdown so a hung HTTP request
            // doesn't block exit until the reqwest timeout.
            tokio::select! {
                result = self.send_heartbeat() => match result {
                    Ok(resp) => {
                        if !was_registered {
                            tracing::info!(
                                ttl_seconds = resp.ttl_seconds,
                                "Registered with joining service"
                            );
                        } else {
                            tracing::debug!(ttl_seconds = resp.ttl_seconds, "Heartbeat renewed");
                        }
                        // Use server-directed interval if provided
                        if let Some(server_interval) = resp.heartbeat_interval_seconds {
                            if server_interval > 0 {
                                interval_secs = server_interval;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Heartbeat failed, retrying in 30s");
                        interval_secs = 30;
                    }
                },
                _ = shutdown.changed() => {
                    self.handle_shutdown().await;
                    return;
                }
            }

            // Race the inter-heartbeat sleep against shutdown.
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(interval_secs)) => {}
                _ = shutdown.changed() => {
                    self.handle_shutdown().await;
                    return;
                }
            }
        }
    }

    /// Best-effort deregistration on shutdown. Skipped if no heartbeat has
    /// ever succeeded (nothing to deregister).
    async fn handle_shutdown(&self) {
        if !self.is_registered {
            tracing::info!(
                "Shutdown received before any successful heartbeat, skipping deregister"
            );
            return;
        }
        tracing::info!("Shutdown signal received, deregistering");
        match tokio::time::timeout(Duration::from_secs(3), self.deregister()).await {
            Ok(Ok(())) => tracing::info!("Deregistered from joining service"),
            Ok(Err(e)) => tracing::warn!(
                error = %e,
                "Deregistration failed (best-effort), entry will expire via TTL"
            ),
            Err(_) => {
                tracing::warn!("Deregistration timed out (best-effort), entry will expire via TTL")
            }
        }
    }

    /// Send a heartbeat to the joining service.
    ///
    /// Snapshots `is_registered` at the top to compute `is_first`, then
    /// writes the new state at the bottom based on the result:
    /// - On success: set to `true` so subsequent heartbeats omit `admin_secret`.
    /// - On any error: reset to `false` so the next attempt re-sends
    ///   `admin_secret`. This is how we recover from server-side eviction
    ///   (TTL expiry during a long outage, server restart, operator delete).
    async fn send_heartbeat(&mut self) -> anyhow::Result<HeartbeatResponse> {
        let is_first = !self.is_registered;

        // Inner async block so `?` short-circuits inside while we still
        // unconditionally update `is_registered` at the bottom of this fn.
        let result: anyhow::Result<HeartbeatResponse> = async {
            let pubkey = self.identity.public_key_base64();
            let linker_url = &self.config.public_url;
            let admin_url = self.config.admin_url();
            let timestamp = now_iso();

            // Build the canonical JSON fields for signing
            let signature = if is_first {
                sign_canonical(
                    self.identity.signing_key(),
                    &[
                        ("admin_secret", &self.admin_secret),
                        ("admin_url", &admin_url),
                        ("linker_url", linker_url),
                        ("pubkey", &pubkey),
                        ("timestamp", &timestamp),
                    ],
                )
            } else {
                sign_canonical(
                    self.identity.signing_key(),
                    &[
                        ("admin_url", &admin_url),
                        ("linker_url", linker_url),
                        ("pubkey", &pubkey),
                        ("timestamp", &timestamp),
                    ],
                )
            };

            // Build the request body
            let mut body = serde_json::Map::new();
            body.insert("pubkey".into(), serde_json::Value::String(pubkey));
            if let Some(ref token) = self.config.invite_token {
                body.insert(
                    "invite_token".into(),
                    serde_json::Value::String(token.clone()),
                );
            }
            body.insert(
                "linker_url".into(),
                serde_json::Value::String(linker_url.clone()),
            );
            body.insert("admin_url".into(), serde_json::Value::String(admin_url));
            if is_first {
                body.insert(
                    "admin_secret".into(),
                    serde_json::Value::String(self.admin_secret.clone()),
                );
            }
            // Secret rotation is not yet supported on the linker side;
            // the operator must restart the linker with a new admin_secret.
            body.insert("rotate_secret".into(), serde_json::Value::Bool(false));
            body.insert("timestamp".into(), serde_json::Value::String(timestamp));
            body.insert("signature".into(), serde_json::Value::String(signature));

            let url = format!("{}/v1/linkers/heartbeat", self.config.joining_service_url);
            let resp = self
                .http
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("Heartbeat request failed: {e}"))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(anyhow::anyhow!("Heartbeat rejected: {status} {text}"));
            }

            let heartbeat_resp: HeartbeatResponse = resp
                .json()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to parse heartbeat response: {e}"))?;

            if !heartbeat_resp.registered {
                return Err(anyhow::anyhow!("Heartbeat response: registered=false"));
            }

            Ok(heartbeat_resp)
        }
        .await;

        self.is_registered = result.is_ok();
        result
    }

    /// Send a deregistration request (best-effort).
    async fn deregister(&self) -> anyhow::Result<()> {
        let pubkey = self.identity.public_key_base64();
        let timestamp = now_iso();
        let signature = sign_canonical(
            self.identity.signing_key(),
            &[("pubkey", &pubkey), ("timestamp", &timestamp)],
        );

        // URL-encode the pubkey since base64 can contain +, /, =
        let encoded_pubkey = url_encode_path_segment(&pubkey);
        let url = format!(
            "{}/v1/linkers/{}",
            self.config.joining_service_url, encoded_pubkey
        );
        let body = DeregisterBody {
            timestamp,
            signature,
        };

        let resp = self
            .http
            .delete(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Deregister request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Deregister rejected: {status} {text}"));
        }

        Ok(())
    }
}

/// Build canonical JSON from key-value pairs and sign it.
///
/// Returns a standard base64-encoded ed25519 signature. The fields are
/// sorted by key inside `canonical_json`, so the input order does not
/// matter.
fn sign_canonical(key: &ed25519_dalek::SigningKey, fields: &[(&str, &str)]) -> String {
    let canonical = canonical_json(fields);
    let signature = key.sign(canonical.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
}

/// Percent-encode a string for use as a URL path segment.
///
/// Encodes characters that are not unreserved per RFC 3986
/// (letters, digits, `-`, `.`, `_`, `~`).
fn url_encode_path_segment(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    encoded
}

/// Produce canonical JSON: a JSON object with the given key-value pairs.
///
/// Keys are sorted alphabetically (via `BTreeMap`), with no extraneous
/// whitespace. Output matches the joining service's `canonicalJson()`
/// in `verify.ts`.
fn canonical_json(fields: &[(&str, &str)]) -> String {
    let map: BTreeMap<&str, &str> = fields.iter().copied().collect();
    serde_json::to_string(&map).expect("BTreeMap<&str, &str> serialization cannot fail")
}

/// Current UTC time as ISO 8601 with milliseconds (matches JS `toISOString()`).
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use ed25519_dalek::Verifier;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    #[test]
    fn test_canonical_json_sorted_keys() {
        let json = canonical_json(&[("a", "1"), ("b", "2"), ("c", "3")]);
        assert_eq!(json, r#"{"a":"1","b":"2","c":"3"}"#);
    }

    #[test]
    fn test_canonical_json_with_special_chars() {
        let json = canonical_json(&[("key", "value with spaces"), ("url", "https://example.com")]);
        assert_eq!(
            json,
            r#"{"key":"value with spaces","url":"https://example.com"}"#
        );
    }

    #[test]
    fn test_sign_canonical_verifies() {
        let secret: [u8; 32] = rand::random();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret);
        let verifying_key = signing_key.verifying_key();

        let fields = &[("foo", "bar"), ("pubkey", "test123")];
        let sig_b64 = sign_canonical(&signing_key, fields);

        // Decode and verify
        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(&sig_b64)
            .unwrap();
        let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes.try_into().unwrap());
        let canonical = canonical_json(fields);
        verifying_key
            .verify(canonical.as_bytes(), &signature)
            .expect("signature should verify");
    }

    #[test]
    fn test_now_iso_format() {
        let ts = now_iso();
        // Should match: YYYY-MM-DDTHH:MM:SS.mmmZ
        assert!(ts.ends_with('Z'), "timestamp should end with Z: {ts}");
        assert_eq!(ts.len(), 24, "timestamp should be 24 chars: {ts}");
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[13..14], ":");
        assert_eq!(&ts[16..17], ":");
        assert_eq!(&ts[19..20], ".");
    }

    #[test]
    fn test_admin_url_derivation() {
        let cfg = RegistrationConfig {
            joining_service_url: "http://js".into(),
            invite_token: None,
            public_url: "wss://host:8090".into(),
            initial_heartbeat_interval_secs: 200,
        };
        assert_eq!(cfg.admin_url(), "https://host:8090");

        let cfg = RegistrationConfig {
            public_url: "ws://host:8090".into(),
            ..cfg.clone()
        };
        assert_eq!(cfg.admin_url(), "http://host:8090");

        // Already http(s) — passthrough
        let cfg = RegistrationConfig {
            public_url: "https://host:8090".into(),
            ..cfg.clone()
        };
        assert_eq!(cfg.admin_url(), "https://host:8090");
    }

    // -- Integration tests with mock joining service --

    /// Helper to start a mock joining service on a random port.
    async fn start_mock_server(
        heartbeat_handler: axum::routing::MethodRouter,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let app = axum::Router::new().route("/v1/linkers/heartbeat", heartbeat_handler);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, handle)
    }

    /// Helper to start a mock server that also handles DELETE for deregistration.
    async fn start_mock_server_with_delete(
        heartbeat_handler: axum::routing::MethodRouter,
        delete_received: Arc<Mutex<Vec<serde_json::Value>>>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let app = axum::Router::new()
            .route("/v1/linkers/heartbeat", heartbeat_handler)
            .route(
                "/v1/linkers/{*rest}",
                axum::routing::delete(move |body: axum::Json<serde_json::Value>| async move {
                    delete_received.lock().unwrap().push(body.0);
                    axum::http::StatusCode::OK
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, handle)
    }

    fn test_identity() -> Arc<LinkerIdentity> {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::identity::IdentityConfig {
            key_file: dir.path().join("test.key"),
            private_key_base64: None,
        };
        // Leak the tempdir so it lives long enough
        let dir = Box::leak(Box::new(dir));
        let _ = dir;
        Arc::new(LinkerIdentity::load(&config).unwrap())
    }

    fn test_config(addr: std::net::SocketAddr) -> RegistrationConfig {
        RegistrationConfig {
            joining_service_url: format!("http://{addr}"),
            invite_token: Some("lnk_test_token".into()),
            public_url: "wss://test-linker.example.com:8090".into(),
            initial_heartbeat_interval_secs: 200,
        }
    }

    #[tokio::test]
    async fn test_first_heartbeat_includes_invite_and_secret() {
        let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let received2 = received.clone();

        let handler = post(move |body: axum::Json<serde_json::Value>| {
            let received = received2.clone();
            async move {
                received.lock().unwrap().push(body.0);
                axum::Json(serde_json::json!({
                    "registered": true,
                    "ttl_seconds": 600
                }))
            }
        });

        let (addr, _server) = start_mock_server(handler).await;
        let identity = test_identity();
        let config = test_config(addr);
        let (_, shutdown_rx) = tokio::sync::watch::channel(false);

        let mut client =
            RegistrationClient::new(identity, config, "test-admin-secret".into(), shutdown_rx);

        let resp = client.send_heartbeat().await.unwrap();
        assert!(resp.registered);

        let bodies = received.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        let body = &bodies[0];

        // First heartbeat should include invite_token and admin_secret
        assert_eq!(body["invite_token"], "lnk_test_token");
        assert_eq!(body["admin_secret"], "test-admin-secret");
        assert_eq!(body["linker_url"], "wss://test-linker.example.com:8090");
        assert_eq!(body["admin_url"], "https://test-linker.example.com:8090");
        assert!(body["pubkey"].is_string());
        assert!(body["signature"].is_string());
        assert!(body["timestamp"].is_string());
        assert_eq!(body["rotate_secret"], false);
    }

    #[tokio::test]
    async fn test_subsequent_heartbeat_omits_secret() {
        let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let received2 = received.clone();

        let handler = post(move |body: axum::Json<serde_json::Value>| {
            let received = received2.clone();
            async move {
                received.lock().unwrap().push(body.0);
                axum::Json(serde_json::json!({
                    "registered": true,
                    "ttl_seconds": 600
                }))
            }
        });

        let (addr, _server) = start_mock_server(handler).await;
        let identity = test_identity();
        let config = test_config(addr);
        let (_, shutdown_rx) = tokio::sync::watch::channel(false);

        let mut client =
            RegistrationClient::new(identity, config, "test-admin-secret".into(), shutdown_rx);

        // First heartbeat -- send_heartbeat updates is_registered itself
        client.send_heartbeat().await.unwrap();
        // Second heartbeat
        client.send_heartbeat().await.unwrap();

        let bodies = received.lock().unwrap();
        assert_eq!(bodies.len(), 2);

        // First should have admin_secret
        assert!(bodies[0].get("admin_secret").is_some());

        // Second should NOT have admin_secret
        assert!(
            bodies[1].get("admin_secret").is_none(),
            "subsequent heartbeat should not include admin_secret"
        );
    }

    #[tokio::test]
    async fn test_heartbeat_always_sends_invite_token() {
        let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let received2 = received.clone();

        let handler = post(move |body: axum::Json<serde_json::Value>| {
            let received = received2.clone();
            async move {
                received.lock().unwrap().push(body.0);
                axum::Json(serde_json::json!({
                    "registered": true,
                    "ttl_seconds": 600
                }))
            }
        });

        let (addr, _server) = start_mock_server(handler).await;
        let identity = test_identity();
        let config = test_config(addr);
        let (_, shutdown_rx) = tokio::sync::watch::channel(false);

        let mut client = RegistrationClient::new(identity, config, "secret".into(), shutdown_rx);

        client.send_heartbeat().await.unwrap();
        client.send_heartbeat().await.unwrap();

        let bodies = received.lock().unwrap();
        // Both should have invite_token
        assert_eq!(bodies[0]["invite_token"], "lnk_test_token");
        assert_eq!(bodies[1]["invite_token"], "lnk_test_token");
    }

    #[tokio::test]
    async fn test_heartbeat_signature_verifies() {
        let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let received2 = received.clone();

        let handler = post(move |body: axum::Json<serde_json::Value>| {
            let received = received2.clone();
            async move {
                received.lock().unwrap().push(body.0);
                axum::Json(serde_json::json!({
                    "registered": true,
                    "ttl_seconds": 600
                }))
            }
        });

        let (addr, _server) = start_mock_server(handler).await;
        let identity = test_identity();
        let pubkey_b64 = identity.public_key_base64();

        // Decode pubkey to get verifying key
        let pubkey_bytes = base64::engine::general_purpose::STANDARD
            .decode(&pubkey_b64)
            .unwrap();
        let verifying_key =
            ed25519_dalek::VerifyingKey::from_bytes(&pubkey_bytes.try_into().unwrap()).unwrap();

        let config = test_config(addr);
        let (_, shutdown_rx) = tokio::sync::watch::channel(false);
        let mut client = RegistrationClient::new(identity, config, "my-secret".into(), shutdown_rx);

        client.send_heartbeat().await.unwrap();

        let bodies = received.lock().unwrap();
        let body = &bodies[0];

        // Reconstruct what should have been signed (first heartbeat)
        let canonical = canonical_json(&[
            ("admin_secret", "my-secret"),
            ("admin_url", body["admin_url"].as_str().unwrap()),
            ("linker_url", body["linker_url"].as_str().unwrap()),
            ("pubkey", body["pubkey"].as_str().unwrap()),
            ("timestamp", body["timestamp"].as_str().unwrap()),
        ]);

        let sig_b64 = body["signature"].as_str().unwrap();
        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(sig_b64)
            .unwrap();
        let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes.try_into().unwrap());

        verifying_key
            .verify(canonical.as_bytes(), &signature)
            .expect("heartbeat signature should verify");
    }

    #[tokio::test]
    async fn test_heartbeat_retries_on_failure() {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count2 = call_count.clone();

        let handler = post(move || {
            let call_count = call_count2.clone();
            async move {
                let n = call_count.fetch_add(1, Ordering::Relaxed);
                if n == 0 {
                    // First call: fail
                    (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        "server error".to_string(),
                    )
                        .into_response()
                } else {
                    // Subsequent calls: succeed
                    axum::Json(serde_json::json!({
                        "registered": true,
                        "ttl_seconds": 600
                    }))
                    .into_response()
                }
            }
        });

        let (addr, _server) = start_mock_server(handler).await;
        let identity = test_identity();
        let config = test_config(addr);
        let (_, shutdown_rx) = tokio::sync::watch::channel(false);
        let mut client = RegistrationClient::new(identity, config, "secret".into(), shutdown_rx);

        // First call should fail
        assert!(client.send_heartbeat().await.is_err());

        // Second call should succeed
        assert!(client.send_heartbeat().await.is_ok());

        assert_eq!(call_count.load(Ordering::Relaxed), 2);
    }

    /// Eviction recovery: if a heartbeat fails (network error, server lost
    /// our entry, etc.) the next attempt must re-send `admin_secret` and
    /// the invite so the joining service can re-register us.
    #[tokio::test]
    async fn test_heartbeat_recovers_after_failure_with_admin_secret() {
        let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let call_count = Arc::new(AtomicU32::new(0));
        let received2 = received.clone();
        let call_count2 = call_count.clone();

        // Sequence: success, success (server lost us, returns 400), success
        let handler = post(move |body: axum::Json<serde_json::Value>| {
            let received = received2.clone();
            let call_count = call_count2.clone();
            async move {
                let n = call_count.fetch_add(1, Ordering::Relaxed);
                received.lock().unwrap().push(body.0);
                if n == 1 {
                    // Simulate server-side eviction: it doesn't know us
                    (axum::http::StatusCode::BAD_REQUEST, "not_registered").into_response()
                } else {
                    axum::Json(serde_json::json!({
                        "registered": true,
                        "ttl_seconds": 600
                    }))
                    .into_response()
                }
            }
        });

        let (addr, _server) = start_mock_server(handler).await;
        let identity = test_identity();
        let config = test_config(addr);
        let (_, shutdown_rx) = tokio::sync::watch::channel(false);
        let mut client = RegistrationClient::new(identity, config, "my-secret".into(), shutdown_rx);

        // 1st: succeeds, registers
        client.send_heartbeat().await.unwrap();
        assert!(client.is_registered);
        // 2nd: 400, is_registered must reset
        assert!(client.send_heartbeat().await.is_err());
        assert!(
            !client.is_registered,
            "is_registered must reset on heartbeat failure for eviction recovery"
        );
        // 3rd: succeeds again as a "first" heartbeat (with admin_secret)
        client.send_heartbeat().await.unwrap();

        let bodies = received.lock().unwrap();
        assert_eq!(bodies.len(), 3);
        assert!(bodies[0]["admin_secret"].is_string(), "1st has secret");
        assert!(
            bodies[1].get("admin_secret").is_none(),
            "2nd is a renewal (no secret) -- this is the heartbeat the server rejects"
        );
        assert!(
            bodies[2]["admin_secret"].is_string(),
            "3rd must re-send admin_secret to recover from server-side eviction"
        );
    }

    /// 4xx error pinning: the linker treats it like any other failure
    /// and retries (via the run() loop's retry path). This pins current
    /// behavior; no exponential backoff or permanent-failure detection.
    #[tokio::test]
    async fn test_heartbeat_4xx_returns_error_and_resets_state() {
        let handler = post(|| async {
            (
                axum::http::StatusCode::BAD_REQUEST,
                "invalid_signature".to_string(),
            )
                .into_response()
        });

        let (addr, _server) = start_mock_server(handler).await;
        let identity = test_identity();
        let config = test_config(addr);
        let (_, shutdown_rx) = tokio::sync::watch::channel(false);
        let mut client = RegistrationClient::new(identity, config, "secret".into(), shutdown_rx);

        let err = client.send_heartbeat().await.unwrap_err();
        assert!(
            err.to_string().contains("400"),
            "error should mention 400, got: {err}"
        );
        assert!(!client.is_registered);
    }

    /// Server-directed interval is respected: assert the second heartbeat
    /// fires after the server's interval, not the client's initial 200s.
    /// Server returns interval=1s; if the client used its initial value
    /// the second heartbeat would not arrive within the test window.
    #[tokio::test]
    async fn test_server_interval_respected() {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count2 = call_count.clone();

        let handler = post(move || {
            let call_count = call_count2.clone();
            async move {
                call_count.fetch_add(1, Ordering::Relaxed);
                axum::Json(serde_json::json!({
                    "registered": true,
                    "ttl_seconds": 60,
                    "heartbeat_interval_seconds": 1
                }))
            }
        });

        let (addr, _server) = start_mock_server(handler).await;
        let identity = test_identity();
        // initial_heartbeat_interval_secs = 60: if the client ignored the
        // server-directed interval, only the first heartbeat would fire
        // within the test window.
        let config = RegistrationConfig {
            joining_service_url: format!("http://{addr}"),
            invite_token: Some("lnk_test_token".into()),
            public_url: "wss://test-linker.example.com:8090".into(),
            initial_heartbeat_interval_secs: 60,
        };
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        let client = RegistrationClient::new(identity, config, "secret".into(), shutdown_rx);

        let handle = tokio::spawn(async move {
            client.run().await;
        });

        // Allow time for first heartbeat + server-directed 1s sleep + second heartbeat
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let _ = shutdown_tx.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;

        let n = call_count.load(Ordering::Relaxed);
        assert!(
            n >= 2,
            "expected at least 2 heartbeats (server interval respected), got {n}"
        );
    }

    /// A hung heartbeat (server never responds) must not delay shutdown
    /// until reqwest's 30s timeout.
    #[tokio::test]
    async fn test_shutdown_during_hung_heartbeat_exits_promptly() {
        let handler = post(|| async {
            // Never responds
            std::future::pending::<()>().await;
            // Unreachable, but give axum a concrete return type
            axum::http::StatusCode::OK
        });

        let (addr, _server) = start_mock_server(handler).await;
        let identity = test_identity();
        let config = test_config(addr);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let client = RegistrationClient::new(identity, config, "secret".into(), shutdown_rx);

        let handle = tokio::spawn(async move {
            client.run().await;
        });

        // Let the heartbeat begin and start waiting for a response
        tokio::time::sleep(Duration::from_millis(150)).await;

        let start = std::time::Instant::now();
        let _ = shutdown_tx.send(true);
        // run() should exit quickly even though the heartbeat is hung;
        // 2s is generous and well under reqwest's 30s default.
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("run() should exit promptly even during a hung heartbeat")
            .expect("run() task should not panic");
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "shutdown took too long: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn test_deregistration_on_shutdown() {
        let delete_received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));

        let handler = post(|| async {
            axum::Json(serde_json::json!({
                "registered": true,
                "ttl_seconds": 600,
                "heartbeat_interval_seconds": 3600
            }))
        });

        let (addr, _server) = start_mock_server_with_delete(handler, delete_received.clone()).await;

        let identity = test_identity();
        let config = test_config(addr);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        let client = RegistrationClient::new(identity, config, "secret".into(), shutdown_rx);

        let handle = tokio::spawn(async move {
            client.run().await;
        });

        // Let it register
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Trigger shutdown
        let _ = shutdown_tx.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;

        // Verify DELETE was received
        let deletes = delete_received.lock().unwrap();
        assert_eq!(deletes.len(), 1, "should have received one DELETE request");
        assert!(deletes[0]["timestamp"].is_string());
        assert!(deletes[0]["signature"].is_string());
    }
}
