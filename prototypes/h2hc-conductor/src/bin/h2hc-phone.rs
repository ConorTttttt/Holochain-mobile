//! A phone-shaped Holochain conductor for benchmarks. Runs either on the
//! H2HC adapter (`--network h2hc`) or as a zero-arc Kitsune2 node
//! (`--network kitsune2`), with Cranelift or wasmi. Controlled by
//! `h2hc_conductor::control` commands on stdin; logs go to stderr.
//!
//! ```text
//! h2hc-phone --network h2hc --linker-url ws://127.0.0.1:8090/conductor/v1 \
//!   --linker-key <base64> --wasm cranelift --data-dir /tmp/phone --happ notes.happ
//! ```

use base64::Engine;
use h2hc_conductor::control::{Command, Ready, Stats, ThreadStats};
use h2hc_conductor::*;
use h2hc_p2p::ws::WsCounters;
use h2hc_protocol::LinkerId;
use holochain::conductor::config::{ConductorConfig, KeystoreConfig, NetworkConfig, WasmBackend};
use holochain::conductor::{ConductorBuilder, ConductorHandle};
use holochain::prelude::*;
use holochain_conductor_api::CellInfo;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::oneshot;

const APP_ID: &str = "notes";
const ZOME: &str = "notes";
const PING_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(serde::Serialize, serde::Deserialize, Debug)]
enum Ping {
    Ping { id: u64 },
    Pong { id: u64 },
}

#[derive(serde::Serialize, Debug)]
struct EchoInput {
    agent: AgentPubKey,
    text: String,
}

#[derive(serde::Serialize, Debug)]
struct PingInput {
    agent: AgentPubKey,
    id: u64,
}

struct Args {
    network: String,
    linker_url: String,
    linker_key: Option<LinkerId>,
    bootstrap_url: String,
    relay_url: String,
    wasm: String,
    keystore: String,
    data_dir: PathBuf,
    happ: PathBuf,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut map = HashMap::new();
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let k = k.trim_start_matches("--").to_string();
        let v = it.next().ok_or_else(|| anyhow::anyhow!("--{k} needs a value"))?;
        map.insert(k, v);
    }
    let get = |k: &str, d: &str| map.get(k).cloned().unwrap_or_else(|| d.to_string());
    let linker_key = match map.get("linker-key") {
        Some(b64) => {
            let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
            Some(LinkerId(bytes.try_into().map_err(|_| anyhow::anyhow!("linker key must be 32 bytes"))?))
        }
        None => None,
    };
    Ok(Args {
        network: get("network", "h2hc"),
        linker_url: get("linker-url", "ws://127.0.0.1:8090/conductor/v1"),
        linker_key,
        bootstrap_url: get("bootstrap-url", ""),
        relay_url: get("relay-url", ""),
        wasm: get("wasm", "cranelift"),
        keystore: get("keystore", "lair"),
        data_dir: PathBuf::from(get("data-dir", "/tmp/h2hc-phone")),
        happ: PathBuf::from(get("happ", "notes.happ")),
    })
}

struct Phone {
    conductor: ConductorHandle,
    happ: PathBuf,
    data_dir: PathBuf,
    counters: Arc<WsCounters>,
    cell: Mutex<Option<(AgentPubKey, CellId)>>,
    pongs: Arc<Mutex<HashMap<u64, oneshot::Sender<()>>>>,
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn hash<T: HashType>(s: &str) -> anyhow::Result<HoloHash<T>>
where
    HoloHashB64<T>: std::str::FromStr,
    <HoloHashB64<T> as std::str::FromStr>::Err: std::fmt::Debug,
{
    let b64: HoloHashB64<T> = s
        .parse()
        .map_err(|e| anyhow::anyhow!("bad hash {s}: {e:?}"))?;
    Ok(b64.into())
}

impl Phone {
    fn cell(&self) -> anyhow::Result<(AgentPubKey, CellId)> {
        self.cell
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("app not installed"))
    }

    async fn call<I, O>(&self, fn_name: &str, payload: I) -> anyhow::Result<O>
    where
        I: serde::Serialize + std::fmt::Debug,
        O: serde::de::DeserializeOwned + std::fmt::Debug,
    {
        let (agent, cell_id) = self.cell()?;
        Ok(self
            .conductor
            .easy_call_zome(&agent, None, cell_id, ZOME, fn_name, payload)
            .await?)
    }

