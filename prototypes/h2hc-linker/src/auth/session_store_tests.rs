//! Shared test functions exercised by both memory and SQLite store tests.

use holochain_types::prelude::{AgentPubKey, DnaHash};
use std::collections::HashSet;

use super::session_store::SessionStore;
use super::types::{AllowedAgent, Capability};

pub(crate) fn test_agent(seed: u8) -> AgentPubKey {
    AgentPubKey::from_raw_32(vec![seed; 32])
}

pub(crate) fn test_allowed_agent(seed: u8, caps: &[Capability]) -> AllowedAgent {
    AllowedAgent {
        agent_pubkey: test_agent(seed),
        capabilities: caps.iter().copied().collect(),
        label: None,
    }
}

pub(crate) async fn test_add_and_list_agents(store: &dyn SessionStore) {
    assert!(store.list_agents().await.unwrap().is_empty());

    store
        .add_agent(test_allowed_agent(1, &[Capability::DhtRead]))
        .await
        .unwrap();
    store
        .add_agent(test_allowed_agent(
            2,
            &[Capability::DhtWrite, Capability::K2],
        ))
        .await
        .unwrap();

    let agents = store.list_agents().await.unwrap();
    assert_eq!(agents.len(), 2);
}

pub(crate) async fn test_is_agent_allowed(store: &dyn SessionStore) {
    store
        .add_agent(test_allowed_agent(1, &[Capability::DhtRead]))
        .await
        .unwrap();

    assert!(store.is_agent_allowed(&test_agent(1)).await.unwrap());
    assert!(!store.is_agent_allowed(&test_agent(2)).await.unwrap());
}

pub(crate) async fn test_remove_agent(store: &dyn SessionStore) {
    store
        .add_agent(test_allowed_agent(1, &[Capability::DhtRead]))
        .await
        .unwrap();

    assert!(store.remove_agent(&test_agent(1)).await.unwrap());
    assert!(!store.is_agent_allowed(&test_agent(1)).await.unwrap());
    // Removing again returns false
    assert!(!store.remove_agent(&test_agent(1)).await.unwrap());
}

pub(crate) async fn test_create_session_for_allowed_agent(store: &dyn SessionStore) {
    store
        .add_agent(test_allowed_agent(
            1,
            &[Capability::DhtRead, Capability::K2],
        ))
        .await
        .unwrap();

    let token = store.create_session(&test_agent(1)).await.unwrap();
    assert!(token.is_some());

    let token = token.unwrap();
    let session = store.validate_session(token.as_str()).await.unwrap();
    assert!(session.is_some());

    let session = session.unwrap();
    assert_eq!(session.agent_pubkey, test_agent(1));
    assert!(session.has_capability(Capability::DhtRead));
    assert!(session.has_capability(Capability::K2));
    assert!(!session.has_capability(Capability::DhtWrite));
}

pub(crate) async fn test_create_session_for_unknown_agent(store: &dyn SessionStore) {
    let token = store.create_session(&test_agent(99)).await.unwrap();
    assert!(token.is_none());
}

pub(crate) async fn test_validate_invalid_token(store: &dyn SessionStore) {
    let session = store.validate_session("bogus-token").await.unwrap();
    assert!(session.is_none());
}

pub(crate) async fn test_revoke_session(store: &dyn SessionStore) {
    store
        .add_agent(test_allowed_agent(1, &[Capability::DhtRead]))
        .await
        .unwrap();

    let token = store.create_session(&test_agent(1)).await.unwrap().unwrap();
    assert!(store
        .validate_session(token.as_str())
        .await
        .unwrap()
        .is_some());

    assert!(store.revoke_session(token.as_str()).await.unwrap());
    assert!(store
        .validate_session(token.as_str())
        .await
        .unwrap()
        .is_none());

    // Revoking again returns false
    assert!(!store.revoke_session(token.as_str()).await.unwrap());
}

pub(crate) async fn test_remove_agent_revokes_all_sessions(store: &dyn SessionStore) {
    store
        .add_agent(test_allowed_agent(1, &[Capability::DhtRead]))
        .await
        .unwrap();

    let token1 = store.create_session(&test_agent(1)).await.unwrap().unwrap();
    let token2 = store.create_session(&test_agent(1)).await.unwrap().unwrap();

    assert!(store
        .validate_session(token1.as_str())
        .await
        .unwrap()
        .is_some());
    assert!(store
        .validate_session(token2.as_str())
        .await
        .unwrap()
        .is_some());

    store.remove_agent(&test_agent(1)).await.unwrap();

    assert!(store
        .validate_session(token1.as_str())
        .await
        .unwrap()
        .is_none());
    assert!(store
        .validate_session(token2.as_str())
        .await
        .unwrap()
        .is_none());
}

