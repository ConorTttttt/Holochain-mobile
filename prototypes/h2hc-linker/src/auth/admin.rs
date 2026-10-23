//! Admin API handlers for agent management.
//!
//! Protected by `require_admin_secret` middleware.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use holo_hash::AgentPubKeyB64;
use holochain_types::prelude::AgentPubKey;
use serde::{Deserialize, Serialize};

use super::types::{AllowedAgent, Capability};
use crate::service::AppState;

/// Request body for adding/updating an allowed agent.
///
/// `agent_pubkey` is a HoloHash base64 string (e.g. "uhCAk..."), matching
/// the format used by the WebSocket protocol and the joining service.
#[derive(Debug, Deserialize)]
pub struct AddAgentRequest {
    pub agent_pubkey: String,
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub label: Option<String>,
}

/// Request body for removing an agent.
#[derive(Debug, Deserialize)]
pub struct RemoveAgentRequest {
    pub agent_pubkey: String,
}

/// Response for listing agents.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentListResponse {
    pub agents: Vec<AgentResponse>,
}

/// Agent info in list response.
///
/// `agent_pubkey` is serialized as a HoloHash base64 string.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentResponse {
    pub agent_pubkey: AgentPubKeyB64,
    pub capabilities: Vec<Capability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// POST /admin/agents - add or update an allowed agent.
pub async fn add_agent(
    State(state): State<AppState>,
    Json(body): Json<AddAgentRequest>,
) -> impl IntoResponse {
    let Some(ref auth_store) = state.auth_store else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Auth not configured").into_response();
    };

    let agent_pubkey = match AgentPubKey::try_from(body.agent_pubkey.as_str()) {
        Ok(k) => k,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Invalid agent_pubkey: {e:?}"),
            )
                .into_response();
        }
    };

    let agent = AllowedAgent {
        agent_pubkey,
        capabilities: body.capabilities.into_iter().collect(),
        label: body.label,
    };

    auth_store.add_agent(agent).await;
    StatusCode::NO_CONTENT.into_response()
}

/// DELETE /admin/agents - remove an agent, revoke sessions, close WS.
pub async fn remove_agent(
    State(state): State<AppState>,
    Json(body): Json<RemoveAgentRequest>,
) -> impl IntoResponse {
    let Some(ref auth_store) = state.auth_store else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Auth not configured").into_response();
    };

    let agent_pubkey = match AgentPubKey::try_from(body.agent_pubkey.as_str()) {
        Ok(k) => k,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Invalid agent_pubkey: {e:?}"),
            )
                .into_response();
        }
    };

    let removed = auth_store.remove_agent(&agent_pubkey).await;
    if removed {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (StatusCode::NOT_FOUND, "Agent not found").into_response()
    }
}

/// GET /admin/agents - list all allowed agents.
pub async fn list_agents(State(state): State<AppState>) -> impl IntoResponse {
    let Some(ref auth_store) = state.auth_store else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Auth not configured").into_response();
    };

    let agents = auth_store.list_agents().await;
    let response = AgentListResponse {
        agents: agents
            .into_iter()
            .map(|a| AgentResponse {
                agent_pubkey: a.agent_pubkey.into(),
                capabilities: a.capabilities.into_iter().collect(),
                label: a.label,
            })
            .collect(),
    };

    Json(response).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::middleware::require_admin_secret;
    use crate::auth::AuthStore;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::{delete, get, post};
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_app_state(auth_store: AuthStore) -> AppState {
        use crate::agent_proxy::AgentProxyManager;
        use crate::config::Configuration;
        use crate::routes::kitsune::KitsuneState;
        use std::sync::Arc;

        let config = Configuration {
            admin_secret: Some("test-secret".to_string()),
            ..Default::default()
        };

        AppState {
            configuration: config,
            agent_proxy: AgentProxyManager::new(),
            gateway_kitsune: None,
            kitsune_state: Arc::new(KitsuneState {
                enabled: false,
                bootstrap_url: None,
                relay_url: None,
                kitsune: None,
            }),
            app_conn: None,
            temp_op_store: None,
            dht_query: None,
            auth_store: Some(auth_store),
        }
    }

    fn build_admin_router(state: AppState) -> Router {
        let admin_routes = Router::new()
            .route("/agents", post(add_agent))
            .route("/agents", delete(remove_agent))
            .route("/agents", get(list_agents))
            .route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                require_admin_secret,
            ));

        Router::new().nest("/admin", admin_routes).with_state(state)
    }

    #[tokio::test]
    async fn test_add_agent_request_deserialization() {
        let agent = AgentPubKey::from_raw_32(vec![42u8; 32]);
        let json = serde_json::json!({
            "agent_pubkey": agent.to_string(),
            "capabilities": ["dht_read", "k2"],
            "label": "Test Agent"
        });
        let req: AddAgentRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.agent_pubkey, agent.to_string());
        assert_eq!(req.capabilities.len(), 2);
        assert_eq!(req.label, Some("Test Agent".to_string()));
    }

    #[tokio::test]
    async fn test_add_and_list_agents_roundtrip() {
        let store = AuthStore::new();
        let state = test_app_state(store);
        let app = build_admin_router(state);

        let agent_pubkey = AgentPubKey::from_raw_32(vec![1u8; 32]);
        let body = serde_json::json!({
            "agent_pubkey": agent_pubkey.to_string(),
            "capabilities": ["dht_read", "dht_write"],
            "label": "My Agent"
        });

        let resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/admin/agents")
                    .header("authorization", "Bearer test-secret")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // List agents - response should contain base64 strings
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/admin/agents")
                    .header("authorization", "Bearer test-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        // Use raw JSON to avoid hc_serde_json HoloHash string deserialization issues
        let raw_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(raw_json["agents"].as_array().unwrap().len(), 1);
        assert_eq!(raw_json["agents"][0]["label"], "My Agent");
        // Verify the response contains a base64 string, not a byte array
        assert!(raw_json["agents"][0]["agent_pubkey"].is_string());
    }

    #[tokio::test]
    async fn test_remove_agent() {
        let store = AuthStore::new();
        let agent_pubkey = AgentPubKey::from_raw_32(vec![1u8; 32]);

        store
            .add_agent(AllowedAgent {
                agent_pubkey: agent_pubkey.clone(),
                capabilities: [Capability::DhtRead].into_iter().collect(),
                label: None,
            })
            .await;

        let state = test_app_state(store);
        let app = build_admin_router(state);

        let body = serde_json::json!({ "agent_pubkey": agent_pubkey.to_string() });

        let resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("DELETE")
                    .uri("/admin/agents")
                    .header("authorization", "Bearer test-secret")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // List should now be empty
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/admin/agents")
                    .header("authorization", "Bearer test-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let raw_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(raw_json["agents"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_remove_nonexistent_agent_returns_404() {
        let store = AuthStore::new();
        let state = test_app_state(store);
        let app = build_admin_router(state);

        let body = serde_json::json!({ "agent_pubkey": AgentPubKey::from_raw_32(vec![99u8; 32]).to_string() });

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("DELETE")
                    .uri("/admin/agents")
                    .header("authorization", "Bearer test-secret")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_admin_requires_secret() {
        let store = AuthStore::new();
        let state = test_app_state(store);
        let app = build_admin_router(state);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/admin/agents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
