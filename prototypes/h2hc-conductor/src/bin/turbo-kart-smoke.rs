//! Isolated Turbo Kart ↔ mobile Holochain integration smoke test.
//!
//! This is NOT an Android app or a production keystore. It deliberately leaves
//! h2hc-phone and the existing benchmark untouched. Run with a fresh data dir.
//!
//! Protocol: one JSON command per stdin line, one JSON result per stdout line.
//! Commands: {"cmd":"install"}, {"cmd":"list_open_lobbies","limit":10},
//!           {"cmd":"get_lobby","id":"uhCkk..."},
//!           {"cmd":"join_lobby","id":"uhCkk...","character_id":"zippy"},
//!           {"cmd":"leave_lobby","id":"uhCkk..."}, {"cmd":"quit"}.

use anyhow::{ensure, Context, Result};
use base64::Engine;
use h2hc_conductor::{build_phone_conductor, Connector, H2hcOptions};
use h2hc_p2p::ws::WsCounters;
use h2hc_protocol::LinkerId;
use holochain::conductor::config::{ConductorConfig, KeystoreConfig, WasmBackend};
use holochain::conductor::ConductorHandle;
use holochain::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};

const DIRECTORY_APP_ID: &str = "gaming_platform";
const KART_APP_ID: &str = "turbo_kart";
const KART_ZOME: &str = "turbo_kart";

#[derive(Deserialize)]
struct ExpectedHashes {
    gaming_directory: String,
    turbo_kart: String,
}

struct Args {
    linker_url: String,
    expected_linker: Option<LinkerId>,
    directory_happ: PathBuf,
    kart_happ: PathBuf,
    expected_hashes: PathBuf,
    data_dir: PathBuf,
}

fn args() -> Result<Args> {
    let mut values = HashMap::new();
    let mut iter = std::env::args().skip(1);
    while let Some(key) = iter.next() {
        ensure!(key.starts_with("--"), "expected --option, got {key}");
        let value = iter.next().with_context(|| format!("{key} requires a value"))?;
        ensure!(values.insert(key, value).is_none(), "duplicate argument");
    }
    let required = |key: &str| -> Result<String> {
        values
            .get(key)
            .cloned()
            .with_context(|| format!("missing required --{key}"))
    };
    let expected_linker = values
        .get("--linker-key")
        .map(|encoded| -> Result<LinkerId> {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .context("invalid --linker-key base64")?;
            Ok(LinkerId(
                bytes.try_into().map_err(|_| anyhow::anyhow!("linker key must be 32 bytes"))?,
            ))
        })
        .transpose()?;
    Ok(Args {
        linker_url: required("--linker-url")?,
        expected_linker,
        directory_happ: required("--directory-happ")?.into(),
        kart_happ: required("--kart-happ")?.into(),
        expected_hashes: required("--expected-hashes")?.into(),
        data_dir: required("--data-dir")?.into(),
    })
}

struct Cells {
    agent: AgentPubKey,
    kart: CellId,
}

struct Smoke {
    conductor: ConductorHandle,
    args: Args,
    cells: Option<Cells>,
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Command {
    Install,
    ListOpenLobbies { limit: u32 },
    GetLobby { id: String },
    JoinLobby { id: String, character_id: String },
    LeaveLobby { id: String },
    Quit,
}

impl Smoke {
    async fn install(&mut self) -> Result<Value> {
        ensure!(self.cells.is_none(), "already installed in this session");
        let expected: ExpectedHashes = serde_json::from_slice(
            &std::fs::read(&self.args.expected_hashes).context("read expected DNA hashes")?,
        )?;

        // Start with the platform hApp. Its generated agent MUST be reused
        // by the Turbo Kart hApp (two separate apps/cells; one player).
        let directory = self
            .conductor
            .clone()
            .install_app_bundle(InstallAppPayload {
                source: AppBundleSource::Bytes(
                    std::fs::read(&self.args.directory_happ)
                        .context("read gaming_platform.happ")?
                        .into(),
                ),
                agent_key: None,
                installed_app_id: Some(DIRECTORY_APP_ID.into()),
                network_seed: None,
                roles_settings: None,
                ignore_genesis_failure: false,
                restore_from_dht: false,
            })
            .await
            .context("install gaming_platform")?;

        let agent = directory.agent_key().clone();
        let kart = self
            .conductor
            .clone()
            .install_app_bundle(InstallAppPayload {
                source: AppBundleSource::Bytes(
                    std::fs::read(&self.args.kart_happ)
                        .context("read turbo_kart.happ")?
                        .into(),
                ),
                agent_key: Some(agent.clone()),
                installed_app_id: Some(KART_APP_ID.into()),
                network_seed: None,
                roles_settings: None,
                ignore_genesis_failure: false,
                restore_from_dht: false,
            })
            .await
            .context("install turbo_kart")?;

        ensure!(kart.agent_key() == &agent, "hApps have different agent keys");
        let mut directory_cells = directory.all_cells().map(|cell| cell.clone());
        let directory_cell = directory_cells.next().context("platform app has no cell")?;
        ensure!(directory_cells.next().is_none(), "platform app has multiple cells");
        let mut kart_cells = kart.all_cells().map(|cell| cell.clone());
        let kart_cell = kart_cells.next().context("Turbo Kart app has no cell")?;
        ensure!(kart_cells.next().is_none(), "Turbo Kart app has multiple cells");

        // Reject accidental DNA changes before enabling either application.
        let directory_hash = DnaHashB64::from(directory_cell.dna_hash().clone()).to_string();
        let kart_hash = DnaHashB64::from(kart_cell.dna_hash().clone()).to_string();
        ensure!(
            directory_hash == expected.gaming_directory,
            "gaming_directory DNA mismatch: got {directory_hash}, expected {}",
            expected.gaming_directory
        );
        ensure!(
            kart_hash == expected.turbo_kart,
            "turbo_kart DNA mismatch: got {kart_hash}, expected {}",
            expected.turbo_kart
        );
        ensure!(
            directory_cell.agent_pubkey() == kart_cell.agent_pubkey(),
            "Turbo Kart cells do not share an agent"
        );

        self.conductor
            .clone()
            .enable_app(DIRECTORY_APP_ID.into())
            .await
            .context("enable gaming_platform")?;
        self.conductor
            .clone()
            .enable_app(KART_APP_ID.into())
            .await
            .context("enable turbo_kart")?;

        self.cells = Some(Cells {
            agent: agent.clone(),
            kart: kart_cell,
        });
        Ok(json!({
            "agent": AgentPubKeyB64::from(agent).to_string(),
            "gaming_directory_dna": directory_hash,
            "turbo_kart_dna": kart_hash,
            "both_enabled": true
        }))
    }

