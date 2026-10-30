//! Conductor-facing protocol between a zero-arc phone conductor and an H2HC
//! linker, version 1.
//!
//! The linker's REST API was built for browser clients and flattens what
//! `HcP2p` returns. This protocol carries Holochain's own wire types
//! unchanged, labels where every answer came from, and gives validation
//! receipts a durable, acknowledged delivery path.
//!
//! One connection carries [`Frame`]s in both directions:
//!
//! | Direction | Frames |
//! |---|---|
//! | phone to linker | `Hello`, `Request`, `Reply`, `Ack` |
//! | linker to phone | `Welcome`, `Response`, `Call`, `Notify`, `Deliver` |
//!
//! Request ids are scoped to the side that allocated them: `Request`/`Response`
//! ids belong to the phone, `Call`/`Reply` ids belong to the linker.

pub mod codec;
pub mod transport;
mod error;
mod receipt;

pub use error::*;
pub use receipt::*;

use holochain_types::prelude::*;
use serde::{Deserialize, Serialize};

/// The protocol version this crate implements.
pub const PROTOCOL_VERSION: u32 = 1;

/// Longest agent-info lifetime a phone will sign.
///
/// Kitsune2 0.5 signs agent infos for 20 minutes. A linker asking for longer
/// would hold a credential that keeps the phone routable after it stops
/// re-signing, so the phone refuses. One minute of slack covers clock skew.
pub const MAX_AGENT_INFO_LIFETIME_MS: u64 = 21 * 60 * 1000;

/// Every message on a connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Frame {
    /// Phone to linker. Must be the first frame on a connection.
    Hello(Hello),
    /// Linker to phone. Reply to `Hello`.
    Welcome(Welcome),
    /// Phone to linker.
    Request { id: u64, body: Request },
    /// Linker to phone. Answers the `Request` with the same id.
    Response {
        id: u64,
        body: Result<Response, ProtocolError>,
    },
    /// Linker to phone. Needs a `Reply` with the same id.
    Call { id: u64, body: Call },
    /// Phone to linker. Answers the `Call` with the same id.
    Reply {
        id: u64,
        body: Result<Reply, ProtocolError>,
    },
    /// Linker to phone. Fire and forget; a hint, never the only copy of data.
    Notify(Notification),
    /// Linker to phone. Durable: the linker redelivers until it sees `Ack`.
    Deliver { seq: u64, item: InboxItem },
    /// Phone to linker. The `Deliver` with this `seq` is durably handled.
    Ack { seq: u64 },
}

/// Opens a connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    /// Protocol versions the phone can speak.
    pub versions: Vec<u32>,
    /// Credential from the linker's existing session auth flow.
    pub session_token: Option<String>,
}

/// Accepts a connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Welcome {
    /// The version both sides will use.
    pub version: u32,
    /// The key the linker signs acceptance receipts with.
    pub linker: LinkerId,
}

/// Network controls copied from `NetworkRequestOptions`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryOptions {
    /// Number of authorities to ask in parallel.
    pub remote_agent_count: u8,
    /// Overall timeout; `None` lets the linker use its default.
    pub timeout_ms: Option<u64>,
    /// Return the first response instead of every response.
    pub as_race: bool,
}

/// Remote controls copied from `holochain_p2p::event::GetActivityOptions`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivityOptions {
    pub include_valid_activity: bool,
    pub include_rejected_activity: bool,
    pub include_warrants: bool,
    pub include_full_records: bool,
}

/// Phone to linker requests, one per forwarded `HcP2p` method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Represent this agent in the DNA's space. The linker joins Kitsune2 as
    /// a zero-arc proxy agent and asks the phone to sign its agent info.
    Join {
        dna_hash: DnaHash,
        agent: AgentPubKey,
    },
    /// Stop representing this agent.
    Leave {
        dna_hash: DnaHash,
        agent: AgentPubKey,
    },
    /// Take custody of ops and publish them. Answered with
    /// [`Response::PublishAccepted`] only after the ops are durably stored.
    Publish(PublishRequest),
    Get {
        dna_hash: DnaHash,
        hash: AnyDhtHash,
        options: QueryOptions,
    },
    GetLinks {
        dna_hash: DnaHash,
        key: WireLinkKey,
        options: QueryOptions,
    },
    CountLinks {
        dna_hash: DnaHash,
        query: WireLinkQuery,
        options: QueryOptions,
    },
    GetAgentActivity {
        dna_hash: DnaHash,
        agent: AgentPubKey,
        filter: ChainQueryFilter,
        activity: ActivityOptions,
        options: QueryOptions,
    },
    /// Fan out to several authorities and return each answer separately,
    /// labelled with its responder. Never merged or raced.
    GetAgentActivityMulti {
        dna_hash: DnaHash,
        agent: AgentPubKey,
        filter: ChainQueryFilter,
        activity: ActivityOptions,
        target_peer_count: u8,
        required_responses: u8,
        timeout_ms: Option<u64>,
    },
    /// Validation depends on this answer, so the linker must only return
    /// authority responses, never its publish buffer.
    MustGetAgentActivity {
        dna_hash: DnaHash,
        author: AgentPubKey,
        filter: ChainFilter,
        options: QueryOptions,
    },
    /// Forward a signed zome call to a remote agent and return its answer.
    CallRemote {
        dna_hash: DnaHash,
        to_agent: AgentPubKey,
        zome_call_params_serialized: ExternIO,
        signature: Signature,
    },
    SendRemoteSignal {
        dna_hash: DnaHash,
        signals: Vec<SignedRemoteSignal>,
    },
    SendRemoteSignalDirect {
        dna_hash: DnaHash,
        to_agents: Vec<AgentPubKey>,
        signal: Vec<u8>,
        from_agent: AgentPubKey,
        signature: Signature,
    },
    /// Stop relaying traffic between the phone and this target.
    Block(BlockRequest),
}

