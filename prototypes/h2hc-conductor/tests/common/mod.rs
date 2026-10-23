//! A phone conductor on the H2HC adapter against an in-memory linker, shared
//! by the integration tests. Needs `scripts/build-fixtures.sh` to have been
//! run.
#![allow(dead_code)]

use ed25519_dalek::SigningKey;
use h2hc_conductor::*;
use h2hc_p2p::MemoryAcceptanceLog;
use h2hc_protocol::Call;
use h2hc_protocol::*;
use holochain::conductor::config::{ConductorConfig, KeystoreConfig};
use holochain::conductor::ConductorHandle;
use holochain::prelude::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

pub enum Event {
    Joined(AgentPubKey),
    AgentInfoSigned(Result<Reply, ProtocolError>),
    Published(PublishRequest),
    Acked(u64),
}

pub struct Linker {
    pub key: SigningKey,
    pub to_phone: mpsc::Sender<Frame>,
    pub events: mpsc::UnboundedReceiver<Event>,
}

impl Linker {
    pub async fn next_event(&mut self, within: Duration) -> Option<Event> {
        tokio::time::timeout(within, self.events.recv()).await.ok().flatten()
    }
}

pub fn op_hash(op: &DhtOp) -> DhtOpHash {
    DhtOpHashed::from_content_sync(op.clone()).into_hash()
}

/// Accepts every publish with a signed receipt and answers queries with no
/// data. Asks for an agent-info signature on every join, as Kitsune2 would.
pub fn spawn_linker() -> (FrameChannel, Linker) {
    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    let linker_id = LinkerId::from_signing_key(&key);
    let (phone_end, mut linker_end) = FrameChannel::pair(256);
    let (events_tx, events) = mpsc::unbounded_channel();
    let to_phone = linker_end.tx.clone();
    let signing_key = key.clone();
    tokio::spawn(async move {
        let mut seq = 0u64;
        let mut call_id = 0u64;
        while let Some(frame) = linker_end.rx.recv().await {
            let reply = match frame {
                Frame::Hello(_) => Frame::Welcome(Welcome {
                    version: PROTOCOL_VERSION,
                    linker: linker_id,
                }),
                Frame::Request { id, body } => {
                    let body = match body {
                        Request::Join { dna_hash, agent } => {
                            let _ = events_tx.send(Event::Joined(agent.clone()));
                            call_id += 1;
                            let now = kitsune2_api::Timestamp::now();
                            let agent_info = kitsune2_api::AgentInfo {
                                agent: agent.to_k2_agent(),
                                space: dna_hash.to_k2_space(),
                                created_at: now,
                                expires_at: now + Duration::from_secs(20 * 60),
                                is_tombstone: false,
                                url: Some(kitsune2_api::Url::from_str("ws://linker.test:443").unwrap()),
                                storage_arc: kitsune2_api::DhtArc::Empty,
                            };
                            let _ = linker_end
                                .tx
                                .send(Frame::Call {
                                    id: call_id,
                                    body: Call::SignAgentInfo { agent_info },
                                })
                                .await;
                            Ok(Response::Done)
                        }
                        Request::Publish(req) => {
                            seq += 1;
                            let now = Timestamp::now();
                            let body = AcceptanceBody {
                                protocol_version: PROTOCOL_VERSION,
                                linker: linker_id,
                                dna_hash: req.dna_hash.clone(),
                                author: req.author.clone(),
                                op_hashes: req.ops.iter().map(op_hash).collect(),
                                seq,
                                accepted_at: now,
                                retain_until: (now + Duration::from_secs(86_400)).unwrap(),
                            };
                            let _ = events_tx.send(Event::Published(req));
                            AcceptanceReceipt::sign(&body, &signing_key).map(Response::PublishAccepted)
                        }
                        Request::Get { .. } => Ok(Response::Get(vec![])),
                        Request::GetLinks { .. } => Ok(Response::GetLinks(vec![])),
                        Request::CountLinks { .. } => Ok(Response::CountLinks(vec![])),
                        Request::GetAgentActivity { .. } | Request::GetAgentActivityMulti { .. } => {
                            Ok(Response::AgentActivity(vec![]))
                        }
                        Request::MustGetAgentActivity { .. } => Ok(Response::MustGetAgentActivity(vec![])),
                        _ => Ok(Response::Done),
                    };
                    Frame::Response { id, body }
                }
                Frame::Reply { body, .. } => {
                    let _ = events_tx.send(Event::AgentInfoSigned(body));
                    continue;
                }
                Frame::Ack { seq } => {
                    let _ = events_tx.send(Event::Acked(seq));
                    continue;
                }
                _ => continue,
            };
            if linker_end.tx.send(reply).await.is_err() {
                return;
            }
        }
    });
    (phone_end, Linker { key, to_phone, events })
}

pub fn happ_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/notes/workdir/notes.happ")
}

pub struct Phone {
    pub conductor: ConductorHandle,
    pub linker: Linker,
    pub acceptance_log: Arc<MemoryAcceptanceLog>,
    _data_root: tempfile::TempDir,
}

pub async fn phone() -> Phone {
    let data_root = tempfile::tempdir().unwrap();
    let config = ConductorConfig {
        data_root_path: Some(data_root.path().to_path_buf().into()),
        keystore: KeystoreConfig::LairServerInProc { lair_root: None },
        ..Default::default()
    };
    let passphrase = Arc::new(Mutex::new(sodoken::LockedArray::from(b"passphrase".to_vec())));
    let (chan, linker) = spawn_linker();
    let acceptance_log = Arc::new(MemoryAcceptanceLog::default());
    let options = H2hcOptions {
        expected_linker: Some(LinkerId::from_signing_key(&linker.key)),
        acceptance_log: acceptance_log.clone(),
        ..Default::default()
    };
    let conductor = build_phone_conductor(
        config,
        passphrase,
        once(chan),
        options,
    )
    .await
    .unwrap();
    Phone {
        conductor,
        linker,
        acceptance_log,
        _data_root: data_root,
    }
}

pub async fn install(phone: &mut Phone) -> (AgentPubKey, CellId) {
    let bytes = std::fs::read(happ_path())
        .unwrap_or_else(|e| panic!("{e}: run scripts/build-fixtures.sh first"));
    let app = phone
        .conductor
        .clone()
        .install_app_bundle(InstallAppPayload {
            source: AppBundleSource::Bytes(bytes.into()),
            agent_key: None,
            installed_app_id: Some("notes".into()),
            network_seed: None,
            roles_settings: None,
            ignore_genesis_failure: false,
            restore_from_dht: false,
        })
        .await
        .unwrap();
    let agent = app.agent_key().clone();
    let cell_id = app.all_cells().next().unwrap();
    phone.conductor.clone().enable_app("notes".into()).await.unwrap();
    (agent, cell_id)
}
