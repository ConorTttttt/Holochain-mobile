//! Embedded Android realtime *endpoint* stage. Reuses the original desktop
//! IrohMeshTransport/MoQ implementation and KartState wire codec by referencing
//! the desktop Rust crates directly from the generated build manifest.
//!
//! The existing APK's H2HC/Wasmi conductor and Lair identity remain a
//! separate JNI library. Gate 2c.2 optionally opens a pinned, digest-checked
//! MoQ mesh and sends one synthetic WP3 diagnostic frame, but deliberately
//! does not run the host/clock controller or a playable race.
//!
//! The 32-byte Iroh endpoint seed is provided from Android Keystore-protected
//! app storage by Kotlin, never persisted or logged by this Rust library.

use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, ensure, Context, Result};
use holochain_moq_kart_wire::{item, KartState, KART_STATE_LEN};
use holochain_moq_mesh_iroh::{iroh_addr_for, iroh_endpoint, IrohConfig, IrohMeshTransport, Reach, SecretKey};
use holochain_moq_turbo_kart::race::RaceRecord;
use holochain_moq_turbo_kart_client::race::RaceAdmission;
use serde::Deserialize;
use holochain_moq_protocol::{
    check_presence, iroh_binding_payload, BindingSignature, EndpointAdvertisement,
    PeerEndpoint, SignedPresence, PRESENCE_TTL_MS,
};
use jni::objects::{JByteArray, JObject, JString};
use jni::sys::jstring;
use jni::JNIEnv;
use serde_json::{json, Value};
use tokio::runtime::{Builder, Runtime};
use zeroize::Zeroize;

mod race_session;
use race_session::RunningRace;

struct LiveEndpoint {
    // Keep Tokio alive: the iroh endpoint and MoQ drivers depend on its tasks.
    _runtime: Runtime,
    transport: IrohMeshTransport,
    race: Option<RunningRace>,
}

static LIVE: OnceLock<Mutex<Option<LiveEndpoint>>> = OnceLock::new();

fn live_slot() -> &'static Mutex<Option<LiveEndpoint>> {
    LIVE.get_or_init(|| Mutex::new(None))
}

fn wire_check() -> Result<()> {
    // Desktop wire.rs golden test, run inside the actual APK-native library.
    let state = KartState {
        flags: 3,
        tick: 0x0102_0304,
        pos: [1.0, -2.0, 0.5],
        vel: [0.0, 0.25, -0.5],
        heading: 3.0,
        drift: 2,
        item: item::GREEN_SHELL,
        item_count: 1,
        lap: 2,
        track_t: 0.75,
    };
    let bytes = state.encode().context("encode desktop kart wire")?;
    let golden = "0103010203043f800000c00000003f000000000000003e800000bf00000040400000020401023f400000";
    ensure!(
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>() == golden,
        "original desktop WP3 golden-vector mismatch"
    );
    ensure!(
        KartState::decode(&bytes).context("decode desktop kart wire")? == state,
        "original desktop WP3 round-trip mismatch"
    );
    Ok(())
}

fn endpoint_status(transport: &IrohMeshTransport, already_running: bool) -> Value {
    let endpoint = transport.endpoint();
    json!({
        "endpoint_id": endpoint.id().to_string(),
        "direct_addrs": endpoint.addr().ip_addrs().map(|ip| ip.to_string()).collect::<Vec<_>>(),
        "already_running": already_running,
        "moq_transport": "original desktop mesh-iroh",
        "kart_wire_bytes": KART_STATE_LEN,
        "signed_presence_published": false,
        "authenticated_race_session": false
    })
}

