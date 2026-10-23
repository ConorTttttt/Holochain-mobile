//! Configuration for h2hc-linker

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::identity::IdentityConfig;

/// How session/agent state is persisted.
#[derive(Debug, Clone, Default)]
pub enum SessionStoreConfig {
    /// In-memory only (lost on restart). Default.
    #[default]
    Memory,
    /// SQLite file at the given path.
    Sqlite { path: PathBuf },
}

/// Configure Kitsune2 Reporting.
///
/// Matches the conductor's `ReportConfig` enum so the log-collector
/// can process linker and conductor reports identically.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportConfig {
    /// No reporting (default).
    #[default]
    None,

    /// Write daily-rotated JSONL report files (`hc-report.YYYY-MM-DD.jsonl`).
    JsonLines {
        /// How many days worth of report files to retain.
        days_retained: u32,

        /// How often to report Fetched-Op aggregated data in seconds.
        fetched_op_interval_s: u32,
    },
}

/// Configuration for joining-service registration (opt-in).
///
/// Note: `admin_url` (the URL the joining service uses to call back into
/// this linker's admin API) is derived from `public_url` by swapping the
/// scheme (`wss://` -> `https://`, `ws://` -> `http://`). This bakes in
/// the assumption that the WS interface and the admin HTTP interface
/// share host and port -- which is true for the current single-axum-server
/// deployment. A future split-port deployment would need an explicit
/// admin_url field here.
#[derive(Debug, Clone)]
pub struct RegistrationConfig {
    /// Joining service base URL.
    pub joining_service_url: String,
    /// Invite token from the joining service operator.
    pub invite_token: Option<String>,
    /// Externally-reachable WSS URL for this linker.
    pub public_url: String,
    /// Initial heartbeat interval in seconds. Replaced by the
    /// `heartbeat_interval_seconds` from the server response after the
    /// first successful heartbeat.
    pub initial_heartbeat_interval_secs: u64,
}

impl RegistrationConfig {
    /// Derive the admin URL from the public URL by swapping the scheme.
    ///
    /// `wss://host:port` -> `https://host:port`
    /// `ws://host:port`  -> `http://host:port`
    pub fn admin_url(&self) -> String {
        if self.public_url.starts_with("wss://") {
            self.public_url.replacen("wss://", "https://", 1)
        } else if self.public_url.starts_with("ws://") {
            self.public_url.replacen("ws://", "http://", 1)
        } else {
            // Already http(s), use as-is
            self.public_url.clone()
        }
    }
}

/// Default timeout for zome calls
pub const DEFAULT_ZOME_CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// WebSocket configuration
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// Interval between heartbeat pings
    pub heartbeat_interval: Duration,
    /// Timeout for heartbeat responses
    pub heartbeat_timeout: Duration,
    /// Idle timeout for connections
    pub idle_timeout: Duration,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(30),
            heartbeat_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(300), // 5 minutes
        }
    }
}

/// Configuration for the Holochain Membrane gateway
#[derive(Debug, Clone)]
pub struct Configuration {
    /// Conductor address for zome call proxying
    pub conductor_url: Option<SocketAddr>,

    /// Bootstrap server URL for Kitsune2 (required)
    pub bootstrap_url: String,

    /// Iroh relay server URL for Kitsune2
    pub relay_url: Option<String>,

    /// Maximum payload size in bytes
    pub payload_limit_bytes: usize,

    /// WebSocket configuration
    pub websocket: WebSocketConfig,

    /// Timeout for zome calls
    pub zome_call_timeout: Duration,

    /// Admin secret for authentication (from H2HC_LINKER_ADMIN_SECRET)
    /// When set, enables the auth layer.
    pub admin_secret: Option<String>,

    /// Session store backend (from H2HC_LINKER_SESSION_STORE)
    pub session_store: SessionStoreConfig,

    /// Kitsune2 report configuration (from H2HC_LINKER_REPORT)
    pub report: ReportConfig,

