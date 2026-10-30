//! One protocol connection to a linker, independent of the byte transport.
//!
//! A transport (WebSocket, QUIC, an in-memory test pipe) only has to move
//! [`Frame`]s through a [`FrameChannel`]. The session does the handshake,
//! matches responses to requests, and hands linker-initiated frames to an
//! [`InboundHandler`].
//!
//! A session does not reconnect: when its channel closes, pending and later
//! requests fail with `ConnectionClosed` and [`LinkerSession::wait_closed`]
//! returns, so the owner can open a new one.

use futures::future::BoxFuture;
use h2hc_protocol::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};

// Keep the existing client API, but use the exact same type as the linker.
pub use h2hc_protocol::transport::FrameChannel;

/// Handles frames the linker starts.
pub trait InboundHandler: 'static + Send + Sync {
    fn call(&self, body: Call) -> BoxFuture<'_, Result<Reply, ProtocolError>>;
    fn notify(&self, body: Notification) -> BoxFuture<'_, ()>;
    /// Return `Ok` only once the item is durably handled; the session then
    /// acknowledges it. An error leaves it in the linker's inbox.
    fn deliver(&self, item: InboxItem) -> BoxFuture<'_, Result<(), ProtocolError>>;
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Response, ProtocolError>>>>>;

pub struct LinkerSession {
    tx: mpsc::Sender<Frame>,
    pending: Pending,
    next_id: AtomicU64,
    welcome: Welcome,
    closed: watch::Receiver<bool>,
}

impl std::fmt::Debug for LinkerSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkerSession")
            .field("linker", &self.welcome.linker)
            .finish()
    }
}

impl LinkerSession {
    /// Send `Hello`, wait for `Welcome`, and start serving the connection.
    ///
    /// When `expected_linker` is set, a linker presenting any other key is
    /// refused, so a substituted linker cannot issue acceptance receipts.
    pub async fn connect(
        chan: FrameChannel,
        hello: Hello,
        expected_linker: Option<LinkerId>,
        handshake_timeout: Duration,
        inbound: Arc<dyn InboundHandler>,
    ) -> Result<Arc<Self>, ProtocolError> {
        let FrameChannel { tx, mut rx } = chan;
        tx.send(Frame::Hello(hello)).await.map_err(|_| closed())?;
        let welcome = match tokio::time::timeout(handshake_timeout, rx.recv()).await {
            Err(_) => {
                return Err(ProtocolError::new(
                    ErrorKind::Timeout,
                    "no Welcome from linker",
                ))
            }
            Ok(None) => return Err(closed()),
            Ok(Some(Frame::Welcome(w))) => w,
            Ok(Some(Frame::Response { body: Err(e), .. })) => return Err(e),
            Ok(Some(other)) => {
                return Err(ProtocolError::malformed(format!(
                    "expected Welcome, got {}",
                    frame_name(&other)
                )))
            }
        };
        if welcome.version != PROTOCOL_VERSION {
            return Err(ProtocolError::new(
                ErrorKind::VersionMismatch,
                format!("linker chose version {}", welcome.version),
            ));
        }
        if let Some(expected) = expected_linker {
            if welcome.linker != expected {
                return Err(ProtocolError::new(
                    ErrorKind::Unauthorized,
                    format!("expected linker {expected:?}, got {:?}", welcome.linker),
                ));
            }
        }

        let pending: Pending = Default::default();
        let (tx_for_serve, pending_for_serve) = (tx.clone(), pending.clone());
        let (closed_tx, closed) = watch::channel(false);
        tokio::spawn(async move {
            serve(rx, tx_for_serve, pending_for_serve, inbound).await;
            let _ = closed_tx.send(true);
        });
        Ok(Arc::new(Self {
            tx,
            pending,
            next_id: AtomicU64::new(1),
            welcome,
            closed,
        }))
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// Returns once the connection has closed.
    pub async fn wait_closed(&self) {
        let mut closed = self.closed.clone();
        let _ = closed.wait_for(|c| *c).await;
    }

    /// The key this linker signs acceptance receipts with.
    pub fn linker(&self) -> LinkerId {
        self.welcome.linker
    }

    pub async fn request(
        &self,
        body: Request,
        timeout: Duration,
    ) -> Result<Response, ProtocolError> {
        if self.is_closed() {
            return Err(closed());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (resp_tx, resp_rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, resp_tx);
        if self.tx.send(Frame::Request { id, body }).await.is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(closed());
        }
        match tokio::time::timeout(timeout, resp_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(closed()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(ProtocolError::new(
                    ErrorKind::Timeout,
                    format!("no response from linker within {timeout:?}"),
                ))
            }
        }
    }
}

async fn serve(
    mut rx: mpsc::Receiver<Frame>,
    tx: mpsc::Sender<Frame>,
    pending: Pending,
    inbound: Arc<dyn InboundHandler>,
) {
    while let Some(frame) = rx.recv().await {
        match frame {
            Frame::Response { id, body } => {
                let waiter = pending.lock().unwrap().remove(&id);
                match waiter {
                    Some(waiter) => {
                        let _ = waiter.send(body);
                    }
                    None => tracing::debug!(id, "response for a request that already ended"),
                }
            }
            Frame::Call { id, body } => {
                let (tx, inbound) = (tx.clone(), inbound.clone());
                tokio::spawn(async move {
                    let body = inbound.call(body).await;
                    let _ = tx.send(Frame::Reply { id, body }).await;
                });
            }
            Frame::Notify(body) => {
                let inbound = inbound.clone();
                tokio::spawn(async move { inbound.notify(body).await });
            }
            Frame::Deliver { seq, item } => {
                let (tx, inbound) = (tx.clone(), inbound.clone());
                tokio::spawn(async move {
                    match inbound.deliver(item).await {
                        Ok(()) => {
                            let _ = tx.send(Frame::Ack { seq }).await;
                        }
                        Err(err) => tracing::warn!(
                            seq,
                            ?err,
                            "inbox item not handled; leaving it unacknowledged"
                        ),
                    }
                });
            }
            other => tracing::warn!(frame = frame_name(&other), "unexpected frame from linker"),
        }
    }
    for (_, waiter) in pending.lock().unwrap().drain() {
        let _ = waiter.send(Err(closed()));
    }
}

fn closed() -> ProtocolError {
    ProtocolError::new(ErrorKind::ConnectionClosed, "linker connection closed")
}

fn frame_name(frame: &Frame) -> &'static str {
    match frame {
        Frame::Hello(_) => "Hello",
        Frame::Welcome(_) => "Welcome",
        Frame::Request { .. } => "Request",
        Frame::Response { .. } => "Response",
        Frame::Call { .. } => "Call",
        Frame::Reply { .. } => "Reply",
        Frame::Notify(_) => "Notify",
        Frame::Deliver { .. } => "Deliver",
        Frame::Ack { .. } => "Ack",
    }
}
