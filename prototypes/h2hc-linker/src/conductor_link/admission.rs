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
        // Holochain 0.7.1's publish workflow rebuilds the basis from raw
        // bytes as an ExternalHash, so compare bytes, not typed hashes.
        if op.dht_basis().get_raw_36() != req.basis.get_raw_36() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use holochain_serialized_bytes::prelude::{SerializedBytes, UnsafeBytes};

    fn entry(bytes: Vec<u8>) -> Entry {
        Entry::App(AppEntryBytes(SerializedBytes::from(UnsafeBytes::from(
            bytes,
        ))))
    }

    fn agent(key: &SigningKey) -> AgentPubKey {
        AgentPubKey::from_raw_32(key.verifying_key().to_bytes().to_vec())
    }

    /// A create-record op for `author`, signed with `signer`.
    fn op(author: &AgentPubKey, signer: &SigningKey, entry: Entry) -> (Action, DhtOp) {
        let action = Action {
            header: ActionHeader {
                author: author.clone(),
                timestamp: Timestamp::from_micros(1_000),
                action_seq: 3,
                prev_action: Some(ActionHash::from_raw_32(vec![9; 32])),
            },
            data: ActionData::Create(CreateData {
                entry_type: EntryType::App(AppEntryDef {
                    entry_index: 0.into(),
                    zome_index: 0.into(),
                    visibility: EntryVisibility::Public,
                }),
                entry_hash: EntryHash::with_data_sync(&entry),
            }),
        };
        let sig = signer.sign(&holochain_serialized_bytes::encode(&action).unwrap());
        let signed = SignedAction::new(action.clone(), Signature::from(sig.to_bytes()));
        (
            action,
            DhtOp::ChainOp(Box::new(ChainOp::CreateRecord(
                signed,
                OpEntry::Present(entry),
            ))),
        )
    }

    fn request(author: AgentPubKey, op: DhtOp) -> PublishRequest {
        PublishRequest {
            dna_hash: DnaHash::from_raw_32(vec![1; 32]),
            author,
            basis: op.dht_basis(),
            ops: vec![op],
        }
    }

    fn key() -> SigningKey {
        SigningKey::from_bytes(&rand::random::<[u8; 32]>())
    }

    #[test]
    fn a_correctly_signed_op_is_admitted() {
        let k = key();
        let (_, op) = op(&agent(&k), &k, entry(vec![1, 2, 3]));
        let admitted = admit(&request(agent(&k), op.clone())).unwrap();
        assert_eq!(
            admitted[0].hash,
            DhtOpHashed::from_content_sync(op).into_hash()
        );
    }

    #[test]
    fn a_forged_signature_is_rejected() {
        let (author, forger) = (key(), key());
        let (_, op) = op(&agent(&author), &forger, entry(vec![1]));
        assert_eq!(
            admit(&request(agent(&author), op)).unwrap_err().kind,
            ErrorKind::BadSignature
        );
    }

    #[test]
    fn an_op_published_by_someone_else_is_rejected() {
        let (author, publisher) = (key(), key());
        let (_, op) = op(&agent(&author), &author, entry(vec![1]));
        assert_eq!(
            admit(&request(agent(&publisher), op)).unwrap_err().kind,
            ErrorKind::Rejected
        );
    }

    #[test]
    fn a_swapped_entry_is_rejected() {
        let k = key();
        let (action, _) = op(&agent(&k), &k, entry(vec![1]));
        let sig = k.sign(&holochain_serialized_bytes::encode(&action).unwrap());
        let swapped = DhtOp::ChainOp(Box::new(ChainOp::CreateRecord(
            SignedAction::new(action, Signature::from(sig.to_bytes())),
            OpEntry::Present(entry(vec![2])),
        )));
        assert_eq!(
            admit(&request(agent(&k), swapped)).unwrap_err().kind,
            ErrorKind::Rejected
        );
    }

    #[test]
    fn an_untyped_basis_with_the_same_bytes_is_admitted() {
        let k = key();
        let (_, op) = op(&agent(&k), &k, entry(vec![1]));
        let mut req = request(agent(&k), op);
        req.basis = ExternalHash::from_raw_36(req.basis.get_raw_36().to_vec()).into();
        assert!(admit(&req).is_ok());
    }

    #[test]
    fn a_basis_that_does_not_match_the_op_is_malformed() {
        let k = key();
        let (_, op) = op(&agent(&k), &k, entry(vec![1]));
        let mut req = request(agent(&k), op);
        req.basis = ActionHash::from_raw_32(vec![7; 32]).into();
        assert_eq!(admit(&req).unwrap_err().kind, ErrorKind::Malformed);
    }
}
