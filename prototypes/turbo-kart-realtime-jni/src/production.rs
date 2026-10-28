//! Android transport adapter around the unchanged desktop Session/RaceCtl.
//! No clock, readiness, progress, AI or item rules are duplicated here.
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use turbo_kart_sidecar::{binary::{Packet, KART_LANE}, netmode::NetMode, realtime::Session};
use holochain_moq_mesh_iroh::Endpoint;
use holochain_moq_turbo_kart_client::race::RaceAdmission;

#[derive(Default)]
struct Pacer {
    client_id: u32,
    native_id: u32,
    pending: VecDeque<Packet>,
    queued: u64,
    published: u64,
    superseded: u64,
    empty_ticks: u64,
    last_send: Option<Instant>,
    intervals: Vec<u64>,
    error: Option<String>,
    ticks: u64,
    skipped_deadlines: u64,
}

fn authenticated(session: &Session) -> bool { session.status(0)["state"] == "connected" }
fn send(session: &mut Session, pacer: &mut Pacer, mut packet: Packet) -> Result<u64> {
    pacer.native_id = pacer.native_id.checked_add(1).context("native IPC sequence exhausted")?;
    // IPC IDs are local acknowledgements, not WP3 ticks or MoQ source identity.
    packet.id = pacer.native_id;
    session.send(&packet).map_err(|e| anyhow::anyhow!("{:?}: {}", e.code, e.message))
}

pub struct ProductionRace {
    pub race_id: String,
    record: Value,
    session: Arc<Mutex<Option<Session>>>,
    pacer: Arc<Mutex<Pacer>>,
    received: mpsc::Receiver<Packet>,
    running: Arc<AtomicBool>,
    publisher: std::thread::JoinHandle<()>,
}

impl ProductionRace {
    pub fn start(endpoint: &Endpoint, admission: RaceAdmission, record: Value, generation: u32, relay: &str) -> Result<Self> {
        let (tx, received) = mpsc::channel(64);
        let mode = if relay.is_empty() { NetMode::Lan } else { NetMode::Internet { relay: relay.parse()? } };
        let race_id = admission.race_id.as_str().to_owned();
        let session = Session::start_with(endpoint, admission, generation, tx, &mode)
            .map_err(|e| anyhow::anyhow!("desktop session: {}", e.message))?;
        let session = Arc::new(Mutex::new(Some(session)));
        let pacer = Arc::new(Mutex::new(Pacer::default()));
        let (shared, paced) = (session.clone(), pacer.clone());
        let running = Arc::new(AtomicBool::new(true));
        let active = running.clone();
        let runtime = tokio::runtime::Handle::current();
        let publisher = std::thread::Builder::new().name("tk-wp3-20hz".into()).stack_size(256*1024).spawn(move || {
            let _entered = runtime.enter();
            let period = Duration::from_millis(50);
            let mut deadline = Instant::now() + period;
            while active.load(Ordering::Relaxed) {
                std::thread::park_timeout(deadline.saturating_duration_since(Instant::now()));
                if !active.load(Ordering::Relaxed) { break; }
                let now = Instant::now();
                if now < deadline { continue; }
                deadline += period;
                let mut guard = shared.lock().expect("production session lock");
                let Some(session) = guard.as_mut() else { break };
                let mut state = paced.lock().expect("production pacer lock");
                state.ticks += 1;
                // Never burst after a suspended/overloaded process.
                let resumed = Instant::now();
                if deadline <= resumed {
                    state.skipped_deadlines += 1;
                    deadline = resumed + period;
                }
                if !authenticated(session) { state.pending.clear(); continue; }
                let Some(packet) = state.pending.pop_front() else { state.empty_ticks += 1; continue };
                match send(session, &mut state, packet) {
                    Ok(_) => {
                        let now = Instant::now();
                        if let Some(last) = state.last_send.replace(now) {
                            if state.intervals.len() == 1200 { state.intervals.remove(0); }
                            state.intervals.push(now.duration_since(last).as_micros() as u64);
                        }
                        state.published += 1;
                    }
                    Err(error) => state.error = Some(format!("{error:#}")),
                }
            }
        }).context("start bounded native WP3 publication thread")?;
        Ok(Self { race_id, record, session, pacer, received, running, publisher })
    }

