/// Serialization Fixture Generator (Holochain 0.7)
///
/// This test generates known-good byte fixtures for `DhtOp::ChainOp` — the
/// wire-level ops the linker relays between conductors — covering all 9
/// `ChainOp` variants plus a `Dna`-action fixture (which exercises
/// `prev_action: None` -> msgpack nil, since `Dna` is the only action
/// variant without a preceding action).
///
/// Every printed base64 line is consumed verbatim by
/// `packages/core/test/serialization-fixtures.test.ts` (see holo-web-conductor
/// Task 12/13), so labels must stay stable and match the `ChainOp` variant
/// names exactly.
///
/// Run with: cargo test --test validation_op_fixtures -- --nocapture
use base64::{engine::general_purpose::STANDARD, Engine};
use holo_hash::{ActionHash, AgentPubKey, AnyLinkableHash, DnaHash, EntryHash};
use holochain_types::prelude::*;

// ---------------------------------------------------------------------------
// Deterministic inputs
//
// Same seeds/signature/timestamp the file already used pre-0.7, so the
// TypeScript-side builders (Task 12) don't need their literal values to
// change, only the surrounding 0.7 shapes:
//   0x11 -> author (every action)
//   0x22 -> prev_action / an action-hash-shaped reference field
//   0x33 -> an entry-hash-shaped reference field ("the entry")
//   0x44 -> an entry/action-hash-shaped reference field ("the target")
//   0x55 -> Delete.deletes_entry_address
//   0xbb x64 -> signature
//   1704067200000000 -> timestamp (2024-01-01 00:00:00 UTC, in micros)
// ---------------------------------------------------------------------------

fn make_hash32(seed: u8) -> [u8; 32] {
    [seed; 32]
}

fn make_agent_pubkey(seed: u8) -> AgentPubKey {
    AgentPubKey::from_raw_32(make_hash32(seed).to_vec())
}

fn make_action_hash(seed: u8) -> ActionHash {
    ActionHash::from_raw_32(make_hash32(seed).to_vec())
}

fn make_entry_hash(seed: u8) -> EntryHash {
    EntryHash::from_raw_32(make_hash32(seed).to_vec())
}

// New in 0.7: `ActionData::Dna` carries a `DnaHash`. No prior TS builder
// pins a seed for this field, so it uses its own seed (0x99) rather than one
// of the five shared seeds above.
fn make_dna_hash(seed: u8) -> DnaHash {
    DnaHash::from_raw_32(make_hash32(seed).to_vec())
}

fn make_signature() -> Signature {
    Signature([0xbb; 64])
}

fn fixed_timestamp() -> Timestamp {
    // 2024-01-01 00:00:00 UTC in microseconds
    Timestamp(1704067200000000)
}

fn make_entry() -> Entry {
    Entry::App(AppEntryBytes(SerializedBytes::from(UnsafeBytes::from(
        vec![1u8, 2, 3, 4, 5],
    ))))
}

// ---------------------------------------------------------------------------
// Action builders (0.7 shape: `Action { header: ActionHeader, data: ActionData }`)
// ---------------------------------------------------------------------------

fn make_create_action() -> Action {
    Action {
        header: ActionHeader {
            author: make_agent_pubkey(0x11),
            timestamp: fixed_timestamp(),
            action_seq: 5,
            prev_action: Some(make_action_hash(0x22)),
        },
        data: ActionData::Create(CreateData {
            entry_type: EntryType::App(AppEntryDef {
                entry_index: 0.into(),
                zome_index: 0.into(),
                visibility: EntryVisibility::Public,
            }),
            entry_hash: make_entry_hash(0x33),
        }),
    }
}

fn make_update_action() -> Action {
    Action {
        header: ActionHeader {
            author: make_agent_pubkey(0x11),
            timestamp: fixed_timestamp(),
            action_seq: 6,
            prev_action: Some(make_action_hash(0x22)),
        },
        data: ActionData::Update(UpdateData {
            original_action_address: make_action_hash(0x22),
            original_entry_address: make_entry_hash(0x33),
            entry_type: EntryType::App(AppEntryDef {
                entry_index: 0.into(),
                zome_index: 0.into(),
                visibility: EntryVisibility::Public,
            }),
            entry_hash: make_entry_hash(0x44),
        }),
    }
}

fn make_delete_action() -> Action {
    Action {
        header: ActionHeader {
            author: make_agent_pubkey(0x11),
            timestamp: fixed_timestamp(),
            action_seq: 7,
            prev_action: Some(make_action_hash(0x22)),
        },
        data: ActionData::Delete(DeleteData {
            deletes_address: make_action_hash(0x44),
            deletes_entry_address: make_entry_hash(0x55),
        }),
    }
}

fn make_create_link_action() -> Action {
    Action {
        header: ActionHeader {
            author: make_agent_pubkey(0x11),
            timestamp: fixed_timestamp(),
            action_seq: 8,
            prev_action: Some(make_action_hash(0x22)),
        },
        data: ActionData::CreateLink(CreateLinkData {
            base_address: AnyLinkableHash::from(make_entry_hash(0x33)),
            target_address: AnyLinkableHash::from(make_entry_hash(0x44)),
            zome_index: 0.into(),
            link_type: 0.into(),
            tag: LinkTag::new(vec![0x01, 0x02, 0x03]),
        }),
    }
}

