//! Gate 2c.2: an opt-in Android Iroh/MoQ peer-session smoke test.
//!
//! The desktop's *unchanged* RaceAdmission::from_locked is checked by lib.rs
//! before entry. This reuses the same mesh, tracks, digest and dial policy as
//! apps/turbo-kart-desktop/sidecar/src/realtime.rs. It does not implement the
//! E6.4 clock/host controller, render a race, or simulate driving. Never
//! count a socket alone as authenticated: the remote's admission digest has
//! to match before a peer is reported as authorized.

use std::collections::BTreeSet;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use holochain_moq_kart_wire::{KartState, KART_STATE_LEN};
use holochain_moq_mesh::{
    DeliveryMode, MeshConfig, MeshSession, Source, TrackName, TrackPublisher, TrackSubscription,
};
use holochain_moq_mesh_iroh::{iroh_addr_for, Endpoint, IrohMeshTransport};
use holochain_moq_turbo_kart_client::race::RaceAdmission;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::task::{JoinHandle, JoinSet};

type Mesh = MeshSession<IrohMeshTransport>;
const PEER_SETUP_DEADLINE: Duration = Duration::from_secs(45);
const DIAL_ATTEMPTS: u32 = 16;
const DIAL_RETRY: Duration = Duration::from_secs(2);
const DIAL_DEADLINE: Duration = Duration::from_secs(42);
const KART_LANE: u8 = 4;

fn track(lane: u8) -> TrackName {
    // Desktop sidecar's *exact* lane mapping. Do not create a mobile format.
    let name = match lane {
        1 => "e62-test-latest",
        2 => "e62-test-events",
        3 => "e62-test-host",
        KART_LANE => "state",
        _ => "e62-test-admission",
    };
    TrackName::new(name).expect("static desktop track name is valid")
}

fn mode(lane: u8) -> DeliveryMode {
    if lane == 0 || lane == 2 {
        DeliveryMode::ReliableOrdered
    } else {
        DeliveryMode::Latest
    }
}

fn mesh_config() -> MeshConfig {
    MeshConfig {
        dial_attempts: DIAL_ATTEMPTS,
        dial_retry: DIAL_RETRY,
        dial_deadline: Some(DIAL_DEADLINE),
        ..MeshConfig::default()
    }
}

/// The exact E6.2 hello digest from the desktop realtime adapter. The
/// ordering is significant; never hash a JSON envelope or unknown fields.
fn admission_digest(admission: &RaceAdmission) -> Result<Vec<u8>> {
    let mut digest = Sha256::new();
    digest.update(admission.lobby.as_bytes());
    digest.update(admission.race_id.as_str().as_bytes());
    digest.update(serde_json::to_vec(&admission.config).context("serialize locked lobby config")?);
    digest.update(admission.host_epoch.to_be_bytes());
    for member in &admission.members {
        digest.update(member.agent.as_bytes());
        digest.update(member.endpoint_id);
        digest.update(member.character_id.as_bytes());
    }
    Ok(digest.finalize().to_vec())
}

async fn read_track(
    mut sub: TrackSubscription,
    lane: u8,
    received: Arc<AtomicU64>,
) -> Result<()> {
    loop {
        let bytes = sub.recv().await.context("MoQ track receive")?
            .context("remote MoQ track closed")?;
        if lane == KART_LANE {
            KartState::decode(&bytes).context("remote sent malformed WP3 KartState")?;
            received.fetch_add(1, Ordering::Relaxed);
        }
        // Other control lanes are subscribed and drained for protocol
        // compatibility, NOT interpreted as host authority or race schedule.
    }
}

/// Authentication lives outside the global JNI mutex: network tasks update
/// these shared diagnostics without ever trying to acquire that mutex.
pub struct RunningRace {
    admission: RaceAdmission,
    mesh: Arc<Mesh>,
    _hello: TrackPublisher,
    _latest: TrackPublisher,
    _events: TrackPublisher,
    _host: Option<TrackPublisher>,
    kart: TrackPublisher,
    authenticated: Arc<Mutex<BTreeSet<String>>>,
    failures: Arc<Mutex<Vec<String>>>,
    received_kart: Arc<AtomicU64>,
    published_kart: u64,
    tasks: Vec<JoinHandle<()>>,
}

