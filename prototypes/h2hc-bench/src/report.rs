//! Results, written as JSON for comparison across runs and Markdown to read.

use h2hc_conductor::control::Stats;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Distribution of one measured operation, in milliseconds.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub n: usize,
    pub failures: usize,
    pub p50: f64,
    pub p95: f64,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
}

impl Summary {
    pub fn of(samples: &[f64], failures: usize) -> Self {
        if samples.is_empty() {
            return Self {
                failures,
                ..Default::default()
            };
        }
        let mut s = samples.to_vec();
        s.sort_by(|a, b| a.total_cmp(b));
        let pct = |p: f64| s[((p * (s.len() - 1) as f64).round() as usize).min(s.len() - 1)];
        Self {
            n: s.len(),
            failures,
            p50: pct(0.5),
            p95: pct(0.95),
            min: s[0],
            max: s[s.len() - 1],
            mean: s.iter().sum::<f64>() / s.len() as f64,
        }
    }
}

/// Collects samples for one operation, keeping failures rather than hiding them.
#[derive(Default)]
pub struct Series {
    pub samples: Vec<f64>,
    pub errors: Vec<String>,
}

impl Series {
    pub fn record(&mut self, result: anyhow::Result<f64>) {
        match result {
            Ok(ms) => self.samples.push(ms),
            Err(e) => self.errors.push(e.to_string()),
        }
    }

    pub fn summary(&self) -> Summary {
        Summary::of(&self.samples, self.errors.len())
    }
}

#[derive(Debug, Default, Serialize)]
pub struct ConfigResult {
    pub network: String,
    pub wasm: String,
    pub startup_ms: f64,
    pub install_ms: f64,
    pub enable_ms: f64,
    pub first_call_ms: f64,
    /// From the app being enabled until the phone could read an authority's note.
    pub network_ready_ms: Option<f64>,
    pub latency: BTreeMap<String, Summary>,
    pub stats_ready: Stats,
    pub stats_after_workload: Stats,
    pub stats_after_idle: Stats,
    pub idle_secs: u64,
    /// Publish batches the linker accepted while the phone sat idle (H2HC):
    /// the conductor republishing ops that lack enough receipts.
    pub idle_linker_acceptances: Option<u64>,
    /// Protocol frames by kind while the phone sat idle (H2HC).
    pub idle_frames: BTreeMap<String, u64>,
    pub errors: Vec<String>,
}

impl ConfigResult {
    pub fn label(&self) -> String {
        format!("{} + {}", self.network, self.wasm)
    }

    pub fn idle_cpu_ms_per_min(&self) -> f64 {
        let before = self.stats_after_workload.cpu_user_ms + self.stats_after_workload.cpu_sys_ms;
        let after = self.stats_after_idle.cpu_user_ms + self.stats_after_idle.cpu_sys_ms;
        per_min(after.saturating_sub(before), self.idle_secs)
    }

    pub fn idle_link_bytes_per_min(&self) -> f64 {
        let before = self.stats_after_workload.link_sent + self.stats_after_workload.link_received;
        let after = self.stats_after_idle.link_sent + self.stats_after_idle.link_received;
        per_min(after.saturating_sub(before), self.idle_secs)
    }
}

fn per_min(delta: u64, secs: u64) -> f64 {
    if secs == 0 {
        0.0
    } else {
        delta as f64 * 60.0 / secs as f64
    }
}

#[derive(Debug, Serialize)]
pub struct ScenarioResult {
    pub name: String,
    pub passed: bool,
    pub detail: String,
    pub metrics: BTreeMap<String, f64>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub started: String,
    /// The same operations between two full-arc authorities, for comparison.
    pub baseline: BTreeMap<String, Summary>,
    pub target: String,
    pub host: String,
    pub authorities: usize,
    pub iterations: usize,
    pub configs: Vec<ConfigResult>,
    pub scenarios: Vec<ScenarioResult>,
}

fn fmt_ms(v: f64) -> String {
    if v >= 1000.0 {
        format!("{:.2} s", v / 1000.0)
    } else {
        format!("{v:.1} ms")
    }
}

fn mb(kb: u64) -> String {
    format!("{:.0} MB", kb as f64 / 1024.0)
}