    async fn handle(&self, cmd: Command) -> anyhow::Result<Value> {
        Ok(match cmd {
            Command::Install => {
                let bytes = std::fs::read(&self.happ)?;
                let start = Instant::now();
                let app = self
                    .conductor
                    .clone()
                    .install_app_bundle(InstallAppPayload {
                        source: AppBundleSource::Bytes(bytes.into()),
                        agent_key: None,
                        installed_app_id: Some(APP_ID.into()),
                        network_seed: None,
                        roles_settings: None,
                        ignore_genesis_failure: false,
                        restore_from_dht: false,
                    })
                    .await?;
                let install_ms = ms(start);
                let start = Instant::now();
                self.conductor.clone().enable_app(APP_ID.into()).await?;
                let enable_ms = ms(start);
                let agent = app.agent_key().clone();
                let cell_id = app
                    .all_cells()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("app has no cell"))?;
                *self.cell.lock().unwrap() = Some((agent.clone(), cell_id));
                self.spawn_signal_listener();
                json!({ "agent": AgentPubKeyB64::from(agent).to_string(),
                        "install_ms": install_ms, "enable_ms": enable_ms })
            }
            Command::Resume => {
                let info = self
                    .conductor
                    .get_app_info(&APP_ID.into())
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("no installed app to resume"))?;
                let cell_id = info
                    .cell_info
                    .values()
                    .flatten()
                    .find_map(|c| match c {
                        CellInfo::Provisioned(p) => Some(p.cell_id.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| anyhow::anyhow!("app has no provisioned cell"))?;
                let agent = info.agent_pub_key.clone();
                *self.cell.lock().unwrap() = Some((agent.clone(), cell_id));
                self.spawn_signal_listener();
                json!({ "agent": AgentPubKeyB64::from(agent).to_string() })
            }
            Command::CreateNote { text } => {
                let start = Instant::now();
                let h: ActionHash = self.call("create_note", text).await?;
                json!({ "hash": ActionHashB64::from(h).to_string(), "ms": ms(start) })
            }
            Command::GetNote { hash: h } => {
                let h: ActionHash = hash(&h)?;
                let start = Instant::now();
                let record: Option<Record> = self.call("get_note", h).await?;
                json!({ "found": record.is_some(), "ms": ms(start) })
            }
            Command::GetNotes { agent } => {
                let agent: AgentPubKey = hash(&agent)?;
                let start = Instant::now();
                let links: Vec<Link> = self.call("get_notes", agent).await?;
                json!({ "count": links.len(), "ms": ms(start) })
            }
            Command::CallEcho { agent, text } => {
                let agent: AgentPubKey = hash(&agent)?;
                let start = Instant::now();
                let reply: String = self.call("call_echo", EchoInput { agent, text }).await?;
                json!({ "reply": reply, "ms": ms(start) })
            }
            Command::Ping { agent, id } => {
                let agent: AgentPubKey = hash(&agent)?;
                let (tx, rx) = oneshot::channel();
                self.pongs.lock().unwrap().insert(id, tx);
                let start = Instant::now();
                let () = self.call("ping", PingInput { agent, id }).await?;
                match tokio::time::timeout(PING_TIMEOUT, rx).await {
                    Ok(Ok(())) => json!({ "rtt_ms": ms(start) }),
                    _ => {
                        self.pongs.lock().unwrap().remove(&id);
                        anyhow::bail!("no pong within {PING_TIMEOUT:?}")
                    }
                }
            }
            Command::Receipts { hash: h } => {
                let h: ActionHash = hash(&h)?;
                let (_, cell_id) = self.cell()?;
                let sets = self
                    .conductor
                    .get_dht_store(cell_id.dna_hash())?
                    .as_read()
                    .validation_receipts_for_action(h)
                    .await?;
                json!({
                    "ops": sets.len(),
                    "receipts": sets.iter().map(|s| s.receipts.len()).sum::<usize>(),
                    "ops_with_receipt": sets.iter().filter(|s| !s.receipts.is_empty()).count(),
                    "complete": sets.iter().filter(|s| s.receipts_complete).count(),
                })
            }
            Command::Stats => serde_json::to_value(self.stats())?,
            Command::Threads => json!({ "threads": threads() }),
            Command::Profile { secs } => self.profile(secs).await?,
            Command::Quit => std::process::exit(0),
        })
    }

