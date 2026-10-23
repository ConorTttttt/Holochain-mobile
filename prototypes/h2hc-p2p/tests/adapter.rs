//! The adapter against a scripted in-memory linker. Each test checks one row
//! of the compatibility matrix.

use ed25519_dalek::{SigningKey, Verifier};
use h2hc_p2p::*;
use h2hc_protocol::*;
use h2hc_protocol::Call;
use holochain_keystore::{spawn_test_keystore, MetaLairClient};
use holochain_p2p::actor::*;
use holochain_p2p::event::{
    CountersigningSessionNegotiationMessage, GetActivityOptions as RemoteActivityOptions,
    GetLinksOptions, HcP2pHandler,
};
use holochain_p2p::{HolochainP2pConfig, HolochainP2pError, HolochainP2pResult};
use holochain_types::prelude::*;
use kitsune2_api::{BoxFut, DhtArc};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const WAIT: Duration = Duration::from_secs(2);

type Responder = Arc<dyn Fn(&Request) -> Result<Response, ProtocolError> + Send + Sync>;

struct Linker {
    key: SigningKey,
    to_phone: mpsc::Sender<Frame>,
    /// `Reply` and `Ack` frames from the phone.
    from_phone: mpsc::Receiver<Frame>,
    requests: Arc<Mutex<Vec<Request>>>,
    close: Option<oneshot::Sender<()>>,
}

impl Linker {
    fn id(&self) -> LinkerId {
        LinkerId::from_signing_key(&self.key)
    }

    async fn next_from_phone(&mut self) -> Option<Frame> {
        tokio::time::timeout(WAIT, self.from_phone.recv()).await.ok().flatten()
    }
}

fn spawn_linker(responder: Responder) -> (FrameChannel, Linker) {
    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    let (phone_end, mut linker_end) = FrameChannel::pair(64);
    let (fwd_tx, fwd_rx) = mpsc::channel(64);
    let (close_tx, mut close_rx) = oneshot::channel();
    let requests: Arc<Mutex<Vec<Request>>> = Default::default();
    let linker = Linker {
        key: key.clone(),
        to_phone: linker_end.tx.clone(),
        from_phone: fwd_rx,
        requests: requests.clone(),
        close: Some(close_tx),
    };
    tokio::spawn(async move {
        loop {
            let frame = tokio::select! {
                _ = &mut close_rx => return,
                f = linker_end.rx.recv() => match f { Some(f) => f, None => return },
            };
            let reply = match frame {
                Frame::Hello(_) => Frame::Welcome(Welcome {
                    version: PROTOCOL_VERSION,
                    linker: LinkerId::from_signing_key(&key),
                }),
                Frame::Request { id, body } => {
                    requests.lock().unwrap().push(body.clone());
                    Frame::Response {
                        id,
                        body: responder(&body),
                    }
                }
                other => {
                    let _ = fwd_tx.send(other).await;
                    continue;
                }
            };
            if linker_end.tx.send(reply).await.is_err() {
                return;
            }
        }
    });
    (phone_end, linker)
}

#[derive(Debug, Default)]
struct RecordingHandler {
    calls: Mutex<Vec<(AgentPubKey, Signature)>>,
    receipts: Mutex<Vec<AgentPubKey>>,
}

fn not_expected<T>() -> BoxFut<'static, HolochainP2pResult<T>> {
    Box::pin(async { Err(HolochainP2pError::other("not expected on a zero-arc phone")) })
}