    /// Directory path for report files (from H2HC_LINKER_REPORT_PATH)
    pub report_path: PathBuf,

    /// Identity configuration for the persistent keypair.
    pub identity: IdentityConfig,

    /// Registration configuration (None = registration disabled).
    pub registration: Option<RegistrationConfig>,

    /// Whether `get`/`get_links` fall back to this gateway's local publish
    /// buffer (`crate::local_authority`) for co-located zero-arc agents when
    /// no remote DHT authority has the data yet (from
    /// `H2HC_LINKER_LOCAL_AUTHORITY_FALLBACK`).
    ///
    /// Defaults to `true` so existing deployments are unaffected by this
    /// field's introduction. Data served through this path carries an
    /// *unknown* validation status (`Judged::none`), not a real authority's
    /// verdict - see the `local_authority` module doc comment - so this is
    /// an operator-facing kill switch for that code path in case it needs to
    /// be turned off in the field without a rebuild.
    pub local_authority_fallback_enabled: bool,

    /// Conductor-facing protocol (`/conductor/v1`) settings.
    pub conductor_link: ConductorLinkConfig,
}

/// Settings for conductors connected over the conductor-facing protocol.
#[derive(Debug, Clone)]
pub struct ConductorLinkConfig {
    /// Directory for the acceptance store (`H2HC_LINKER_DATA_DIR`).
    pub data_dir: PathBuf,
    /// How long accepted ops are kept (`H2HC_LINKER_RETENTION_S`).
    pub retention: Duration,
    /// Leave Kitsune2 when a conductor disconnects
    /// (`H2HC_LINKER_LEAVE_ON_DISCONNECT`). Off by default, so a phone that
    /// drops briefly stays routable until its agent info expires.
    pub leave_on_disconnect: bool,
}

impl Default for ConductorLinkConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("/tmp/h2hc-linker-data"),
            retention: Duration::from_secs(7 * 24 * 60 * 60),
            leave_on_disconnect: false,
        }
    }
}

fn parse_bool(name: &str, val: &str) -> anyhow::Result<bool> {
    match val.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" | "enabled" => Ok(true),
        "false" | "0" | "no" | "off" | "disabled" => Ok(false),
        other => Err(anyhow::anyhow!(
            "Unknown {name} value: '{other}'. Use 'true' or 'false'."
        )),
    }
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            conductor_url: None,
            bootstrap_url: String::new(),
            relay_url: None,
            payload_limit_bytes: 20 * 1024 * 1024, // 20MB - must fit largest Holochain entry (16MB) + base64/JSON overhead
            websocket: WebSocketConfig::default(),
            zome_call_timeout: DEFAULT_ZOME_CALL_TIMEOUT,
            admin_secret: None,
            session_store: SessionStoreConfig::default(),
            report: ReportConfig::None,
            report_path: PathBuf::from("/tmp/h2hc-linker-reports"),
            identity: IdentityConfig::default(),
            registration: None,
            local_authority_fallback_enabled: true,
            conductor_link: ConductorLinkConfig::default(),
        }
    }
}

