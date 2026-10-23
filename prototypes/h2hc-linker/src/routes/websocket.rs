//! WebSocket handler for browser extension connections.
//!
//! This module provides a WebSocket endpoint for browser extensions to:
//! - Authenticate using session tokens
//! - Register agents for specific DNAs
//! - Receive signals forwarded from the Holochain network

use crate::agent_proxy::WsSender;
use crate::service::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::Response,
};
use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use futures::{SinkExt, StreamExt};
use holochain_types::prelude::{AgentPubKey, DnaHash};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::time::interval;

/// Messages sent from browser to gateway.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Authenticate the connection with an agent pubkey.
    /// When auth is enabled, starts challenge-response flow.
    /// When auth is disabled, immediately returns AuthOk.
    Auth {
        /// Agent public key (HoloHash string).
        agent_pubkey: String,
    },
    /// Response to an auth challenge (sign the nonce).
    AuthChallengeResponse {
        /// Ed25519 signature of the challenge (base64 encoded, 64 bytes).
        signature: String,
    },
    /// Register an agent for a specific DNA to receive signals.
    Register {
        /// DNA hash (base64 encoded).
        dna_hash: String,
        /// Agent public key (base64 encoded).
        agent_pubkey: String,
    },
    /// Unregister an agent from a DNA.
    Unregister {
        /// DNA hash (base64 encoded).
        dna_hash: String,
        /// Agent public key (base64 encoded).
        agent_pubkey: String,
    },
    /// Ping for heartbeat.
    Ping,
    /// Response to a signing request.
    SignResponse {
        /// Request ID to correlate with the original request.
        request_id: String,
        /// The signature (base64 encoded), if successful.
        signature: Option<String>,
        /// Error message if signing failed.
        error: Option<String>,
    },
    /// Send remote signals to target agents via kitsune2.
    SendRemoteSignal {
        /// DNA hash (base64 encoded).
        dna_hash: String,
        /// Signed signals to send.
        signals: Vec<SignedRemoteSignalInput>,
    },
}

/// Signed remote signal input from browser.
#[derive(Debug, Clone, Deserialize)]
pub struct SignedRemoteSignalInput {
    /// Target agent public key (as byte array).
    pub target_agent: Vec<u8>,
    /// Serialized ZomeCallParams (as byte array).
    pub zome_call_params: Vec<u8>,
    /// Ed25519 signature (64 bytes, as byte array).
    pub signature: Vec<u8>,
}

/// Messages sent from gateway to browser.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Authentication succeeded. Includes session token for HTTP Bearer auth.
    AuthOk {
        /// Session token for HTTP requests. Empty string when auth is disabled.
        session_token: String,
    },
    /// Authentication challenge - client must sign this nonce.
    AuthChallenge {
        /// Random nonce to sign (hex encoded, 32 bytes).
        challenge: String,
    },
    /// Authentication failed.
    AuthError {
        /// Error message.
        message: String,
    },
    /// Agent registration confirmed.
    Registered {
        /// DNA hash.
        dna_hash: String,
        /// Agent public key.
        agent_pubkey: String,
    },
    /// Agent unregistration confirmed.
    Unregistered {
        /// DNA hash.
        dna_hash: String,
        /// Agent public key.
        agent_pubkey: String,
    },
    /// Signal forwarded from the network.
    Signal {
        /// DNA hash.
        dna_hash: String,
        /// Target agent (the local agent this signal is addressed to).
        to_agent: String,
        /// Sender agent.
        from_agent: String,
        /// Zome that emitted the signal.
        zome_name: String,
        /// Signal payload (base64 encoded msgpack).
        signal: String,
    },
    /// Pong response to ping. Includes network peer count.
    Pong {
        /// Number of peers known to kitsune2 across all spaces.
        peer_count: usize,
    },
    /// Error message.
    Error {
        /// Error description.
        message: String,
    },
    /// Request browser to sign agent info with agent's private key.
    /// Sends structured agent info fields so the browser can validate
    /// what it's signing (transparent signing protocol).
    SignAgentInfo {
        /// Unique request ID for correlating response.
        request_id: String,
        /// Agent public key that should sign (HoloHash base64 encoded).
        agent_pubkey: String,
        /// Structured agent info fields for the browser to validate and
        /// reconstruct the canonical JSON to sign.
        agent_info: serde_json::Value,
    },
}

