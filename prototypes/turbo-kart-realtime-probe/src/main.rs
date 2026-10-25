//! Android ARM64 compatibility probe for the *existing desktop* Iroh/MoQ
//! transport and 42-byte WP3 kart format. This is NOT race admission,
//! Holochain presence publishing, a mesh session or gameplay.
//!
//! Cargo dependencies are supplied by scripts/build-turbo-kart-realtime-probe.sh
//! from the unmodified local Turbo Kart desktop source tree.

use anyhow::{ensure, Context, Result};
use holochain_moq_kart_wire::{item, flags, KartState, KART_STATE_LEN};
use holochain_moq_mesh_iroh::{IrohConfig, IrohMeshTransport, Reach};
use serde_json::json;

const DESKTOP_GOLDEN_STATE: &str =
    "0103010203043f800000c00000003f000000000000003e800000bf00000040400000020401023f400000";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn wire_check() -> Result<String> {
    // Identical to crates/kart-wire/tests/wire.rs in the desktop repository.
    // No new kart protocol and no hand-written Android codec.
    let state = KartState {
        flags: flags::AIRBORNE | flags::DRIFTING,
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
    let encoded = state.encode().context("encode desktop WP3 kart state")?;
    ensure!(encoded.len() == KART_STATE_LEN, "unexpected kart payload size");
    let actual = hex(&encoded);
    ensure!(
        actual == DESKTOP_GOLDEN_STATE,
        "desktop golden vector mismatch; refusing to claim compatibility"
    );
    let decoded = KartState::decode(&encoded).context("decode desktop WP3 kart state")?;
    ensure!(decoded == state, "WP3 round-trip mismatch");
    // Rejecting corrupt packets is as important as parsing valid ones.
    ensure!(
        KartState::decode(&encoded[..KART_STATE_LEN - 1]).is_err(),
        "truncated kart packet was accepted"
    );
    let mut wrong_version = encoded;
    wrong_version[0] = 2;
    ensure!(
        KartState::decode(&wrong_version).is_err(),
        "unsupported kart wire version was accepted"
    );
    Ok(actual)
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("{}", json!({
        "event": "starting",
        "kind": "iroh_moq_android_realtime_probe",
        "test_only": true,
        "race_admitted": false
    }));
    let golden = wire_check()?;
    println!("{}", json!({
        "event": "kart_wire_pass",
        "wire_version": 1,
        "payload_bytes": KART_STATE_LEN,
        "golden_hex": golden
    }));

    // Uses the real production mesh-iroh transport crate, not an unrelated
    // UDP socket or an invented MoQ shim. Minimal direct-only endpoint:
    // do not publish any endpoint presence to the Holochain directory.
    let config = IrohConfig::new(Reach::DirectOnly);
    let transport = IrohMeshTransport::bind(config, "android-realtime-probe")
        .await
        .context("bind existing desktop Iroh/MoQ stack on Android")?;
    let endpoint = transport.endpoint();
    let address = endpoint.addr();
    let ips: Vec<String> = address.ip_addrs().map(|a| a.to_string()).collect();
    ensure!(!ips.is_empty(), "iroh endpoint has no bound IP address");
    println!("{}", json!({
        "event": "iroh_endpoint_bound",
        "endpoint_id": endpoint.id().to_string(),
        "direct_addrs": ips,
        "moq_transport": "desktop production mesh-iroh",
        "reach": "direct_only",
        "signed_presence_published": false,
        "authenticated_race_session": false
    }));

    endpoint.close().await;
    println!("{}", json!({
        "event": "complete",
        "ok": true,
        "test_only": true,
        "race_admitted": false
    }));
    Ok(())
}