impl Configuration {
    /// Create a new configuration from environment variables
    pub fn from_env() -> anyhow::Result<Self> {
        let mut config = Self::default();

        // Conductor address for zome call proxying
        if let Ok(url) = std::env::var("H2HC_LINKER_CONDUCTOR_URL") {
            config.conductor_url = Some(url.parse()?);
        }

        // Kitsune2 configuration (bootstrap URL is required)
        match std::env::var("H2HC_LINKER_BOOTSTRAP_URL") {
            Ok(url) => config.bootstrap_url = url,
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "H2HC_LINKER_BOOTSTRAP_URL is required. \
                     h2hc-linker cannot operate without kitsune2 networking. \
                     Set it to your bootstrap server URL (e.g. http://127.0.0.1:PORT)"
                ));
            }
        }
        if let Ok(url) = std::env::var("H2HC_LINKER_RELAY_URL") {
            config.relay_url = Some(url);
        }

        // Payload limit
        if let Ok(limit) = std::env::var("H2HC_LINKER_PAYLOAD_LIMIT_BYTES") {
            config.payload_limit_bytes = limit.parse()?;
        }

        // Zome call timeout
        if let Ok(timeout) = std::env::var("H2HC_LINKER_ZOME_CALL_TIMEOUT_MS") {
            config.zome_call_timeout = Duration::from_millis(timeout.parse()?);
        }

        // Report configuration
        if let Ok(report_type) = std::env::var("H2HC_LINKER_REPORT") {
            match report_type.to_lowercase().as_str() {
                "json_lines" | "jsonlines" | "jsonl" => {
                    let days_retained: u32 = std::env::var("H2HC_LINKER_REPORT_DAYS_RETAINED")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(5);
                    let fetched_op_interval_s: u32 = std::env::var("H2HC_LINKER_REPORT_INTERVAL_S")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(60);
                    config.report = ReportConfig::JsonLines {
                        days_retained,
                        fetched_op_interval_s,
                    };
                }
                "none" | "" => {}
                other => {
                    return Err(anyhow::anyhow!(
                        "Unknown H2HC_LINKER_REPORT value: '{other}'. Use 'json_lines' or 'none'."
                    ));
                }
            }
        }
        if let Ok(path) = std::env::var("H2HC_LINKER_REPORT_PATH") {
            config.report_path = PathBuf::from(path);
        }

        // Identity configuration
        if let Ok(path) = std::env::var("H2HC_LINKER_KEY_FILE") {
            config.identity.key_file = PathBuf::from(path);
        }
        if let Ok(key) = std::env::var("H2HC_LINKER_PRIVATE_KEY") {
            config.identity.private_key_base64 = Some(key);
        }

        // Auth configuration
        if let Ok(secret) = std::env::var("H2HC_LINKER_ADMIN_SECRET") {
            config.admin_secret = Some(secret);
        }

        // Registration configuration (opt-in)
        let joining_service_url = std::env::var("H2HC_LINKER_JOINING_SERVICE_URL").ok();
        let public_url = std::env::var("H2HC_LINKER_PUBLIC_URL").ok();

        match (&joining_service_url, &public_url) {
            (Some(js_url), Some(pub_url)) => {
                // Validate URLs at config load so misconfiguration surfaces
                // at startup rather than as a server-side rejection.
                let parsed_js = url::Url::parse(js_url).map_err(|e| {
                    anyhow::anyhow!("H2HC_LINKER_JOINING_SERVICE_URL is not a valid URL: {e}")
                })?;
                if !matches!(parsed_js.scheme(), "http" | "https") {
                    return Err(anyhow::anyhow!(
                        "H2HC_LINKER_JOINING_SERVICE_URL must use http:// or https:// scheme, got: {}",
                        parsed_js.scheme()
                    ));
                }

                let parsed_pub = url::Url::parse(pub_url).map_err(|e| {
                    anyhow::anyhow!("H2HC_LINKER_PUBLIC_URL is not a valid URL: {e}")
                })?;
                if !matches!(parsed_pub.scheme(), "ws" | "wss" | "http" | "https") {
                    return Err(anyhow::anyhow!(
                        "H2HC_LINKER_PUBLIC_URL must use ws://, wss://, http://, or https:// scheme, got: {}",
                        parsed_pub.scheme()
                    ));
                }

                let interval: u64 = std::env::var("H2HC_LINKER_HEARTBEAT_INTERVAL_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(200);

                config.registration = Some(RegistrationConfig {
                    joining_service_url: js_url.clone(),
                    invite_token: std::env::var("H2HC_LINKER_INVITE_TOKEN").ok(),
                    public_url: pub_url.clone(),
                    initial_heartbeat_interval_secs: interval,
                });
            }
            (Some(_), None) => {
                return Err(anyhow::anyhow!(
                    "H2HC_LINKER_JOINING_SERVICE_URL is set but H2HC_LINKER_PUBLIC_URL is missing. \
                     Both are required for registration."
                ));
            }
            (None, Some(_)) => {
                return Err(anyhow::anyhow!(
                    "H2HC_LINKER_PUBLIC_URL is set but H2HC_LINKER_JOINING_SERVICE_URL is missing. \
                     Both are required for registration."
                ));
            }
            (None, None) => {} // Registration disabled
        }

        // Session store backend
        if let Ok(val) = std::env::var("H2HC_LINKER_SESSION_STORE") {
            match val.as_str() {
                "" | "memory" => {} // default
                s if s.starts_with("sqlite://") => {
                    let path = s.trim_start_matches("sqlite://");
                    config.session_store = SessionStoreConfig::Sqlite {
                        path: PathBuf::from(path),
                    };
                }
                other => {
                    return Err(anyhow::anyhow!(
                        "Unknown H2HC_LINKER_SESSION_STORE value: '{other}'. \
                         Use 'memory' or 'sqlite:///path/to/sessions.db'."
                    ));
                }
            }
        }

        // Local-authority get/get_links fallback (opt-out; defaults to enabled).
        // Parsing is strict, matching H2HC_LINKER_SESSION_STORE's pattern above: an
        // unrecognized value (typo, "disable", empty string, ...) is a startup error
        // rather than silently leaving the fallback enabled - this is a field kill
        // switch, so a value that fails to parse must not be treated as a no-op.
        if let Ok(val) = std::env::var("H2HC_LINKER_LOCAL_AUTHORITY_FALLBACK") {
            config.local_authority_fallback_enabled = match val.to_lowercase().as_str() {
                "true" | "1" | "yes" | "on" | "enabled" => true,
                "false" | "0" | "no" | "off" | "disabled" => false,
                other => {
                    return Err(anyhow::anyhow!(
                        "Unknown H2HC_LINKER_LOCAL_AUTHORITY_FALLBACK value: '{other}'. \
                         Use 'true' or 'false' (also accepts '1'/'0', 'yes'/'no', 'on'/'off', 'enabled'/'disabled')."
                    ));
                }
            };
        }

        if let Ok(dir) = std::env::var("H2HC_LINKER_DATA_DIR") {
            config.conductor_link.data_dir = PathBuf::from(dir);
        }
        if let Ok(secs) = std::env::var("H2HC_LINKER_RETENTION_S") {
            config.conductor_link.retention = Duration::from_secs(secs.parse()?);
        }
        if let Ok(val) = std::env::var("H2HC_LINKER_LEAVE_ON_DISCONNECT") {
            config.conductor_link.leave_on_disconnect =
                parse_bool("H2HC_LINKER_LEAVE_ON_DISCONNECT", &val)?;
        }

        Ok(config)
    }

    /// Kitsune2 is always enabled (bootstrap URL is required at startup).
    /// This method exists for backwards compatibility with code that checks it.
    pub fn kitsune_enabled(&self) -> bool {
        true
    }

    /// Check if conductor integration is configured
    pub fn conductor_enabled(&self) -> bool {
        self.conductor_url.is_some()
    }

    /// Check if authentication is enabled (admin secret is set)
    pub fn auth_enabled(&self) -> bool {
        self.admin_secret.is_some()
    }

    /// Check if registration with a joining service is configured.
    pub fn registration_enabled(&self) -> bool {
        self.registration.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_auth_disabled() {
        let config = Configuration::default();
        assert!(!config.auth_enabled());
        assert!(config.admin_secret.is_none());
    }

    #[test]
    fn test_auth_enabled_with_secret() {
        let mut config = Configuration::default();
        config.admin_secret = Some("test-secret".to_string());
        assert!(config.auth_enabled());
    }
}