/// Connection state for a WebSocket client.
#[derive(Debug)]
struct ConnectionState {
    /// Whether the client has authenticated.
    authenticated: bool,
    /// Last activity timestamp.
    last_activity: Instant,
    /// Registered agent-DNA pairs using proper Holochain types.
    registrations: Vec<(DnaHash, AgentPubKey)>,
    /// Agent waiting for challenge response (auth flow step 1 complete).
    pending_auth_agent: Option<AgentPubKey>,
    /// Challenge nonce waiting for signature (auth flow step 1 complete).
    pending_challenge: Option<Vec<u8>>,
    /// The authenticated agent (after successful challenge-response).
    authenticated_agent: Option<AgentPubKey>,
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self {
            authenticated: false,
            last_activity: Instant::now(),
            registrations: Vec::new(),
            pending_auth_agent: None,
            pending_challenge: None,
            authenticated_agent: None,
        }
    }
}

/// WebSocket upgrade handler.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Handle an upgraded WebSocket connection.
async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut conn_state = ConnectionState::default();

    // Get config values
    let heartbeat_interval = state.configuration.websocket.heartbeat_interval;
    let heartbeat_timeout = state.configuration.websocket.heartbeat_timeout;
    let idle_timeout = state.configuration.websocket.idle_timeout;

    // Channel for sending messages to the client
    let (tx, mut rx) = mpsc::channel::<ServerMessage>(32);

    // Spawn task to forward messages from channel to WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    tracing::error!("Failed to serialize message: {}", e);
                    continue;
                }
            };
            if sender.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // Create heartbeat interval timer
    let mut heartbeat = interval(heartbeat_interval);
    let mut last_pong = Instant::now();

    loop {
        tokio::select! {
            // Handle incoming messages
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        conn_state.last_activity = Instant::now();
                        // Update last_pong on any text message - proves client is alive
                        last_pong = Instant::now();

                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(client_msg) => {
                                tracing::debug!(?client_msg, "Received WebSocket message");
                                let response = handle_client_message(
                                    client_msg,
                                    &mut conn_state,
                                    &state,
                                    &tx,
                                ).await;

                                if let Some(resp) = response {
                                    if tx.send(resp).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = tx.send(ServerMessage::Error {
                                    message: format!("Invalid message format: {e}"),
                                }).await;
                            }
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        last_pong = Instant::now();
                        conn_state.last_activity = Instant::now();
                    }
                    Some(Ok(Message::Close(_))) => {
                        tracing::debug!("Client closed connection");
                        break;
                    }
                    Some(Err(e)) => {
                        tracing::warn!("WebSocket error: {}", e);
                        break;
                    }
                    None => {
                        tracing::debug!("WebSocket stream ended");
                        break;
                    }
                    _ => {}
                }
            }

            // Heartbeat tick
            _ = heartbeat.tick() => {
                // Check if we received a pong recently
                if last_pong.elapsed() > heartbeat_interval + heartbeat_timeout {
                    tracing::debug!("Client heartbeat timeout");
                    break;
                }

                // Check idle timeout
                if conn_state.last_activity.elapsed() > idle_timeout {
                    tracing::debug!("Client idle timeout");
                    break;
                }
            }
        }
    }

    // Cleanup: unregister all agents from the proxy manager
    state.agent_proxy.unregister_all(&tx).await;

    // If kitsune2 is configured, leave all agents from their spaces
    if let Some(ref gateway_kitsune) = state.gateway_kitsune {
        for (dna_hash, agent_pubkey) in &conn_state.registrations {
            gateway_kitsune.agent_leave(dna_hash, agent_pubkey).await;
        }
    }

    // Unregister WS sender from auth store but keep sessions alive.
    // Sessions are NOT revoked on disconnect — the browser will reconnect
    // momentarily (especially through Cloudflare tunnels) and HTTP requests
    // should continue working with the existing session token in the meantime.
    // New sessions created on reconnect will replace the old ones naturally.
    if let (Some(ref auth_store), Some(ref agent)) =
        (&state.auth_store, &conn_state.authenticated_agent)
    {
        auth_store.unregister_ws_sender(agent, &tx).await;
    }

    // Wait for send task to complete
    send_task.abort();
}

