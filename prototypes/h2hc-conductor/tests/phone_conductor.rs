//! A real conductor running on the H2HC adapter, with no Kitsune2, against an
//! in-memory linker. Needs `scripts/build-fixtures.sh` to have been run.

mod common;

use common::*;
use h2hc_protocol::*;
use holochain::prelude::*;
use holochain_keystore::{spawn_test_keystore, ValidationReceiptExt};
use std::collections::HashSet;
use std::time::Duration;

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

/// The fixture's signal from its scheduled functions.
#[derive(serde::Deserialize, Debug)]
enum Tick {
    Ephemeral,
    Cron,
}

/// Scheduled functions still run when the scheduler sleeps until one is due
/// instead of polling: `schedule` wakes it, an ephemeral function re-arms on
/// time and no faster, and a persisted crontab keeps firing.
#[tokio::test(flavor = "multi_thread")]
async fn scheduled_functions_run() {
    let mut phone = phone().await;
    let (agent, cell_id) = install(&mut phone).await;
    let mut signals = phone.conductor.subscribe_to_app_signals("notes".into());
    let () = phone
        .conductor
        .easy_call_zome(&agent, None, cell_id, "notes", "start_ticks", ())
        .await
        .unwrap();
    let started = tokio::time::Instant::now();
    let (mut ephemeral, mut cron) = (Vec::new(), 0);
    while (ephemeral.len() < 5 || cron < 2) && started.elapsed() < Duration::from_secs(20) {
        let Ok(Ok(Signal::App { signal, .. })) = tokio::time::timeout(Duration::from_secs(1), signals.recv()).await
        else {
            continue;
        };
        match signal.into_inner().decode::<Tick>() {
            Ok(Tick::Ephemeral) => ephemeral.push(started.elapsed()),
            Ok(Tick::Cron) => cron += 1,
            Err(_) => {}
        }
    }
    assert!(ephemeral.len() >= 5, "ephemeral ticks at {ephemeral:?}");
    assert!(cron >= 2, "{cron} cron ticks in 20 s");
    assert!(ephemeral[0] < Duration::from_secs(2), "first tick only after {:?}", ephemeral[0]);
    for pair in ephemeral.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap >= Duration::from_millis(250) && gap < Duration::from_millis(1500),
            "a 300 ms ephemeral schedule re-ran after {gap:?}"
        );
    }
}
