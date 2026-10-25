//! Android JNI host. A proof-of-concept, not the completed Turbo Kart game.
//! Default CLI benchmarks and the successful Android adb-shell test are separate.

use anyhow::{bail, ensure, Context, Result};
use base64::Engine as _;
use h2hc_conductor::{build_phone_conductor, Connector, H2hcOptions};
use h2hc_p2p::ws::WsCounters;
use h2hc_protocol::LinkerId;
use holochain::conductor::config::{ConductorConfig, KeystoreConfig, WasmBackend};
use holochain::conductor::ConductorHandle;
use holochain::prelude::*;
use holochain_conductor_api::{AppInfo, CellInfo};
use jni::objects::{JByteArray, JObject, JString};
use jni::sys::{jint, jstring};
use jni::JNIEnv;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

const DIRECTORY_APP: &str = "gaming_platform";
const KART_APP: &str = "turbo_kart";
const KART_ZOME: &str = "turbo_kart";
static HOST: OnceLock<Mutex<Option<Host>>> = OnceLock::new();

struct Host {
    runtime: tokio::runtime::Runtime,
    conductor: ConductorHandle,
    agent: AgentPubKey,
    kart: CellId,
}

#[derive(Deserialize)]
struct ExpectedHashes {
    gaming_directory: String,
    turbo_kart: String,
}

fn slot() -> &'static Mutex<Option<Host>> {
    HOST.get_or_init(|| Mutex::new(None))
}

fn linker_id(key: &str) -> Result<LinkerId> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(key).context("linker key must be padded standard base64")?;
    Ok(LinkerId(decoded.try_into().map_err(
        |_| anyhow::anyhow!("linker key must be 32 bytes")
    )?))
}

fn saved_cell(app: &AppInfo) -> Result<CellId> {
    let cells: Vec<_> = app.cell_info.values().flatten().filter_map(|c| {
        if let CellInfo::Provisioned(p) = c { Some(p.cell_id.clone()) }
        else { None }
    }).collect();
    ensure!(cells.len() == 1, "expected exactly one provisioned cell per app");
    Ok(cells.into_iter().next().expect("checked one cell"))
}

async fn install_or_resume(
    conductor: &ConductorHandle,
    dir_happ: &Path,
    kart_happ: &Path,
    hashes: &Path,
) -> Result<(AgentPubKey, CellId, Value)> {
    let expected: ExpectedHashes = serde_json::from_slice(
        &std::fs::read(hashes).context("read expected DNA hashes")?
    )?;
    let dir_saved = conductor.get_app_info(&DIRECTORY_APP.into()).await?;
    let kart_saved = conductor.get_app_info(&KART_APP.into()).await?;

    let (agent, directory_cell, kart_cell, resumed) = match (dir_saved, kart_saved) {
        (None, None) => {
            let directory = conductor.clone().install_app_bundle(InstallAppPayload {
                source: AppBundleSource::Bytes(
                    std::fs::read(dir_happ).context("read gaming_platform.happ")?.into()
                ),
                agent_key: None,
                installed_app_id: Some(DIRECTORY_APP.into()),
                network_seed: None,
                roles_settings: None,
                ignore_genesis_failure: false,
                restore_from_dht: false,
            }).await.context("install gaming_platform")?;
            let agent = directory.agent_key().clone();
            let kart = conductor.clone().install_app_bundle(InstallAppPayload {
                source: AppBundleSource::Bytes(
                    std::fs::read(kart_happ).context("read turbo_kart.happ")?.into()
                ),
                agent_key: Some(agent.clone()),
                installed_app_id: Some(KART_APP.into()),
                network_seed: None,
                roles_settings: None,
                ignore_genesis_failure: false,
                restore_from_dht: false,
            }).await.context("install turbo_kart")?;
            ensure!(kart.agent_key() == &agent, "two installed apps have different agents");
            let dc: Vec<_> = directory.all_cells().cloned().collect();
            let kc: Vec<_> = kart.all_cells().cloned().collect();
            ensure!(dc.len() == 1 && kc.len() == 1, "expected one cell per app");
            (agent, dc[0].clone(), kc[0].clone(), false)
        },
        (Some(dir), Some(kart)) => {
            ensure!(dir.agent_pub_key == kart.agent_pub_key, "saved app agents differ");
            let dc = saved_cell(&dir)?;
            let kc = saved_cell(&kart)?;
            (dir.agent_pub_key, dc, kc, true)
        },
        _ => bail!("partial hApp installation; preserve app data and inspect before retrying"),
    };
    ensure!(directory_cell.agent_pubkey() == kart_cell.agent_pubkey()
        && directory_cell.agent_pubkey() == &agent, "cell agents differ");
    let dh = DnaHashB64::from(directory_cell.dna_hash().clone()).to_string();
    let kh = DnaHashB64::from(kart_cell.dna_hash().clone()).to_string();
    ensure!(dh == expected.gaming_directory, "gaming directory DNA mismatch");
    ensure!(kh == expected.turbo_kart, "Turbo Kart DNA mismatch");

    // Validate agent + production DNAs before enabling either app.
    conductor.clone().enable_app(DIRECTORY_APP.into()).await
        .context("enable gaming_platform")?;
    conductor.clone().enable_app(KART_APP.into()).await
        .context("enable turbo_kart")?;

    Ok((agent.clone(), kart_cell, json!({
        "agent": AgentPubKeyB64::from(agent).to_string(),
        "gaming_directory_dna": dh,
        "turbo_kart_dna": kh,
        "both_enabled": true,
        "resumed": resumed,
    })))
}

