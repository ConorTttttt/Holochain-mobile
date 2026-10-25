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
use std::time::{SystemTime, UNIX_EPOCH};

const DIRECTORY_APP: &str = "gaming_platform";
const KART_APP: &str = "turbo_kart";
const KART_ZOME: &str = "turbo_kart";
const DIRECTORY_ZOME: &str = "moq_directory";
static HOST: OnceLock<Mutex<Option<Host>>> = OnceLock::new();

struct Host {
    runtime: tokio::runtime::Runtime,
    conductor: ConductorHandle,
    agent: AgentPubKey,
    kart: CellId,
    directory: CellId,
    summary: Value,
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
) -> Result<(AgentPubKey, CellId, CellId, Value)> {
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
            let dc: Vec<_> = directory.all_cells().collect();
            let kc: Vec<_> = kart.all_cells().collect();
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

    Ok((agent.clone(), kart_cell, directory_cell, json!({
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
    // An Android Activity can be recreated (e.g. rotation) while this native
    // library stays loaded. Reuse the existing host instead of reinstalling.
    if let Some(host) = locked.as_ref() {
        let mut summary = host.summary.clone();
        summary["session_reused"] = json!(true);
        return Ok(summary);
    }
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
    let (conductor, agent, kart, directory, summary) = runtime.block_on(async {
        let conductor = build_phone_conductor(config, passphrase, connector, H2hcOptions {
            expected_linker: Some(expected_linker),
            ..Default::default()
        }).await.context("build Android H2HC conductor")?;
        let (agent, kart, directory, summary) =
            install_or_resume(&conductor, &dir_happ, &kart_happ, &hashes).await?;
        Ok::<_, anyhow::Error>((conductor, agent, kart, directory, summary))
    })?;
    *locked = Some(Host {
        runtime,
        conductor,
        agent,
        kart,
        directory,
        summary: summary.clone(),
    });
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

fn get_lobby(lobby_id: String) -> Result<Value> {
    ensure!(!lobby_id.is_empty() && lobby_id.len() <= 128, "invalid lobby ID length");
    let locked = slot().lock()
        .map_err(|_| anyhow::anyhow!("native host lock poisoned"))?;
    let host = locked.as_ref().context("start the conductor first")?;
    let lobby: Value = host.runtime.block_on(host.conductor.easy_call_zome(
        &host.agent, None, host.kart.clone(), KART_ZOME, "get_lobby", lobby_id
    )).context("get_lobby")?;
    Ok(json!({ "lobby": lobby }))
}

/// Read the original immutable E6.1 RaceRecord after the desktop host
/// starts a race. This is a diagnostic only: it does NOT authorize a peer
/// session. The separate realtime JNI must validate the frozen roster with
/// RaceAdmission::from_locked and signed directory bindings before dialling.
fn get_locked_race(lobby_id: String) -> Result<Value> {
    ensure!(!lobby_id.is_empty() && lobby_id.len() <= 128, "invalid lobby ID length");
    let locked = slot().lock()
        .map_err(|_| anyhow::anyhow!("native host lock poisoned"))?;
    let host = locked.as_ref().context("start the conductor first")?;
    let record: Option<Value> = host.runtime.block_on(host.conductor.easy_call_zome(
        &host.agent, None, host.kart.clone(), KART_ZOME, "get_race", lobby_id.clone()
    )).context("original turbo_kart.get_race")?;
    let own_agent = AgentPubKeyB64::from(host.agent.clone()).to_string();
    let Some(record) = record else {
        return Ok(json!({
            "lobby_id": lobby_id,
            "locked_race_exists": false,
            "local_agent": own_agent,
            "authenticated_race_session": false,
            "next": "Desktop host must start a race after both players publish signed Iroh presence"
        }));
    };
    let setup = record.get("setup").context("locked race has no setup")?;
    ensure!(setup.get("lobby").and_then(Value::as_str) == Some(lobby_id.as_str()),
        "locked race refers to a different lobby");
    let members = setup.get("participants").and_then(Value::as_array)
        .context("locked race has no participant array")?;
    ensure!(!members.is_empty() && members.len() <= 8,
        "unexpected locked-race participant count");
    let agents = members.iter().map(|p| {
        p.get("member").and_then(|m| m.get("player")).and_then(Value::as_str)
            .context("locked race participant has no agent")
    }).collect::<Result<Vec<_>>>()?;
    let included = agents.contains(&own_agent.as_str());
    Ok(json!({
        "lobby_id": lobby_id,
        "locked_race_exists": true,
        "race_id": setup.get("race_id"),
        "host_agent": setup.get("config").and_then(|c| c.get("host")),
        "local_agent": own_agent,
        "local_agent_in_locked_roster": included,
        "roster_agents": agents,
        "roster_size": members.len(),
        "authenticated_race_session": false,
        "next": "Validate signed participant bindings using desktop RaceAdmission::from_locked in realtime JNI before connecting"
    }))
}

/// Fetch the authoritative locked race and each participant's exact
/// signed endpoint binding using the original Turbo Kart + gaming directory
/// zomes. This returns an UNTRUSTED envelope for independent verification
/// with the desktop's RaceAdmission::from_locked in the realtime JNI module.
/// This function never starts a race and never changes a player identity.
fn locked_admission_bundle(lobby_id: String) -> Result<Value> {
    ensure!(!lobby_id.is_empty() && lobby_id.len() <= 128, "invalid lobby ID length");
    let locked = slot().lock()
        .map_err(|_| anyhow::anyhow!("native host lock poisoned"))?;
    let host = locked.as_ref().context("start the conductor first")?;
    let record: Option<Value> = host.runtime.block_on(host.conductor.easy_call_zome(
        &host.agent, None, host.kart.clone(), KART_ZOME, "get_race", lobby_id.clone()
    )).context("original turbo_kart.get_race")?;
    let record = record.context("desktop host has not locked a race for this lobby")?;
    let setup = record.get("setup").context("race record missing setup")?;
    ensure!(setup.get("lobby").and_then(Value::as_str) == Some(lobby_id.as_str()),
        "locked race refers to a different lobby");
    let participants = setup.get("participants").and_then(Value::as_array)
        .context("locked race has no participants")?;
    ensure!((1..=8).contains(&participants.len()),
        "invalid locked race participant count");
    let mut bindings = Vec::with_capacity(participants.len());
    for participant in participants {
        let binding = participant.get("binding").and_then(Value::as_str)
            .context("locked race participant missing binding action")?;
        // holo_hash's parser can panic on malformed text: screen the canonical
        // ActionHashB64 shape before passing it to the actual hash parser.
        ensure!(binding.len() == 53 && binding.starts_with("uhCkk")
            && binding.is_ascii() && binding.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
            "race contains malformed signed binding action");
        let action = ActionHash::try_from(binding).context("parse race binding action hash")?;
        ensure!(action.to_string() == binding, "noncanonical race binding action hash");
        let presence: Option<Value> = host.runtime.block_on(host.conductor.easy_call_zome(
            &host.agent, None, host.directory.clone(), DIRECTORY_ZOME,
            "get_endpoint_binding", action
        )).context("original moq_directory.get_endpoint_binding for race")?;
        bindings.push(presence.context("locked race references a missing signed presence action")?);
    }
    Ok(json!({
        "record": record,
        "signed_bindings": bindings,
        "local_agent": AgentPubKeyB64::from(host.agent.clone()).to_string(),
    }))
}

fn join_lobby(lobby_id: String, character_id: String) -> Result<Value> {
    ensure!(!lobby_id.is_empty() && lobby_id.len() <= 128, "invalid lobby ID length");
    ensure!(
        matches!(
            character_id.as_str(),
            "blaze" | "zippy" | "bella" | "toadly" |
            "rex" | "grumbo" | "koopz" | "dotty"
        ),
        "unknown Turbo Kart character"
    );
    let locked = slot().lock()
        .map_err(|_| anyhow::anyhow!("native host lock poisoned"))?;
    let host = locked.as_ref().context("start the conductor first")?;
    // Public lobbies only; the unmodified game's zome expects invite_code.
    // The zome returns binary-encoded ActionHash, not serde_json::Value.
    let action: ActionHash = host.runtime.block_on(host.conductor.easy_call_zome(
        &host.agent,
        None,
        host.kart.clone(),
        KART_ZOME,
        "join_lobby",
        json!({"lobby": lobby_id, "character_id": character_id, "invite_code": null}),
    )).context("join_lobby")?;
    Ok(json!({ "action": ActionHashB64::from(action).to_string() }))
}

fn leave_lobby(lobby_id: String) -> Result<Value> {
    ensure!(!lobby_id.is_empty() && lobby_id.len() <= 128, "invalid lobby ID length");
    let locked = slot().lock()
        .map_err(|_| anyhow::anyhow!("native host lock poisoned"))?;
    let host = locked.as_ref().context("start the conductor first")?;
    let action: ActionHash = host.runtime.block_on(host.conductor.easy_call_zome(
        &host.agent, None, host.kart.clone(), KART_ZOME, "leave_lobby", lobby_id
    )).context("leave_lobby")?;
    Ok(json!({ "action": ActionHashB64::from(action).to_string() }))
}

/// Submit the original SignedPresence record through the *original*
/// gaming-directory hApp using the existing persistent Holochain agent.
/// The separate Iroh JNI module already signed the EndpointId binding.
fn publish_signed_presence(signed_json: String) -> Result<Value> {
    ensure!(signed_json.len() <= 8192, "presence JSON exceeds 8 KiB");
    let signed: Value = serde_json::from_str(&signed_json)
        .context("decode Iroh-generated SignedPresence")?;
    let ad = signed.get("advertisement").context("missing advertisement")?;
    let peer = ad.get("peer_endpoint").context("missing peer_endpoint")?;
    let claimed_agent = peer.get("agent").and_then(Value::as_str)
        .context("missing advertised Holochain agent")?;
    let endpoint = peer.get("endpoint").context("missing endpoint")?;
    ensure!(endpoint.get("kind").and_then(Value::as_str) == Some("Iroh"),
        "advertisement is not for an Iroh endpoint");
    let address = endpoint.get("address").and_then(Value::as_str)
        .context("missing Iroh address")?;
    ensure!(!address.is_empty() && address.len() <= 2048
        && address.split_whitespace().all(|tok|
            tok.starts_with("ip:") || tok.starts_with("relay:")),
        "invalid Iroh dial hints");
    let fingerprint = endpoint.get("transport_identity")
        .and_then(|v| v.get("fingerprint"))
        .and_then(Value::as_array)
        .context("missing EndpointId fingerprint")?;
    ensure!(fingerprint.len() == 32
        && fingerprint.iter().all(|b| b.as_u64().is_some_and(|n| n <= 255)),
        "Iroh EndpointId must contain 32 public bytes");
    let sig = signed.get("binding_signature").context("missing signed binding")?;
    for component in ["r", "s"] {
        let b = sig.get(component).and_then(Value::as_array)
            .context("malformed Ed25519 binding signature")?;
        ensure!(b.len() == 32
            && b.iter().all(|v| v.as_u64().is_some_and(|n| n <= 255)),
            "binding signature needs two 32-byte components");
    }
    let published = ad.get("published_at").and_then(Value::as_i64)
        .context("missing published_at")?;
    let expires = ad.get("expires_at").and_then(Value::as_i64)
        .context("signed presence must have expiry")?;
    ensure!(expires.checked_sub(published) == Some(120_000),
        "signed presence TTL must match the desktop's 120 seconds");
    let now: i64 = SystemTime::now()
        .duration_since(UNIX_EPOCH).context("Android clock before epoch")?
        .as_millis().try_into().context("Android clock overflow")?;
    ensure!(published >= now - 60_000 && published <= now + 30_000
        && expires > now + 30_000,
        "signed presence is stale or future-dated");

    let locked = slot().lock().map_err(|_| anyhow::anyhow!("native host lock poisoned"))?;
    let host = locked.as_ref().context("start Holochain before publishing presence")?;
    let own_agent = AgentPubKeyB64::from(host.agent.clone()).to_string();
    ensure!(claimed_agent == own_agent,
        "refusing signed presence authored for another Holochain agent");

    let action: ActionHash = host.runtime.block_on(host.conductor.easy_call_zome(
        &host.agent, None, host.directory.clone(), DIRECTORY_ZOME,
        "publish_signed_presence", signed.clone()
    )).context("original moq_directory.publish_signed_presence")?;

    // A successful write alone is not sufficient: verify the exact action
    // round-trips via the production directory's read API on this conductor.
    let saved: Option<Value> = host.runtime.block_on(host.conductor.easy_call_zome(
        &host.agent, None, host.directory.clone(), DIRECTORY_ZOME,
        "get_endpoint_binding", action.clone()
    )).context("original moq_directory.get_endpoint_binding")?;
    ensure!(saved.as_ref() == Some(&signed),
        "published signature/endpoint binding failed exact readback");

    let id_hex = fingerprint.iter()
        .map(|b| format!("{:02x}", b.as_u64().unwrap_or(0)))
        .collect::<String>();
    Ok(json!({
        "agent": own_agent,
        "binding_action": ActionHashB64::from(action).to_string(),
        "endpoint_fingerprint_hex": id_hex,
        "published_at_ms": published,
        "expires_at_ms": expires,
        "signed_presence_published": true,
        "readback_verified": true,
        "authenticated_race_session": false
    }))
}

/// Read the caller's latest directory bindings, without expanding signatures
/// or authorising any race. The original zome bounds this read to four.
fn recent_signed_bindings() -> Result<Value> {
    let locked = slot().lock().map_err(|_| anyhow::anyhow!("native host lock poisoned"))?;
    let host = locked.as_ref().context("start Holochain first")?;
    let values: Vec<(String, Value)> = host.runtime.block_on(
        host.conductor.easy_call_zome(
            &host.agent, None, host.directory.clone(), DIRECTORY_ZOME,
            "get_recent_endpoint_bindings", host.agent.clone()
        )
    ).context("moq_directory.get_recent_endpoint_bindings")?;
    ensure!(values.len() <= 4, "directory returned too many bindings");
    let agent = AgentPubKeyB64::from(host.agent.clone()).to_string();
    let recent = values.into_iter().filter_map(|(action, signed)| {
        let ad = signed.get("advertisement")?;
        let peer = ad.get("peer_endpoint")?;
        if peer.get("agent")?.as_str()? != agent { return None; }
        let endpoint = peer.get("endpoint")?;
        Some(json!({
            "binding_action": action,
            "kind": endpoint.get("kind"),
            "transport_identity": endpoint.get("transport_identity"),
            "address": endpoint.get("address"),
            "published_at_ms": ad.get("published_at"),
            "expires_at_ms": ad.get("expires_at")
        }))
    }).collect::<Vec<_>>();
    Ok(json!({"agent": agent, "recent_bindings": recent}))
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

/// Read the selected desktop lobby and its current membership.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_NativeBridge_getLobby(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    lobby_id: JString<'_>,
) -> jstring {
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        get_lobby(str_arg(&mut env, lobby_id)?)
    }));
    result_as_jstring(&mut env, match response {
        Ok(v) => v,
        Err(_) => Err(anyhow::anyhow!("native panic during get_lobby")),
    })
}

/// Inspect the desktop's committed E6.1 race setup, without joining
/// the race or treating the returned roster as authenticated admission.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_NativeBridge_getLockedRace(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    lobby_id: JString<'_>,
) -> jstring {
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        get_locked_race(str_arg(&mut env, lobby_id)?)
    }));
    result_as_jstring(&mut env, match response {
        Ok(v) => v,
        Err(_) => Err(anyhow::anyhow!("native panic during get_race")),
    })
}