impl RunningRace {
    /// Start a race-scoped MoQ mesh on the *existing* persistent Iroh endpoint.
    /// Called inside the already-running Iroh Tokio runtime context.
    pub fn start(endpoint: &Endpoint, admission: RaceAdmission) -> Result<Self> {
        ensure!(admission.members.len() >= 2,
            "authenticated peer test requires at least two admitted players");
        ensure!(admission.host_epoch == 1, "unexpected locked host epoch");
        let mine = admission.members.iter().find(|m| m.agent == admission.local)
            .context("local player is not in signed locked roster")?;
        ensure!(endpoint.id().as_bytes() == &mine.endpoint_id,
            "running Android Iroh endpoint differs from frozen roster");

        let roster = admission.roster(|member| {
            let address = iroh_addr_for(&member.endpoints)
                .with_context(|| format!("decode frozen address for {}", member.agent))?;
            ensure!(address.id.as_bytes() == &member.endpoint_id,
                "peer endpoint address differs from frozen Iroh key");
            Ok::<_, anyhow::Error>(address)
        })?;
        let transport = IrohMeshTransport::from_shared_endpoint(endpoint.clone(), &admission.local)
            .context("prepare original desktop MoQ transport on persistent endpoint")?;
        let (mesh, _events_rx) = Mesh::start(
            transport,
            admission.local.clone(),
            admission.race_id.clone(),
            roster,
            mesh_config(),
        ).context("start authenticated frozen-roster MoQ mesh")?;
        let mesh = Arc::new(mesh);

        // Desktop E6.2 subscribes to these four tracks on every admitted peer.
        // Publish all of them even when this APK is not rendering a race.
        let latest = mesh.publish(track(1), mode(1))?;
        let events = mesh.publish(track(2), mode(2))?;
        let host = if admission.local == admission.host {
            Some(mesh.publish_ai(0, track(3), mode(3))?)
        } else {
            None
        };
        let kart = mesh.publish(track(KART_LANE), mode(KART_LANE))?;
        let mut hello = mesh.publish(track(0), mode(0))?;
        let digest = admission_digest(&admission)?;
        hello.send(digest.clone()).context("publish desktop-compatible admission digest")?;

        let authenticated = Arc::new(Mutex::new(BTreeSet::new()));
        let failures = Arc::new(Mutex::new(Vec::new()));
        let received_kart = Arc::new(AtomicU64::new(0));
        let tasks = admission.peers().map(|m| {
            let peer = m.agent.clone();
            let local = admission.local.clone();
            let is_host = peer == admission.host;
            let mesh = mesh.clone();
            let verified = authenticated.clone();
            let diagnostics = failures.clone();
            let count = received_kart.clone();
            let expected_digest = digest.clone();
            tokio::spawn(async move {
                let outcome = async {
                    // The Iroh/QUIC handshake authenticates EndpointId, and
                    // MeshSession checks that identity against the frozen
                    // agent-to-endpoint roster. Both peers must opt in.
                    let deadline = Instant::now() + PEER_SETUP_DEADLINE;
                    loop {
                        let status = mesh.peers().into_iter().find(|p| p.agent == peer);
                        if status.as_ref().is_some_and(|p| p.connected) { break; }
                        if holochain_moq_mesh::dials(&local, &peer)
                            && status.as_ref().is_some_and(|p| p.initial_dial.finished)
                        {
                            let last = status.as_ref().and_then(|p| p.initial_dial.last_error.as_deref())
                                .unwrap_or("no response");
                            anyhow::bail!("initial authenticated dial to {peer} failed: {last}");
                        }
                        if Instant::now() >= deadline {
                            anyhow::bail!("no authenticated Iroh/QUIC link to {peer} after 45 seconds");
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }

                    let mut remote_hello = mesh.subscribe(&peer, track(0), mode(0))
                        .context("subscribe to remote E6.2 admission digest")?;
                    let bytes = tokio::time::timeout(
                        Duration::from_secs(15), remote_hello.recv()
                    ).await.context("peer did not send an admission digest in 15 seconds")??
                        .context("peer admission track ended")?;
                    ensure!(bytes.as_ref() == expected_digest.as_slice(),
                        "peer's authenticated admission digest differs from the frozen race");
                    // Match the desktop's required subscriptions. The host's
                    // AI source must only be accepted from the host transport.
                    let mut subscriptions = Vec::new();
                    for lane in 1..=KART_LANE {
                        if lane == 3 && !is_host { continue; }
                        let sub = if lane == 3 {
                            mesh.subscribe_source(&peer, Source::Ai(0), track(lane), mode(lane))
                        } else {
                            mesh.subscribe(&peer, track(lane), mode(lane))
                        }.with_context(|| format!("subscribe to peer {peer} lane {lane}"))?;
                        subscriptions.push((lane, sub));
                    }
                    verified.lock().expect("authenticated peer lock").insert(peer.clone());

                    let mut readers = JoinSet::new();
                    for (lane, sub) in subscriptions {
                        readers.spawn(read_track(sub, lane, count.clone()));
                    }
                    // No peer is marked healthy if its reliable track fails.
                    if let Some(first) = readers.join_next().await {
                        first.context("peer's MoQ reader stopped")??;
                    }
                    Ok::<(), anyhow::Error>(())
                }.await;
                verified.lock().expect("authenticated peer lock").remove(&peer);
                if let Err(err) = outcome {
                    let mut errors = diagnostics.lock().expect("mesh error lock");
                    if errors.len() < 8 { errors.push(format!("{peer}: {err:#}")); }
                }
            })
        }).collect();

        Ok(Self {
            admission,
            mesh,
            _hello: hello,
            _latest: latest,
            _events: events,
            _host: host,
            kart,
            authenticated,
            failures,
            received_kart,
            published_kart: 0,
            tasks,
        })
    }

    pub fn race_id(&self) -> &str {
        self.admission.race_id.as_str()
    }

    /// The socket is not a race. Report authenticated MoQ only after QUIC
    /// endpoint-key pinning AND matching race digest; race control remains off.
    pub fn status(&self) -> Value {
        let peers = self.mesh.peers();
        let verified = self.authenticated.lock().expect("authenticated peer lock");
        let authenticated_count = peers.iter()
            .filter(|p| p.connected && verified.contains(&p.agent)).count();
        let all_authenticated = !peers.is_empty() && authenticated_count == peers.len();
        let state = if all_authenticated {
            "authenticated_moq"
        } else if !self.failures.lock().expect("mesh error lock").is_empty() {
            "failed_or_degraded"
        } else {
            "connecting"
        };
        json!({
            "state": state,
            "race_id": self.race_id(),
            "local_agent": self.admission.local,
            "host_agent": self.admission.host,
            "admitted_players": self.admission.members.len(),
            "authenticated_peer_count": authenticated_count,
            "expected_peer_count": peers.len(),
            "authenticated_moq_peer_session": all_authenticated,
            "race_control_active": false,
            "authenticated_race_session": false,
            "peers": peers.iter().map(|p| json!({
                "agent": p.agent,
                "connected": p.connected && verified.contains(&p.agent),
                "link_established": p.connected,
                "rtt_ms": p.rtt_ms,
                "state": format!("{:?}", p.state),
                "path": format!("{:?}", p.kind),
                "we_dial": holochain_moq_mesh::dials(&self.admission.local, &p.agent),
                "last_dial_error": p.initial_dial.last_error,
                "dial_attempts": p.initial_dial.attempts,
            })).collect::<Vec<_>>(),
            "setup_errors": *self.failures.lock().expect("mesh error lock"),
            "kart_wire_bytes": KART_STATE_LEN,
            "synthetic_kart_packets_sent": self.published_kart,
            "remote_kart_packets_decoded": self.received_kart.load(Ordering::Relaxed),
        })
    }

    /// A single explicit, synthetic 42-byte WP3 probe (not race physics).
    /// Only allowed after every remote passed the *matching* admission hello.
    pub fn send_test_kart(&mut self) -> Result<Value> {
        let peers = self.mesh.peers();
        let verified = self.authenticated.lock().expect("authenticated peer lock");
        ensure!(!peers.is_empty()
            && peers.iter().all(|p| p.connected && verified.contains(&p.agent)),
            "cannot publish synthetic WP3 packet before ALL admitted peers authenticate");
        drop(verified);
        let tick: u32 = self.published_kart.checked_add(1)
            .context("test-kart tick overflow")?.try_into()
            .context("too many test kart frames")?;
        let state = KartState {
            flags: 0,
            tick,
            pos: [0.0, 0.0, 0.0],
            vel: [0.0, 0.0, 0.0],
            heading: 0.0,
            drift: 0,
            item: 0,
            item_count: 0,
            lap: 0,
            track_t: 0.0,
        };
        let encoded = state.encode().context("encode original desktop WP3 state")?;
        ensure!(encoded.len() == KART_STATE_LEN, "WP3 wire size changed");
        let mesh_seq = self.kart.send(encoded.to_vec())
            .context("send synthetic 42-byte WP3 state on MoQ lane 4")?;
        self.published_kart += 1;
        Ok(json!({
            "synthetic_test_only": true,
            "race_id": self.race_id(),
            "tick": tick,
            "wp3_bytes": KART_STATE_LEN,
            "moq_sequence": mesh_seq,
            "packet_queued_by_transport": true,
            "desktop_receipt_not_yet_verified": true,
            "race_control_active": false,
        }))
    }

    /// Release all race-only tracks and MoQ sessions, keeping the original
    /// Keystore-owned endpoint and Holochain agent alive for another race.
    pub async fn stop(mut self) {
        for task in &self.tasks { task.abort(); }
        for task in self.tasks.drain(..) { let _ = task.await; }
        let mut mesh = self.mesh;
        // A reader should now have released its Arc. Do not accidentally
        // close the persistent underlying Iroh endpoint: from_shared_endpoint
        // expressly does not own it.
        for _ in 0..40 {
            match Arc::try_unwrap(mesh) {
                Ok(inner) => { inner.close().await; return; }
                Err(shared) => {
                    mesh = shared;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
        drop(mesh);
    }
}
