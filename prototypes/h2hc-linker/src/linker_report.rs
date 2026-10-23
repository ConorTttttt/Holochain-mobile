//! Kitsune2 reporting for h2hc-linker.
//!
//! When enabled in the linker config, this module collects reports about
//! ops fetched from remote peers, aggregates them periodically, and writes
//! them to daily-rotated JSONL files on disk. The file format is identical
//! to holochain's `HcReport` so the unyt log-collector can process both.
//!
//! Unlike holochain's `HcReportFactory`, this implementation uses a
//! linker-owned ed25519 keypair (generated at startup) for signing
//! report entries instead of a lair keystore.

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use holochain_types::report::{ReportEntry, ReportEntryFetchedOps};
use kitsune2_api::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

/// Module-level configuration for the report module.
///
/// Field names and serde attributes match holochain_p2p's private
/// `HcReportModConfig` / `HcReportConfig` so that
/// `Config::get_module_config` / `set_module_config` produce identical
/// JSON keys in the kitsune2 config.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HcReportModConfig {
    /// The inner report configuration.
    pub hc_report: HcReportConfig,
}

/// Configuration for report file output.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HcReportConfig {
    /// How many days worth of report files to retain.
    pub days_retained: u32,
    /// Directory path for the report files.
    /// Files will be named `hc-report.YYYY-MM-DD.jsonl`.
    pub path: std::path::PathBuf,
    /// How often to report Fetched-Op aggregated data in seconds.
    pub fetched_op_interval_s: u32,
}

impl Default for HcReportConfig {
    fn default() -> Self {
        Self {
            days_retained: 5,
            path: "/tmp/h2hc-linker-reports".into(),
            fetched_op_interval_s: 60,
        }
    }
}

/// Generate a random ed25519 signing key (used in tests).
#[cfg(test)]
fn generate_signing_key() -> SigningKey {
    let secret: [u8; 32] = rand::random();
    SigningKey::from_bytes(&secret)
}

/// Factory for creating [`LinkerReport`] instances.
///
/// Implements the kitsune2 `ReportFactory` trait. Uses the linker's
/// persistent ed25519 keypair (from [`crate::identity::LinkerIdentity`])
/// for signing report entries.
pub struct LinkerReportFactory {
    signing_key: SigningKey,
}

impl std::fmt::Debug for LinkerReportFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkerReportFactory").finish()
    }
}

impl LinkerReportFactory {
    /// Construct a new [`LinkerReportFactory`] with the given signing key.
    ///
    /// The signing key should come from [`crate::identity::LinkerIdentity`]
    /// so that reports and registration share the same identity.
    pub fn create(signing_key: SigningKey) -> DynReportFactory {
        let out: DynReportFactory = Arc::new(Self { signing_key });
        out
    }
}

impl ReportFactory for LinkerReportFactory {
    fn default_config(&self, config: &mut Config) -> K2Result<()> {
        config.set_module_config(&HcReportModConfig {
            hc_report: HcReportConfig::default(),
        })?;
        Ok(())
    }

    fn validate_config(&self, _config: &Config) -> K2Result<()> {
        Ok(())
    }

    fn create(
        &self,
        builder: Arc<Builder>,
        _tx: DynTransport,
    ) -> BoxFut<'static, K2Result<DynReport>> {
        let signing_key = self.signing_key.clone();
        let result: K2Result<DynReport> = (|| {
            let config: HcReportModConfig = builder.config.get_module_config()?;
            let report = LinkerReport::create(config.hc_report, signing_key)?;
            let out: DynReport = report;
            Ok(out)
        })();

        Box::pin(async move { result })
    }
}

/// Type sent on our internal command channel.
enum Cmd {
    /// Indicates we have received op data from a remote peer.
    FetchedOp { space_id: SpaceId, size_bytes: u64 },
}

struct LinkerReport {
    #[allow(dead_code)] // Kept for Arc weak-self pattern consistency
    this: Weak<Self>,

    /// Ed25519 signing key for report entries.
    signing_key: SigningKey,

    /// AgentPubKey derived from the signing key (base64url encoded for reports).
    agent_pubkey_str: String,

    /// Timing loop task.
    task: tokio::task::AbortHandle,

    /// Sender side of command channel.
    cmd_send: tokio::sync::mpsc::Sender<Cmd>,

    /// Receiver side of command channel.
    cmd_recv: Mutex<tokio::sync::mpsc::Receiver<Cmd>>,

    /// The log file writer.
    file_writer: Mutex<tracing_appender::non_blocking::NonBlocking>,

    /// The guard that shuts down the non-blocking task on drop.
    _file_guard: tracing_appender::non_blocking::WorkerGuard,
}

impl Drop for LinkerReport {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl std::fmt::Debug for LinkerReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkerReport").finish()
    }
}