    fn spawn_signal_listener(&self) {
        let mut rx = self.conductor.subscribe_to_app_signals(APP_ID.into());
        let pongs = self.pongs.clone();
        tokio::spawn(async move {
            while let Ok(signal) = rx.recv().await {
                if let Signal::App { signal, .. } = signal {
                    if let Ok(Ping::Pong { id }) = signal.into_inner().decode::<Ping>() {
                        if let Some(tx) = pongs.lock().unwrap().remove(&id) {
                            let _ = tx.send(());
                        }
                    }
                }
            }
        });
    }

    #[cfg(feature = "profile")]
    async fn profile(&self, secs: u64) -> anyhow::Result<Value> {
        use std::fmt::Write;
        // SIGPROF counts CPU time, so an idle process is rarely interrupted.
        let guard = pprof::ProfilerGuardBuilder::default()
            .frequency(997)
            .blocklist(&["libc", "libgcc", "pthread", "vdso"])
            .build()?;
        tokio::time::sleep(Duration::from_secs(secs)).await;
        let report = guard.report().build()?;
        let base = self.data_dir.join(format!(
            "profile-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs()
        ));
        // Collapsed stacks, outermost frame first, as inferno reads them.
        let mut folded = String::new();
        let mut samples = 0;
        for (frames, count) in &report.data {
            samples += count;
            let _ = write!(folded, "{}", frames.thread_name_or_id());
            for frame in frames.frames.iter().rev() {
                for symbol in frame.iter().rev() {
                    let _ = write!(folded, ";{symbol}");
                }
            }
            let _ = writeln!(folded, " {count}");
        }
        std::fs::write(base.with_extension("folded"), folded)?;
        report.flamegraph(std::fs::File::create(base.with_extension("svg"))?)?;
        Ok(json!({ "samples": samples, "folded": base.with_extension("folded"), "svg": base.with_extension("svg") }))
    }

    #[cfg(not(feature = "profile"))]
    async fn profile(&self, _secs: u64) -> anyhow::Result<Value> {
        anyhow::bail!("built without the profile feature")
    }

    fn stats(&self) -> Stats {
        use std::sync::atomic::Ordering::Relaxed;
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let field = |name: &str| {
            status
                .lines()
                .find(|l| l.starts_with(name))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        };
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
        let tv_ms = |tv: libc::timeval| tv.tv_sec as u64 * 1000 + tv.tv_usec as u64 / 1000;
        let (heap_in_use_kb, heap_free_kb) = heap_kb();
        Stats {
            rss_kb: field("VmRSS:"),
            peak_rss_kb: field("VmHWM:"),
            rss_anon_kb: field("RssAnon:"),
            rss_file_kb: field("RssFile:"),
            heap_in_use_kb,
            heap_free_kb,
            cpu_user_ms: tv_ms(usage.ru_utime),
            cpu_sys_ms: tv_ms(usage.ru_stime),
            threads: field("Threads:"),
            data_bytes: dir_size(&self.data_dir),
            link_sent: self.counters.bytes_sent.load(Relaxed),
            link_received: self.counters.bytes_received.load(Relaxed),
            link_connects: self.counters.connects.load(Relaxed),
        }
    }
}

/// The allocator's live and free bytes, in KiB. Rust allocates through the
/// system allocator here, so this covers Rust allocations, wasmi's linear
/// memories and SQLite's heap.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn heap_kb() -> (u64, u64) {
    // Large blocks are mmapped (hblkhd) rather than taken from an arena.
    let m = unsafe { libc::mallinfo2() };
    (((m.uordblks + m.hblkhd) / 1024) as u64, (m.fordblks / 1024) as u64)
}

#[cfg(target_os = "android")]
fn heap_kb() -> (u64, u64) {
    let m = unsafe { libc::mallinfo() };
    (((m.uordblks + m.hblkhd) / 1024) as u64, (m.fordblks / 1024) as u64)
}

#[cfg(not(any(all(target_os = "linux", target_env = "gnu"), target_os = "android")))]
fn heap_kb() -> (u64, u64) {
    (0, 0)
}