/// Ops for one basis, as `HcP2p::publish` groups them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishRequest {
    pub dna_hash: DnaHash,
    pub author: AgentPubKey,
    pub basis: OpBasis,
    pub ops: Vec<DhtOp>,
}

/// One entry of `HcP2p::send_remote_signal`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedRemoteSignal {
    pub to_agent: AgentPubKey,
    pub zome_call_params_serialized: ExternIO,
    pub signature: Signature,
}

/// A serializable form of `holochain_zome_types::block::Block`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockRequest {
    pub target: BlockTargetId,
    pub reason: BlockTargetReason,
    pub start: Timestamp,
    pub end: Timestamp,
}

/// Linker to phone answers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    /// The request succeeded and has no payload.
    Done,
    PublishAccepted(AcceptanceReceipt),
    Get(Vec<Sourced<WireOps>>),
    GetLinks(Vec<Sourced<WireLinkOps>>),
    CountLinks(Vec<Sourced<CountLinksResponse>>),
    /// Answers both `GetAgentActivity` and `GetAgentActivityMulti`.
    AgentActivity(Vec<Sourced<AgentActivityResponse>>),
    MustGetAgentActivity(Vec<Sourced<MustGetAgentActivityResponse>>),
    CallRemote(SerializedBytes),
}

/// Data plus where it came from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sourced<T> {
    pub source: DataSource,
    pub data: T,
}

/// Where an answer came from. A signed action proves authorship only; this
/// says whose validation judgment, if any, the data carries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DataSource {
    /// Returned by this DHT authority. Judgments inside the data are its own.
    Authority { responder: AgentPubKey },
    /// Served from the linker's publish buffer. No authority has judged it.
    LinkerBuffer,
    /// The linker's cached copy of an earlier authority answer.
    LinkerCache {
        responder: AgentPubKey,
        fetched_at: Timestamp,
    },
}

impl DataSource {
    /// The authority whose judgment the data carries, if any.
    pub fn responder(&self) -> Option<&AgentPubKey> {
        match self {
            DataSource::Authority { responder } | DataSource::LinkerCache { responder, .. } => {
                Some(responder)
            }
            DataSource::LinkerBuffer => None,
        }
    }
}

/// Linker to phone requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Call {
    /// A remote agent is calling a zome function on one of the phone's agents.
    /// The phone verifies the signature and capability as for any zome call.
    CallRemote {
        dna_hash: DnaHash,
        to_agent: AgentPubKey,
        zome_call_params_serialized: ExternIO,
        signature: Signature,
    },
    /// Kitsune2 needs this agent info signed. The phone checks the fields and
    /// signs `serde_json::to_string(&agent_info)`, the bytes Kitsune2 signs.
    SignAgentInfo { agent_info: kitsune2_api::AgentInfo },
}

/// Phone to linker answers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Reply {
    CallRemote(SerializedBytes),
    AgentInfoSignature(Signature),
}

/// Linker to phone hints. A missed notification loses nothing durable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Notification {
    /// Delivered to the phone as a zome call, as `holochain_p2p` does, so the
    /// original signature must be present.
    RemoteSignal {
        dna_hash: DnaHash,
        to_agent: AgentPubKey,
        zome_call_params_serialized: ExternIO,
        signature: Signature,
    },
    RemoteSignalDirect {
        dna_hash: DnaHash,
        to_agent: AgentPubKey,
        signal: Vec<u8>,
        from_agent: AgentPubKey,
        signature: Signature,
    },
}

/// Durable linker to phone items.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InboxItem {
    /// Receipts a validator sent for this agent's ops, held unchanged.
    ValidationReceipts {
        dna_hash: DnaHash,
        to_agent: AgentPubKey,
        receipts: ValidationReceiptBundle,
        received_at: Timestamp,
    },
}
