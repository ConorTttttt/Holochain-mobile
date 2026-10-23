//! Line-based JSON control protocol for `h2hc-phone`: one [`Command`] per
//! line on stdin, one JSON object per line on stdout. It works the same over
//! a local pipe and over `adb shell`.

use serde::{Deserialize, Serialize};

/// Hashes and agent keys travel as Holochain's base64 strings (`uhC...`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Install and enable the fixture hApp.
    Install,
    /// Attach to the hApp installed by an earlier run on the same data.
    Resume,
    CreateNote { text: String },
    GetNote { hash: String },
    GetNotes { agent: String },
    /// Call `echo` on another agent through `call_remote`.
    CallEcho { agent: String, text: String },
    /// Send a ping signal and wait for the pong.
    Ping { agent: String, id: u64 },
    /// Validation receipts the phone holds for an action's ops.
    Receipts { hash: String },
    /// Process resource use.
    Stats,
    Quit,
}

/// The first line `h2hc-phone` prints, once the conductor is built.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ready {
    pub event: String,
    pub network: String,
    pub wasm: String,
    /// `lair` (Lair with a Unix socket and passphrase) or `memory`.
    #[serde(default)]
    pub keystore: String,
    /// From process start to a running conductor.
    pub startup_ms: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub rss_kb: u64,
    pub peak_rss_kb: u64,
    pub cpu_user_ms: u64,
    pub cpu_sys_ms: u64,
    pub threads: u64,
    pub data_bytes: u64,
    /// Bytes over the linker connection (H2HC only).
    pub link_sent: u64,
    pub link_received: u64,
    /// Connections opened to the linker, including reconnects.
    pub link_connects: u64,
}