fn start(key: SecretKey) -> Result<Value> {
    wire_check()?;
    let mut guard = live_slot().lock()
        .map_err(|_| anyhow::anyhow!("Iroh JNI endpoint lock poisoned"))?;
    if let Some(ref existing) = *guard {
        ensure!(
            existing.transport.endpoint().id() == key.public(),
            "different Iroh endpoint key already running; refusing identity switch"
        );
        return Ok(endpoint_status(&existing.transport, true));
    }
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("tk-iroh")
        .enable_all()
        .build()
        .context("create Android Iroh runtime")?;

    let mut config = IrohConfig::new(Reach::DirectOnly);
    config.secret_key = Some(key);
    let transport = runtime
        .block_on(IrohMeshTransport::bind(config, "android-realtime-probe"))
        .context("bind original desktop Iroh/MoQ transport inside APK")?;
    let status = endpoint_status(&transport, false);
    let ips = status["direct_addrs"].as_array().map_or(0, |v| v.len());
    if ips == 0 {
        runtime.block_on(transport.endpoint().close());
        bail!("Iroh bound but published no local IP address");
    }
    *guard = Some(LiveEndpoint {
        _runtime: runtime,
        transport,
        race: None,
    });
    Ok(status)
}

/// Generate the exact original gaming-directory SignedPresence record:
///   author's Holochain AgentPubKeyB64 + Iroh EndpointId proof of possession.
///
/// Does NOT write to Holochain. The owning Holochain conductor validates the
/// author and publishes the return payload with its own agent signer.
fn signed_presence(agent: String) -> Result<Value> {
    ensure!(
        agent.starts_with("uhCAk")
            && agent.len() <= 64
            && agent.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "invalid Holochain agent key syntax"
    );
    let guard = live_slot().lock()
        .map_err(|_| anyhow::anyhow!("Iroh JNI endpoint lock poisoned"))?;
    let live = guard.as_ref().context("start or resume the Iroh endpoint first")?;
    let endpoint = live.transport.endpoint();
    let id = *endpoint.id().as_bytes();
    let now_ms: i64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("Android clock is before Unix epoch")?
        .as_millis()
        .try_into()
        .context("Android clock overflow")?;

    // The desktop's own codec defines "ip:<host>:<port>" tokens,
    // the Ed25519 binding payload, and the 120 s expiry.
    let peer = PeerEndpoint {
        agent: agent.clone(),
        endpoint: iroh_endpoint(&endpoint.addr())
            .context("encode Android Iroh endpoint using original desktop hints")?,
    };
    ensure!(
        peer.endpoint.transport_identity.fingerprint == id,
        "encoded Iroh transport fingerprint changed"
    );
    let advertisement = EndpointAdvertisement::new(
        peer,
        now_ms,
        Some(PRESENCE_TTL_MS),
    ).context("build expiring endpoint advertisement")?;
    check_presence(&advertisement).context("original directory presence rules")?;

    // The endpoint secret never crosses JNI. Only the signature and public
    // SignedPresence envelope are handed back to the Holochain JNI host.
    let sig = endpoint.secret_key().sign(&iroh_binding_payload(&id, &agent));
    let signed = SignedPresence {
        advertisement,
        binding_signature: BindingSignature::from_bytes(&sig.to_bytes()),
    };
    Ok(json!({
        "endpoint_id": endpoint.id().to_string(),
        "agent": agent,
        "presence": signed,
        "signed_presence_published": false,
        "authenticated_race_session": false,
    }))
}

/// The Wasmi JNI host obtained this from the *original* get_race and
/// get_endpoint_binding zomes. Treat every field as untrusted until the
/// unmodified desktop admission validator accepts the frozen roster.
#[derive(Deserialize)]
struct AdmissionEnvelope {
    record: RaceRecord,
    signed_bindings: Vec<SignedPresence>,
    local_agent: String,
}

/// Never trust the Kotlin/JNI race envelope: it is checked by the exact
/// original desktop validator and by comparing the running Iroh public key.
/// This is shared by the read-only check and the connection entry point.
fn verified_admission(raw: &str, live: &LiveEndpoint) -> Result<RaceAdmission> {
    ensure!(raw.len() <= 65536, "locked race payload exceeds 64 KiB");
    let bundle: AdmissionEnvelope = serde_json::from_str(raw)
        .context("decode desktop race record and signed directory bindings")?;
    let admission = RaceAdmission::from_locked(
        &bundle.record,
        &bundle.local_agent,
        bundle.signed_bindings,
    ).context("desktop RaceAdmission::from_locked rejected the locked race")?;
    // The local player's immutable roster binding must name the endpoint
    // associated with this app-private Android Keystore seed.
    let member = admission.members.iter()
        .find(|m| m.agent == bundle.local_agent)
        .context("local Holochain agent is missing from locked race admission")?;
    let actual = live.transport.endpoint().id();
    ensure!(actual.as_bytes() == &member.endpoint_id,
        "persistent Iroh EndpointId does not match this agent's frozen signed binding");
    // Original desktop address decoder verifies the frozen Iroh EndpointId.
    for peer in admission.peers() {
        let addr = iroh_addr_for(&peer.endpoints)
            .with_context(|| format!("invalid locked Iroh address for {}", peer.agent))?;
        ensure!(addr.id.as_bytes() == &peer.endpoint_id,
            "peer Iroh address differs from locked authenticated endpoint");
    }
    Ok(admission)
}

