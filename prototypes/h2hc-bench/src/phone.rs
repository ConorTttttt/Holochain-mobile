//! Drives an `h2hc-phone` process, on this machine or on an Android device
//! over adb.

use anyhow::Context;
use h2hc_conductor::control::{Command as PhoneCommand, Ready, Stats, ThreadStats};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(180);

/// Where phone processes run.
#[derive(Clone)]
pub enum Target {
    Local {
        /// The cargo target directory holding the phone builds.
        builds: PathBuf,
        happ: PathBuf,
        /// Short, because Lair's Unix socket lives inside each data directory
        /// and socket paths are limited to about 108 bytes.
        data_root: PathBuf,
    },
    /// The binary and hApp are pushed to `remote_dir`; the phone reaches
    /// this machine's ports through `adb reverse`.
    Adb {
        serial: Option<String>,
        remote_dir: String,
    },
}

impl Target {
    pub fn name(&self) -> &'static str {
        match self {
            Target::Local { .. } => "local",
            Target::Adb { .. } => "android",
        }
    }

    fn adb(serial: &Option<String>) -> Command {
        let mut cmd = Command::new("adb");
        if let Some(s) = serial {
            cmd.args(["-s", s]);
        }
        cmd
    }

    /// Copy both phone builds and the hApp to the device and forward ports.
    pub async fn prepare(&self, builds: &Path, happ: &Path, ports: &[u16]) -> anyhow::Result<()> {
        let Target::Adb { serial, remote_dir } = self else {
            return Ok(());
        };
        let run = |args: Vec<String>| {
            let mut cmd = Self::adb(serial);
            cmd.args(&args);
            async move {
                let out = cmd.output().await?;
                anyhow::ensure!(out.status.success(), "adb {args:?}: {}", String::from_utf8_lossy(&out.stderr));
                anyhow::Ok(())
            }
        };
        run(vec!["shell".into(), format!("mkdir -p {remote_dir}")]).await?;
        for wasm in ["cranelift", "wasmi"] {
            let bin = phone_bin(builds, "aarch64-linux-android", wasm);
            run(vec!["push".into(), bin.display().to_string(), format!("{remote_dir}/h2hc-phone-{wasm}")]).await?;
        }
        run(vec!["push".into(), happ.display().to_string(), format!("{remote_dir}/notes.happ")]).await?;
        run(vec!["shell".into(), format!("chmod 755 {remote_dir}/h2hc-phone-*")]).await?;
        for port in ports {
            run(vec!["reverse".into(), format!("tcp:{port}"), format!("tcp:{port}")]).await?;
        }
        Ok(())
    }

    fn data_dir(&self, name: &str) -> String {
        match self {
            Target::Local { data_root, .. } => data_root.join(name).display().to_string(),
            Target::Adb { remote_dir, .. } => format!("{remote_dir}/{name}"),
        }
    }

    /// Copy a file the phone wrote to this machine.
    pub async fn fetch(&self, remote: &str, local: &Path) -> anyhow::Result<()> {
        match self {
            Target::Local { .. } => {
                std::fs::copy(remote, local)?;
            }
            Target::Adb { serial, .. } => {
                let out = Self::adb(serial).args(["pull", remote, &local.display().to_string()]).output().await?;
                anyhow::ensure!(out.status.success(), "adb pull {remote}: {}", String::from_utf8_lossy(&out.stderr));
            }
        }
        Ok(())
    }

    /// Remove a phone's data so the next run starts fresh.
    pub async fn wipe(&self, name: &str) -> anyhow::Result<()> {
        match self {
            Target::Local { .. } => {
                let _ = std::fs::remove_dir_all(self.data_dir(name));
            }
            Target::Adb { serial, .. } => {
                Self::adb(serial)
                    .args(["shell", &format!("rm -rf {}", self.data_dir(name))])
                    .output()
                    .await?;
            }
        }
        Ok(())
    }
}

/// The next JSON line, skipping anything else on stdout (Lair prints its
/// setup lines there).
async fn next_json(lines: &mut Lines<BufReader<ChildStdout>>, within: Duration) -> anyhow::Result<String> {
    loop {
        let line = tokio::time::timeout(within, lines.next_line())
            .await
            .context("phone did not answer in time")??
            .context("phone exited")?;
        if line.trim_start().starts_with('{') {
            return Ok(line);
        }
    }
}