/// Handle a client message and return an optional response.
async fn handle_client_message(
    msg: ClientMessage,
    state: &mut ConnectionState,
    app_state: &AppState,
    sender: &WsSender,
) -> Option<ServerMessage> {
    match msg {
        ClientMessage::Auth { agent_pubkey } => {
            handle_auth(agent_pubkey, state, app_state, sender).await
        }

        ClientMessage::AuthChallengeResponse { signature } => {
            handle_auth_challenge_response(signature, state, app_state, sender).await
        }

        ClientMessage::Register {
            dna_hash,
            agent_pubkey,
        } => {
            if !state.authenticated {
                return Some(ServerMessage::Error {
                    message: "Not authenticated".to_string(),
                });
            }

            // Parse browser base64 strings to proper Holochain types at the boundary.
            // HoloHash uses URL-safe base64 with a 'u' prefix.
            let dna = match DnaHash::try_from(dna_hash.as_str()) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(
                        dna = %dna_hash,
                        error = ?e,
                        "Failed to parse DNA hash"
                    );
                    return Some(ServerMessage::Error {
                        message: format!("Invalid DNA hash: {e:?}"),
                    });
                }
            };

            let agent = match AgentPubKey::try_from(agent_pubkey.as_str()) {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(
                        agent = %agent_pubkey,
                        error = ?e,
                        "Failed to parse agent pubkey"
                    );
                    return Some(ServerMessage::Error {
                        message: format!("Invalid agent pubkey: {e:?}"),
                    });
                }
            };

            // When auth is enabled, verify the registered agent matches the authenticated agent
            if app_state.auth_store.is_some() {
                if let Some(ref auth_agent) = state.authenticated_agent {
                    if &agent != auth_agent {
                        return Some(ServerMessage::Error {
                            message: "Cannot register a different agent than authenticated"
                                .to_string(),
                        });
                    }
                }
            }

            // Check if already registered locally
            let key = (dna.clone(), agent.clone());
            if !state.registrations.contains(&key) {
                state.registrations.push(key);
            }

            // Register with agent proxy manager to receive signals
            app_state
                .agent_proxy
                .register(dna.clone(), agent.clone(), sender.clone())
                .await;

            // Register DNA in auth session so HTTP requests are scoped to this cell
            if let Some(ref auth_store) = app_state.auth_store {
                auth_store.register_dna_for_agent(&agent, &dna).await;
            }

            // If kitsune2 is configured, join the agent to the space
            if let Some(ref gateway_kitsune) = app_state.gateway_kitsune {
                tracing::info!(
                    dna = %dna,
                    agent = %agent,
                    "Joining agent to kitsune2 space"
                );

                if let Err(e) = gateway_kitsune.agent_join(&dna, &agent).await {
                    tracing::warn!(
                        dna = %dna,
                        agent = %agent,
                        error = %e,
                        "Failed to join agent to kitsune2 space"
                    );
                } else {
                    tracing::info!(
                        dna = %dna,
                        agent = %agent,
                        "Successfully joined agent to kitsune2 space"
                    );
                }
            }

            Some(ServerMessage::Registered {
                dna_hash,
                agent_pubkey,
            })
        }

        ClientMessage::Unregister {
            dna_hash,
            agent_pubkey,
        } => {
            if !state.authenticated {
                return Some(ServerMessage::Error {
                    message: "Not authenticated".to_string(),
                });
            }

            // Parse browser base64 strings to proper Holochain types at the boundary.
            let dna = match DnaHash::try_from(dna_hash.as_str()) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(
                        dna = %dna_hash,
                        error = ?e,
                        "Failed to parse DNA hash for unregister"
                    );
                    return Some(ServerMessage::Error {
                        message: format!("Invalid DNA hash: {e:?}"),
                    });
                }
            };

            let agent = match AgentPubKey::try_from(agent_pubkey.as_str()) {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(
                        agent = %agent_pubkey,
                        error = ?e,
                        "Failed to parse agent pubkey for unregister"
                    );
                    return Some(ServerMessage::Error {
                        message: format!("Invalid agent pubkey: {e:?}"),
                    });
                }
            };

            let key = (dna.clone(), agent.clone());
            state.registrations.retain(|r| r != &key);

            // Unregister from agent proxy manager
            app_state.agent_proxy.unregister(&dna, &agent).await;

            // If kitsune2 is configured, leave the agent from the space
            if let Some(ref gateway_kitsune) = app_state.gateway_kitsune {
                gateway_kitsune.agent_leave(&dna, &agent).await;
            }

            Some(ServerMessage::Unregistered {
                dna_hash,
                agent_pubkey,
            })
        }

        ClientMessage::Ping => {
            let peer_count = if let Some(ref gw_kitsune) = app_state.gateway_kitsune {
                gw_kitsune.peer_count().await
            } else {
                0
            };
            Some(ServerMessage::Pong { peer_count })
        }

        ClientMessage::SignResponse {
            request_id,
            signature,
            error,
        } => {
            // Deliver the signature response to the pending request
            let result = match (signature, error) {
                (Some(sig_b64), _) => {
                    // Decode the base64 signature
                    match base64::engine::general_purpose::STANDARD.decode(&sig_b64) {
                        Ok(sig_bytes) => Ok(bytes::Bytes::from(sig_bytes)),
                        Err(e) => Err(format!("Invalid signature encoding: {e}")),
                    }
                }
                (None, Some(err)) => Err(err),
                (None, None) => Err("No signature or error provided".to_string()),
            };

            app_state
                .agent_proxy
                .deliver_signature(&request_id, result)
                .await;

            // No response needed for sign_response
            None
        }

        ClientMessage::SendRemoteSignal { dna_hash, signals } => {
            // Check if authenticated
            if !state.authenticated {
                tracing::warn!("send_remote_signal received before authentication");
                return Some(ServerMessage::Error {
                    message: "Must authenticate before sending signals".to_string(),
                });
            }

            // Check if kitsune2 is enabled
            if let Some(ref gateway_kitsune) = app_state.gateway_kitsune {
                // Parse DNA hash from base64 string
                let dna = match DnaHash::try_from(dna_hash.as_str()) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(?e, dna = %dna_hash, "Invalid DNA hash in send_remote_signal");
                        return Some(ServerMessage::Error {
                            message: format!("Invalid DNA hash: {e:?}"),
                        });
                    }
                };

                // Forward to kitsune2
                let signal_count = signals.len();
                let (success, failed) = gateway_kitsune.send_remote_signals(&dna, signals).await;
                tracing::info!(
                    total = signal_count,
                    success,
                    failed,
                    "send_remote_signal complete"
                );

                // No response needed (fire-and-forget)
                None
            } else {
                tracing::warn!("send_remote_signal received but kitsune2 not enabled");
                Some(ServerMessage::Error {
                    message: "Remote signals not available (kitsune2 not enabled)".to_string(),
                })
            }
        }
    }
}

