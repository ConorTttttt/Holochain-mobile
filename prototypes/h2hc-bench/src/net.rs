//! The local network a phone talks to: a bootstrap and relay server,
//! full-arc authority conductors in this process, and the linker.

use anyhow::Context;
use base64::Engine;
use holochain::conductor::config::{ConductorConfig, KeystoreConfig, NetworkConfig};
use holochain::conductor::{ConductorBuilder, ConductorHandle};
use holochain::prelude::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;

pub const APP_ID: &str = "notes";
const ZOME: &str = "notes";

pub fn free_port() -> anyhow::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

async fn wait_for_port(port: u16, within: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    anyhow::bail!("nothing listening on port {port} after {within:?}")
}

fn log_file(path: &Path) -> anyhow::Result<Stdio> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(Stdio::from(file))
}

/// Plain HTTP/1.0 GET returning the body, to avoid an HTTP client dependency.
pub async fn http_get(port: u16, path: &str) -> anyhow::Result<String> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    stream
        .write_all(format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n").as_bytes())
        .await?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await?;
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    anyhow::ensure!(head.contains(" 200 "), "GET {path}: {}", head.lines().next().unwrap_or(""));
    Ok(body.to_string())
}

pub struct Bootstrap {
    _child: Child,
    pub port: u16,
}

impl Bootstrap {
    pub async fn start(run_dir: &Path) -> anyhow::Result<Self> {
        let port = free_port()?;
        let quic = free_port()?;
        let child = Command::new("kitsune2-bootstrap-srv")
            .args([
                "--listen",
                &format!("127.0.0.1:{port}"),
                "--quic-bind-addr",
                &format!("127.0.0.1:{quic}"),
            ])
            .stdout(log_file(&run_dir.join("bootstrap.log"))?)
            .stderr(log_file(&run_dir.join("bootstrap.log"))?)
            .kill_on_drop(true)
            .spawn()
            .context("starting kitsune2-bootstrap-srv (is it on PATH?)")?;
        wait_for_port(port, Duration::from_secs(15)).await?;
        Ok(Self { _child: child, port })
    }

