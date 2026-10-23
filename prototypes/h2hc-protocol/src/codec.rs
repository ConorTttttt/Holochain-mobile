//! Frame encoding: MessagePack with named fields, as Holochain uses.

use crate::{Frame, ProtocolError};

/// Frames larger than this are refused before decoding.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

pub fn encode(frame: &Frame) -> Result<Vec<u8>, ProtocolError> {
    let bytes = rmp_serde::to_vec_named(frame)
        .map_err(|e| ProtocolError::malformed(format!("encode frame: {e}")))?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::malformed(format!(
            "frame is {} bytes, limit is {MAX_FRAME_BYTES}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> Result<Frame, ProtocolError> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::malformed(format!(
            "frame is {} bytes, limit is {MAX_FRAME_BYTES}",
            bytes.len()
        )));
    }
    rmp_serde::from_slice(bytes).map_err(|e| ProtocolError::malformed(format!("decode frame: {e}")))
}

#[cfg(test)]
mod tests {
    use super::{decode, encode, MAX_FRAME_BYTES};
    use crate::*;
    use holochain_types::prelude::{
        ActionHash, AgentPubKey, DnaHash, WireOps, WireRecordOps,
    };

    fn roundtrip(frame: Frame) -> Frame {
        decode(&encode(&frame).unwrap()).unwrap()
    }

    #[test]
    fn get_request_roundtrips() {
        let frame = Frame::Request {
            id: 9,
            body: Request::Get {
                dna_hash: DnaHash::from_raw_32(vec![1; 32]),
                hash: ActionHash::from_raw_32(vec![2; 32]).into(),
                options: QueryOptions {
                    remote_agent_count: 3,
                    timeout_ms: Some(500),
                    as_race: true,
                },
            },
        };
        let Frame::Request {
            id: 9,
            body: Request::Get { options, .. },
        } = roundtrip(frame)
        else {
            panic!("wrong frame");
        };
        assert_eq!(options.remote_agent_count, 3);
    }

    #[test]
    fn sourced_wire_ops_keep_their_source() {
        let responder = AgentPubKey::from_raw_32(vec![4; 32]);
        let frame = Frame::Response {
            id: 1,
            body: Ok(Response::Get(vec![
                Sourced {
                    source: DataSource::Authority {
                        responder: responder.clone(),
                    },
                    data: WireOps::Record(WireRecordOps::default()),
                },
                Sourced {
                    source: DataSource::LinkerBuffer,
                    data: WireOps::Record(WireRecordOps::default()),
                },
            ])),
        };
        let Frame::Response {
            body: Ok(Response::Get(got)),
            ..
        } = roundtrip(frame)
        else {
            panic!("wrong frame");
        };
        assert_eq!(got[0].source.responder(), Some(&responder));
        assert_eq!(got[1].source, DataSource::LinkerBuffer);
    }

    #[test]
    fn errors_roundtrip() {
        let frame = Frame::Response {
            id: 2,
            body: Err(ProtocolError::new(
                ErrorKind::InsufficientResponses {
                    received: 1,
                    required: 2,
                },
                "short",
            )),
        };
        let Frame::Response { body: Err(e), .. } = roundtrip(frame) else {
            panic!("wrong frame");
        };
        assert!(e.is_retryable());
    }

    #[test]
    fn oversize_frames_are_refused_before_decoding() {
        let bytes = vec![0u8; MAX_FRAME_BYTES + 1];
        assert_eq!(decode(&bytes).unwrap_err().kind, ErrorKind::Malformed);
    }
}