/// Every thread's CPU time and voluntary context switches. CPU time comes
/// from `schedstat` (nanoseconds) where the kernel has it, else from `stat`
/// (clock ticks).
fn threads() -> Vec<ThreadStats> {
    let Ok(dir) = std::fs::read_dir("/proc/self/task") else {
        return Vec::new();
    };
    let tick_us = 1_000_000 / unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    dir.flatten()
        .filter_map(|e| {
            let tid = e.file_name().to_str()?.parse().ok()?;
            let path = e.path();
            let read = |f: &str| std::fs::read_to_string(path.join(f)).ok();
            let cpu_us = match read("schedstat") {
                Some(s) => s.split_whitespace().next()?.parse::<u64>().ok()? / 1000,
                None => {
                    // utime and stime follow the parenthesised name.
                    let stat = read("stat")?;
                    let rest = stat.get(stat.rfind(')')? + 2..)?;
                    let ticks: u64 = rest.split_whitespace().skip(11).take(2).filter_map(|v| v.parse::<u64>().ok()).sum();
                    ticks * tick_us
                }
            };
            let wakeups = read("status")?
                .lines()
                .find_map(|l| l.strip_prefix("voluntary_ctxt_switches:"))?
                .trim()
                .parse()
                .ok()?;
            Some(ThreadStats { tid, name: read("comm")?.trim().to_string(), cpu_us, wakeups })
        })
        .collect()
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

fn print(value: &Value) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let process_start = Instant::now();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let args = parse_args()?;
    std::fs::create_dir_all(&args.data_dir)?;

    let mut config = ConductorConfig {
        data_root_path: Some(args.data_dir.clone().into()),
        // `memory` is for adb-shell runs: Android does not let the shell
        // user bind Lair's Unix socket. Keys then do not survive a restart.
        keystore: match args.keystore.as_str() {
            "memory" => KeystoreConfig::DangerTestKeystore,
            _ => KeystoreConfig::LairServerInProc { lair_root: None },
        },
        wasm_backend: Some(match args.wasm.as_str() {
            "wasmi" => WasmBackend::Wasmi,
            _ => WasmBackend::Cranelift,
        }),
        // Fails at startup if this build has a different engine.
        ..Default::default()
    };
    let passphrase = Arc::new(Mutex::new(sodoken::LockedArray::from(
        b"benchmark-passphrase".to_vec(),
    )));
    let counters = Arc::new(WsCounters::default());

    let conductor = match args.network.as_str() {
        "kitsune2" => {
            config.network = NetworkConfig {
                bootstrap_url: url2::Url2::parse(&args.bootstrap_url),
                relay_url: url2::Url2::parse(&args.relay_url),
                target_arc_factor: 0,
                advanced: Some(json!({ "irohTransport": { "relayAllowPlainText": true } })),
                ..Default::default()
            };
            ConductorBuilder::new()
                .config(config)
                .passphrase(Some(passphrase))
                .no_print_setup()
                .build()
                .await?
        }
        _ => {
            let (url, counters) = (args.linker_url.clone(), counters.clone());
            let connector: Connector = Arc::new(move || {
                let (url, counters) = (url.clone(), counters.clone());
                Box::pin(async move { h2hc_p2p::ws::connect(&url, counters).await })
            });
            let options = H2hcOptions {
                expected_linker: args.linker_key,
                ..Default::default()
            };
            build_phone_conductor(config, passphrase, connector, options).await?
        }
    };

    print(&serde_json::to_value(Ready {
        event: "ready".into(),
        network: args.network.clone(),
        wasm: args.wasm.clone(),
        keystore: args.keystore.clone(),
        startup_ms: ms(process_start),
    })?);

    let phone = Arc::new(Phone {
        conductor,
        happ: args.happ,
        data_dir: args.data_dir,
        counters,
        cell: Mutex::new(None),
        pongs: Default::default(),
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Command>(&line) {
            Ok(cmd) => match phone.handle(cmd).await {
                Ok(mut v) => {
                    v["ok"] = json!(true);
                    v
                }
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            },
            Err(e) => json!({ "ok": false, "error": format!("bad command: {e}") }),
        };
        print(&reply);
    }
    Ok(())
}