/// Handle the Auth message.
/// When auth is disabled: immediately authenticate and return AuthOk with empty token.
/// When auth is enabled: verify agent is allowed, generate challenge.
async fn handle_auth(
    agent_pubkey_str: String,
    state: &mut ConnectionState,
    app_state: &AppState,
    _sender: &WsSender,
) -> Option<ServerMessage> {
    // Parse agent pubkey
    let agent = match AgentPubKey::try_from(agent_pubkey_str.as_str()) {
        Ok(a) => a,
        Err(e) => {
            return Some(ServerMessage::AuthError {
                message: format!("Invalid agent pubkey: {e:?}"),
            });
        }
    };

    // If auth is disabled, auto-accept
    let Some(ref auth_store) = app_state.auth_store else {
        state.authenticated = true;
        state.authenticated_agent = Some(agent);
        return Some(ServerMessage::AuthOk {
            session_token: String::new(),
        });
    };

    // Check if agent is in the allowed list
    if !auth_store.is_agent_allowed(&agent).await {
        tracing::warn!("WS auth rejected: agent {} not in allowlist", agent);
        return Some(ServerMessage::AuthError {
            message: "Agent not allowed".to_string(),
        });
    }

    // Generate 32-byte random challenge
    let challenge: [u8; 32] = rand::rng().random();

    state.pending_auth_agent = Some(agent);
    state.pending_challenge = Some(challenge.to_vec());

    Some(ServerMessage::AuthChallenge {
        challenge: hex::encode(challenge),
    })
}

