//! Reconnecting H2HC connector for an already-authenticated byte stream.
//!
//! On Android, the BLE L2CAP dialer will eventually provide the `open`
//! callback; on Linux, a BLE L2CAP acceptor can pass its accepted stream to
//! `ConductorLinks::serve_channel`. This module does **not** dial a BLE
//! device or perform trust/pairing checks.

use crate::link::Connector;
use h2hc_protocol::transport::channel_over_io;
use h2hc_protocol::ProtocolError;
use std::future::Future;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};

/// Turn an async byte-stream dialer into the client's reconnecting Connector.
///
/// The H2HC link supervisor calls `open` again after every disconnection.
/// The caller must establish an encrypted, authenticated connection, and
/// pin/verify the intended gateway identity before returning the stream.
/// A `Welcome` linker key check alone is not a transport authentication.
/// Choose a realistic byte limit for the BLE radio profile. Frames larger
/// than this limit close the stream until an application-level chunking
/// protocol is implemented.
pub fn connector_from_stream<S, Open, OpenFuture>(
    open: Open,
    queue_capacity: usize,
    max_frame_bytes: usize,
) -> Connector
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    Open: Fn() -> OpenFuture + Send + Sync + 'static,
    OpenFuture: Future<Output = Result<S, ProtocolError>> + Send + 'static,
{
    let open = Arc::new(open);
    Arc::new(move || {
        let open = open.clone();
        Box::pin(async move {
            let io = open().await?;
            Ok(channel_over_io(io, queue_capacity, max_frame_bytes))
        })
    })
}
