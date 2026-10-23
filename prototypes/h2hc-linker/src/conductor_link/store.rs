//! Durable state for conductor links: accepted ops, the append-only
//! acceptance log, and the validation-receipt inbox.
//!
//! A receipt is signed inside the transaction that stores the ops and is
//! returned only after that transaction commits, so a receipt never exists
//! for data the linker has not durably stored.

use ed25519_dalek::SigningKey;
use h2hc_protocol::*;
use holochain_types::prelude::{AgentPubKey, DhtOpHash, DnaHash, Timestamp};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

/// An op that passed admission, ready to store.
pub(crate) struct AdmittedOp {
    pub hash: DhtOpHash,
    pub bytes: Vec<u8>,
}

pub(crate) struct AcceptanceStore {
    conn: Mutex<Connection>,
    key: SigningKey,
    linker: LinkerId,
    retention: Duration,
}

fn internal(e: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::new(ErrorKind::Internal, e.to_string())
}

impl AcceptanceStore {
    pub fn open(dir: &Path, key: SigningKey, retention: Duration) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let conn = Connection::open(dir.join("conductor-link.sqlite3"))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             CREATE TABLE IF NOT EXISTS accepted_op (
                 op_hash BLOB PRIMARY KEY,
                 dna BLOB NOT NULL,
                 op BLOB NOT NULL,
                 retain_until INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS acceptance_log (
                 seq INTEGER PRIMARY KEY AUTOINCREMENT,
                 dna BLOB NOT NULL,
                 author BLOB NOT NULL,
                 receipt BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS inbox (
                 seq INTEGER PRIMARY KEY AUTOINCREMENT,
                 dna BLOB NOT NULL,
                 agent BLOB NOT NULL,
                 item BLOB NOT NULL
             );",
        )?;
        let linker = LinkerId::from_signing_key(&key);
        Ok(Self {
            conn: Mutex::new(conn),
            key,
            linker,
            retention,
        })
    }

    pub fn linker(&self) -> LinkerId {
        self.linker
    }

    pub fn retention(&self) -> Duration {
        self.retention
    }

    /// Store the ops, append to the acceptance log and sign the receipt, all
    /// in one transaction.
    pub fn accept(
        &self,
        dna_hash: &DnaHash,
        author: &AgentPubKey,
        ops: &[AdmittedOp],
    ) -> Result<AcceptanceReceipt, ProtocolError> {
        let accepted_at = Timestamp::now();
        let retain_until = (accepted_at + self.retention).map_err(internal)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(internal)?;
        for op in ops {
            tx.execute(
                "INSERT INTO accepted_op (op_hash, dna, op, retain_until) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(op_hash) DO UPDATE SET retain_until = MAX(retain_until, excluded.retain_until)",
                params![
                    op.hash.get_raw_39(),
                    dna_hash.get_raw_39(),
                    op.bytes,
                    retain_until.as_micros()
                ],
            )
            .map_err(internal)?;
        }
        tx.execute(
            "INSERT INTO acceptance_log (dna, author, receipt) VALUES (?1, ?2, x'')",
            params![dna_hash.get_raw_39(), author.get_raw_39()],
        )
        .map_err(internal)?;
        let seq = tx.last_insert_rowid() as u64;
        let receipt = AcceptanceReceipt::sign(
            &AcceptanceBody {
                protocol_version: PROTOCOL_VERSION,
                linker: self.linker,
                dna_hash: dna_hash.clone(),
                author: author.clone(),
                op_hashes: ops.iter().map(|op| op.hash.clone()).collect(),
                seq,
                accepted_at,
                retain_until,
            },
            &self.key,
        )?;
        let encoded = rmp_serde::to_vec_named(&receipt).map_err(internal)?;
        tx.execute(
            "UPDATE acceptance_log SET receipt = ?1 WHERE seq = ?2",
            params![encoded, seq as i64],
        )
        .map_err(internal)?;
        tx.commit().map_err(internal)?;
        Ok(receipt)
    }

    /// Ops still within their retention period, with how long is left.
    pub fn retained_ops(&self) -> anyhow::Result<Vec<(Vec<u8>, Duration)>> {
        let now = Timestamp::now().as_micros();
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT op, retain_until FROM accepted_op WHERE retain_until > ?1")?;
        let rows = stmt.query_map(params![now], |row| {
            let until: i64 = row.get(1)?;
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                Duration::from_micros((until - now) as u64),
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn acceptance_count(&self) -> anyhow::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM acceptance_log", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    /// Keep an item for an agent until the phone acknowledges it.
    pub fn inbox_push(
        &self,
        dna_hash: &DnaHash,
        agent: &AgentPubKey,
        item: &InboxItem,
    ) -> Result<u64, ProtocolError> {
        let bytes = rmp_serde::to_vec_named(item).map_err(internal)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO inbox (dna, agent, item) VALUES (?1, ?2, ?3)",
            params![dna_hash.get_raw_39(), agent.get_raw_39(), bytes],
        )
        .map_err(internal)?;
        Ok(conn.last_insert_rowid() as u64)
    }

    pub fn inbox_for(
        &self,
        dna_hash: &DnaHash,
        agent: &AgentPubKey,
    ) -> Result<Vec<(u64, InboxItem)>, ProtocolError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT seq, item FROM inbox WHERE dna = ?1 AND agent = ?2 ORDER BY seq")
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![dna_hash.get_raw_39(), agent.get_raw_39()], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(internal)?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, bytes) = row.map_err(internal)?;
            out.push((seq, rmp_serde::from_slice(&bytes).map_err(internal)?));
        }
        Ok(out)
    }

    /// The DNA and agent an inbox item belongs to, if it is still held.
    pub fn inbox_owner(&self, seq: u64) -> Result<Option<(DnaHash, AgentPubKey)>, ProtocolError> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(Vec<u8>, Vec<u8>)> = conn
            .query_row(
                "SELECT dna, agent FROM inbox WHERE seq = ?1",
                params![seq as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(internal)?;
        Ok(row.map(|(dna, agent)| (DnaHash::from_raw_39(dna), AgentPubKey::from_raw_39(agent))))
    }

    pub fn inbox_ack(&self, seq: u64) -> Result<(), ProtocolError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM inbox WHERE seq = ?1", params![seq as i64])
            .map_err(internal)?;
        Ok(())
    }

    pub fn inbox_len(&self) -> anyhow::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM inbox", [], |r| r.get(0))?;
        Ok(n as u64)
    }
}