/// Handle the AuthChallengeResponse message.
/// Verify the signature against the pending challenge and agent pubkey.
async fn handle_auth_challenge_response(
    signature_b64: String,
    state: &mut ConnectionState,
    app_state: &AppState,
    sender: &WsSender,
) -> Option<ServerMessage> {
    // Must have a pending challenge
    let Some(agent) = state.pending_auth_agent.take() else {
        return Some(ServerMessage::AuthError {
            message: "No pending auth challenge".to_string(),
        });
    };
    let Some(challenge) = state.pending_challenge.take() else {
        return Some(ServerMessage::AuthError {
            message: "No pending auth challenge".to_string(),
        });
    };

    // Decode the signature
    let sig_bytes = match base64::engine::general_purpose::STANDARD.decode(&signature_b64) {
        Ok(b) => b,
        Err(e) => {
            return Some(ServerMessage::AuthError {
                message: format!("Invalid signature encoding: {e}"),
            });
        }
    };

    // Parse as ed25519 signature (64 bytes)
    let signature = match Signature::from_slice(&sig_bytes) {
        Ok(s) => s,
        Err(e) => {
            return Some(ServerMessage::AuthError {
                message: format!("Invalid signature format: {e}"),
            });
        }
    };

    // Extract the 32 core bytes from the AgentPubKey for ed25519 verification
    let raw_key = agent.get_raw_32();
    let verifying_key = match VerifyingKey::from_bytes(raw_key.try_into().unwrap_or(&[0u8; 32])) {
        Ok(k) => k,
        Err(e) => {
            return Some(ServerMessage::AuthError {
                message: format!("Invalid agent public key for verification: {e}"),
            });
        }
    };

    // Verify the signature
    use ed25519_dalek::Verifier;
    if let Err(e) = verifying_key.verify(&challenge, &signature) {
        tracing::warn!(
            "Auth signature verification failed for agent {}: {e}",
            agent
        );
        return Some(ServerMessage::AuthError {
            message: "Signature verification failed".to_string(),
        });
    }

    // Auth succeeded - create session
    let auth_store = app_state.auth_store.as_ref().unwrap();
    let Some(session_token) = auth_store.create_session(&agent).await else {
        tracing::warn!("Failed to create session for agent {}", agent);
        return Some(ServerMessage::AuthError {
            message: "Failed to create session".to_string(),
        });
    };

    // Register WS sender for forced disconnect on agent removal
    auth_store.register_ws_sender(&agent, sender.clone()).await;

    state.authenticated = true;
    state.authenticated_agent = Some(agent.clone());

    tracing::info!("Auth OK for agent {}, session token issued", agent);

    Some(ServerMessage::AuthOk {
        session_token: session_token.as_str().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_message_auth_deserialization() {
        let auth_json = r#"{"type": "auth", "agent_pubkey": "uhCAkAQEB"}"#;
        let msg: ClientMessage = serde_json::from_str(auth_json).unwrap();
        assert!(matches!(msg, ClientMessage::Auth { agent_pubkey } if agent_pubkey == "uhCAkAQEB"));
    }

    #[test]
    fn test_client_message_auth_challenge_response_deserialization() {
        let json = r#"{"type": "auth_challenge_response", "signature": "AQID"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(msg, ClientMessage::AuthChallengeResponse { signature } if signature == "AQID")
        );
    }

    #[test]
    fn test_client_message_register_deserialization() {
        let register_json = r#"{"type": "register", "dna_hash": "dna1", "agent_pubkey": "agent1"}"#;
        let msg: ClientMessage = serde_json::from_str(register_json).unwrap();
        assert!(
            matches!(msg, ClientMessage::Register { dna_hash, agent_pubkey }
            if dna_hash == "dna1" && agent_pubkey == "agent1")
        );
    }

    #[test]
    fn test_client_message_unregister_deserialization() {
        let unregister_json =
            r#"{"type": "unregister", "dna_hash": "dna1", "agent_pubkey": "agent1"}"#;
        let msg: ClientMessage = serde_json::from_str(unregister_json).unwrap();
        assert!(
            matches!(msg, ClientMessage::Unregister { dna_hash, agent_pubkey }
            if dna_hash == "dna1" && agent_pubkey == "agent1")
        );
    }

    #[test]
    fn test_client_message_ping_deserialization() {
        let ping_json = r#"{"type": "ping"}"#;
        let msg: ClientMessage = serde_json::from_str(ping_json).unwrap();
        assert!(matches!(msg, ClientMessage::Ping));
    }

    #[test]
    fn test_client_message_invalid_type() {
        let invalid_json = r#"{"type": "invalid_type"}"#;
        let result: Result<ClientMessage, _> = serde_json::from_str(invalid_json);
        assert!(result.is_err());
    }

    #[test]
    fn test_server_message_auth_ok() {
        let msg = ServerMessage::AuthOk {
            session_token: "abc123".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"auth_ok""#));
        assert!(json.contains(r#""session_token":"abc123""#));
    }

    #[test]
    fn test_server_message_auth_ok_empty_token() {
        let msg = ServerMessage::AuthOk {
            session_token: String::new(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"auth_ok""#));
        assert!(json.contains(r#""session_token":"""#));
    }

    #[test]
    fn test_server_message_auth_challenge() {
        let msg = ServerMessage::AuthChallenge {
            challenge: "deadbeef".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"auth_challenge""#));
        assert!(json.contains(r#""challenge":"deadbeef""#));
    }

    #[test]
    fn test_server_message_registered() {
        let msg = ServerMessage::Registered {
            dna_hash: "dna1".to_string(),
            agent_pubkey: "agent1".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"registered""#));
        assert!(json.contains(r#""dna_hash":"dna1""#));
        assert!(json.contains(r#""agent_pubkey":"agent1""#));
    }

    #[test]
    fn test_server_message_pong() {
        let msg = ServerMessage::Pong { peer_count: 0 };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"pong","peer_count":0}"#);
    }

    #[test]
    fn test_server_message_pong_with_peers() {
        let msg = ServerMessage::Pong { peer_count: 42 };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"pong","peer_count":42}"#);
    }

    #[test]
    fn test_server_message_sign_agent_info() {
        let agent_info = serde_json::json!({
            "agent": "dGVzdC1hZ2VudA",
            "space": "dGVzdC1zcGFjZQ",
            "createdAt": "1731690797907204",
            "expiresAt": "1731762797907204",
            "isTombstone": false,
            "url": "ws://test.com:80/test-url",
            "storageArc": [42, 330382099]
        });
        let msg = ServerMessage::SignAgentInfo {
            request_id: "sign-0-123".to_string(),
            agent_pubkey: "uhCAkAQEB".to_string(),
            agent_info,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"sign_agent_info""#));
        assert!(json.contains(r#""request_id":"sign-0-123""#));
        assert!(json.contains(r#""agent_pubkey":"uhCAkAQEB""#));
        assert!(json.contains(r#""agent":"dGVzdC1hZ2VudA""#));
    }

    #[test]
    fn test_connection_state_default() {
        let state = ConnectionState::default();
        assert!(!state.authenticated);
        assert!(state.registrations.is_empty());
    }
}