pub(crate) async fn test_register_dna_for_agent(store: &dyn SessionStore) {
    store
        .add_agent(test_allowed_agent(1, &[Capability::DhtRead]))
        .await
        .unwrap();

    let token = store.create_session(&test_agent(1)).await.unwrap().unwrap();

    let dna1 = DnaHash::from_raw_32(vec![1u8; 32]);
    let dna2 = DnaHash::from_raw_32(vec![2u8; 32]);

    // Initially no DNAs registered
    let session = store
        .validate_session(token.as_str())
        .await
        .unwrap()
        .unwrap();
    assert!(!session.has_dna(&dna1));

    // Register DNA
    store
        .register_dna_for_agent(&test_agent(1), &dna1)
        .await
        .unwrap();
    let session = store
        .validate_session(token.as_str())
        .await
        .unwrap()
        .unwrap();
    assert!(session.has_dna(&dna1));
    assert!(!session.has_dna(&dna2));

    // Register second DNA
    store
        .register_dna_for_agent(&test_agent(1), &dna2)
        .await
        .unwrap();
    let session = store
        .validate_session(token.as_str())
        .await
        .unwrap()
        .unwrap();
    assert!(session.has_dna(&dna1));
    assert!(session.has_dna(&dna2));
}

pub(crate) async fn test_revoke_sessions_for_agent(store: &dyn SessionStore) {
    store
        .add_agent(test_allowed_agent(1, &[Capability::DhtRead]))
        .await
        .unwrap();
    store
        .add_agent(test_allowed_agent(2, &[Capability::DhtRead]))
        .await
        .unwrap();

    let token1 = store.create_session(&test_agent(1)).await.unwrap().unwrap();
    let token2 = store.create_session(&test_agent(1)).await.unwrap().unwrap();
    let token3 = store.create_session(&test_agent(2)).await.unwrap().unwrap();
    assert_eq!(store.session_count().await.unwrap(), 3);

    let removed = store
        .revoke_sessions_for_agent(&test_agent(1))
        .await
        .unwrap();
    assert_eq!(removed, 2);
    assert!(store
        .validate_session(token1.as_str())
        .await
        .unwrap()
        .is_none());
    assert!(store
        .validate_session(token2.as_str())
        .await
        .unwrap()
        .is_none());
    // Agent 2's session is untouched
    assert!(store
        .validate_session(token3.as_str())
        .await
        .unwrap()
        .is_some());
}

pub(crate) async fn test_agent_with_label(store: &dyn SessionStore) {
    let agent = AllowedAgent {
        agent_pubkey: test_agent(1),
        capabilities: HashSet::from([Capability::DhtRead]),
        label: Some("test-browser".to_string()),
    };
    store.add_agent(agent).await.unwrap();

    let retrieved = store.get_agent(&test_agent(1)).await.unwrap().unwrap();
    assert_eq!(retrieved.label.as_deref(), Some("test-browser"));
}

pub(crate) async fn test_add_agent_overwrites(store: &dyn SessionStore) {
    store
        .add_agent(test_allowed_agent(1, &[Capability::DhtRead]))
        .await
        .unwrap();
    // Overwrite with different capabilities
    store
        .add_agent(test_allowed_agent(
            1,
            &[Capability::DhtWrite, Capability::K2],
        ))
        .await
        .unwrap();

    let agent = store.get_agent(&test_agent(1)).await.unwrap().unwrap();
    assert!(agent.capabilities.contains(&Capability::DhtWrite));
    assert!(agent.capabilities.contains(&Capability::K2));
    assert!(!agent.capabilities.contains(&Capability::DhtRead));
}

/// Macro to generate tokio test wrappers for a given store constructor.
macro_rules! session_store_test_suite {
    ($create_store:expr) => {
        use crate::auth::session_store_tests;

        #[tokio::test]
        async fn test_add_and_list_agents() {
            let store = $create_store;
            session_store_tests::test_add_and_list_agents(&store).await;
        }

        #[tokio::test]
        async fn test_is_agent_allowed() {
            let store = $create_store;
            session_store_tests::test_is_agent_allowed(&store).await;
        }

        #[tokio::test]
        async fn test_remove_agent() {
            let store = $create_store;
            session_store_tests::test_remove_agent(&store).await;
        }

        #[tokio::test]
        async fn test_create_session_for_allowed_agent() {
            let store = $create_store;
            session_store_tests::test_create_session_for_allowed_agent(&store).await;
        }

        #[tokio::test]
        async fn test_create_session_for_unknown_agent() {
            let store = $create_store;
            session_store_tests::test_create_session_for_unknown_agent(&store).await;
        }

        #[tokio::test]
        async fn test_validate_invalid_token() {
            let store = $create_store;
            session_store_tests::test_validate_invalid_token(&store).await;
        }

        #[tokio::test]
        async fn test_revoke_session() {
            let store = $create_store;
            session_store_tests::test_revoke_session(&store).await;
        }

        #[tokio::test]
        async fn test_remove_agent_revokes_all_sessions() {
            let store = $create_store;
            session_store_tests::test_remove_agent_revokes_all_sessions(&store).await;
        }

        #[tokio::test]
        async fn test_register_dna_for_agent() {
            let store = $create_store;
            session_store_tests::test_register_dna_for_agent(&store).await;
        }

        #[tokio::test]
        async fn test_revoke_sessions_for_agent() {
            let store = $create_store;
            session_store_tests::test_revoke_sessions_for_agent(&store).await;
        }

        #[tokio::test]
        async fn test_agent_with_label() {
            let store = $create_store;
            session_store_tests::test_agent_with_label(&store).await;
        }

        #[tokio::test]
        async fn test_add_agent_overwrites() {
            let store = $create_store;
            session_store_tests::test_add_agent_overwrites(&store).await;
        }
    };
}