impl Report {
    pub fn markdown(&self) -> String {
        let mut md = String::new();
        let _ = writeln!(md, "# H2HC phone benchmark\n");
        let _ = writeln!(
            md,
            "Run {} on {} ({}). {} full-arc authorities, {} iterations per operation.\n",
            self.started, self.target, self.host, self.authorities, self.iterations
        );

        let _ = writeln!(md, "## Startup\n");
        let _ = writeln!(md, "| Configuration | Process start to conductor | Install | Enable | First zome call | Network ready |");
        let _ = writeln!(md, "|---|---|---|---|---|---|");
        for c in &self.configs {
            let _ = writeln!(
                md,
                "| {} | {} | {} | {} | {} | {} |",
                c.label(),
                fmt_ms(c.startup_ms),
                fmt_ms(c.install_ms),
                fmt_ms(c.enable_ms),
                fmt_ms(c.first_call_ms),
                c.network_ready_ms.map(fmt_ms).unwrap_or_else(|| "never".into())
            );
        }

        let mut ops: Vec<&String> = self
            .configs
            .iter()
            .flat_map(|c| c.latency.keys())
            .collect();
        ops.sort();
        ops.dedup();
        let _ = writeln!(md, "\n## Latency (p50 / p95)\n");
        let _ = write!(md, "| Operation |");
        for c in &self.configs {
            let _ = write!(md, " {} |", c.label());
        }
        let _ = write!(md, "\n|---|");
        for _ in &self.configs {
            let _ = write!(md, "---|");
        }
        let _ = writeln!(md);
        for op in ops {
            let _ = write!(md, "| {op} |");
            for c in &self.configs {
                let cell = match c.latency.get(op) {
                    Some(s) if s.n > 0 => {
                        let fails = if s.failures > 0 {
                            format!(" ({} failed)", s.failures)
                        } else {
                            String::new()
                        };
                        format!("{} / {}{fails}", fmt_ms(s.p50), fmt_ms(s.p95))
                    }
                    Some(s) => format!("all {} failed", s.failures),
                    None => "–".into(),
                };
                let _ = write!(md, " {cell} |");
            }
            let _ = writeln!(md);
        }

        if !self.baseline.is_empty() {
            let _ = writeln!(md, "\nBaseline between two full-arc authorities (Kitsune2, Cranelift) on the same network:\n");
            let _ = writeln!(md, "| Operation | p50 / p95 |");
            let _ = writeln!(md, "|---|---|");
            for (op, s) in &self.baseline {
                let _ = writeln!(md, "| {op} | {} / {} |", fmt_ms(s.p50), fmt_ms(s.p95));
            }
        }

        let _ = writeln!(md, "\n## Resources (phone process)\n");
        let _ = writeln!(md, "| Configuration | RSS ready | RSS after workload | Peak RSS | Threads | Idle CPU | Idle link traffic | Republish batches while idle | Data on disk |");
        let _ = writeln!(md, "|---|---|---|---|---|---|---|---|---|");
        for c in &self.configs {
            let link = if c.network == "h2hc" {
                format!("{:.1} KB/min", c.idle_link_bytes_per_min() / 1024.0)
            } else {
                "not measured".into()
            };
            let republish = c
                .idle_linker_acceptances
                .map(|n| format!("{n} in {} s", c.idle_secs))
                .unwrap_or_else(|| "not measured".into());
            let _ = writeln!(
                md,
                "| {} | {} | {} | {} | {} | {:.0} ms/min | {} | {} | {:.1} MB |",
                c.label(),
                mb(c.stats_ready.rss_kb),
                mb(c.stats_after_workload.rss_kb),
                mb(c.stats_after_idle.peak_rss_kb),
                c.stats_after_idle.threads,
                c.idle_cpu_ms_per_min(),
                link,
                republish,
                c.stats_after_idle.data_bytes as f64 / 1_048_576.0
            );
        }

        let h2hc: Vec<&ConfigResult> = self.configs.iter().filter(|c| c.network == "h2hc").collect();
        let mut kinds: Vec<&String> = h2hc.iter().flat_map(|c| c.idle_frames.keys()).collect();
        kinds.sort();
        kinds.dedup();
        if !kinds.is_empty() {
            let _ = writeln!(md, "\n## Linker traffic while idle\n");
            let _ = write!(md, "| Frames in {} s |", self.configs.first().map(|c| c.idle_secs).unwrap_or(0));
            for c in &h2hc {
                let _ = write!(md, " {} |", c.label());
            }
            let _ = write!(md, "\n|---|");
            for _ in &h2hc {
                let _ = write!(md, "---|");
            }
            let _ = writeln!(md);
            for kind in kinds {
                let _ = write!(md, "| {kind} |");
                for c in &h2hc {
                    let _ = write!(md, " {} |", c.idle_frames.get(kind).copied().unwrap_or(0));
                }
                let _ = writeln!(md);
            }
        }

        if !self.scenarios.is_empty() {
            let _ = writeln!(md, "\n## Scenarios\n");
            let _ = writeln!(md, "| Scenario | Result | Detail |");
            let _ = writeln!(md, "|---|---|---|");
            for s in &self.scenarios {
                let metrics: Vec<String> = s
                    .metrics
                    .iter()
                    .map(|(k, v)| format!("{k} {}", fmt_ms(*v)))
                    .collect();
                let detail = if metrics.is_empty() {
                    s.detail.clone()
                } else {
                    format!("{} ({})", s.detail, metrics.join(", "))
                };
                let _ = writeln!(
                    md,
                    "| {} | {} | {} |",
                    s.name,
                    if s.passed { "pass" } else { "FAIL" },
                    detail
                );
            }
        }

        let errors: Vec<String> = self
            .configs
            .iter()
            .flat_map(|c| c.errors.iter().map(move |e| format!("{}: {e}", c.label())))
            .collect();
        if !errors.is_empty() {
            let _ = writeln!(md, "\n## Errors\n");
            for e in errors.iter().take(40) {
                let _ = writeln!(md, "- {e}");
            }
        }
        md
    }
}
