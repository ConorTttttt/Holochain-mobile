//! Starts a local Holochain network, runs `h2hc-phone` against it in each
//! configuration, and writes a benchmark report.
//!
//! ```text
//! h2hc-bench [--configs h2hc:cranelift,h2hc:wasmi,kitsune2:cranelift,kitsune2:wasmi]
//!            [--iterations 20] [--receipt-samples 5] [--authorities 3]
//!            [--idle-secs 60] [--no-scenarios] [--long]
//!            [--target local|android] [--serial <adb serial>]
//! ```

mod net;
mod phone;
mod report;

use anyhow::Context;
use h2hc_conductor::control::Command as Cmd;
use holochain::prelude::*;
use net::{Authority, Bootstrap, EchoInput, Linker};
use phone::{Phone, Target};
use report::{ConfigResult, Report, ScenarioResult, Series};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Opts {
    configs: Vec<(String, String)>,
    iterations: usize,
    receipt_samples: usize,
    authorities: usize,
    idle_secs: u64,
    scenarios: bool,
    long: bool,
    target: String,
    serial: Option<String>,
    root: PathBuf,
}

fn parse() -> anyhow::Result<Opts> {
    let mut map = HashMap::new();
    let mut flags = Vec::new();
    let mut it = std::env::args().skip(1).peekable();
    while let Some(k) = it.next() {
        let k = k.trim_start_matches("--").to_string();
        match it.peek() {
            Some(v) if !v.starts_with("--") => {
                map.insert(k, it.next().unwrap());
            }
            _ => flags.push(k),
        }
    }
    let num = |k: &str, d: u64| -> anyhow::Result<u64> {
        Ok(map.get(k).map(|v| v.parse()).transpose()?.unwrap_or(d))
    };
    let configs = map
        .get("configs")
        .map(String::as_str)
        .unwrap_or("h2hc:cranelift,h2hc:wasmi,kitsune2:cranelift,kitsune2:wasmi")
        .split(',')
        .map(|c| {
            let (n, w) = c.split_once(':').context("config is network:wasm")?;
            Ok((n.to_string(), w.to_string()))
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(Opts {
        configs,
        iterations: num("iterations", 20)? as usize,
        receipt_samples: num("receipt-samples", 5)? as usize,
        authorities: num("authorities", 3)? as usize,
        idle_secs: num("idle-secs", 60)?,
        scenarios: !flags.iter().any(|f| f == "no-scenarios"),
        long: flags.iter().any(|f| f == "long"),
        target: map.get("target").cloned().unwrap_or_else(|| "local".into()),
        serial: map.get("serial").cloned(),
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    })
}

fn progress(msg: impl AsRef<str>) {
    eprintln!("[h2hc-bench] {}", msg.as_ref());
}

fn agent_from(b64: &str) -> anyhow::Result<AgentPubKey> {
    let k: AgentPubKeyB64 = b64.parse().map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(k.into())
}

fn hash_from(b64: &str) -> anyhow::Result<ActionHash> {
    let h: ActionHashB64 = b64.parse().map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(h.into())
}

fn hash_b64(h: &ActionHash) -> String {
    ActionHashB64::from(h.clone()).to_string()
}

struct Net {
    bootstrap: Bootstrap,
    authorities: Vec<Authority>,
    linker: Linker,
}

impl Net {
    fn phone_args(&self, network: &str, wasm: &str) -> Vec<String> {
        let mut args = vec!["--network".into(), network.into(), "--wasm".into(), wasm.into()];
        if network == "h2hc" {
            args.extend([
                "--linker-url".into(),
                self.linker.ws_url(),
                "--linker-key".into(),
                self.linker.key_b64.clone(),
            ]);
        } else {
            args.extend([
                "--bootstrap-url".into(),
                self.bootstrap.url(),
                "--relay-url".into(),
                self.bootstrap.url(),
            ]);
        }
        args
    }

    fn authority(&self, i: usize) -> &Authority {
        &self.authorities[i % self.authorities.len()]
    }
}

/// A phone-side operation's own timing, which excludes the control pipe.
fn phone_ms(v: &serde_json::Value) -> anyhow::Result<f64> {
    v["ms"].as_f64().context("reply has no timing")
}

async fn poll<F>(within: Duration, every: Duration, mut f: F) -> anyhow::Result<f64>
where
    F: AsyncFnMut() -> anyhow::Result<bool>,
{
    let start = Instant::now();
    while start.elapsed() < within {
        if f().await.unwrap_or(false) {
            return Ok(start.elapsed().as_secs_f64() * 1000.0);
        }
        tokio::time::sleep(every).await;
    }
    anyhow::bail!("not reached within {within:?}")
}

async fn receipts_complete(phone: &mut Phone, hash: &str) -> anyhow::Result<bool> {
    let r = phone.cmd(&Cmd::Receipts { hash: hash.into() }).await?;
    let ops = r["ops"].as_u64().unwrap_or(0);
    Ok(ops > 0 && r["ops_with_receipt"].as_u64() == Some(ops))
}

async fn run_config(net: &Net, target: &Target, run_dir: &Path, network: &str, wasm: &str, opts: &Opts) -> ConfigResult {
    let mut r = ConfigResult {
        network: network.into(),
        wasm: wasm.into(),
        idle_secs: opts.idle_secs,
        ..Default::default()
    };
    if let Err(e) = run_config_inner(net, target, run_dir, opts, &mut r).await {
        r.errors.push(format!("{e:#}"));
    }
    r
}

async fn run_config_inner(net: &Net, target: &Target, run_dir: &Path, opts: &Opts, r: &mut ConfigResult) -> anyhow::Result<()> {
    let name = format!("phone-{}-{}", r.network, r.wasm);
    target.wipe(&name).await?;
    let args = net.phone_args(&r.network, &r.wasm);
    let mut phone = Phone::spawn(target, &name, &r.wasm, &args, &run_dir.join(format!("{name}.log"))).await?;
    r.startup_ms = phone.ready.startup_ms;

    let installed = phone.cmd(&Cmd::Install).await?;
    r.install_ms = installed["install_ms"].as_f64().unwrap_or(0.0);
    r.enable_ms = installed["enable_ms"].as_f64().unwrap_or(0.0);
    let enabled_at = Instant::now();
    let me = agent_from(installed["agent"].as_str().context("no agent")?)?;
    progress(format!("{}: installed in {:.0} ms", r.label(), r.install_ms + r.enable_ms));

    let first = phone.cmd(&Cmd::CreateNote { text: "first".into() }).await?;
    r.first_call_ms = phone_ms(&first)?;
    let own = first["hash"].as_str().context("no hash")?.to_string();

    // Ready once the phone can read data held only by authorities.
    let probe = hash_b64(&net.authority(0).create_note("network probe").await?);
    let found = poll(Duration::from_secs(180), Duration::from_millis(200), async || {
        let v = phone.cmd(&Cmd::GetNote { hash: probe.clone() }).await?;
        Ok(v["found"].as_bool() == Some(true))
    })
    .await;
    match found {
        Ok(_) => r.network_ready_ms = Some(enabled_at.elapsed().as_secs_f64() * 1000.0),
        Err(e) => r.errors.push(format!("phone never read an authority's note: {e}")),
    }
    progress(format!("{}: network ready after {:?}", r.label(), r.network_ready_ms));
    r.stats_ready = phone.stats().await?;

    let n = opts.iterations;
    let mut series: BTreeMap<&str, Series> = BTreeMap::new();

    // Notes held by authorities, each fetched once by the phone.
    let mut remote_notes = Vec::new();
    for i in 0..n {
        let h = net.authority(i).create_note(&format!("authority note {i}")).await?;
        net.authority(i + 1).wait_for_note(&h, Duration::from_secs(60)).await?;
        remote_notes.push(hash_b64(&h));
    }

    for i in 0..n {
        let s = series.entry("create note (local commit)").or_default();
        s.record(async { phone_ms(&phone.cmd(&Cmd::CreateNote { text: format!("note {i}") }).await?) }.await);

        let s = series.entry("get own note").or_default();
        s.record(async { phone_ms(&phone.cmd(&Cmd::GetNote { hash: own.clone() }).await?) }.await);

        let s = series.entry("get authority's note (first fetch)").or_default();
        s.record(
            async {
                let v = phone.cmd(&Cmd::GetNote { hash: remote_notes[i].clone() }).await?;
                anyhow::ensure!(v["found"].as_bool() == Some(true), "note not found");
                phone_ms(&v)
            }
            .await,
        );

        let s = series.entry("get links (authority's notes)").or_default();
        s.record(
            async {
                let v = phone.cmd(&Cmd::GetNotes { agent: net.authority(i).agent_b64() }).await?;
                anyhow::ensure!(v["count"].as_u64().unwrap_or(0) > 0, "no links");
                phone_ms(&v)
            }
            .await,
        );

        let s = series.entry("remote call phone to authority").or_default();
        s.record(
            async {
                let v = phone
                    .cmd(&Cmd::CallEcho { agent: net.authority(i).agent_b64(), text: format!("hi {i}") })
                    .await?;
                phone_ms(&v)
            }
            .await,
        );

        let s = series.entry("signal round trip phone to authority").or_default();
        s.record(
            async {
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let v = phone.cmd(&Cmd::Ping { agent: net.authority(i).agent_b64(), id }).await?;
                v["rtt_ms"].as_f64().context("no rtt")
            }
            .await,
        );

        let s = series.entry("remote call authority to phone").or_default();
        s.record(
            async {
                let start = Instant::now();
                let reply: String = net
                    .authority(i)
                    .call("call_echo", EchoInput { agent: me.clone(), text: format!("hello {i}") })
                    .await?;
                anyhow::ensure!(reply == format!("hello {i}"), "wrong echo {reply}");
                Ok(start.elapsed().as_secs_f64() * 1000.0)
            }
            .await,
        );

        let s = series.entry("signal round trip authority to phone").or_default();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        s.record(net.authority(i).ping(&me, id, Duration::from_secs(30)).await);
    }

    for k in 0..opts.receipt_samples {
        let created = phone.cmd(&Cmd::CreateNote { text: format!("published {k}") }).await?;
        let created_at = Instant::now();
        let hash = created["hash"].as_str().context("no hash")?.to_string();
        let h = hash_from(&hash)?;
        series
            .entry("phone note visible at an authority")
            .or_default()
            .record(net.authority(k).wait_for_note(&h, Duration::from_secs(120)).await);
        let receipts = async {
            poll(Duration::from_secs(180), Duration::from_millis(100), async || {
                receipts_complete(&mut phone, &hash).await
            })
            .await?;
            Ok(created_at.elapsed().as_secs_f64() * 1000.0)
        }
        .await;
        series
            .entry("validation receipts for every op")
            .or_default()
            .record(receipts);
    }

    for (name, s) in &series {
        for e in s.errors.iter().take(3) {
            r.errors.push(format!("{name}: {e}"));
        }
        r.latency.insert(name.to_string(), s.summary());
    }
    r.stats_after_workload = phone.stats().await?;
    progress(format!("{}: workload done; idling {} s", r.label(), opts.idle_secs));
    let idle_from = match r.network.as_str() {
        "h2hc" => linker_number(&net.linker, "acceptances").await.ok(),
        _ => None,
    };
    tokio::time::sleep(Duration::from_secs(opts.idle_secs)).await;
    r.stats_after_idle = phone.stats().await?;
    if let Some(from) = idle_from {
        r.idle_linker_acceptances = linker_number(&net.linker, "acceptances")
            .await
            .ok()
            .map(|to| to.saturating_sub(from));
    }
    phone.quit().await?;
    Ok(())
}

/// The phone's operations between two full nodes, for comparison.
async fn baseline(authorities: &[Authority], n: usize) -> BTreeMap<String, report::Summary> {
    let (a, b) = (&authorities[0], &authorities[1]);
    let mut series: BTreeMap<&str, Series> = BTreeMap::new();
    for i in 0..n {
        let hash = match b.create_note(&format!("baseline {i}")).await {
            Ok(h) => h,
            Err(e) => {
                series.entry("create note (local commit)").or_default().record(Err(e));
                continue;
            }
        };
        series
            .entry("authority note visible at another authority")
            .or_default()
            .record(a.wait_for_note(&hash, Duration::from_secs(60)).await);
        series.entry("remote call").or_default().record(
            async {
                let start = Instant::now();
                let _: String = a.call("call_echo", EchoInput { agent: b.agent.clone(), text: "hi".into() }).await?;
                Ok(start.elapsed().as_secs_f64() * 1000.0)
            }
            .await,
        );
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        series
            .entry("signal round trip")
            .or_default()
            .record(a.ping(&b.agent, id, Duration::from_secs(30)).await);
    }
    for (name, s) in &series {
        for e in s.errors.iter().take(2) {
            progress(format!("baseline {name}: {e}"));
        }
    }
    series.into_iter().map(|(k, s)| (k.to_string(), s.summary())).collect()
}

fn scenario(name: &str, result: anyhow::Result<(String, BTreeMap<String, f64>)>) -> ScenarioResult {
    match result {
        Ok((detail, metrics)) => ScenarioResult { name: name.into(), passed: true, detail, metrics },
        Err(e) => ScenarioResult { name: name.into(), passed: false, detail: format!("{e:#}"), metrics: BTreeMap::new() },
    }
}

async fn linker_number(linker: &Linker, field: &str) -> anyhow::Result<u64> {
    linker.status().await?[field].as_u64().context("missing status field")
}

/// Whether the linker has durably accepted ops of this action.
async fn linker_accepted(linker: &Linker, action: &str) -> anyhow::Result<bool> {
    Ok(linker.status().await?["recent_actions"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(action))))
}

const SCENARIO_PHONE: &str = "phone-scenarios";

/// Start the scenario phone, fresh or on the data an earlier run left.
async fn scenario_phone(target: &Target, args: &[String], log: &Path, fresh: bool) -> anyhow::Result<Phone> {
    if fresh {
        target.wipe(SCENARIO_PHONE).await?;
    }
    let mut phone = Phone::spawn(target, SCENARIO_PHONE, "cranelift", args, log).await?;
    phone.cmd(if fresh { &Cmd::Install } else { &Cmd::Resume }).await?;
    Ok(phone)
}

type Outcome = anyhow::Result<(String, BTreeMap<String, f64>)>;

async fn rejection(phone: &mut Phone) -> Outcome {
    match phone.cmd(&Cmd::CreateNote { text: "   ".into() }).await {
        Ok(_) => anyhow::bail!("an empty note was accepted"),
        Err(e) if e.to_string().contains("cannot be empty") => Ok((
            "rejected by the phone's own validation before publishing".into(),
            BTreeMap::new(),
        )),
        Err(e) => anyhow::bail!("unexpected error: {e}"),
    }
}

/// Kill the phone the moment the linker holds a new note, so the
/// authorities' receipts arrive while it is gone; restart it and check they
/// are delivered. Returns the phone if it is still running.
async fn offline_receipts(net: &Net, target: &Target, args: &[String], log: &Path, mut phone: Phone) -> (Outcome, Option<Phone>) {
    let created = match phone.cmd(&Cmd::CreateNote { text: "written before going offline".into() }).await {
        Ok(v) => v,
        Err(e) => return (Err(e), Some(phone)),
    };
    let Some(hash) = created["hash"].as_str().map(str::to_string) else {
        return (Err(anyhow::anyhow!("no hash")), Some(phone));
    };
    let accepted = poll(Duration::from_secs(30), Duration::from_millis(2), async || {
        linker_accepted(&net.linker, &hash).await
    })
    .await;
    if let Err(e) = accepted {
        return (Err(e.context("linker never accepted the note")), Some(phone));
    }
    let _ = phone.kill().await;
    let killed_at = Instant::now();

    let result = async {
        let h = hash_from(&hash)?;
        let mut metrics = BTreeMap::new();
        metrics.insert(
            "authority read with phone killed".to_string(),
            net.authority(0).wait_for_note(&h, Duration::from_secs(60)).await?,
        );
        let held = poll(Duration::from_secs(30), Duration::from_millis(50), async || {
            Ok(linker_number(&net.linker, "inbox").await? > 0)
        })
        .await
        .is_ok();
        metrics.insert("phone offline for".into(), killed_at.elapsed().as_secs_f64() * 1000.0);
        let mut back = Phone::spawn(target, SCENARIO_PHONE, "cranelift", args, log).await?;
        back.cmd(&Cmd::Resume).await?;
        let restarted = Instant::now();
        poll(Duration::from_secs(120), Duration::from_millis(100), async || {
            receipts_complete(&mut back, &hash).await
        })
        .await
        .context("receipts never reached the restarted phone")?;
        metrics.insert("receipts complete after restart".into(), restarted.elapsed().as_secs_f64() * 1000.0);
        poll(Duration::from_secs(60), Duration::from_millis(100), async || {
            Ok(linker_number(&net.linker, "inbox").await? == 0)
        })
        .await
        .context("linker inbox not drained after acknowledgement")?;
        anyhow::ensure!(held, "receipts arrived before the kill, so the inbox was not exercised");
        anyhow::Ok((
            "authorities fetched the note from the linker with the phone killed; their receipts waited in the linker's inbox and were delivered after restart".to_string(),
            metrics,
            back,
        ))
    }
    .await;
    match result {
        Ok((detail, metrics, back)) => (Ok((detail, metrics)), Some(back)),
        Err(e) => (Err(e), None),
    }
}

/// Crash the linker, write during the outage, restart it on the same data.
async fn linker_crash(net: &mut Net, phone: &mut Phone) -> Outcome {
    let acceptances = linker_number(&net.linker, "acceptances").await?;
    let connects = phone.stats().await?.link_connects;
    net.linker.kill().await?;
    let during = phone.cmd(&Cmd::CreateNote { text: "written during a linker outage".into() }).await?;
    let hash = hash_from(during["hash"].as_str().context("no hash")?)?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    net.linker.restart().await?;
    let kept = linker_number(&net.linker, "acceptances").await?;
    anyhow::ensure!(kept >= acceptances, "acceptance log shrank from {acceptances} to {kept}");
    let mut metrics = BTreeMap::new();
    metrics.insert(
        "phone reconnected after restart".into(),
        poll(Duration::from_secs(60), Duration::from_millis(50), async || {
            Ok(phone.stats().await?.link_connects > connects)
        })
        .await
        .context("phone did not reconnect")?,
    );
    metrics.insert(
        "outage note visible after restart".into(),
        net.authority(1).wait_for_note(&hash, Duration::from_secs(600)).await?,
    );
    Ok((
        format!("{acceptances} acceptances survived the crash; the note written during the outage was published after reconnecting"),
        metrics,
    ))
}

async fn long_sleep(target: &Target, args: &[String], log: &Path, mut phone: Phone) -> Outcome {
    let created = phone.cmd(&Cmd::CreateNote { text: "written before a long sleep".into() }).await?;
    let hash = created["hash"].as_str().context("no hash")?.to_string();
    tokio::time::sleep(Duration::from_secs(1)).await;
    phone.kill().await?;
    progress("phone killed; sleeping 21 minutes so its agent info expires");
    tokio::time::sleep(Duration::from_secs(21 * 60)).await;
    let mut back = Phone::spawn(target, SCENARIO_PHONE, "cranelift", args, log).await?;
    back.cmd(&Cmd::Resume).await?;
    let mut metrics = BTreeMap::new();
    metrics.insert(
        "receipts complete after waking".into(),
        poll(Duration::from_secs(900), Duration::from_secs(1), async || {
            receipts_complete(&mut back, &hash).await
        })
        .await?,
    );
    let _ = back.quit().await;
    Ok(("receipts reached the phone after a sleep longer than the agent-info lifetime".into(), metrics))
}

/// Correctness scenarios, on H2HC with Cranelift. Each one restarts the
/// phone if the previous one left it dead.
async fn run_scenarios(net: &mut Net, target: &Target, run_dir: &Path, opts: &Opts) -> Vec<ScenarioResult> {
    let mut out = Vec::new();
    let args = net.phone_args("h2hc", "cranelift");
    let log = run_dir.join(format!("{SCENARIO_PHONE}.log"));
    let mut phone = match scenario_phone(target, &args, &log, true).await {
        Ok(p) => p,
        Err(e) => return vec![scenario("start the scenario phone", Err(e))],
    };

    out.push(scenario("invalid note is rejected on the phone", rejection(&mut phone).await));

    let (result, back) = offline_receipts(net, target, &args, &log, phone).await;
    out.push(scenario("receipts wait in the linker while the phone is killed", result));
    let mut phone = match back {
        Some(p) => p,
        None => match scenario_phone(target, &args, &log, false).await {
            Ok(p) => p,
            Err(e) => {
                out.push(scenario("restart the scenario phone", Err(e)));
                return out;
            }
        },
    };

    out.push(scenario("linker crash and restart", linker_crash(net, &mut phone).await));

    if opts.long {
        out.push(scenario("receipts after a 21-minute sleep", long_sleep(target, &args, &log, phone).await));
    } else {
        let _ = phone.quit().await;
    }
    out
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "error".into()))
        .with_writer(std::io::stderr)
        .init();
    let opts = parse()?;
    let root = opts.root.canonicalize()?;
    let stamp = String::from_utf8(std::process::Command::new("date").arg("+%Y%m%d-%H%M%S").output()?.stdout)?
        .trim()
        .to_string();
    let run_dir = root.join("target/bench-runs").join(&stamp);
    std::fs::create_dir_all(&run_dir)?;
    progress(format!("logs in {}", run_dir.display()));

    let happ_path = root.join("tests/fixtures/notes/workdir/notes.happ");
    let happ = std::fs::read(&happ_path).context("run scripts/build-fixtures.sh first")?;
    let linker_bin = root.join("prototypes/h2hc-linker/target/release/h2hc-linker");

    let bootstrap = Bootstrap::start(&run_dir).await?;
    progress(format!("bootstrap and relay at {}", bootstrap.url()));
    let mut authorities = Vec::new();
    for i in 0..opts.authorities {
        authorities.push(Authority::start(i, &run_dir, &bootstrap.url(), &happ).await?);
    }
    let probe = authorities[0].create_note("mesh probe").await?;
    for a in &authorities[1..] {
        a.wait_for_note(&probe, Duration::from_secs(120))
            .await
            .context("authorities did not find each other")?;
    }
    progress(format!("{} authorities connected", authorities.len()));
    let baseline = baseline(&authorities, opts.iterations).await;
    let linker = Linker::start(&linker_bin, &run_dir, &bootstrap.url()).await?;
    progress(format!("linker at {}", linker.ws_url()));
    let mut net = Net { bootstrap, authorities, linker };

    let target = match opts.target.as_str() {
        "android" => {
            let t = Target::Adb {
                serial: opts.serial.clone(),
                remote_dir: "/data/local/tmp/h2hc".into(),
            };
            t.prepare(&root, &happ_path, &[net.linker.port, net.bootstrap.port]).await?;
            t
        }
        _ => Target::Local {
            root: root.clone(),
            happ: happ_path.clone(),
            data_root: std::env::temp_dir().join(format!("h2hc-{stamp}")),
        },
    };

    let mut configs = Vec::new();
    for (network, wasm) in &opts.configs {
        progress(format!("running {network} + {wasm}"));
        configs.push(run_config(&net, &target, &run_dir, network, wasm, &opts).await);
    }
    let scenarios = if opts.scenarios {
        progress("running scenarios");
        run_scenarios(&mut net, &target, &run_dir, &opts).await
    } else {
        Vec::new()
    };

    let host = String::from_utf8(std::process::Command::new("uname").arg("-srm").output()?.stdout)?
        .trim()
        .to_string();
    let report = Report {
        started: stamp.clone(),
        baseline,
        target: target.name().into(),
        host,
        authorities: opts.authorities,
        iterations: opts.iterations,
        configs,
        scenarios,
    };
    let out_dir = root.join("benchmarks/results");
    std::fs::create_dir_all(&out_dir)?;
    let base = out_dir.join(format!("{stamp}-{}", target.name()));
    std::fs::write(base.with_extension("json"), serde_json::to_string_pretty(&report)?)?;
    std::fs::write(base.with_extension("md"), report.markdown())?;
    println!("{}", report.markdown());
    progress(format!("report written to {}.md", base.display()));
    Ok(())
}