/// Gate 2c.1 remains a read-only check. No network session is opened.
fn validate_locked_admission(raw: String) -> Result<Value> {
    let guard = live_slot().lock()
        .map_err(|_| anyhow::anyhow!("Iroh JNI endpoint lock poisoned"))?;
    let live = guard.as_ref().context("start/resume the persistent Iroh endpoint first")?;
    let admission = verified_admission(&raw, live)?;
    Ok(json!({
        "race_id": admission.race_id.as_str(),
        "lobby_id": admission.lobby,
        "host_agent": admission.host,
        "local_agent": admission.local,
        "endpoint_id": live.transport.endpoint().id().to_string(),
        "admitted_agents": admission.agents(),
        "admitted_human_count": admission.members.len(),
        "locked_roster_verified": true,
        "local_endpoint_matches_locked_binding": true,
        "peer_address_hints_verified": true,
        "authenticated_race_session": false,
        "next": "Opt in to Connect Iroh/MoQ (test), then check the admission digest status"
    }))
}

/// Gate 2c.2: from the *same validated locked roster*, create an Iroh/MoQ
/// mesh using the existing app-private endpoint, publish desktop-compatible
/// race tracks/hello and authenticate the desktop hello asynchronously.
fn connect_locked_race(raw: String) -> Result<Value> {
    let mut guard = live_slot().lock()
        .map_err(|_| anyhow::anyhow!("Iroh JNI endpoint lock poisoned"))?;
    let live = guard.as_mut().context("start/resume the persistent Iroh endpoint first")?;
    let admission = verified_admission(&raw, live)?;
    if let Some(ref old) = live.race {
        ensure!(old.race_id() == admission.race_id.as_str(),
            "another race is already running; disconnect that race first");
        return Ok(old.status());
    }
    // MeshSession::start publishes origins and spawns driver tasks, and must
    // be called within the Tokio runtime attached to this endpoint.
    let race = {
        let _entered = live._runtime.enter();
        RunningRace::start(live.transport.endpoint(), admission)?
    };
    let status = race.status();
    live.race = Some(race);
    Ok(status)
}

fn race_status() -> Result<Value> {
    let guard = live_slot().lock()
        .map_err(|_| anyhow::anyhow!("Iroh JNI endpoint lock poisoned"))?;
    let live = guard.as_ref().context("start Iroh endpoint first")?;
    let race = live.race.as_ref().context("connect an admitted race first")?;
    Ok(race.status())
}

fn send_test_kart() -> Result<Value> {
    let mut guard = live_slot().lock()
        .map_err(|_| anyhow::anyhow!("Iroh JNI endpoint lock poisoned"))?;
    let live = guard.as_mut().context("start Iroh endpoint first")?;
    let race = live.race.as_mut().context("connect an admitted race first")?;
    race.send_test_kart()
}

fn disconnect_race() -> Result<Value> {
    let mut guard = live_slot().lock()
        .map_err(|_| anyhow::anyhow!("Iroh JNI endpoint lock poisoned"))?;
    let live = guard.as_mut().context("start Iroh endpoint first")?;
    let Some(race) = live.race.take() else {
        return Ok(json!({"race_disconnected": true, "already_stopped": true}));
    };
    let race_id = race.race_id().to_string();
    live._runtime.block_on(race.stop());
    Ok(json!({"race_disconnected": true, "race_id": race_id,
        "persistent_iroh_endpoint_preserved": true}))
}