impl LinkerReport {
    pub fn create(config: HcReportConfig, signing_key: SigningKey) -> K2Result<Arc<LinkerReport>> {
        // Build the agent pubkey string for report entries.
        // Use the same format as holochain: "u" prefix + base64url of 39-byte AgentPubKey.
        let pubkey_bytes = signing_key.verifying_key().to_bytes();
        let agent_pubkey = holo_hash::AgentPubKey::from_raw_32(pubkey_bytes.to_vec());
        let raw_39 = agent_pubkey.get_raw_39();
        let agent_pubkey_str = format!(
            "u{}",
            base64::prelude::BASE64_URL_SAFE_NO_PAD.encode(raw_39)
        );

        // Set up daily-rotated JSONL file writer (identical to holochain)
        let file = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .max_log_files(config.days_retained as usize)
            .filename_prefix("hc-report")
            .filename_suffix("jsonl")
            .build(config.path)
            .map_err(K2Error::other)?;

        let (file_writer, _file_guard) = tracing_appender::non_blocking(file);

        let (cmd_send, cmd_recv) = tokio::sync::mpsc::channel(4096);

        let out = Arc::new_cyclic(move |this: &Weak<Self>| {
            let freq = config.fetched_op_interval_s;
            let this2 = this.clone();

            let task = tokio::task::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(freq as u64)).await;

                    if let Some(this) = this2.upgrade() {
                        this.process_reports();
                    } else {
                        tracing::debug!("linker report loop ending");
                        return;
                    }
                }
            })
            .abort_handle();

            LinkerReport {
                this: this.clone(),
                signing_key,
                agent_pubkey_str,
                task,
                cmd_send,
                cmd_recv: Mutex::new(cmd_recv),
                file_writer: Mutex::new(file_writer),
                _file_guard,
            }
        });

        // Write a start entry
        out.write(ReportEntry::start());

        Ok(out)
    }

    /// Lowest-level write function.
    fn write_raw(&self, b: &[u8]) {
        if let Err(err) = std::io::Write::write_all(&mut *self.file_writer.lock().unwrap(), b) {
            tracing::error!(?err, "failed to write data to linker report writer");
        }
    }

    /// Encode a report entry and write it to the file.
    fn write(&self, data: ReportEntry) {
        let mut data = match serde_json::to_string(&data) {
            Ok(data) => data,
            Err(err) => {
                tracing::error!(?err, "failed to encode report entry");
                return;
            }
        };
        data.push('\n');
        self.write_raw(data.as_bytes());
    }

    /// Aggregate and write report entries for the current interval.
    fn process_reports(&self) {
        // Aggregate fetched ops by space
        let mut fetched_ops: HashMap<SpaceId, (u64, u64)> = HashMap::new();

        {
            let mut lock = self.cmd_recv.lock().unwrap();
            while let Ok(cmd) = lock.try_recv() {
                match cmd {
                    Cmd::FetchedOp {
                        space_id,
                        size_bytes,
                    } => {
                        let e = fetched_ops.entry(space_id).or_default();
                        e.0 += 1;
                        e.1 += size_bytes;
                    }
                }
            }
        }

        if fetched_ops.is_empty() {
            return;
        }

        for (space_id, (op_count, total_bytes)) in fetched_ops {
            let timestamp = Timestamp::now().as_micros().to_string();
            let space = space_id.to_string();
            let op_count = op_count.to_string();
            let total_bytes = total_bytes.to_string();

            let mut entry = ReportEntryFetchedOps {
                timestamp,
                space,
                op_count,
                total_bytes,
                agent_pubkeys: vec![self.agent_pubkey_str.clone()],
                signatures: Vec::with_capacity(1),
            };

            // Sign the report using the linker's own keypair
            let to_sign = entry.encode_for_verification();
            let signature = self.signing_key.sign(&to_sign);
            entry
                .signatures
                .push(base64::prelude::BASE64_URL_SAFE_NO_PAD.encode(signature.to_bytes()));

            let entry = ReportEntry::FetchedOps(entry);
            self.write(entry);
        }
    }
}

impl Report for LinkerReport {
    fn space(&self, _space_id: SpaceId, _local_agent_store: DynLocalAgentStore) {
        // The linker doesn't need to track spaces for report signing
        // since it uses its own keypair rather than per-space agent keys.
    }