/// Hand the realtime JNI library the untrusted locked race + exact signed
/// directory records so the original desktop validation can run unchanged.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_NativeBridge_lockedAdmissionBundle(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    lobby_id: JString<'_>,
) -> jstring {
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        locked_admission_bundle(str_arg(&mut env, lobby_id)?)
    }));
    result_as_jstring(&mut env, match response {
        Ok(v) => v,
        Err(_) => Err(anyhow::anyhow!("native panic reading locked race directory bindings")),
    })
}

/// Join a public lobby using the same Turbo Kart zome as the desktop.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_NativeBridge_joinLobby(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    lobby_id: JString<'_>,
    character_id: JString<'_>,
) -> jstring {
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let lobby_id = str_arg(&mut env, lobby_id)?;
        let character_id = str_arg(&mut env, character_id)?;
        join_lobby(lobby_id, character_id)
    }));
    result_as_jstring(&mut env, match response {
        Ok(v) => v,
        Err(_) => Err(anyhow::anyhow!("native panic during join_lobby")),
    })
}

/// Leave a previously joined public lobby and return the committed action hash.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_NativeBridge_leaveLobby(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    lobby_id: JString<'_>,
) -> jstring {
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        leave_lobby(str_arg(&mut env, lobby_id)?)
    }));
    result_as_jstring(&mut env, match response {
        Ok(v) => v,
        Err(_) => Err(anyhow::anyhow!("native panic during leave_lobby")),
    })
}

/// Publish signed Iroh presence as the persistent Holochain agent.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_NativeBridge_publishSignedPresence(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    presence_json: JString<'_>,
) -> jstring {
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        publish_signed_presence(str_arg(&mut env, presence_json)?)
    }));
    result_as_jstring(&mut env, match response {
        Ok(v) => v,
        Err(_) => Err(anyhow::anyhow!("native panic during Iroh presence publish")),
    })
}

/// Read this agent's latest signed presence records.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_NativeBridge_recentSignedBindings(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
) -> jstring {
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        recent_signed_bindings()
    }));
    result_as_jstring(&mut env, match response {
        Ok(v) => v,
        Err(_) => Err(anyhow::anyhow!("native panic reading signed presence")),
    })
}