    pub fn status(&self) -> Value {
        let session = self.session.lock().expect("production session lock");
        let mut status = session.as_ref().map(|s| s.status(0)).unwrap_or_else(|| json!({"state":"disconnected","realtime_active":false}));
        let pacer = self.pacer.lock().expect("production pacer lock");
        let mut intervals = pacer.intervals.clone(); intervals.sort_unstable();
        status["android_publication"] = json!({"nominal_hz":20,"publisher":"native-thread","queue_capacity":3,
            "queued":pacer.queued,"published":pacer.published,"superseded":pacer.superseded,
            "empty_ticks":pacer.empty_ticks,"ticks":pacer.ticks,"skipped_deadlines":pacer.skipped_deadlines,
            "interval_samples":intervals.len(),"mean_interval_us":if intervals.is_empty(){0}else{intervals.iter().sum::<u64>()/intervals.len() as u64},
            "p95_interval_us":intervals.get(intervals.len()*95/100).copied(),"max_interval_us":intervals.last(),"error":pacer.error});
        status
    }

    pub fn request(&mut self, value: &Value) -> Result<Value> {
        match value["op"].as_str().context("missing production operation")? {
            "bootstrap" => Ok(json!({"record":self.record,"status":self.status()})),
            "status" => Ok(self.status()),
            "publication_metrics" => Ok(json!({"interval_us":self.pacer.lock().expect("production pacer lock").intervals})),
            "ready" => {
                let guard = self.session.lock().expect("production session lock");
                let session = guard.as_ref().context("session closed")?;
                ensure!(authenticated(session), "all admitted peers must authenticate before readiness");
                session.race.as_ref().context("original race controller unavailable")?.renderer_ready();
                Ok(json!({"renderer_ready":true}))
            }
            "packet" => {
                let encoded = value["packet"].as_str().context("missing packet")?;
                ensure!(encoded.len() <= 4096, "production packet exceeds limit");
                let bytes = STANDARD.decode(encoded).context("invalid packet base64")?;
                let packet = Packet::parse(&bytes).map_err(anyhow::Error::msg)?;
                ensure!(packet.kind == 1, "only original game sends accepted");
                let mut guard = self.session.lock().expect("production session lock");
                let session = guard.as_mut().context("session closed")?;
                ensure!(authenticated(session), "peer authentication lost");
                ensure!(packet.race == self.race_id && packet.generation == session.generation, "retired race or generation");
                let mut pacer = self.pacer.lock().expect("production pacer lock");
                ensure!(packet.id > pacer.client_id, "replayed game request");
                pacer.client_id = packet.id;
                let result = if packet.lane == KART_LANE {
                    holochain_moq_kart_wire::KartState::decode(&packet.payload)?;
                    // Three actual simulation samples absorb event-loop jitter.
                    // Excess old samples are superseded; no state or tick is invented.
                    if pacer.pending.len() == 3 { pacer.pending.pop_front(); pacer.superseded += 1; }
                    pacer.pending.push_back(packet.clone());
                    pacer.queued += 1;
                    Ok(0) // Acknowledge bounded Latest queue acceptance, never remote delivery.
                } else {
                    send(session, &mut pacer, packet.clone()).map_err(|e| turbo_kart_sidecar::protocol::IpcError::new(
                        turbo_kart_sidecar::protocol::IpcErrorCode::InvalidInput, format!("{e:#}")))
                };
                Ok(json!({"reply":STANDARD.encode(packet.reply(result).bytes())}))
            }
            "poll" => {
                let mut packets = Vec::new();
                for _ in 0..32 {
                    match self.received.try_recv() {
                        Ok(packet) => packets.push(STANDARD.encode(packet.bytes())),
                        Err(_) => break,
                    }
                }
                Ok(json!({"packets":packets}))
            }
            _ => anyhow::bail!("unknown production operation"),
        }
    }

    pub async fn stop(self) {
        self.running.store(false, Ordering::Relaxed);
        self.publisher.thread().unpark(); let _ = self.publisher.join();
        let session = self.session.lock().expect("production session lock").take();
        if let Some(session) = session { session.stop().await; }
    }
}