impl HcP2pHandler for RecordingHandler {
    fn handle_call_remote(
        &self,
        _dna_hash: DnaHash,
        to_agent: AgentPubKey,
        _zome_call_params_serialized: ExternIO,
        signature: Signature,
    ) -> BoxFut<'_, HolochainP2pResult<SerializedBytes>> {
        self.calls.lock().unwrap().push((to_agent, signature));
        Box::pin(async { Ok(SerializedBytes::from(UnsafeBytes::from(vec![42]))) })
    }

    fn handle_remote_signal_direct(
        &self,
        _dna_hash: DnaHash,
        _to_agent: AgentPubKey,
        _signal: Vec<u8>,
        _from_agent: AgentPubKey,
        _signature: Signature,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        Box::pin(async { Ok(()) })
    }

    fn handle_publish(
        &self,
        _dna_hash: DnaHash,
        _ops: Vec<(DhtOp, bool)>,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        not_expected()
    }

    fn handle_get(
        &self,
        _dna_hash: DnaHash,
        _to_agent: AgentPubKey,
        _dht_hash: AnyDhtHash,
    ) -> BoxFut<'_, HolochainP2pResult<WireOps>> {
        not_expected()
    }

    fn handle_get_links(
        &self,
        _dna_hash: DnaHash,
        _to_agent: AgentPubKey,
        _link_key: WireLinkKey,
        _options: GetLinksOptions,
    ) -> BoxFut<'_, HolochainP2pResult<WireLinkOps>> {
        not_expected()
    }

    fn handle_count_links(
        &self,
        _dna_hash: DnaHash,
        _to_agent: AgentPubKey,
        _query: WireLinkQuery,
    ) -> BoxFut<'_, HolochainP2pResult<CountLinksResponse>> {
        not_expected()
    }

    fn handle_get_agent_activity(
        &self,
        _dna_hash: DnaHash,
        _to_agent: AgentPubKey,
        _agent: AgentPubKey,
        _query: ChainQueryFilter,
        _options: RemoteActivityOptions,
    ) -> BoxFut<'_, HolochainP2pResult<AgentActivityResponse>> {
        not_expected()
    }

    fn handle_must_get_agent_activity(
        &self,
        _dna_hash: DnaHash,
        _to_agent: AgentPubKey,
        _author: AgentPubKey,
        _filter: ChainFilter,
    ) -> BoxFut<'_, HolochainP2pResult<MustGetAgentActivityResponse>> {
        not_expected()
    }

    fn handle_validation_receipts_received(
        &self,
        _dna_hash: DnaHash,
        to_agent: AgentPubKey,
        _receipts: ValidationReceiptBundle,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        self.receipts.lock().unwrap().push(to_agent);
        Box::pin(async { Ok(()) })
    }

    fn handle_publish_countersign(
        &self,
        _dna_hash: DnaHash,
        _op: ChainOp,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        not_expected()
    }

    fn handle_countersigning_session_negotiation(
        &self,
        _dna_hash: DnaHash,
        _to_agent: AgentPubKey,
        _message: CountersigningSessionNegotiationMessage,
    ) -> BoxFut<'_, HolochainP2pResult<()>> {
        not_expected()
    }
}

struct Phone {
    p2p: DynHcP2p,
    linker: Linker,
    keystore: MetaLairClient,
    handler: Arc<RecordingHandler>,
}

async fn phone_with(responder: Responder, policy: SourcePolicy) -> Phone {
    let keystore = spawn_test_keystore().await.unwrap();
    let (chan, linker) = spawn_linker(responder);
    let options = H2hcOptions {
        expected_linker: Some(linker.id()),
        source_policy: policy,
        ..Default::default()
    };
    let p2p = H2hcP2p::connect(HolochainP2pConfig::default(), keystore.clone(), once(chan), options)
        .await
        .unwrap();
    Phone {
        p2p,
        linker,
        keystore,
        handler: Default::default(),
    }
}

async fn phone(responder: Responder) -> Phone {
    phone_with(responder, SourcePolicy::default()).await
}

fn done() -> Responder {
    Arc::new(|_| Ok(Response::Done))
}

fn dna() -> DnaHash {
    DnaHash::from_raw_32(vec![1; 32])
}

fn agent(n: u8) -> AgentPubKey {
    AgentPubKey::from_raw_32(vec![n; 32])
}

fn authority(n: u8) -> DataSource {
    DataSource::Authority {
        responder: agent(n),
    }
}

fn record() -> WireOps {
    WireOps::Record(WireRecordOps::default())
}

fn activity() -> AgentActivityResponse {
    AgentActivityResponse {
        agent: agent(9),
        valid_activity: ChainItems::NotRequested,
        rejected_activity: ChainItems::NotRequested,
        status: ChainStatus::Empty,
        highest_observed: None,
        warrants: vec![],
    }
}