    /// The server is both the bootstrap service and the iroh relay.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

pub fn network_config(bootstrap_url: &str, target_arc_factor: u32) -> NetworkConfig {
    NetworkConfig {
        bootstrap_url: url2::Url2::parse(bootstrap_url),
        relay_url: url2::Url2::parse(bootstrap_url),
        target_arc_factor,
        advanced: Some(json!({ "irohTransport": { "relayAllowPlainText": true } })),
        ..Default::default()
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
enum Ping {
    Ping { id: u64 },
    Pong { id: u64 },
}

#[derive(serde::Serialize, Debug)]
pub struct EchoInput {
    pub agent: AgentPubKey,
    pub text: String,
}

#[derive(serde::Serialize, Debug)]
pub struct PingInput {
    pub agent: AgentPubKey,
    pub id: u64,
}

/// A full-arc conductor holding the DHT, running in this process.
pub struct Authority {
    pub conductor: ConductorHandle,
    pub agent: AgentPubKey,
    pub cell_id: CellId,
    pongs: Arc<Mutex<HashMap<u64, oneshot::Sender<()>>>>,
}

impl Authority {
    pub async fn start(index: usize, run_dir: &Path, bootstrap_url: &str, happ: &[u8]) -> anyhow::Result<Self> {
        let config = ConductorConfig {
            data_root_path: Some(run_dir.join(format!("authority-{index}")).into()),
            keystore: KeystoreConfig::LairServerInProc { lair_root: None },
            network: network_config(bootstrap_url, 1),
            ..Default::default()
        };
        let passphrase = Arc::new(Mutex::new(sodoken::LockedArray::from(b"authority".to_vec())));
        let conductor = ConductorBuilder::new()
            .config(config)
            .passphrase(Some(passphrase))
            .no_print_setup()
            .build()
            .await?;
        let app = conductor
            .clone()
            .install_app_bundle(InstallAppPayload {
                source: AppBundleSource::Bytes(happ.to_vec().into()),
                agent_key: None,
                installed_app_id: Some(APP_ID.into()),
                network_seed: None,
                roles_settings: None,
                ignore_genesis_failure: false,
                restore_from_dht: false,
            })
            .await?;
        conductor.clone().enable_app(APP_ID.into()).await?;
        let agent = app.agent_key().clone();
        let cell_id = app.all_cells().next().context("app has no cell")?;

        let pongs: Arc<Mutex<HashMap<u64, oneshot::Sender<()>>>> = Default::default();
        let mut signals = conductor.subscribe_to_app_signals(APP_ID.into());
        let pong_map = pongs.clone();
        tokio::spawn(async move {
            while let Ok(signal) = signals.recv().await {
                if let Signal::App { signal, .. } = signal {
                    if let Ok(Ping::Pong { id }) = signal.into_inner().decode::<Ping>() {
                        if let Some(tx) = pong_map.lock().unwrap().remove(&id) {
                            let _ = tx.send(());
                        }
                    }
                }
            }
        });
        Ok(Self {
            conductor,
            agent,
            cell_id,
            pongs,
        })
    }

    pub async fn call<I, O>(&self, fn_name: &str, payload: I) -> anyhow::Result<O>
    where
        I: serde::Serialize + std::fmt::Debug,
        O: serde::de::DeserializeOwned + std::fmt::Debug,
    {
        Ok(self
            .conductor
            .easy_call_zome(&self.agent, None, self.cell_id.clone(), ZOME, fn_name, payload)
            .await?)
    }

    pub fn agent_b64(&self) -> String {
        AgentPubKeyB64::from(self.agent.clone()).to_string()
    }

    pub async fn create_note(&self, text: &str) -> anyhow::Result<ActionHash> {
        self.call("create_note", text.to_string()).await
    }

    pub async fn has_note(&self, hash: &ActionHash) -> anyhow::Result<bool> {
        let record: Option<Record> = self.call("get_note", hash.clone()).await?;
        Ok(record.is_some())
    }

    /// Time until this authority can read `hash`, polling every 25 ms.
    pub async fn wait_for_note(&self, hash: &ActionHash, within: Duration) -> anyhow::Result<f64> {
        let start = Instant::now();
        while start.elapsed() < within {
            if self.has_note(hash).await.unwrap_or(false) {
                return Ok(start.elapsed().as_secs_f64() * 1000.0);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        anyhow::bail!("note not readable after {within:?}")
    }

    /// Signal round trip from this authority to `agent` and back.
    pub async fn ping(&self, agent: &AgentPubKey, id: u64, within: Duration) -> anyhow::Result<f64> {
        let (tx, rx) = oneshot::channel();
        self.pongs.lock().unwrap().insert(id, tx);
        let start = Instant::now();
        let () = self
            .call("ping", PingInput { agent: agent.clone(), id })
            .await?;
        match tokio::time::timeout(within, rx).await {
            Ok(Ok(())) => Ok(start.elapsed().as_secs_f64() * 1000.0),
            _ => {
                self.pongs.lock().unwrap().remove(&id);
                anyhow::bail!("no pong within {within:?}")
            }
        }
    }
}

/// The forked linker, as its own process.
pub struct Linker {
    child: Option<Child>,
    bin: PathBuf,
    run_dir: PathBuf,
    bootstrap_url: String,
    seed_b64: String,
    pub port: u16,
    /// Base64 of the Ed25519 public key it signs receipts with.
    pub key_b64: String,
}

impl Linker {
    pub async fn start(bin: &Path, run_dir: &Path, bootstrap_url: &str) -> anyhow::Result<Self> {
        let key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut linker = Self {
            child: None,
            bin: bin.to_path_buf(),
            run_dir: run_dir.to_path_buf(),
            bootstrap_url: bootstrap_url.to_string(),
            seed_b64: b64.encode(key.to_bytes()),
            port: free_port()?,
            key_b64: b64.encode(key.verifying_key().to_bytes()),
        };
        linker.spawn().await?;
        Ok(linker)
    }

    async fn spawn(&mut self) -> anyhow::Result<()> {
        let dir = &self.run_dir;
        let child = Command::new(&self.bin)
            .args(["--address", "127.0.0.1", "--port", &self.port.to_string()])
            .env("H2HC_LINKER_BOOTSTRAP_URL", &self.bootstrap_url)
            .env("H2HC_LINKER_RELAY_URL", &self.bootstrap_url)
            .env("H2HC_LINKER_PRIVATE_KEY", &self.seed_b64)
            .env("H2HC_LINKER_KEY_FILE", dir.join("linker.key"))
            .env("H2HC_LINKER_DATA_DIR", dir.join("linker-data"))
            .env("H2HC_LINKER_REPORT_PATH", dir.join("linker-reports"))
            .env("RUST_LOG", std::env::var("LINKER_LOG").unwrap_or_else(|_| "warn".into()))
            .stdout(log_file(&dir.join("linker.log"))?)
            .stderr(log_file(&dir.join("linker.log"))?)
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting {}", self.bin.display()))?;
        self.child = Some(child);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if http_get(self.port, "/health").await.is_ok() {
                return Ok(());
            }
            anyhow::ensure!(Instant::now() < deadline, "linker did not become healthy");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Kill without warning, as a crash would.
    pub async fn kill(&mut self) -> anyhow::Result<()> {
        if let Some(mut child) = self.child.take() {
            child.kill().await?;
        }
        Ok(())
    }

    /// Start again on the same port, key and data.
    pub async fn restart(&mut self) -> anyhow::Result<()> {
        self.kill().await?;
        self.spawn().await
    }

    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}/conductor/v1", self.port)
    }

    pub async fn status(&self) -> anyhow::Result<Value> {
        Ok(serde_json::from_str(&http_get(self.port, "/conductor/status").await?)?)
    }
}
