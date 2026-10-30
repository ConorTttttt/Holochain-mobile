//! Bidirectional H2HC frame channels shared by the conductor and linker.
//!
//! Each transport adapter supplies a `FrameChannel`: WebSocket uses one
//! MessagePack frame per binary message; a reliable BLE L2CAP stream can use
//! `channel_over_io` with explicit length framing. This module deliberately
//! does not open or authenticate BLE links. The caller must authenticate the
//! peer *before* passing an I/O stream into the H2HC session.

use crate::{codec, Frame};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

/// The two directions of one established H2HC connection.
///
/// `tx` sends to the remote end; `rx` receives from it. Dropping the
/// channel closes its transport worker. A transport losing its connection
/// must drop `rx` so the Link supervisor can reconnect.
pub struct FrameChannel {
    pub tx: mpsc::Sender<Frame>,
    pub rx: mpsc::Receiver<Frame>,
}

impl FrameChannel {
    /// Two connected ends for tests and in-process linkers.
    pub fn pair(buffer: usize) -> (FrameChannel, FrameChannel) {
        let (a_tx, b_rx) = mpsc::channel(buffer);
        let (b_tx, a_rx) = mpsc::channel(buffer);
        (
            FrameChannel { tx: a_tx, rx: a_rx },
            FrameChannel { tx: b_tx, rx: b_rx },
        )
    }
}

/// Run H2HC frames over an established, reliable, ordered byte stream.
///
/// Each packet is `u32` big-endian MessagePack byte length followed by
/// that many bytes. Frame sizes are checked *before allocation*, and the
/// worker exits on disconnect, malformed input, or write failure. Queues
/// are bounded in both directions to provide backpressure.
///
/// This is suitable for BLE L2CAP credit-based channels once an authenticated
/// BLE connection supplies the underlying stream. **GATT characteristics
/// are packet/fragment based, not byte streams**, so they require a separate
/// fragmentation/reassembly adapter before they can use this function.
///
/// A caller should choose a much smaller `max_frame_bytes` for BLE than
/// the 16 MiB wire-protocol maximum. Large operations must be handled by
/// a future chunking protocol rather than allocating huge BLE frames.
///
/// This function does not establish encryption, pairing or device identity.
pub fn channel_over_io<S>(io: S, buffer: usize, max_frame_bytes: usize) -> FrameChannel
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    assert!(buffer > 0, "frame queue must be nonempty");
    assert!(
        max_frame_bytes > 0 && max_frame_bytes <= codec::MAX_FRAME_BYTES,
        "invalid H2HC frame limit"
    );

    let (to_wire_tx, mut to_wire_rx) = mpsc::channel::<Frame>(buffer);
    let (from_wire_tx, from_wire_rx) = mpsc::channel::<Frame>(buffer);
    tokio::spawn(async move {
        let (mut reader, mut writer) = tokio::io::split(io);
        // Keep both futures alive across wakeups. Re-creating a pending
        // read_u32/read_exact future after every outgoing send could discard
        // a partially read header or payload (Tokio's read_exact is not
        // cancellation-safe).
        let read_frames = async {
            loop {
                let incoming_len = match reader.read_u32().await {
                    Ok(len) => len,
                    Err(_) => break,
                };
                let len = incoming_len as usize;
                if len == 0 || len > max_frame_bytes {
                    tracing::warn!(len, max_frame_bytes,
                        "invalid incoming H2HC stream frame length");
                    break;
                }
                let mut bytes = vec![0_u8; len];
                if reader.read_exact(&mut bytes).await.is_err() {
                    break;
                }
                let frame = match codec::decode(&bytes) {
                    Ok(frame) => frame,
                    Err(err) => {
                        tracing::warn!(?err, "invalid incoming H2HC stream frame");
                        break;
                    }
                };
                if from_wire_tx.send(frame).await.is_err() {
                    break;
                }
            }
        };
        let write_frames = async {
            while let Some(frame) = to_wire_rx.recv().await {
                let bytes = match codec::encode(&frame) {
                    Ok(bytes) if bytes.len() <= max_frame_bytes => bytes,
                    Ok(bytes) => {
                        tracing::warn!(length = bytes.len(), max_frame_bytes,
                            "outgoing H2HC frame exceeds BLE stream limit");
                        break;
                    }
                    Err(err) => {
                        tracing::warn!(?err, "cannot encode H2HC stream frame");
                        break;
                    }
                };
                if writer.write_u32(bytes.len() as u32).await.is_err()
                    || writer.write_all(&bytes).await.is_err()
                    || writer.flush().await.is_err()
                {
                    break;
                }
            }
        };
        // If either side closes, terminate the other and drop the receive
        // sender, waking the H2HC session even when only writing failed.
        tokio::select! {
            _ = read_frames => {},
            _ = write_frames => {},
        }
        // Dropping from_wire_tx makes the receiving LinkerSession / linker
        // session detect the disconnect, including failures during writes.
    });

    FrameChannel {
        tx: to_wire_tx,
        rx: from_wire_rx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Hello, PROTOCOL_VERSION};
    use std::time::Duration;

    #[tokio::test]
    async fn in_memory_channel_is_bidirectional() {
        let (mut a, mut b) = FrameChannel::pair(4);
        a.tx.send(Frame::Hello(Hello {
            versions: vec![PROTOCOL_VERSION],
            session_token: None,
        }))
        .await
        .unwrap();
        assert!(matches!(b.rx.recv().await, Some(Frame::Hello(_))));
        b.tx.send(Frame::Ack { seq: 17 }).await.unwrap();
        assert!(matches!(a.rx.recv().await, Some(Frame::Ack { seq: 17 })));
    }

    #[tokio::test]
    async fn duplex_stream_transfers_frames_in_both_directions() {
        let (a_io, b_io) = tokio::io::duplex(4096);
        let mut a = channel_over_io(a_io, 4, 1024);
        let mut b = channel_over_io(b_io, 4, 1024);
        a.tx.send(Frame::Hello(Hello {
            versions: vec![PROTOCOL_VERSION],
            session_token: None,
        }))
        .await
        .unwrap();
        assert!(matches!(b.rx.recv().await, Some(Frame::Hello(_))));
        b.tx.send(Frame::Ack { seq: 42 }).await.unwrap();
        assert!(matches!(a.rx.recv().await, Some(Frame::Ack { seq: 42 })));

        drop(a);
        let closed = tokio::time::timeout(Duration::from_secs(2), b.rx.recv())
            .await
            .expect("stream did not close");
        assert!(closed.is_none());
    }

    #[tokio::test]
    async fn oversized_length_closes_without_allocating_payload() {
        let (mut raw, io) = tokio::io::duplex(64);
        let mut framed = channel_over_io(io, 2, 32);
        raw.write_all(&33_u32.to_be_bytes()).await.unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(2), framed.rx.recv())
            .await
            .expect("oversized frame was not refused");
        assert!(closed.is_none());
    }

    #[tokio::test]
    async fn truncated_frame_drops_connection() {
        let (mut raw, io) = tokio::io::duplex(64);
        let mut framed = channel_over_io(io, 2, 1024);
        raw.write_all(&10_u32.to_be_bytes()).await.unwrap();
        raw.write_all(&[0, 1]).await.unwrap();
        drop(raw);
        let closed = tokio::time::timeout(Duration::from_secs(2), framed.rx.recv())
            .await
            .expect("truncated frame was not refused");
        assert!(closed.is_none());
    }
}