#[allow(clippy::too_many_arguments)]
fn start(
    data_dir: PathBuf,
    url: String,
    key: String,
    dir_happ: PathBuf,
    kart_happ: PathBuf,
    hashes: PathBuf,
    passphrase: Vec<u8>,
) -> Result<Value> {
    ensure!((32..=128).contains(&passphrase.len()), "invalid Lair secret length");
    ensure!(url.starts_with("ws://") || url.starts_with("wss://"),
        "linker URL must begin with ws:// or wss://");
    let expected_linker = linker_id(&key)?;
    let mut locked = slot().lock()
        .map_err(|_| anyhow::anyhow!("native host lock poisoned"))?;
    ensure!(locked.is_none(), "conductor already started in this process");
    std::fs::create_dir_all(&data_dir).context("create app-private conductor dir")?;
    // Lair in Android's app-private directory is NOT yet validated on-device.
    let passphrase = Arc::new(Mutex::new(sodoken::LockedArray::from(passphrase)));
    let config = ConductorConfig {
        data_root_path: Some(data_dir.into()),
        keystore: KeystoreConfig::LairServerInProc { lair_root: None },
        wasm_backend: Some(WasmBackend::Wasmi),
        ..Default::default()
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2).enable_all().build().context("create Tokio runtime")?;
    let counters = Arc::new(WsCounters::default());
    let connector: Connector = Arc::new(move || {
        let url = url.clone();
        let counters = counters.clone();
        Box::pin(async move { h2hc_p2p::ws::connect(&url, counters).await })
    });
    let (conductor, agent, kart, summary) = runtime.block_on(async {
        let conductor = build_phone_conductor(config, passphrase, connector, H2hcOptions {
            expected_linker: Some(expected_linker),
            ..Default::default()
        }).await.context("build Android H2HC conductor")?;
        let (agent, kart, summary) =
            install_or_resume(&conductor, &dir_happ, &kart_happ, &hashes).await?;
        Ok::<_, anyhow::Error>((conductor, agent, kart, summary))
    })?;
    *locked = Some(Host { runtime, conductor, agent, kart });
    Ok(summary)
}

fn list_lobbies(limit: u32) -> Result<Value> {
    ensure!(limit <= 50, "limit must not exceed 50");
    let locked = slot().lock()
        .map_err(|_| anyhow::anyhow!("native host lock poisoned"))?;
    let host = locked.as_ref().context("start the conductor first")?;
    let lobbies: Value = host.runtime.block_on(host.conductor.easy_call_zome(
        &host.agent, None, host.kart.clone(), KART_ZOME, "list_open_lobbies", limit
    )).context("list_open_lobbies")?;
    Ok(json!({ "lobbies": lobbies }))
}

fn str_arg(env: &mut JNIEnv<'_>, val: JString<'_>) -> Result<String> {
    Ok(env.get_string(&val)?.into())
}

fn result_as_jstring(env: &mut JNIEnv<'_>, result: Result<Value>) -> jstring {
    let value = match result {
        Ok(result) => json!({"ok":true,"result":result}),
        Err(err) => json!({"ok":false,"error":format!("{err:#}")}),
    };
    env.new_string(value.to_string())
        .map(JString::into_raw).unwrap_or(std::ptr::null_mut())
}

/// Kotlin NativeBridge.start; always invoke on a background thread.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_NativeBridge_start(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    data_dir: JString<'_>,
    linker_url: JString<'_>,
    linker_key: JString<'_>,
    directory_happ: JString<'_>,
    kart_happ: JString<'_>,
    expected_hashes: JString<'_>,
    passphrase: JByteArray<'_>,
) -> jstring {
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        start(
            PathBuf::from(str_arg(&mut env, data_dir)?),
            str_arg(&mut env, linker_url)?,
            str_arg(&mut env, linker_key)?,
            PathBuf::from(str_arg(&mut env, directory_happ)?),
            PathBuf::from(str_arg(&mut env, kart_happ)?),
            PathBuf::from(str_arg(&mut env, expected_hashes)?),
            env.convert_byte_array(&passphrase)?,
        )
    }));
    result_as_jstring(&mut env, match response {
        Ok(v) => v,
        Err(_) => Err(anyhow::anyhow!("native panic during start")),
    })
}

/// Kotlin NativeBridge.listOpenLobbies; read-only network smoke command.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_NativeBridge_listOpenLobbies(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    limit: jint,
) -> jstring {
    let response = std::panic::catch_unwind(|| {
        ensure!(limit >= 0, "limit must be nonnegative");
        list_lobbies(limit as u32)
    });
    result_as_jstring(&mut env, match response {
        Ok(v) => v,
        Err(_) => Err(anyhow::anyhow!("native panic during lobby query")),
    })
}