fn protocol_error(err: &HolochainP2pError) -> Option<&ProtocolError> {
    match err {
        HolochainP2pError::Other(e) => e.downcast_ref::<ProtocolError>(),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_linker_with_an_unexpected_key_is_refused() {
    let keystore = spawn_test_keystore().await.unwrap();
    let (chan, _linker) = spawn_linker(done());
    let impostor = SigningKey::generate(&mut rand::rngs::OsRng);
    let options = H2hcOptions {
        expected_linker: Some(LinkerId::from_signing_key(&impostor)),
        ..Default::default()
    };
    let p2p = H2hcP2p::connect(HolochainP2pConfig::default(), keystore, once(chan), options)
        .await
        .unwrap();
    let hash: AnyDhtHash = ActionHash::from_raw_32(vec![5; 32]).into();
    let err = p2p.get(dna(), hash, Default::default(), None).await.unwrap_err();
    assert_eq!(protocol_error(&err).unwrap().kind, ErrorKind::ConnectionClosed);
}

#[tokio::test(flavor = "multi_thread")]
async fn get_drops_buffered_answers_unless_the_policy_accepts_them() {
    let responder: Responder = Arc::new(|_| {
        Ok(Response::Get(vec![
            Sourced { source: authority(2), data: record() },
            Sourced { source: DataSource::LinkerBuffer, data: record() },
            Sourced {
                source: DataSource::LinkerCache {
                    responder: agent(3),
                    fetched_at: Timestamp::from_micros(1),
                },
                data: record(),
            },
        ]))
    });
    let hash: AnyDhtHash = ActionHash::from_raw_32(vec![5; 32]).into();

    let strict = phone(responder.clone()).await;
    let got = strict
        .p2p
        .get(dna(), hash.clone(), Default::default(), None)
        .await
        .unwrap();
    assert_eq!(got.len(), 2, "authority and cache only");

    let trusting = phone_with(
        responder,
        SourcePolicy {
            accept_linker_buffer: true,
            accept_linker_cache: true,
        },
    )
    .await;
    let got = trusting.p2p.get(dna(), hash, Default::default(), None).await.unwrap();
    assert_eq!(got.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn must_get_agent_activity_accepts_only_authority_answers() {
    let responder: Responder = Arc::new(|_| {
        Ok(Response::MustGetAgentActivity(vec![
            Sourced {
                source: DataSource::LinkerBuffer,
                data: MustGetAgentActivityResponse::IncompleteChain,
            },
            Sourced {
                source: DataSource::LinkerCache {
                    responder: agent(3),
                    fetched_at: Timestamp::from_micros(1),
                },
                data: MustGetAgentActivityResponse::IncompleteChain,
            },
            Sourced {
                source: authority(2),
                data: MustGetAgentActivityResponse::IncompleteChain,
            },
        ]))
    });
    let phone = phone_with(
        responder,
        SourcePolicy {
            accept_linker_buffer: true,
            accept_linker_cache: true,
        },
    )
    .await;
    let filter = ChainFilter::new(ActionHash::from_raw_32(vec![6; 32]));
    let got = phone
        .p2p
        .must_get_agent_activity(dna(), agent(9), filter, Default::default(), None)
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn activity_multi_counts_each_authority_once() {
    let responder: Responder = Arc::new(|_| {
        Ok(Response::AgentActivity(vec![
            Sourced { source: authority(2), data: activity() },
            Sourced { source: authority(2), data: activity() },
            Sourced { source: DataSource::LinkerBuffer, data: activity() },
        ]))
    });
    let phone = phone(responder).await;
    let err = phone
        .p2p
        .get_agent_activity_multi(dna(), agent(9), ChainQueryFilter::new(), Default::default())
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        HolochainP2pError::InsufficientResponses { received: 1, required: 2, .. }
    ));

    let responder: Responder = Arc::new(|_| {
        Ok(Response::AgentActivity(vec![
            Sourced { source: authority(2), data: activity() },
            Sourced { source: authority(3), data: activity() },
        ]))
    });
    let phone = self::phone(responder).await;
    let got = phone
        .p2p
        .get_agent_activity_multi(dna(), agent(9), ChainQueryFilter::new(), Default::default())
        .await
        .unwrap();
    let responders: Vec<_> = got.into_iter().map(|(a, _)| a).collect();
    assert_eq!(responders, vec![agent(2), agent(3)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn contradictory_multi_options_fail_without_contacting_the_linker() {
    let phone = phone(done()).await;
    let options = GetActivityMultiOptions {
        required_responses: 0,
        ..Default::default()
    };
    let err = phone
        .p2p
        .get_agent_activity_multi(dna(), agent(9), ChainQueryFilter::new(), options)
        .await
        .unwrap_err();
    assert!(matches!(err, HolochainP2pError::InvalidRequest(_)));
    assert!(phone.linker.requests.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn zero_arc_answers_are_local() {
    let phone = phone(done()).await;
    let p2p = &phone.p2p;
    let basis: OpBasis = ActionHash::from_raw_32(vec![5; 32]).into();

    assert!(!p2p.authority_for_hash(dna(), basis.clone()).await.unwrap());
    assert!(!p2p.was_agent_recently_online(dna(), agent(2)).await.unwrap());
    p2p.new_integrated_data(dna().to_k2_space(), vec![]).await.unwrap();
    // The default config's DHT store panics if touched, so this also shows an
    // empty publish never reads the store or contacts the linker.
    p2p.publish(dna(), basis, agent(2), vec![], None).await.unwrap();

    let err = p2p
        .send_validation_receipts(dna(), agent(2), Vec::new().into())
        .await
        .unwrap_err();
    assert_eq!(protocol_error(&err).unwrap().kind, ErrorKind::Unsupported);
    let err = p2p
        .countersigning_session_negotiation(
            dna(),
            vec![],
            CountersigningSessionNegotiationMessage::AuthorityResponse(vec![]),
        )
        .await
        .unwrap_err();
    assert_eq!(protocol_error(&err).unwrap().kind, ErrorKind::Unsupported);

    let peers = p2p.peer_store(dna()).await.unwrap();
    assert!(peers.get_all().await.unwrap().is_empty());
    assert!(peers.insert(vec![]).await.is_err());

    assert!(phone.linker.requests.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn joined_agents_have_empty_arcs() {
    let phone = phone(done()).await;
    assert!(phone.p2p.target_arcs(dna()).await.unwrap().is_empty());
    phone.p2p.join(dna(), agent(2), None, None).await.unwrap();
    assert_eq!(phone.p2p.target_arcs(dna()).await.unwrap(), vec![DhtArc::Empty]);
    phone.p2p.leave(dna(), agent(2)).await.unwrap();
    assert!(phone.p2p.target_arcs(dna()).await.unwrap().is_empty());
}

fn agent_info(agent: &AgentPubKey, lifetime: Duration, arc: DhtArc) -> kitsune2_api::AgentInfo {
    let now = kitsune2_api::Timestamp::now();
    kitsune2_api::AgentInfo {
        agent: agent.to_k2_agent(),
        space: dna().to_k2_space(),
        created_at: now,
        expires_at: now + lifetime,
        is_tombstone: false,
        url: Some(kitsune2_api::Url::from_str("ws://linker.example:443").unwrap()),
        storage_arc: arc,
    }
}

async fn ask_signature(linker: &mut Linker, id: u64, info: kitsune2_api::AgentInfo) -> Result<Reply, ProtocolError> {
    linker
        .to_phone
        .send(Frame::Call { id, body: Call::SignAgentInfo { agent_info: info } })
        .await
        .unwrap();
    match linker.next_from_phone().await {
        Some(Frame::Reply { id: got, body }) if got == id => body,
        other => panic!("expected Reply {id}, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_infos_are_signed_only_for_joined_zero_arc_short_lived_agents() {
    let mut phone = phone(done()).await;
    let me = phone.keystore.new_sign_keypair_random().await.unwrap();
    let twenty_min = Duration::from_secs(20 * 60);

    let err = ask_signature(&mut phone.linker, 1, agent_info(&me, twenty_min, DhtArc::Empty))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotJoined);

    phone.p2p.join(dna(), me.clone(), None, None).await.unwrap();

    let info = agent_info(&me, twenty_min, DhtArc::Empty);
    let Reply::AgentInfoSignature(sig) = ask_signature(&mut phone.linker, 2, info.clone())
        .await
        .unwrap()
    else {
        panic!("expected a signature");
    };
    // Kitsune2 signs exactly serde_json::to_string(&agent_info).
    let message = serde_json::to_string(&info).unwrap();
    let key = ed25519_dalek::VerifyingKey::from_bytes(me.get_raw_32().try_into().unwrap()).unwrap();
    key.verify(message.as_bytes(), &ed25519_dalek::Signature::from_bytes(&sig.0))
        .unwrap();

    let err = ask_signature(&mut phone.linker, 3, agent_info(&me, twenty_min, DhtArc::FULL))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Rejected, "a phone never claims an arc");

    let err = ask_signature(
        &mut phone.linker,
        4,
        agent_info(&me, Duration::from_secs(60 * 60), DhtArc::Empty),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Rejected, "no long-lived agent infos");
}

#[tokio::test(flavor = "multi_thread")]
async fn receipts_are_acknowledged_only_after_the_conductor_takes_them() {
    let mut phone = phone(done()).await;
    let deliver = |seq| Frame::Deliver {
        seq,
        item: InboxItem::ValidationReceipts {
            dna_hash: dna(),
            to_agent: agent(2),
            receipts: Vec::new().into(),
            received_at: Timestamp::from_micros(1),
        },
    };

    phone.linker.to_phone.send(deliver(1)).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), phone.linker.from_phone.recv())
            .await
            .is_err(),
        "no Ack before a handler is registered"
    );

    phone.p2p.register_handler(phone.handler.clone()).await.unwrap();
    phone.linker.to_phone.send(deliver(1)).await.unwrap();
    assert!(matches!(phone.linker.next_from_phone().await, Some(Frame::Ack { seq: 1 })));
    assert_eq!(*phone.handler.receipts.lock().unwrap(), vec![agent(2)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_signals_reach_the_conductor_as_signed_calls() {
    let phone = phone(done()).await;
    phone.p2p.register_handler(phone.handler.clone()).await.unwrap();
    let signature = Signature::from([7; 64]);
    phone
        .linker
        .to_phone
        .send(Frame::Notify(Notification::RemoteSignal {
            dna_hash: dna(),
            to_agent: agent(2),
            zome_call_params_serialized: ExternIO(vec![1]),
            signature: signature.clone(),
        }))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(*phone.handler.calls.lock().unwrap(), vec![(agent(2), signature)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn incoming_remote_calls_return_the_conductor_answer() {
    let mut phone = phone(done()).await;
    phone.p2p.register_handler(phone.handler.clone()).await.unwrap();
    phone
        .linker
        .to_phone
        .send(Frame::Call {
            id: 5,
            body: Call::CallRemote {
                dna_hash: dna(),
                to_agent: agent(2),
                zome_call_params_serialized: ExternIO(vec![1]),
                signature: Signature::from([7; 64]),
            },
        })
        .await
        .unwrap();
    match phone.linker.next_from_phone().await {
        Some(Frame::Reply { id: 5, body: Ok(Reply::CallRemote(bytes)) }) => {
            assert_eq!(bytes.bytes(), &vec![42]);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_fail_when_the_linker_goes_away() {
    let mut phone = phone(done()).await;
    phone.linker.close.take().unwrap().send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let hash: AnyDhtHash = ActionHash::from_raw_32(vec![5; 32]).into();
    let err = phone.p2p.get(dna(), hash, Default::default(), None).await.unwrap_err();
    assert_eq!(protocol_error(&err).unwrap().kind, ErrorKind::ConnectionClosed);
}

#[test]
fn acceptance_receipts_must_be_signed_and_complete() {
    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    let linker = LinkerId::from_signing_key(&key);
    let ops = vec![DhtOpHash::from_raw_32(vec![3; 32]), DhtOpHash::from_raw_32(vec![4; 32])];
    let body = AcceptanceBody {
        protocol_version: PROTOCOL_VERSION,
        linker,
        dna_hash: dna(),
        author: agent(2),
        op_hashes: ops.clone(),
        seq: 1,
        accepted_at: Timestamp::from_micros(1),
        retain_until: Timestamp::from_micros(2),
    };
    let receipt = AcceptanceReceipt::sign(&body, &key).unwrap();

    assert!(check_acceptance(&receipt, &linker, &dna(), &agent(2), &ops).is_ok());
    assert!(check_acceptance(&receipt, &linker, &dna(), &agent(3), &ops).is_err());
    let more = [ops.clone(), vec![DhtOpHash::from_raw_32(vec![5; 32])]].concat();
    assert!(check_acceptance(&receipt, &linker, &dna(), &agent(2), &more).is_err());
    let other = LinkerId::from_signing_key(&SigningKey::generate(&mut rand::rngs::OsRng));
    assert!(check_acceptance(&receipt, &other, &dna(), &agent(2), &ops).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnects_and_rejoins_after_the_linker_drops() {
    // Every connection gets a fresh scripted linker sharing one key.
    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    let (linkers_tx, mut linkers) = mpsc::unbounded_channel::<Linker>();
    let connector_key = key.clone();
    let connector: Connector = Arc::new(move || {
        let (chan, mut linker) = spawn_linker(done());
        linker.key = connector_key.clone();
        let _ = linkers_tx.send(linker);
        Box::pin(async move { Ok(chan) })
    });
    // spawn_linker signs Welcome with its own key, so don't pin one here.
    let p2p = H2hcP2p::connect(
        HolochainP2pConfig::default(),
        spawn_test_keystore().await.unwrap(),
        connector,
        H2hcOptions::default(),
    )
    .await
    .unwrap();
    let mut first = linkers.recv().await.unwrap();
    p2p.join(dna(), agent(2), None, None).await.unwrap();

    first.close.take().unwrap().send(()).unwrap();
    // Dropping the handle drops its sender too, so the phone sees the close.
    drop(first);
    let second = tokio::time::timeout(Duration::from_secs(5), linkers.recv())
        .await
        .expect("adapter reconnects")
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let rejoined = second
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| matches!(r, Request::Join { agent: a, .. } if *a == agent(2)));
        if rejoined {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "agent re-joined on the new connection");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    p2p.leave(dna(), agent(2)).await.unwrap();
    let _ = key;
}
