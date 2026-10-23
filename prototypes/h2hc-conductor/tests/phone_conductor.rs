//! A real conductor running on the H2HC adapter, with no Kitsune2, against an
//! in-memory linker. Needs `scripts/build-fixtures.sh` to have been run.

use ed25519_dalek::SigningKey;
use h2hc_conductor::*;
use h2hc_p2p::MemoryAcceptanceLog;
use h2hc_protocol::Call;
use h2hc_protocol::*;
use holochain::conductor::config::{ConductorConfig, KeystoreConfig};
use holochain::conductor::ConductorHandle;
use holochain::prelude::*;
use holochain_keystore::{spawn_test_keystore, ValidationReceiptExt};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

enum Event {
    Joined(AgentPubKey),
    AgentInfoSigned(Result<Reply, ProtocolError>),
    Published(PublishRequest),
    Acked(u64),
}

struct Linker {
    key: SigningKey,
    to_phone: mpsc::Sender<Frame>,
    events: mpsc::UnboundedReceiver<Event>,
}

impl Linker {
    async fn next_event(&mut self, within: Duration) -> Option<Event> {
        tokio::time::timeout(within, self.events.recv()).await.ok().flatten()
    }
}

fn op_hash(op: &DhtOp) -> DhtOpHash {
    DhtOpHashed::from_content_sync(op.clone()).into_hash()
}

/// Accepts every publish with a signed receipt and answers queries with no
/// data. Asks for an agent-info signature on every join, as Kitsune2 would.
fn spawn_linker() -> (FrameChannel, Linker) {
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

fn happ_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/notes/workdir/notes.happ")
}

struct Phone {
    conductor: ConductorHandle,
    linker: Linker,
    acceptance_log: Arc<MemoryAcceptanceLog>,
    _data_root: tempfile::TempDir,
}

async fn phone() -> Phone {
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

async fn install(phone: &mut Phone) -> (AgentPubKey, CellId) {
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

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_conductor_authors_and_publishes_through_the_linker() {
    let mut phone = phone().await;
    let (agent, cell_id) = install(&mut phone).await;

    // Enabling the app joins the network through the linker, and the phone
    // signs the zero-arc agent info the linker asks for.
    let (mut joined, mut signed) = (false, false);
    while !(joined && signed) {
        match phone.linker.next_event(Duration::from_secs(30)).await {
            Some(Event::Joined(a)) => {
                assert_eq!(a, agent);
                joined = true;
            }
            Some(Event::AgentInfoSigned(reply)) => {
                assert!(matches!(reply, Ok(Reply::AgentInfoSignature(_))), "{reply:?}");
                signed = true;
            }
            Some(_) => {}
            None => panic!("no join and agent-info signature within 30 s"),
        }
    }

    let note: ActionHash = phone
        .conductor
        .easy_call_zome(&agent, None, cell_id.clone(), "notes", "create_note", "hello from a phone")
        .await
        .unwrap();

    // The note's ops reach the linker with their real content, and the
    // adapter verifies and logs the linker's signed receipt for each batch.
    let mut published: HashSet<DhtOpHash> = HashSet::new();
    let mut note_ops = 0;
    while note_ops < 3 {
        match phone.linker.next_event(Duration::from_secs(60)).await {
            Some(Event::Published(req)) => {
                assert_eq!(req.author, agent);
                for op in &req.ops {
                    published.insert(op_hash(op));
                    if let DhtOp::ChainOp(op) = op {
                        if ActionHash::with_data_sync(op.signed_action().data()) == note {
                            note_ops += 1;
                        }
                    }
                }
            }
            Some(_) => {}
            None => panic!("only {note_ops} of the note's 3 ops published within 60 s"),
        }
    }
    // The linker reports a publish before the adapter has checked its
    // receipt, so give the adapter a moment to log the last one.
    let logged = || -> HashSet<DhtOpHash> {
        phone
            .acceptance_log
            .0
            .lock()
            .unwrap()
            .iter()
            .flat_map(|(_, body)| body.op_hashes.clone())
            .collect()
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !published.is_subset(&logged()) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let missing = published.difference(&logged()).count();
    assert_eq!(missing, 0, "{missing} of {} published ops have no verified receipt", published.len());

    // Restrictive validation runs on the phone, so an invalid note is refused
    // before it can be published.
    let err = phone
        .conductor
        .easy_call_zome::<_, ActionHash, _>(&agent, None, cell_id.clone(), "notes", "create_note", "")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("a note cannot be empty"), "{err}");

    // The author can read its own note back.
    let record: Option<Record> = phone
        .conductor
        .easy_call_zome(&agent, None, cell_id.clone(), "notes", "get_note", note.clone())
        .await
        .unwrap();
    assert_eq!(record.unwrap().action_address(), &note);

    // A validation receipt delivered through the linker's inbox reaches the
    // conductor, and is acknowledged only after the conductor takes it.
    let validator_keystore = spawn_test_keystore().await.unwrap();
    let validator = validator_keystore.new_sign_keypair_random().await.unwrap();
    let store_record_op = published.iter().next().unwrap().clone();
    let receipt = ValidationReceipt {
        dht_op_hash: store_record_op,
        validation_status: ValidationStatus::Valid,
        validators: vec![validator],
        when_integrated: Timestamp::now(),
    }
    .sign(&validator_keystore)
    .await
    .unwrap()
    .unwrap();
    phone
        .linker
        .to_phone
        .send(Frame::Deliver {
            seq: 1,
            item: InboxItem::ValidationReceipts {
                dna_hash: cell_id.dna_hash().clone(),
                to_agent: agent.clone(),
                receipts: vec![receipt].into(),
                received_at: Timestamp::now(),
            },
        })
        .await
        .unwrap();
    loop {
        match phone.linker.next_event(Duration::from_secs(30)).await {
            Some(Event::Acked(1)) => break,
            Some(_) => {}
            None => panic!("validation receipt not acknowledged within 30 s"),
        }
    }
}
