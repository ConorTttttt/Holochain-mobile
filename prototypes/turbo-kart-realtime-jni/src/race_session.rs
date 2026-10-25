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

use crate::kart_stream;

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

/// One publisher owns both the desktop latest-track send handle and its
/// monotonically increasing tick. The opt-in burst and manual one-shot share
/// this short-held mutex; no JNI call can reuse a tick within the race.
struct TestKartPublisher {
    track: TrackPublisher,
    published: u64,
}

impl TestKartPublisher {
    fn send(&mut self) -> Result<(u32, u64)> {
        let tick: u32 = self.published.checked_add(1)
            .context("synthetic kart-state tick overflow")?.try_into()
            .context("synthetic kart-state tick outside u32 range")?;
        let bytes = kart_stream::encode_sample(tick)?;
        let seq = self.track.send(bytes.to_vec())
            .context("publish original 42-byte WP3 on MoQ lane 4")?;
        self.published += 1;
        Ok((tick, seq))
    }
}

#[derive(Default)]
struct BurstProgress {
    requested: bool,
    running: bool,
    completed: bool,
    sent: u32,
    last_error: Option<String>,
    started_at: Option<Instant>,
    ended_at: Option<Instant>,
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
    kart: Arc<Mutex<TestKartPublisher>>,
    authenticated: Arc<Mutex<BTreeSet<String>>>,
    failures: Arc<Mutex<Vec<String>>>,
    received_kart: Arc<AtomicU64>,
    burst_progress: Arc<Mutex<BurstProgress>>,
    burst_task: Option<JoinHandle<()>>,
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
        let kart = Arc::new(Mutex::new(TestKartPublisher {
            track: mesh.publish(track(KART_LANE), mode(KART_LANE))?,
            published: 0,
        }));
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
            burst_progress: Arc::new(Mutex::new(BurstProgress::default())),
            burst_task: None,
            tasks,
        })
    }

    pub fn race_id(&self) -> &str {
        self.admission.race_id.as_str()
    }

    /// Must be checked at *every* emission, not just at the start of a
    /// bounded test burst: a link may drop or be revoked mid-stream.
    fn all_peers_authenticated(mesh: &Mesh, authenticated: &Mutex<BTreeSet<String>>) -> bool {
        let peers = mesh.peers();
        let verified = authenticated.lock().expect("authenticated peer lock");
        !peers.is_empty() && peers.iter()
            .all(|peer| peer.connected && verified.contains(&peer.agent))
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
        let published = self.kart.lock().expect("kart publisher lock").published;
        let burst = self.burst_progress.lock().expect("diagnostic burst lock");
        let elapsed_ms = burst.started_at.map(|start| {
            burst.ended_at.unwrap_or_else(Instant::now)
                .saturating_duration_since(start).as_millis() as u64
        });
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
            "synthetic_kart_packets_sent": published,
            "remote_kart_packets_decoded": self.received_kart.load(Ordering::Relaxed),
            "diagnostic_wp3_stream": {
                "test_only": true,
                "requested": burst.requested,
                "running": burst.running,
                "completed": burst.completed,
                "nominal_hz": kart_stream::DIAGNOSTIC_HZ,
                "target_frames": kart_stream::DIAGNOSTIC_FRAMES,
                "sent": burst.sent,
                "elapsed_ms": elapsed_ms,
                "error": burst.last_error,
                "desktop_receipt_confirmed": false,
            },
        })
    }

    /// One explicit synthetic WP3 snapshot. The same serialized tick counter
    /// is shared with the bounded stream; no double publishing or replay.
    pub fn send_test_kart(&mut self) -> Result<Value> {
        ensure!(Self::all_peers_authenticated(&self.mesh, &self.authenticated),
            "cannot publish synthetic WP3 packet before ALL admitted peers authenticate");
        ensure!(!self.burst_progress.lock().expect("burst lock").running,
            "single-frame probe cannot run during the 20 Hz diagnostic burst");
        let (tick, mesh_seq) = self.kart.lock()
            .map_err(|_| anyhow::anyhow!("kart publisher lock poisoned"))?
            .send()?;
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

    /// Five seconds, 100 42-byte packets at 20 Hz. Explicit opt-in only.
    /// Every tick checks the authenticated roster; QUIC loss or digest
    /// rejection stops output immediately. Does not call Holochain at 20 Hz.
    ///
    /// Must be called from within the Iroh endpoint's Tokio runtime context.
    pub fn start_test_stream(&mut self) -> Result<Value> {
        ensure!(Self::all_peers_authenticated(&self.mesh, &self.authenticated),
            "cannot start 20 Hz WP3 stream until ALL peers are authenticated");
        {
            let mut progress = self.burst_progress.lock()
                .map_err(|_| anyhow::anyhow!("diagnostic stream lock poisoned"))?;
            ensure!(!progress.requested,
                "bounded WP3 test already requested this race; start a new verified mesh for a repeat");
            progress.requested = true;
            progress.running = true;
            progress.completed = false;
            progress.sent = 0;
            progress.last_error = None;
            progress.started_at = Some(Instant::now());
            progress.ended_at = None;
        }
        let progress = self.burst_progress.clone();
        let mesh = self.mesh.clone();
        let authenticated = self.authenticated.clone();
        let publisher = self.kart.clone();

        self.burst_task = Some(tokio::spawn(async move {
            let mut timer = tokio::time::interval(
                Duration::from_millis(kart_stream::DIAGNOSTIC_INTERVAL_MS)
            );
            // Never produce a sudden catch-up burst after Android is paused.
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut failure = None;
            for _ in 0..kart_stream::DIAGNOSTIC_FRAMES {
                timer.tick().await;
                if !Self::all_peers_authenticated(&mesh, &authenticated) {
                    failure = Some("authenticated peer disconnected; stopped WP3 burst".to_string());
                    break;
                }
                // TrackPublisher::send is synchronous. Hold the mutex only
                // while encoding/sending one packet; do not await inside it.
                let sent = publisher.lock()
                    .map_err(|_| anyhow::anyhow!("kart publisher lock poisoned"))
                    .and_then(|mut guard| guard.send());
                match sent {
                    Ok(_) => {
                        progress.lock().expect("diagnostic progress lock").sent += 1;
                    }
                    Err(error) => {
                        failure = Some(format!("WP3 diagnostic send stopped: {error:#}"));
                        break;
                    }
                }
            }
            let mut state = progress.lock().expect("diagnostic progress lock");
            state.ended_at = Some(Instant::now());
            state.running = false;
            state.completed = failure.is_none()
                && state.sent == kart_stream::DIAGNOSTIC_FRAMES;
            state.last_error = failure;
        }));
        Ok(json!({
            "synthetic_test_only": true,
            "diagnostic_stream_started": true,
            "race_id": self.race_id(),
            "rate_hz": kart_stream::DIAGNOSTIC_HZ,
            "duration_seconds": kart_stream::DIAGNOSTIC_SECONDS,
            "max_frames": kart_stream::DIAGNOSTIC_FRAMES,
            "wp3_bytes_each": KART_STATE_LEN,
            "requires_matching_peer_admission": true,
            "desktop_receipt_not_yet_verified": true,
            "race_control_active": false,
        }))
    }

    /// Release all race-only tracks and MoQ sessions, keeping the original
    /// Keystore-owned endpoint and Holochain agent alive for another race.
    pub async fn stop(mut self) {
        // Cancels any in-flight diagnostic emitter before the race mesh is
        // closed, so no test kart packets escape a disconnected session.
        if let Some(task) = self.burst_task.take() {
            task.abort();
            let _ = task.await;
        }
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
