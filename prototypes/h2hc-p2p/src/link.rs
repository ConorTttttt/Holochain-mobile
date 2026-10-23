//! Keeps a session to the linker open: connects, reconnects with backoff
//! when the connection drops, and re-joins every agent afterwards.
//!
//! Requests made while disconnected fail at once with `ConnectionClosed`,
//! which is retryable; the conductor's workflows retry on their own schedule.
//! Re-joining also makes the linker deliver anything held in its inbox.

use crate::inbound::Inbound;
use crate::session::{FrameChannel, LinkerSession};
use h2hc_protocol::*;
use kitsune2_api::BoxFut;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::Duration;

/// Opens a fresh byte transport to the linker. Called again on every reconnect.
pub type Connector =
    Arc<dyn Fn() -> BoxFut<'static, Result<FrameChannel, ProtocolError>> + Send + Sync>;

/// A connector that yields `chan` once and fails afterwards, for in-process
/// linkers and tests.
pub fn once(chan: FrameChannel) -> Connector {
    let slot = std::sync::Mutex::new(Some(chan));
    Arc::new(move || {
        let chan = slot.lock().unwrap().take();
        Box::pin(async move {
            chan.ok_or_else(|| ProtocolError::new(ErrorKind::ConnectionClosed, "single-use channel already used"))
        })
    })
}

const MIN_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub(crate) struct Link {
    current: RwLock<Option<Arc<LinkerSession>>>,
    connector: Connector,
    hello: Hello,
    expected_linker: Option<LinkerId>,
    handshake_timeout: Duration,
    request_timeout: Duration,
    inbound: Arc<Inbound>,
    connects: AtomicU64,
}

impl Link {
    /// Connect if the linker is reachable, and keep trying in the background
    /// if it is not: a phone may well start offline.
    pub(crate) async fn start(
        connector: Connector,
        hello: Hello,
        expected_linker: Option<LinkerId>,
        handshake_timeout: Duration,
        request_timeout: Duration,
        inbound: Arc<Inbound>,
    ) -> Arc<Self> {
        let link = Arc::new(Self {
            current: RwLock::new(None),
            connector,
            hello,
            expected_linker,
            handshake_timeout,
            request_timeout,
            inbound,
            connects: AtomicU64::new(0),
        });
        match link.connect_once().await {
            Ok(session) => *link.current.write().unwrap() = Some(session),
            Err(err) => tracing::warn!(?err, "linker unreachable at startup; will keep trying"),
        }
        tokio::spawn(supervise(Arc::downgrade(&link)));
        link
    }

    async fn connect_once(&self) -> Result<Arc<LinkerSession>, ProtocolError> {
        let chan = (self.connector)().await?;
        let session = LinkerSession::connect(
            chan,
            self.hello.clone(),
            self.expected_linker,
            self.handshake_timeout,
            self.inbound.clone(),
        )
        .await?;
        self.connects.fetch_add(1, Ordering::Relaxed);
        Ok(session)
    }

    fn session(&self) -> Result<Arc<LinkerSession>, ProtocolError> {
        match &*self.current.read().unwrap() {
            Some(s) if !s.is_closed() => Ok(s.clone()),
            _ => Err(ProtocolError::new(
                ErrorKind::ConnectionClosed,
                "not connected to the linker",
            )),
        }
    }

    /// The key receipts must be signed with: the pinned one if configured,
    /// otherwise whatever the current linker presented.
    pub(crate) fn linker(&self) -> Result<LinkerId, ProtocolError> {
        match self.expected_linker {
            Some(id) => Ok(id),
            None => self.session().map(|s| s.linker()),
        }
    }

    pub(crate) async fn request(
        &self,
        body: Request,
        timeout: Duration,
    ) -> Result<Response, ProtocolError> {
        self.session()?.request(body, timeout).await
    }

    pub(crate) fn connects(&self) -> u64 {
        self.connects.load(Ordering::Relaxed)
    }

    async fn rejoin(&self, session: &LinkerSession) {
        for (dna_hash, agent) in self.inbound.joined_list() {
            if let Err(err) = session
                .request(Request::Join { dna_hash, agent }, self.request_timeout)
                .await
            {
                tracing::warn!(?err, "re-join after reconnect failed");
            }
        }
    }
}

async fn supervise(link: Weak<Link>) {
    loop {
        let current = match link.upgrade() {
            Some(l) => l.current.read().unwrap().clone(),
            None => return,
        };
        if let Some(session) = current {
            session.wait_closed().await;
            tracing::info!("linker connection closed; reconnecting");
        }
        let mut delay = MIN_BACKOFF;
        loop {
            let Some(l) = link.upgrade() else { return };
            match l.connect_once().await {
                Ok(session) => {
                    *l.current.write().unwrap() = Some(session.clone());
                    l.rejoin(&session).await;
                    tracing::info!("reconnected to linker");
                    break;
                }
                Err(err) => {
                    tracing::debug!(?err, ?delay, "reconnect failed");
                    drop(l);
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(MAX_BACKOFF);
                }
            }
        }
    }
}
