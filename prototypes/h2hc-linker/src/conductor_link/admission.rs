//! Checks before the linker takes custody of published ops.
//!
//! This is admission, not DNA validation: the linker confirms each op is what
//! it claims to be (signed by its author, entry matching its hash, basis
//! consistent) so it never stores or serves forged data. Whether the data is
//! valid under the DNA's rules is the authorities' decision.

use super::store::AdmittedOp;
use ed25519_dalek::{Verifier, VerifyingKey};
use h2hc_protocol::{ErrorKind, ProtocolError, PublishRequest};
use holochain_types::prelude::*;

fn rejected(msg: String) -> ProtocolError {
    ProtocolError::new(ErrorKind::Rejected, msg)
}

pub(crate) fn admit(req: &PublishRequest) -> Result<Vec<AdmittedOp>, ProtocolError> {
    if req.ops.is_empty() {
        return Err(ProtocolError::malformed("publish request has no ops"));
    }
    let mut out = Vec::with_capacity(req.ops.len());
    for op in &req.ops {
        let DhtOp::ChainOp(chain_op) = op else {
            return Err(ProtocolError::unsupported(
                "warrant publishing through a linker",
            ));
        };
        let signed = chain_op.signed_action();
        let action = signed.data();
        if action.author() != &req.author {
            return Err(rejected(format!(
                "op authored by {}, published by {}",
                action.author(),
                req.author
            )));
        }
        verify_action_signature(action, signed.signature())?;
        if let (Some(expected), Some(OpEntry::Present(entry))) =
            (action.entry_hash(), chain_op.op_entry())
        {
            if EntryHash::with_data_sync(entry) != *expected {
                return Err(rejected(
                    "op entry does not match the action's entry hash".into(),
                ));
            }
        }
        if op.dht_basis() != req.basis {
            return Err(ProtocolError::malformed(
                "op basis differs from the request's basis",
            ));
        }
        let bytes = holochain_serialized_bytes::encode(op)
            .map_err(|e| ProtocolError::malformed(format!("encode op: {e}")))?;
        out.push(AdmittedOp {
            hash: DhtOpHashed::from_content_sync(op.clone()).into_hash(),
            bytes,
        });
    }
    Ok(out)
}

/// Holochain signs the MessagePack encoding of the action with the author's key.
fn verify_action_signature(action: &Action, signature: &Signature) -> Result<(), ProtocolError> {
    let bad = |m: &str| ProtocolError::new(ErrorKind::BadSignature, m);
    let bytes = holochain_serialized_bytes::encode(action)
        .map_err(|e| ProtocolError::malformed(format!("encode action: {e}")))?;
    let key_bytes: [u8; 32] = action
        .author()
        .get_raw_32()
        .try_into()
        .map_err(|_| bad("author key is not 32 bytes"))?;
    let key = VerifyingKey::from_bytes(&key_bytes).map_err(|_| bad("invalid author key"))?;
    key.verify(&bytes, &ed25519_dalek::Signature::from_bytes(&signature.0))
        .map_err(|_| bad("action signature does not verify"))
}