/// Each WASM engine is a separate build (see h2hc-conductor's features);
/// `builds` is the cargo target directory.
pub fn phone_bin(builds: &Path, triple: &str, wasm: &str) -> PathBuf {
    let dir = match (triple, wasm) {
        ("", "wasmi") => builds.join("wasmi/release"),
        ("", _) => builds.join("release"),
        (t, "wasmi") => builds.join("wasmi").join(t).join("release"),
        (t, _) => builds.join(t).join("release"),
    };
    dir.join("h2hc-phone")
}

pub struct Phone {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    pub ready: Ready,
}

impl Phone {
    /// Start a phone. `args` are h2hc-phone arguments other than the data
    /// directory and hApp, which come from the target.
    pub async fn spawn(target: &Target, name: &str, wasm: &str, args: &[String], log: &Path) -> anyhow::Result<Self> {
        Self::spawn_with_env(target, name, wasm, args, &[], log).await
    }

    /// As [`Phone::spawn`], with extra environment variables for the phone.
    pub async fn spawn_with_env(
        target: &Target,
        name: &str,
        wasm: &str,
        args: &[String],
        env: &[(String, String)],
        log: &Path,
    ) -> anyhow::Result<Self> {
        let data_dir = target.data_dir(name);
        let mut cmd = match target {
            Target::Local { builds, happ, .. } => {
                let mut cmd = Command::new(phone_bin(builds, "", wasm));
                cmd.args(args)
                    .args(["--data-dir", &data_dir, "--happ", &happ.display().to_string()])
                    .envs(env.iter().map(|(k, v)| (k, v)));
                cmd
            }
            Target::Adb { serial, remote_dir } => {
                let mut cmd = Target::adb(serial);
                let env: String = env.iter().map(|(k, v)| format!("{k}={v} ")).collect();
                let line = format!(
                    "cd {remote_dir} && {env}RUST_LOG=warn ./h2hc-phone-{wasm} {} --data-dir {data_dir} --happ {remote_dir}/notes.happ",
                    args.join(" ")
                );
                cmd.args(["shell", &line]);
                cmd
            }
        };
        let log = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .kill_on_drop(true)
            .spawn()
            .context("starting h2hc-phone")?;
        let stdin = child.stdin.take().context("no stdin")?;
        let mut lines = BufReader::new(child.stdout.take().context("no stdout")?).lines();
        let first = next_json(&mut lines, COMMAND_TIMEOUT)
            .await
            .context("phone did not become ready")?;
        let ready: Ready = serde_json::from_str(&first)
            .with_context(|| format!("unexpected first line from phone: {first}"))?;
        Ok(Self {
            child,
            stdin,
            lines,
            ready,
        })
    }

    pub async fn cmd(&mut self, cmd: &PhoneCommand) -> anyhow::Result<Value> {
        self.cmd_within(cmd, COMMAND_TIMEOUT).await
    }

    pub async fn cmd_within(&mut self, cmd: &PhoneCommand, within: Duration) -> anyhow::Result<Value> {
        let mut line = serde_json::to_string(cmd)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        let reply = next_json(&mut self.lines, within)
            .await
            .with_context(|| format!("no reply to {cmd:?}"))?;
        let value: Value = serde_json::from_str(&reply)?;
        if value["ok"].as_bool() == Some(true) {
            Ok(value)
        } else {
            anyhow::bail!("{}", value["error"].as_str().unwrap_or(&reply))
        }
    }

    pub async fn stats(&mut self) -> anyhow::Result<Stats> {
        Ok(serde_json::from_value(self.cmd(&PhoneCommand::Stats).await?)?)
    }

    pub async fn threads(&mut self) -> anyhow::Result<Vec<ThreadStats>> {
        Ok(serde_json::from_value(self.cmd(&PhoneCommand::Threads).await?["threads"].take())?)
    }

    /// Kill without warning, as Android does to a background app.
    pub async fn kill(mut self) -> anyhow::Result<()> {
        self.child.kill().await?;
        Ok(())
    }

    pub async fn quit(mut self) -> anyhow::Result<()> {
        let _ = self.stdin.write_all(b"{\"cmd\":\"quit\"}\n").await;
        let _ = tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await;
        let _ = self.child.kill().await;
        Ok(())
    }
}