fn make_delete_link_action() -> Action {
    Action {
        header: ActionHeader {
            author: make_agent_pubkey(0x11),
            timestamp: fixed_timestamp(),
            action_seq: 9,
            prev_action: Some(make_action_hash(0x22)),
        },
        data: ActionData::DeleteLink(DeleteLinkData {
            base_address: AnyLinkableHash::from(make_entry_hash(0x33)),
            link_add_address: make_action_hash(0x44),
        }),
    }
}

fn make_dna_action() -> Action {
    Action {
        header: ActionHeader {
            author: make_agent_pubkey(0x11),
            timestamp: fixed_timestamp(),
            action_seq: 0,
            // The only action variant with no preceding action: this is what
            // exercises msgpack nil encoding for `prev_action`.
            prev_action: None,
        },
        data: ActionData::Dna(DnaData {
            dna_hash: make_dna_hash(0x99),
        }),
    }
}

// ---------------------------------------------------------------------------
// Fixture printing / round-trip helper
// ---------------------------------------------------------------------------

/// Wrap `op` as `DhtOp::ChainOp`, encode with `holochain_serialized_bytes`,
/// print the base64 fixture labeled by the `ChainOp` variant name, and
/// assert the encoding round-trips through decode.
fn print_fixture(label: &str, op: ChainOp) {
    let dht_op = DhtOp::from(op);
    let bytes = holochain_serialized_bytes::encode(&dht_op).unwrap();
    println!("{label}: {}", STANDARD.encode(&bytes));

    let decoded: DhtOp = holochain_serialized_bytes::decode(&bytes).unwrap();
    assert_eq!(decoded, dht_op, "{label} op failed to round-trip");
}

// ---------------------------------------------------------------------------
// The 9 ChainOp variant fixtures
// ---------------------------------------------------------------------------

#[test]
fn generate_create_record_op_fixture() {
    let action = make_create_action();
    let action_hash = ActionHash::with_data_sync(&action);
    let signed = SignedAction::new(action, make_signature());
    let op = ChainOp::CreateRecord(signed, OpEntry::Present(make_entry()));
    let op_hash = op.to_hash();

    println!(
        "Create ActionHash (39 bytes): {}",
        STANDARD.encode(action_hash.get_raw_39())
    );
    println!(
        "CreateRecord DhtOpHash (39 bytes): {}",
        STANDARD.encode(op_hash.get_raw_39())
    );

    print_fixture("CreateRecord", op);
}

#[test]
fn generate_create_entry_op_fixture() {
    let action = make_create_action();
    let signed = SignedAction::new(action, make_signature());
    let op = ChainOp::CreateEntry(signed, OpEntry::Present(make_entry()));
    print_fixture("CreateEntry", op);
}

#[test]
fn generate_agent_activity_op_fixture() {
    let action = make_create_action();
    let signed = SignedAction::new(action, make_signature());
    let op = ChainOp::AgentActivity(signed);
    let op_hash = op.to_hash();

    println!(
        "AgentActivity DhtOpHash (39 bytes): {}",
        STANDARD.encode(op_hash.get_raw_39())
    );

    print_fixture("AgentActivity", op);
}

#[test]
fn generate_update_entry_op_fixture() {
    let action = make_update_action();
    let signed = SignedAction::new(action, make_signature());
    let op = ChainOp::UpdateEntry(signed, OpEntry::Present(make_entry()));
    print_fixture("UpdateEntry", op);
}

#[test]
fn generate_update_record_op_fixture() {
    let action = make_update_action();
    let signed = SignedAction::new(action, make_signature());
    let op = ChainOp::UpdateRecord(signed, OpEntry::Present(make_entry()));
    print_fixture("UpdateRecord", op);
}

#[test]
fn generate_delete_entry_op_fixture() {
    let action = make_delete_action();
    let signed = SignedAction::new(action, make_signature());
    let op = ChainOp::DeleteEntry(signed);
    print_fixture("DeleteEntry", op);
}

#[test]
fn generate_delete_record_op_fixture() {
    let action = make_delete_action();
    let signed = SignedAction::new(action, make_signature());
    let op = ChainOp::DeleteRecord(signed);
    print_fixture("DeleteRecord", op);
}

#[test]
fn generate_create_link_op_fixture() {
    let action = make_create_link_action();
    let signed = SignedAction::new(action, make_signature());
    let op = ChainOp::CreateLink(signed);
    print_fixture("CreateLink", op);
}

#[test]
fn generate_delete_link_op_fixture() {
    let action = make_delete_link_action();
    let signed = SignedAction::new(action, make_signature());
    let op = ChainOp::DeleteLink(signed);
    print_fixture("DeleteLink", op);
}

// ---------------------------------------------------------------------------
// Dna action fixture: exercises `prev_action: None` -> msgpack nil
// ---------------------------------------------------------------------------

#[test]
fn generate_dna_action_fixture() {
    let action = make_dna_action();
    assert_eq!(action.header.prev_action, None);

    let bytes = holochain_serialized_bytes::encode(&action).unwrap();
    println!("Dna: {}", STANDARD.encode(&bytes));

    let decoded: Action = holochain_serialized_bytes::decode(&bytes).unwrap();
    assert_eq!(decoded, action, "Dna action failed to round-trip");
    assert_eq!(decoded.header.prev_action, None);
}