    fn fetched_op(&self, space_id: SpaceId, _source: Url, _op_id: OpId, size_bytes: u64) {
        if let Err(err) = self.cmd_send.try_send(Cmd::FetchedOp {
            space_id,
            size_bytes,
        }) {
            tracing::warn!(?err, "failed to process fetched op report");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linker_report_factory_creates() {
        let signing_key = generate_signing_key();
        let factory = LinkerReportFactory::create(signing_key);
        assert!(format!("{factory:?}").contains("LinkerReportFactory"));
    }

    #[tokio::test]
    async fn test_report_writes_start_entry() {
        let dir = tempfile::tempdir().unwrap();
        let signing_key = generate_signing_key();

        let config = HcReportConfig {
            days_retained: 5,
            path: dir.path().into(),
            fetched_op_interval_s: 60 * 60 * 24, // very long, we'll call manually
        };

        let report = LinkerReport::create(config, signing_key).unwrap();

        // Give the non-blocking writer time to flush
        drop(report);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Read the report files
        let mut found_start = false;
        let mut d = tokio::fs::read_dir(dir.path()).await.unwrap();
        while let Ok(Some(e)) = d.next_entry().await {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("hc-report.") && name.ends_with(".jsonl") {
                let data = tokio::fs::read_to_string(e.path()).await.unwrap();
                for line in data.lines() {
                    if let Ok(ReportEntry::Start(_)) = serde_json::from_str(line) {
                        found_start = true;
                    }
                }
            }
        }

        assert!(found_start, "expected a Start entry in the report file");
    }

    #[tokio::test]
    async fn test_report_writes_fetched_ops() {
        let dir = tempfile::tempdir().unwrap();
        let signing_key = generate_signing_key();

        let config = HcReportConfig {
            days_retained: 5,
            path: dir.path().into(),
            fetched_op_interval_s: 60 * 60 * 24,
        };

        let report = LinkerReport::create(config, signing_key).unwrap();

        let space_id = SpaceId(Id(bytes::Bytes::from_static(
            b"12345678901234567890123456789012",
        )));

        // Send some fetched op notifications
        report.fetched_op(
            space_id.clone(),
            Url::from_str("ws://localhost:5000").unwrap(),
            bytes::Bytes::from_static(b"op1").into(),
            100,
        );
        report.fetched_op(
            space_id,
            Url::from_str("ws://localhost:5000").unwrap(),
            bytes::Bytes::from_static(b"op2").into(),
            50,
        );

        // Process the reports manually
        report.process_reports();

        // Drop to flush
        drop(report);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Read the report files
        let mut found_ops = false;
        let mut d = tokio::fs::read_dir(dir.path()).await.unwrap();
        while let Ok(Some(e)) = d.next_entry().await {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("hc-report.") && name.ends_with(".jsonl") {
                let data = tokio::fs::read_to_string(e.path()).await.unwrap();
                for line in data.lines() {
                    if let Ok(ReportEntry::FetchedOps(ops)) = serde_json::from_str(line) {
                        assert_eq!(ops.op_count, "2");
                        assert_eq!(ops.total_bytes, "150");
                        assert_eq!(ops.agent_pubkeys.len(), 1);
                        assert_eq!(ops.signatures.len(), 1);
                        found_ops = true;
                    }
                }
            }
        }

        assert!(found_ops, "expected a FetchedOps entry in the report file");
    }

    /// Integration test that exercises the full report pipeline and verifies:
    /// - File naming follows the `hc-report.YYYY-MM-DD.jsonl` pattern
    /// - Raw JSON uses the abbreviated field names the log-collector expects
    /// - Start and FetchedOps entries are present and correctly formatted
    /// - Signatures verify against the agent pubkey using ed25519
    /// - Multiple spaces aggregate independently
    #[tokio::test]
    async fn test_report_full_pipeline_with_raw_json_and_signature_verification() {
        use ed25519_dalek::Verifier;

        let dir = tempfile::tempdir().unwrap();
        let signing_key = generate_signing_key();
        let verifying_key = signing_key.verifying_key();

        let config = HcReportConfig {
            days_retained: 5,
            path: dir.path().into(),
            fetched_op_interval_s: 60 * 60 * 24,
        };

        let report = LinkerReport::create(config, signing_key).unwrap();

        // Two different spaces
        let space_a = SpaceId(Id(bytes::Bytes::from_static(
            b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        )));
        let space_b = SpaceId(Id(bytes::Bytes::from_static(
            b"BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
        )));

        // Send ops for space A (3 ops, total 300 bytes)
        for i in 0..3 {
            report.fetched_op(
                space_a.clone(),
                Url::from_str("ws://localhost:5000").unwrap(),
                bytes::Bytes::from(format!("op-a-{i}").into_bytes()).into(),
                100,
            );
        }

        // Send ops for space B (2 ops, total 500 bytes)
        for i in 0..2 {
            report.fetched_op(
                space_b.clone(),
                Url::from_str("ws://localhost:5001").unwrap(),
                bytes::Bytes::from(format!("op-b-{i}").into_bytes()).into(),
                250,
            );
        }

        // Process reports (aggregates and writes)
        report.process_reports();

        // Drop to flush the non-blocking writer
        drop(report);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Find the report file
        let mut report_file = None;
        let mut d = tokio::fs::read_dir(dir.path()).await.unwrap();
        while let Ok(Some(e)) = d.next_entry().await {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("hc-report.") && name.ends_with(".jsonl") {
                // Verify file naming: hc-report.YYYY-MM-DD.jsonl
                let parts: Vec<&str> = name.split('.').collect();
                assert_eq!(
                    parts.len(),
                    3,
                    "file name should have 3 dot-separated parts"
                );
                assert_eq!(parts[0], "hc-report");
                assert_eq!(parts[2], "jsonl");
                // Date part should be YYYY-MM-DD
                let date_parts: Vec<&str> = parts[1].split('-').collect();
                assert_eq!(
                    date_parts.len(),
                    3,
                    "date should be YYYY-MM-DD, got: {}",
                    parts[1]
                );

                report_file = Some(e.path());
            }
        }

        let report_file = report_file.expect("should have found an hc-report.*.jsonl file");
        let raw_data = tokio::fs::read_to_string(&report_file).await.unwrap();
        let lines: Vec<&str> = raw_data.lines().collect();

        // Should have at least 3 lines: 1 Start + 2 FetchedOps (one per space)
        assert!(
            lines.len() >= 3,
            "expected at least 3 lines (start + 2 spaces), got {}",
            lines.len()
        );

        // -- Verify Start entry raw JSON --
        let start_json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        // Must use abbreviated keys: "k" for kind, "t" for timestamp
        assert_eq!(
            start_json["k"], "start",
            "Start entry must use 'k' field with value 'start'"
        );
        assert!(
            start_json["t"].is_string(),
            "Start entry must have 't' (timestamp) field"
        );
        // Timestamp should be a microsecond value (numeric string)
        let ts: u128 = start_json["t"]
            .as_str()
            .unwrap()
            .parse()
            .expect("timestamp should be a numeric string (microseconds)");
        assert!(ts > 0, "timestamp should be positive");

        // -- Verify FetchedOps entries raw JSON and signatures --
        let mut found_space_a = false;
        let mut found_space_b = false;

        for line in &lines[1..] {
            let json: serde_json::Value = serde_json::from_str(line).unwrap();

            // Skip non-fetchedOps entries
            if json["k"] != "fetchedOps" {
                continue;
            }

            // Verify abbreviated field names match holochain's format
            assert_eq!(
                json["k"], "fetchedOps",
                "FetchedOps entry must use 'k' = 'fetchedOps'"
            );
            assert!(json["t"].is_string(), "must have 't' (timestamp)");
            assert!(json["d"].is_string(), "must have 'd' (dna/space)");
            assert!(json["c"].is_string(), "must have 'c' (count)");
            assert!(json["b"].is_string(), "must have 'b' (bytes)");
            assert!(json["a"].is_array(), "must have 'a' (agent_pubkeys)");
            assert!(json["s"].is_array(), "must have 's' (signatures)");

            // Parse as the typed struct to verify round-trip
            let entry: ReportEntry = serde_json::from_str(line).unwrap();
            let ops = match entry {
                ReportEntry::FetchedOps(ops) => ops,
                _ => panic!("expected FetchedOps"),
            };

            // Verify signature using ed25519_dalek directly
            let to_verify = ops.encode_for_verification();
            assert_eq!(ops.signatures.len(), 1, "should have exactly 1 signature");
            let sig_bytes = base64::prelude::BASE64_URL_SAFE_NO_PAD
                .decode(&ops.signatures[0])
                .unwrap();
            assert_eq!(sig_bytes.len(), 64, "ed25519 signature should be 64 bytes");
            let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes.try_into().unwrap());
            verifying_key
                .verify(&to_verify, &signature)
                .expect("signature verification should succeed");

            // Verify agent pubkey format: "u" prefix + base64url of 39-byte AgentPubKey
            assert_eq!(ops.agent_pubkeys.len(), 1);
            let agent_str = &ops.agent_pubkeys[0];
            assert!(
                agent_str.starts_with('u'),
                "agent pubkey should start with 'u' prefix"
            );
            let agent_bytes = base64::prelude::BASE64_URL_SAFE_NO_PAD
                .decode(agent_str.trim_start_matches('u'))
                .unwrap();
            assert_eq!(
                agent_bytes.len(),
                39,
                "decoded agent pubkey should be 39 bytes (32 key + 3 prefix + 4 location)"
            );

            // Check per-space aggregation
            let count: u64 = ops.op_count.parse().unwrap();
            let bytes: u64 = ops.total_bytes.parse().unwrap();

            if count == 3 && bytes == 300 {
                found_space_a = true;
            } else if count == 2 && bytes == 500 {
                found_space_b = true;
            }
        }

        assert!(
            found_space_a,
            "should find aggregated data for space A (3 ops, 300 bytes)"
        );
        assert!(
            found_space_b,
            "should find aggregated data for space B (2 ops, 500 bytes)"
        );
    }
}
