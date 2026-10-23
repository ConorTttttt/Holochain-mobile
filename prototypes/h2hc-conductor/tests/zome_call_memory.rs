//! Zome calls must not accumulate memory. Each call instantiates its zome in
//! a fresh wasmer store, and wasmer 7.1.0's wasmi backend leaked every one,
//! linear memory included (`patches/wasmer-7.1.0-wasmi-store-ownership.patch`).
//! Run it on both engines:
//!
//! ```text
//! cargo test -p h2hc-conductor --test zome_call_memory
//! cargo test -p h2hc-conductor --test zome_call_memory --no-default-features --features wasmi --target-dir target/wasmi
//! ```
//!
//! The only test in its binary, so no other test's allocations land in the
//! heap figures.
#![cfg(all(target_os = "linux", target_env = "gnu"))]

mod common;

use common::*;
use holochain::prelude::*;

/// Bytes the allocator has handed out and not had back.
fn live_heap() -> usize {
    let m = unsafe { libc::mallinfo2() };
    m.uordblks + m.hblkhd
}

/// A write with its validation callbacks, a read and a pure call: several
/// zome instances.
async fn round(phone: &Phone, agent: &AgentPubKey, cell_id: &CellId, i: usize) {
    let c = &phone.conductor;
    let hash: ActionHash = c
        .easy_call_zome(agent, None, cell_id.clone(), "notes", "create_note", format!("note {i}"))
        .await
        .unwrap();
    let _: Option<Record> = c
        .easy_call_zome(agent, None, cell_id.clone(), "notes", "get_note", hash)
        .await
        .unwrap();
    let _: String = c
        .easy_call_zome(agent, None, cell_id.clone(), "notes", "echo", "hi")
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn zome_calls_do_not_accumulate_memory() {
    let mut phone = phone().await;
    let (agent, cell_id) = install(&mut phone).await;
    // Fill caches: compiled modules, database connections and page caches.
    for i in 0..10 {
        round(&phone, &agent, &cell_id, i).await;
    }
    let before = live_heap();
    for i in 10..50 {
        round(&phone, &agent, &cell_id, i).await;
    }
    let grown = live_heap().saturating_sub(before);
    eprintln!("live heap grew {} KiB over 40 rounds", grown >> 10);
    // Leaking each zome instance costs about 2 MB, so several hundred MB here.
    assert!(grown < 32 << 20, "live heap grew {} MiB over 40 rounds", grown >> 20);
}
