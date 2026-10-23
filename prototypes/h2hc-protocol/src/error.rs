use crate::LinkerId;
use serde::{Deserialize, Serialize};

/// An error carried on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct ProtocolError {
    pub kind: ErrorKind,
    pub message: String,
}

impl ProtocolError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn unsupported(what: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unsupported, what)
    }

    pub fn malformed(what: impl Into<String>) -> Self {
        Self::new(ErrorKind::Malformed, what)
    }

    /// Whether sending the same request again later might succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::NoPeers { .. }
                | ErrorKind::Timeout
                | ErrorKind::InsufficientResponses { .. }
                | ErrorKind::Retryable { .. }
                | ErrorKind::QuotaExceeded
                | ErrorKind::Offline
                | ErrorKind::ConnectionClosed
                | ErrorKind::Internal
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorKind {
    /// No common protocol version.
    VersionMismatch,
    Unauthorized,
    /// The frame or request cannot be decoded or is inconsistent.
    Malformed,
    /// A signature inside the request does not verify.
    BadSignature,
    /// The receiver deliberately does not support this operation.
    Unsupported,
    /// The agent has not joined this DNA on this connection.
    NotJoined,
    NoPeers { loc: u32 },
    Timeout,
    InsufficientResponses { received: u32, required: u32 },
    /// Publish admission refused; the same ops will be refused again.
    Rejected,
    /// Temporary refusal.
    Retryable { retry_after_ms: Option<u64> },
    QuotaExceeded,
    /// The ops are now held by another linker, which has committed them.
    HandedOff { receiver: LinkerId },
    /// The remote peer answered with an error.
    Remote,
    /// The phone is not connected, so a call into it cannot be delivered.
    Offline,
    ConnectionClosed,
    Internal,
}