fn response_to_jstring(env: &mut JNIEnv<'_>, result: Result<Value>) -> jstring {
    let body = match result {
        Ok(value) => json!({ "ok": true, "result": value }),
        Err(err) => json!({ "ok": false, "error": format!("{err:#}") }),
    };
    match env.new_string(body.to_string()) {
        Ok(text) => text.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Kotlin object IrohBridge.startEndpoint(seed: ByteArray), synchronous JNI.
/// Must be called on the Activity's background worker (never the UI thread).
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_IrohBridge_startEndpoint(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    seed: JByteArray<'_>,
) -> jstring {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value> {
        let mut input = env.convert_byte_array(seed)
            .context("copy 32-byte endpoint seed from Android Keystore bridge")?;
        if input.len() != 32 {
            input.zeroize();
            bail!("Iroh endpoint seed must contain exactly 32 bytes");
        }
        let mut raw = [0u8; 32];
        raw.copy_from_slice(&input);
        input.zeroize();
        let secret = SecretKey::from_bytes(&raw);
        raw.zeroize();
        start(secret)
    }));
    response_to_jstring(
        &mut env,
        match outcome {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("native panic while starting Android Iroh endpoint")),
        },
    )
}

/// Create an Ed25519 EndpointId → Holochain Agent binding with the desktop
/// protocol's domain separation. This does not publish; call the Holochain
/// native JNI host's publishSignedPresence with the returned presence JSON.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_IrohBridge_signPresence(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    agent_key: JString<'_>,
) -> jstring {
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value> {
        let agent: String = env.get_string(&agent_key)
            .context("read local Holochain agent from Android")?.into();
        signed_presence(agent)
    }));
    response_to_jstring(&mut env, match response {
        Ok(value) => value,
        Err(_) => Err(anyhow::anyhow!("native panic generating Iroh signed presence")),
    })
}

/// Kotlin IrohBridge.validateLockedAdmission(bundleJson):
/// verifies the original RaceAdmission using exact frozen signed bindings.
/// Do not misreport this as an authenticated Iroh/MoQ session.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_IrohBridge_validateLockedAdmission(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    bundle_json: JString<'_>,
) -> jstring {
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value> {
        let raw: String = env.get_string(&bundle_json)
            .context("read locked admission envelope from Android")?.into();
        validate_locked_admission(raw)
    }));
    response_to_jstring(&mut env, match response {
        Ok(value) => value,
        Err(_) => Err(anyhow::anyhow!("native panic validating race admission")),
    })
}

/// Begin an opt-in E6.2 MoQ mesh using frozen Holochain-authorized peers.
/// A successful return means "connecting", NOT "authenticated".
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_IrohBridge_connectLockedRace(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    bundle_json: JString<'_>,
) -> jstring {
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value> {
        let raw: String = env.get_string(&bundle_json)
            .context("read desktop immutable race admission")?.into();
        connect_locked_race(raw)
    }));
    response_to_jstring(&mut env, match response {
        Ok(value) => value,
        Err(_) => Err(anyhow::anyhow!("native panic starting authenticated MoQ mesh")),
    })
}

/// Query actual Iroh/QUIC state and the remote admission-digest verdict.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_IrohBridge_raceStatus(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
) -> jstring {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(race_status));
    response_to_jstring(&mut env, match result {
        Ok(value) => value,
        Err(_) => Err(anyhow::anyhow!("native panic reading MoQ session status")),
    })
}

/// Send exactly one explicit synthetic desktop-compatible WP3 kart packet.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_IrohBridge_sendTestKart(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
) -> jstring {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(send_test_kart));
    response_to_jstring(&mut env, match result {
        Ok(value) => value,
        Err(_) => Err(anyhow::anyhow!("native panic sending test WP3 packet")),
    })
}

/// Close the test race's MoQ mesh without closing/minting the saved endpoint.
#[no_mangle]
pub extern "system" fn Java_dev_holochain_turbokart_mobile_IrohBridge_disconnectRace(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
) -> jstring {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(disconnect_race));
    response_to_jstring(&mut env, match result {
        Ok(value) => value,
        Err(_) => Err(anyhow::anyhow!("native panic stopping MoQ race mesh")),
    })
}
