//! Embedded Android realtime *endpoint* stage. Reuses the original desktop
//! IrohMeshTransport/MoQ implementation and KartState wire codec by referencing
//! the desktop Rust crates directly from the generated build manifest.
//!
//! This component deliberately does NOT advertise Holochain signed presence,
//! establish an admitted race, or transmit game snapshots. The existing APK's
//! H2HC/Wasmi conductor and Lair identity remain a separate JNI library.
//!
//! The 32-byte Iroh endpoint seed is provided from Android Keystore-protected
//! app storage by Kotlin, never persisted or logged by this Rust library.

use std::sync::{Mutex, OnceLock};

use anyhow::{bail, ensure, Context, Result};
use holochain_moq_kart_wire::{item, KartState, KART_STATE_LEN};
use holochain_moq_mesh_iroh::{IrohConfig, IrohMeshTransport, Reach, SecretKey};
use jni::objects::{JByteArray, JObject};
use jni::sys::jstring;
use jni::JNIEnv;
use serde_json::{json, Value};
use tokio::runtime::{Builder, Runtime};
use zeroize::Zeroize;

struct LiveEndpoint {
    // Keep Tokio alive: the iroh endpoint and MoQ drivers depend on its tasks.
    _runtime: Runtime,
    transport: IrohMeshTransport,
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
    });
    Ok(status)
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
