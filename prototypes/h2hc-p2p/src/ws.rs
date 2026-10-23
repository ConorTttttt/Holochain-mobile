//! WebSocket byte transport for a [`FrameChannel`]: one binary message per
//! MessagePack-encoded frame.

use crate::session::FrameChannel;
use futures::{SinkExt, StreamExt};
use h2hc_protocol::{codec, ErrorKind, Frame, ProtocolError};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Bytes moved over a connection, counted at the WebSocket layer.
#[derive(Debug, Default)]
pub struct WsCounters {
    pub bytes_sent: std::sync::atomic::AtomicU64,
    pub bytes_received: std::sync::atomic::AtomicU64,
}

/// Connect to a linker's conductor endpoint, e.g. `ws://127.0.0.1:8090/conductor/v1`.
pub async fn connect(
    url: &str,
    counters: std::sync::Arc<WsCounters>,
) -> Result<FrameChannel, ProtocolError> {
    use std::sync::atomic::Ordering::Relaxed;

    let (ws, _) = tokio_tungstenite::connect_async(url).await.map_err(|e| {
        ProtocolError::new(ErrorKind::ConnectionClosed, format!("connect {url}: {e}"))
    })?;
    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Frame>(256);
    let (in_tx, in_rx) = mpsc::channel::<Frame>(256);

    let sent = counters.clone();
    tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let bytes = match codec::encode(&frame) {
                Ok(b) => b,
                Err(err) => {
                    tracing::warn!(?err, "dropping frame that cannot be encoded");
                    continue;
                }
            };
            sent.bytes_sent.fetch_add(bytes.len() as u64, Relaxed);
            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    tokio::spawn(async move {
        while let Some(msg) = stream.next().await {
            let bytes = match msg {
                Ok(Message::Binary(b)) => b,
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            };
            counters.bytes_received.fetch_add(bytes.len() as u64, Relaxed);
            match codec::decode(&bytes) {
                Ok(frame) => {
                    if in_tx.send(frame).await.is_err() {
                        break;
                    }
                }
                Err(err) => tracing::warn!(?err, "dropping undecodable frame from linker"),
            }
        }
    });

    Ok(FrameChannel {
        tx: out_tx,
        rx: in_rx,
    })
}