    async fn handle(&mut self, cmd: Command) -> Result<Value> {
        match cmd {
            Command::Install => self.install().await,
            Command::ListOpenLobbies { limit } => {
                ensure!(limit <= 50, "limit must not exceed 50");
                let cells = self.cells.as_ref().context("install hApps first")?;
                let lobbies: Value = self
                    .conductor
                    .easy_call_zome(
                        &cells.agent,
                        None,
                        cells.kart.clone(),
                        KART_ZOME,
                        "list_open_lobbies",
                        limit,
                    )
                    .await
                    .context("Turbo Kart list_open_lobbies")?;
                Ok(json!({"lobbies": lobbies}))
            }
            Command::GetLobby { id } => {
                ensure!(id.len() <= 128, "lobby ID too long");
                let cells = self.cells.as_ref().context("install hApps first")?;
                let lobby: Value = self
                    .conductor
                    .easy_call_zome(
                        &cells.agent,
                        None,
                        cells.kart.clone(),
                        KART_ZOME,
                        "get_lobby",
                        id,
                    )
                    .await
                    .context("Turbo Kart get_lobby")?;
                Ok(json!({"lobby": lobby}))
            }
            Command::JoinLobby { id, character_id } => {
                ensure!(id.len() <= 128, "lobby ID too long");
                ensure!(
                    matches!(
                        character_id.as_str(),
                        "blaze" | "zippy" | "bella" | "toadly" |
                        "rex" | "grumbo" | "koopz" | "dotty"
                    ),
                    "unknown Turbo Kart character"
                );
                let cells = self.cells.as_ref().context("install hApps first")?;
                // Only public lobbies are supported by this smoke command.
                // The zome's JoinLobbyInput needs an explicit invite_code field.
                // The zome returns a Holochain ActionHash encoded as MessagePack
                // bytes, not a JSON value. Decode it as ActionHash first.
                let action: ActionHash = self
                    .conductor
                    .easy_call_zome(
                        &cells.agent,
                        None,
                        cells.kart.clone(),
                        KART_ZOME,
                        "join_lobby",
                        json!({
                            "lobby": id,
                            "character_id": character_id,
                            "invite_code": null
                        }),
                    )
                    .await
                    .context("Turbo Kart join_lobby")?;
                Ok(json!({"action": ActionHashB64::from(action).to_string()}))
            }
            Command::LeaveLobby { id } => {
                ensure!(id.len() <= 128, "lobby ID too long");
                let cells = self.cells.as_ref().context("install hApps first")?;
                let action: ActionHash = self
                    .conductor
                    .easy_call_zome(
                        &cells.agent,
                        None,
                        cells.kart.clone(),
                        KART_ZOME,
                        "leave_lobby",
                        id,
                    )
                    .await
                    .context("Turbo Kart leave_lobby")?;
                Ok(json!({"action": ActionHashB64::from(action).to_string()}))
            }
            Command::Quit => std::process::exit(0),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = args()?;
    std::fs::create_dir_all(&args.data_dir)?;

    // WARNING: test-only keystore. Agent keys are NOT persisted. Use a new
    // data directory each run; do not connect this build to production data.
    let config = ConductorConfig {
        data_root_path: Some(args.data_dir.clone().into()),
        keystore: KeystoreConfig::DangerTestKeystore,
        wasm_backend: Some(WasmBackend::Wasmi),
        ..Default::default()
    };
    let passphrase = Arc::new(Mutex::new(sodoken::LockedArray::from(
        b"turbo-kart-smoke-test-only".to_vec(),
    )));
    let linker_url = args.linker_url.clone();
    let counters = Arc::new(WsCounters::default());
    let connector: Connector = Arc::new(move || {
        let url = linker_url.clone();
        let counters = counters.clone();
        Box::pin(async move { h2hc_p2p::ws::connect(&url, counters).await })
    });
    let conductor = build_phone_conductor(
        config,
        passphrase,
        connector,
        H2hcOptions {
            expected_linker: args.expected_linker.clone(),
            ..Default::default()
        },
    )
    .await
    .context("build H2HC conductor")?;

    let mut smoke = Smoke {
        conductor,
        args,
        cells: None,
    };
    println!("{}", json!({"event":"ready","network":"h2hc","wasm":"wasmi","test_only":true}));

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let result = match serde_json::from_str::<Command>(&line) {
            Ok(command) => smoke.handle(command).await,
            Err(error) => Err(error.into()),
        };
        let response = match result {
            Ok(value) => json!({"ok":true,"result":value}),
            Err(error) => json!({"ok":false,"error":format!("{error:#}")}),
        };
        println!("{response}");
    }
    Ok(())
}
