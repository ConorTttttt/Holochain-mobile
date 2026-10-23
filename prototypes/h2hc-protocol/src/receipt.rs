//! Signed evidence that a linker has durably taken custody of ops.
//!
//! This is a storage commitment, not a validation receipt: it says the linker
//! holds the bytes until `retain_until` or hands them to a named receiver. A
//! `ValidationReceipt` from an authority remains the only evidence of
//! validation.

use crate::{ErrorKind, ProtocolError, PROTOCOL_VERSION};
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use holochain_types::prelude::*;
use serde::{Deserialize, Serialize};

/// Signatures are over this prefix followed by the encoded body, so they can
/// never be confused with any other message the linker key signs.
const ACCEPTANCE_DOMAIN: &[u8] = b"h2hc-linker/acceptance-receipt/v1\0";

/// A linker's Ed25519 public key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkerId(pub [u8; 32]);

impl LinkerId {
    pub fn from_signing_key(key: &SigningKey) -> Self {
        Self(key.verifying_key().to_bytes())
    }
}

impl std::fmt::Debug for LinkerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LinkerId(")?;
        for b in &self.0[..8] {
            write!(f, "{b:02x}")?;
        }
        write!(f, "..)")
    }
}

impl Serialize for LinkerId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serde_bytes::Bytes::new(&self.0).serialize(s)
    }
}

impl<'de> Deserialize<'de> for LinkerId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let buf = serde_bytes::ByteBuf::deserialize(d)?;
        let bytes: [u8; 32] = buf
            .into_vec()
            .try_into()
            .map_err(|_| serde::de::Error::custom("LinkerId must be 32 bytes"))?;
        Ok(Self(bytes))
    }
}

/// What the linker commits to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcceptanceBody {
    pub protocol_version: u32,
    pub linker: LinkerId,
    pub dna_hash: DnaHash,
    pub author: AgentPubKey,
    pub op_hashes: Vec<DhtOpHash>,
    /// Position in the linker's append-only acceptance log.
    pub seq: u64,
    pub accepted_at: Timestamp,
    /// The linker holds the ops at least until this time, or hands them off.
    pub retain_until: Timestamp,
}

/// A signed [`AcceptanceBody`]. Carries the exact signed bytes so a verifier
/// never depends on re-encoding being canonical.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcceptanceReceipt {
    #[serde(with = "serde_bytes")]
    pub body: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

impl AcceptanceReceipt {
    /// Sign `body` with the linker's key, which must match `body.linker`.
    pub fn sign(body: &AcceptanceBody, key: &SigningKey) -> Result<Self, ProtocolError> {
        if body.linker != LinkerId::from_signing_key(key) {
            return Err(ProtocolError::new(
                ErrorKind::Internal,
                "acceptance body names a different linker than the signing key",
            ));
        }
        let body = rmp_serde::to_vec_named(body)
            .map_err(|e| ProtocolError::new(ErrorKind::Internal, e.to_string()))?;
        let signature = key.sign(&signed_bytes(&body)).to_bytes().to_vec();
        Ok(Self { body, signature })
    }

    /// Check the signature against the linker the phone expects and return
    /// the body. Callers still check the body covers what they asked for.
    pub fn verify(&self, linker: &LinkerId) -> Result<AcceptanceBody, ProtocolError> {
        let bad = |m: &str| ProtocolError::new(ErrorKind::BadSignature, m);
        let key = VerifyingKey::from_bytes(&linker.0).map_err(|_| bad("invalid linker key"))?;
        let signature = ed25519_dalek::Signature::from_slice(&self.signature)
            .map_err(|_| bad("signature is not 64 bytes"))?;
        key.verify(&signed_bytes(&self.body), &signature)
            .map_err(|_| bad("acceptance receipt signature does not verify"))?;
        let body: AcceptanceBody = rmp_serde::from_slice(&self.body)
            .map_err(|e| ProtocolError::malformed(format!("acceptance body: {e}")))?;
        if body.linker != *linker {
            return Err(bad("acceptance body names a different linker"));
        }
        if body.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolError::new(
                ErrorKind::VersionMismatch,
                format!("acceptance body version {}", body.protocol_version),
            ));
        }
        Ok(body)
    }
}

fn signed_bytes(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ACCEPTANCE_DOMAIN.len() + body.len());
    out.extend_from_slice(ACCEPTANCE_DOMAIN);
    out.extend_from_slice(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(linker: LinkerId) -> AcceptanceBody {
        AcceptanceBody {
            protocol_version: PROTOCOL_VERSION,
            linker,
            dna_hash: DnaHash::from_raw_32(vec![1; 32]),
            author: AgentPubKey::from_raw_32(vec![2; 32]),
            op_hashes: vec![DhtOpHash::from_raw_32(vec![3; 32])],
            seq: 7,
            accepted_at: Timestamp::from_micros(1_000),
            retain_until: Timestamp::from_micros(2_000),
        }
    }

    #[test]
    fn sign_then_verify_returns_body() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let linker = LinkerId::from_signing_key(&key);
        let receipt = AcceptanceReceipt::sign(&body(linker), &key).unwrap();
        assert_eq!(receipt.verify(&linker).unwrap(), body(linker));
    }

    #[test]
    fn tampered_body_is_rejected() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let linker = LinkerId::from_signing_key(&key);
        let mut receipt = AcceptanceReceipt::sign(&body(linker), &key).unwrap();
        let last = receipt.body.len() - 1;
        receipt.body[last] ^= 1;
        assert_eq!(
            receipt.verify(&linker).unwrap_err().kind,
            ErrorKind::BadSignature
        );
    }

    #[test]
    fn receipt_from_another_linker_is_rejected() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let other = SigningKey::generate(&mut rand::rngs::OsRng);
        let receipt =
            AcceptanceReceipt::sign(&body(LinkerId::from_signing_key(&key)), &key).unwrap();
        let err = receipt
            .verify(&LinkerId::from_signing_key(&other))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadSignature);
    }

    #[test]
    fn signing_with_a_mismatched_key_fails() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let other = SigningKey::generate(&mut rand::rngs::OsRng);
        assert!(AcceptanceReceipt::sign(&body(LinkerId::from_signing_key(&other)), &key).is_err());
    }
}
