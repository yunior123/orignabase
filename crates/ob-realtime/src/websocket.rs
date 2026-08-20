use crate::registry::{RealtimeMessage, Subscription, SubscriptionRegistry};
use axum::{
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use ob_auth::jwt::{JwtKeys, verify_token};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Maximum subscriptions per connection to prevent DoS
const MAX_SUBS_PER_CONNECTION: usize = 100;
/// Collections that clients are allowed to subscribe to via WebSocket.
/// Internal collections (prefixed with _) and sensitive collections are excluded.
const ALLOWED_COLLECTIONS: &[&str] = &[
    "products",
    "orders",
    "cart",
    "users__cart",
    "favorites",
    "notifications",
    "users__notifications",
    "chats",
    "chats__messages",
    "chat_messages",
    "chat_threads",
    "product_questions",
    "users",
    "reviews",
    "seller_profiles",
    "warehouses",
    "return_requests",
    "subscriptions",
];

/// Maximum WebSocket message size (64KB)
const MAX_WS_MESSAGE_SIZE: usize = 65_536;

/// Maximum concurrent WebSocket connections per user
const MAX_CONNECTIONS_PER_USER: usize = 5;

/// State shared with WebSocket handler.
#[derive(Clone)]
pub struct RealtimeState {
    pub registry: Arc<SubscriptionRegistry>,
    pub jwt_keys: JwtKeys,
    pub connection_counts: Arc<dashmap::DashMap<String, usize>>,
}

/// Query params for WebSocket upgrade (token extraction).
#[derive(Debug, Deserialize)]
pub struct WsQuery {
    #[serde(default)]
    pub token: Option<String>,
}

/// Client → Server messages.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Subscribe {
        id: String,
        collection: String,
        #[serde(default)]
        filter_hash: u64,
        /// Optional: subscribe to a specific document
        #[serde(default)]
        document_id: Option<String>,
    },
    Unsubscribe {
        id: String,
    },
    /// Set presence for this connection.
    /// Note: user_id is NOT used from client message.
    /// Presence is set using the authenticated user from JWT.
    Presence {
        #[serde(default)]
        metadata: serde_json::Value,
    },
    Ping,
}

/// Server → Client messages.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Subscribed {
        id: String,
    },
    Unsubscribed {
        id: String,
    },
    Change {
        subscription_id: String,
        event: Box<crate::registry::ChangeEvent>,
    },
    PresenceUpdate {
        online: Vec<crate::registry::PresenceInfo>,
    },
    Pong,
    Error {
        message: String,
    },
}

/// Extract JWT token from query param `?token=xxx` or `Authorization: Bearer xxx` header.
fn extract_ws_token(query: &WsQuery, headers: &axum::http::HeaderMap) -> Option<String> {
    // Prefer query param (WebSocket clients can't always set headers)
    if let Some(token) = &query.token
        && !token.is_empty()
    {
        return Some(token.clone());
    }
    // Fall back to Authorization header
    if let Some(auth_header) = headers.get(axum::http::header::AUTHORIZATION)
        && let Ok(header_str) = auth_header.to_str()
        && let Some(token) = header_str.strip_prefix("Bearer ")
    {
        return Some(token.to_string());
    }
    None
}

/// Axum handler for WebSocket upgrade with JWT authentication.
pub async fn ws_handler(
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
    State(state): State<RealtimeState>,
    headers: axum::http::HeaderMap,
) -> Response {
    // Extract and verify JWT before upgrading
    let token = match extract_ws_token(&query, &headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                "Missing authentication token. Provide ?token=<jwt> or Authorization header.",
            )
                .into_response();
        }
    };

    let claims = match verify_token(&token, &state.jwt_keys) {
        Ok(claims) if claims.typ == "access" => claims,
        Ok(_) => {
            return (StatusCode::UNAUTHORIZED, "Invalid token type").into_response();
        }
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "Invalid or expired token").into_response();
        }
    };

    let user_id = claims.sub;
    ws.on_upgrade(move |socket| handle_socket(socket, state, user_id))
}

async fn handle_socket(socket: WebSocket, state: RealtimeState, user_id: String) {
    let connection_id = uuid::Uuid::new_v4().to_string();

    // Check per-user connection limit
    {
        let mut current_conns = state.connection_counts.entry(user_id.clone()).or_insert(0);
        if *current_conns >= MAX_CONNECTIONS_PER_USER {
            tracing::warn!(
                user_id = %user_id,
                active_connections = *current_conns,
                "websocket_connection_limit_exceeded"
            );
            return;
        }
        *current_conns += 1;
    }

    tracing::info!(conn_id = %connection_id, user_id = %user_id, "WebSocket connected");

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Single channel for all outbound ServerMessages (confirmations + change events)
    let (srv_tx, mut srv_rx) = mpsc::channel::<ServerMessage>(256);

    // Bridge channel: subscriptions receive RealtimeMessage, convert to ServerMessage
    let (rt_tx, mut rt_rx) = mpsc::channel::<RealtimeMessage>(256);
    let srv_tx_bridge = srv_tx.clone();
    tokio::spawn(async move {
        while let Some(rt_msg) = rt_rx.recv().await {
            let server_msg = ServerMessage::Change {
                subscription_id: rt_msg.subscription_id,
                event: Box::new(rt_msg.event),
            };
            // Add timeout to detect slow consumers and prevent stalls
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                srv_tx_bridge.send(server_msg),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(_)) => break, // channel closed
                Err(_) => {
                    tracing::warn!("websocket_bridge_send_timeout: slow consumer disconnecting");
                    break; // slow consumer — disconnect
                }
            }
        }
    });

    // Send task: forward all ServerMessages to WebSocket
    let conn_id_clone = connection_id.clone();
    let send_task = tokio::spawn(async move {
        use futures_util::SinkExt;

        while let Some(server_msg) = srv_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&server_msg)
                && ws_sender.send(Message::Text(json.into())).await.is_err()
            {
                break;
            }
        }
        tracing::debug!(conn_id = %conn_id_clone, "WebSocket send task ended");
    });

    // Receive loop: handle client messages
    use futures_util::StreamExt;
    while let Some(Ok(msg)) = ws_receiver.next().await {
        match msg {
            Message::Text(text) => {
                // Check message size
                if text.len() > MAX_WS_MESSAGE_SIZE {
                    tracing::warn!(
                        user_id = %user_id,
                        msg_size = text.len(),
                        "websocket_message_too_large"
                    );
                    let _ = srv_tx
                        .send(ServerMessage::Error {
                            message: "Message too large".to_string(),
                        })
                        .await;
                    continue;
                }

                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Subscribe {
                        id,
                        collection,
                        filter_hash,
                        document_id,
                    }) => {
                        // HIGH FIX: Enforce collection allowlist
                        if !ALLOWED_COLLECTIONS.contains(&collection.as_str()) {
                            tracing::warn!(
                                user_id = %user_id,
                                collection = %collection,
                                "websocket_collection_not_allowed"
                            );
                            let _ = srv_tx
                                .send(ServerMessage::Error {
                                    message: format!(
                                        "Subscription to '{}' is not allowed",
                                        collection
                                    ),
                                })
                                .await;
                            continue;
                        }

                        // Enforce subscription limit per connection
                        let sub_count =
                            state.registry.connection_subscription_count(&connection_id);
                        if sub_count >= MAX_SUBS_PER_CONNECTION {
                            let _ = srv_tx
                                .send(ServerMessage::Error {
                                    message: format!(
                                        "Max subscriptions ({}) reached",
                                        MAX_SUBS_PER_CONNECTION
                                    ),
                                })
                                .await;
                            continue;
                        }

                        let sub = Subscription {
                            id: id.clone(),
                            collection: Arc::from(collection.as_str()),
                            filter_hash,
                            document_id,
                            user_id: Some(user_id.clone()),
                            sender: rt_tx.clone(),
                        };
                        state.registry.subscribe(&connection_id, sub);
                        let _ = srv_tx.send(ServerMessage::Subscribed { id }).await;
                    }
                    Ok(ClientMessage::Unsubscribe { id }) => {
                        state.registry.unsubscribe(&id);
                        let _ = srv_tx.send(ServerMessage::Unsubscribed { id }).await;
                    }
                    Ok(ClientMessage::Presence { metadata }) => {
                        // FIX: Cap metadata size to prevent DoS attacks
                        let metadata_str = serde_json::to_string(&metadata).unwrap_or_default();
                        if metadata_str.len() > 4096 {
                            tracing::warn!(
                                user_id = %user_id,
                                size = metadata_str.len(),
                                "presence_metadata_too_large"
                            );
                            let _ = srv_tx
                                .send(ServerMessage::Error {
                                    message: "Presence metadata too large (max 4KB)".to_string(),
                                })
                                .await;
                            continue;
                        }

                        // Use authenticated user_id from JWT, NOT from client message
                        state
                            .registry
                            .set_presence(&user_id, &connection_id, metadata);
                        // Send current online users to the client
                        let online = state.registry.get_online_users();
                        let _ = srv_tx.send(ServerMessage::PresenceUpdate { online }).await;
                    }
                    Ok(ClientMessage::Ping) => {
                        state.registry.heartbeat(&connection_id);
                        let _ = srv_tx.send(ServerMessage::Pong).await;
                    }
                    Err(e) => {
                        tracing::warn!("Invalid WebSocket message: {e}");
                        let _ = srv_tx
                            .send(ServerMessage::Error {
                                message: e.to_string(),
                            })
                            .await;
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Cleanup on disconnect
    state.registry.remove_presence(&connection_id);
    state.registry.disconnect(&connection_id);
    send_task.abort();
    // Decrement connection count for this user
    if let Some(mut count) = state.connection_counts.get_mut(&user_id) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            drop(count);
            state.connection_counts.remove(&user_id);
        }
    }
    tracing::info!(conn_id = %connection_id, "WebSocket disconnected");
}

/// GET /presence — Get all online users. Requires valid JWT.
async fn presence_handler(
    Query(query): Query<WsQuery>,
    State(state): State<RealtimeState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let token = match extract_ws_token(&query, &headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                "Missing authentication token. Provide ?token=<jwt> or Authorization header.",
            )
                .into_response();
        }
    };
    match verify_token(&token, &state.jwt_keys) {
        Ok(claims) if claims.typ == "access" => claims,
        Ok(_) => return (StatusCode::UNAUTHORIZED, "Invalid token type").into_response(),
        Err(_) => return (StatusCode::UNAUTHORIZED, "Invalid or expired token").into_response(),
    };

    let online = state.registry.get_online_users();
    axum::Json(serde_json::json!({
        "online": online,
        "count": state.registry.online_count(),
    }))
    .into_response()
}

/// GET /presence/:user_id — Check if a specific user is online. Requires valid JWT.
async fn presence_user_handler(
    Query(query): Query<WsQuery>,
    State(state): State<RealtimeState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(user_id): axum::extract::Path<String>,
) -> Response {
    let token = match extract_ws_token(&query, &headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                "Missing authentication token. Provide ?token=<jwt> or Authorization header.",
            )
                .into_response();
        }
    };
    match verify_token(&token, &state.jwt_keys) {
        Ok(claims) if claims.typ == "access" => claims,
        Ok(_) => return (StatusCode::UNAUTHORIZED, "Invalid token type").into_response(),
        Err(_) => return (StatusCode::UNAUTHORIZED, "Invalid or expired token").into_response(),
    };

    let info = state.registry.get_presence(&user_id);
    axum::Json(serde_json::json!({
        "user_id": user_id,
        "online": info.is_some(),
        "presence": info,
    }))
    .into_response()
}

/// Build the realtime WebSocket router.
pub fn realtime_router(registry: Arc<SubscriptionRegistry>, jwt_keys: JwtKeys) -> axum::Router {
    let state = RealtimeState {
        registry,
        jwt_keys,
        connection_counts: Arc::new(dashmap::DashMap::new()),
    };
    axum::Router::new()
        .route("/realtime", axum::routing::get(ws_handler))
        .route("/presence", axum::routing::get(presence_handler))
        .route(
            "/presence/{user_id}",
            axum::routing::get(presence_user_handler),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ob_database::fields;

    // ── extract_ws_token tests ──

    #[test]
    fn test_extract_token_from_query_param() {
        let query = WsQuery {
            token: Some("jwt_from_query".to_string()),
        };
        let headers = axum::http::HeaderMap::new();
        assert_eq!(
            extract_ws_token(&query, &headers),
            Some("jwt_from_query".to_string())
        );
    }

    #[test]
    fn test_extract_token_from_auth_header() {
        let query = WsQuery { token: None };
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer jwt_from_header"),
        );
        assert_eq!(
            extract_ws_token(&query, &headers),
            Some("jwt_from_header".to_string())
        );
    }

    #[test]
    fn test_extract_token_query_param_preferred_over_header() {
        let query = WsQuery {
            token: Some("query_token".to_string()),
        };
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer header_token"),
        );
        assert_eq!(
            extract_ws_token(&query, &headers),
            Some("query_token".to_string())
        );
    }

    #[test]
    fn test_extract_token_empty_query_param_falls_back_to_header() {
        let query = WsQuery {
            token: Some("".to_string()),
        };
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer header_token"),
        );
        assert_eq!(
            extract_ws_token(&query, &headers),
            Some("header_token".to_string())
        );
    }

    #[test]
    fn test_extract_token_no_token_returns_none() {
        let query = WsQuery { token: None };
        let headers = axum::http::HeaderMap::new();
        assert_eq!(extract_ws_token(&query, &headers), None);
    }

    #[test]
    fn test_extract_token_non_bearer_header_returns_none() {
        let query = WsQuery { token: None };
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Basic dXNlcjpwYXNz"),
        );
        assert_eq!(extract_ws_token(&query, &headers), None);
    }

    #[test]
    fn test_extract_token_invalid_header_encoding_returns_none() {
        let query = WsQuery { token: None };
        let mut headers = axum::http::HeaderMap::new();
        // Insert an Authorization header with an invalid Bearer token format
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("NotBearer invalid"),
        );
        assert_eq!(extract_ws_token(&query, &headers), None);
    }

    // ── ClientMessage deserialization tests ──

    #[test]
    fn test_deserialize_subscribe_message() {
        let json = r#"{"type":"subscribe","id":"sub1","collection":"products","filter_hash":123}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Subscribe {
                id,
                collection,
                filter_hash,
                document_id,
            } => {
                assert_eq!(id, "sub1");
                assert_eq!(collection, "products");
                assert_eq!(filter_hash, 123);
                assert!(document_id.is_none());
            }
            _ => panic!("Expected Subscribe"),
        }
    }

    #[test]
    fn test_deserialize_subscribe_with_document_id() {
        let json =
            r#"{"type":"subscribe","id":"sub1","collection":"users","document_id":"user_42"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Subscribe { document_id, .. } => {
                assert_eq!(document_id, Some("user_42".to_string()));
            }
            _ => panic!("Expected Subscribe"),
        }
    }

    #[test]
    fn test_deserialize_unsubscribe_message() {
        let json = r#"{"type":"unsubscribe","id":"sub1"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Unsubscribe { id } => assert_eq!(id, "sub1"),
            _ => panic!("Expected Unsubscribe"),
        }
    }

    #[test]
    fn test_deserialize_presence_message() {
        let json = r#"{"type":"presence","metadata":{"device":"mobile"}}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Presence { metadata } => {
                assert_eq!(metadata["device"], "mobile");
            }
            _ => panic!("Expected Presence"),
        }
    }

    #[test]
    fn test_deserialize_presence_without_metadata() {
        let json = r#"{"type":"presence"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Presence { metadata } => {
                assert_eq!(metadata, serde_json::Value::Null);
            }
            _ => panic!("Expected Presence"),
        }
    }

    #[test]
    fn test_deserialize_ping_message() {
        let json = r#"{"type":"ping"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Ping => {}
            _ => panic!("Expected Ping"),
        }
    }

    #[test]
    fn test_deserialize_invalid_type_fails() {
        let json = r#"{"type":"unknown"}"#;
        let result = serde_json::from_str::<ClientMessage>(json);
        assert!(result.is_err());
    }

    // ── ServerMessage serialization tests ──

    #[test]
    fn test_serialize_subscribed_message() {
        let msg = ServerMessage::Subscribed {
            id: "sub1".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "subscribed");
        assert_eq!(parsed[fields::ID], "sub1");
    }

    #[test]
    fn test_serialize_unsubscribed_message() {
        let msg = ServerMessage::Unsubscribed {
            id: "sub1".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "unsubscribed");
        assert_eq!(parsed[fields::ID], "sub1");
    }

    #[test]
    fn test_serialize_pong_message() {
        let msg = ServerMessage::Pong;
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "pong");
    }

    #[test]
    fn test_serialize_error_message() {
        let msg = ServerMessage::Error {
            message: "something went wrong".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "error");
        assert_eq!(parsed["message"], "something went wrong");
    }

    #[test]
    fn test_serialize_change_message() {
        let event = crate::registry::ChangeEvent {
            action: crate::registry::ChangeAction::Create,
            collection: "products".to_string(),
            document_id: "prod_1".to_string(),
            data: serde_json::json!({"title": "Widget"}),
            before_data: None,
            after_data: None,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
        };
        let msg = ServerMessage::Change {
            subscription_id: "sub1".to_string(),
            event: Box::new(event),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "change");
        assert_eq!(parsed["subscription_id"], "sub1");
        assert_eq!(parsed["event"]["action"], "create");
        assert_eq!(parsed["event"]["collection"], "products");
    }

    #[test]
    fn test_serialize_presence_update_message() {
        let msg = ServerMessage::PresenceUpdate {
            online: vec![crate::registry::PresenceInfo {
                user_id: "alice".to_string(),
                connection_id: "conn1".to_string(),
                status: "online".to_string(),
                last_seen: "2024-01-01T00:00:00Z".to_string(),
                metadata: serde_json::json!({}),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "presence_update");
        assert_eq!(parsed["online"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["online"][0]["user_id"], "alice");
    }

    // ── Constant and struct tests ──

    #[test]
    fn test_max_subs_per_connection() {
        assert_eq!(MAX_SUBS_PER_CONNECTION, 100);
    }

    #[test]
    fn test_ws_query_default_token_is_none() {
        let json = r#"{}"#;
        let query: WsQuery = serde_json::from_str(json).unwrap();
        assert!(query.token.is_none());
    }

    #[test]
    fn test_ws_query_with_token() {
        let json = r#"{"token":"abc123"}"#;
        let query: WsQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.token, Some("abc123".to_string()));
    }

    #[test]
    fn test_realtime_state_clone() {
        fn assert_clone<T: Clone>() {}
        assert_clone::<RealtimeState>();
    }

    #[test]
    fn test_subscribe_filter_hash_defaults_to_zero() {
        let json = r#"{"type":"subscribe","id":"sub1","collection":"products"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Subscribe { filter_hash, .. } => {
                assert_eq!(filter_hash, 0);
            }
            _ => panic!("Expected Subscribe"),
        }
    }

    #[test]
    fn test_realtime_router_builds() {
        let registry = SubscriptionRegistry::new();
        let jwt_keys = JwtKeys::from_secret("test_secret");
        let _router = realtime_router(registry, jwt_keys);
    }

    #[test]
    fn test_deserialize_subscribe_with_empty_collection() {
        let json = r#"{"type":"subscribe","id":"sub1","collection":""}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Subscribe { collection, .. } => {
                assert!(collection.is_empty());
            }
            _ => panic!("Expected Subscribe"),
        }
    }

    #[test]
    fn test_deserialize_subscribe_missing_filter_hash_defaults() {
        let json = r#"{"type":"subscribe","id":"s1","collection":"orders"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Subscribe { filter_hash, .. } => {
                assert_eq!(filter_hash, 0);
            }
            _ => panic!("Expected Subscribe"),
        }
    }

    #[test]
    fn test_deserialize_presence_with_complex_metadata() {
        let json = r#"{"type":"presence","metadata":{"device":"mobile","os":"iOS","version":15}}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Presence { metadata } => {
                assert_eq!(metadata["device"], "mobile");
                assert_eq!(metadata["os"], "iOS");
                assert_eq!(metadata["version"], 15);
            }
            _ => panic!("Expected Presence"),
        }
    }

    #[test]
    fn test_serialize_error_with_empty_message() {
        let msg = ServerMessage::Error {
            message: "".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "error");
        assert_eq!(parsed["message"], "");
    }

    #[test]
    fn test_serialize_subscribed_with_empty_id() {
        let msg = ServerMessage::Subscribed { id: "".to_string() };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "subscribed");
        assert_eq!(parsed[fields::ID], "");
    }

    #[test]
    fn test_serialize_unsubscribed_roundtrip() {
        let msg = ServerMessage::Unsubscribed {
            id: "sub_42".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "unsubscribed");
        assert_eq!(parsed[fields::ID], "sub_42");
    }

    #[test]
    fn test_serialize_presence_update_empty_online() {
        let msg = ServerMessage::PresenceUpdate { online: vec![] };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["online"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_deserialize_unsubscribe_with_extra_fields_ignored() {
        let json = r#"{"type":"unsubscribe","id":"s1","extra":"ignored"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Unsubscribe { id } => assert_eq!(id, "s1"),
            _ => panic!("Expected Unsubscribe"),
        }
    }

    #[test]
    fn test_deserialize_ping_case_sensitive() {
        let json = r#"{"type":"ping"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ClientMessage::Ping));
    }

    #[test]
    fn test_ws_query_debug() {
        let query = WsQuery {
            token: Some("abc".to_string()),
        };
        let debug = format!("{:?}", query);
        assert!(debug.contains("abc"));
    }

    #[test]
    fn test_ws_query_empty_token() {
        let json = r#"{"token":""}"#;
        let query: WsQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.token, Some("".to_string()));
    }

    #[test]
    fn test_extract_token_header_with_spaces() {
        let query = WsQuery { token: None };
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer   token_with_spaces"),
        );
        // "Bearer   token_with_spaces" has extra spaces after Bearer
        // strip_prefix("Bearer ") will match first space and return "  token_with_spaces"
        let result = extract_ws_token(&query, &headers);
        assert!(result.is_some());
    }

    #[test]
    fn test_deserialize_subscribe_with_large_filter_hash() {
        let json = format!(
            r#"{{"type":"subscribe","id":"s1","collection":"products","filter_hash":{}}}"#,
            u64::MAX
        );
        let msg: ClientMessage = serde_json::from_str(&json).unwrap();
        match msg {
            ClientMessage::Subscribe { filter_hash, .. } => {
                assert_eq!(filter_hash, u64::MAX);
            }
            _ => panic!("Expected Subscribe"),
        }
    }

    // ── Presence handler HTTP tests ──

    /// Generate a valid test JWT for presence endpoint tests.
    fn make_test_token(secret: &str) -> String {
        use ob_auth::jwt::issue_access_token;
        let keys = JwtKeys::from_secret(secret);
        issue_access_token("users:test_user", &[], &keys, 3600, true)
            .expect("token creation failed")
    }

    #[tokio::test]
    async fn test_presence_handler_rejects_unauthenticated() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::util::ServiceExt;

        let registry = SubscriptionRegistry::new();
        let jwt_keys = JwtKeys::from_secret("test_secret");
        let router = realtime_router(registry, jwt_keys);

        let req = Request::builder()
            .method("GET")
            .uri("/presence")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_presence_user_handler_rejects_unauthenticated() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::util::ServiceExt;

        let registry = SubscriptionRegistry::new();
        let jwt_keys = JwtKeys::from_secret("test_secret");
        let router = realtime_router(registry, jwt_keys);

        let req = Request::builder()
            .method("GET")
            .uri("/presence/user_xyz")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_presence_handler_returns_empty_with_auth() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::util::ServiceExt;

        let registry = SubscriptionRegistry::new();
        let jwt_keys = JwtKeys::from_secret("test_secret");
        let token = make_test_token("test_secret");
        let router = realtime_router(registry, jwt_keys);

        let req = Request::builder()
            .method("GET")
            .uri(format!("/presence?token={token}"))
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 10_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["count"], 0);
        assert!(json["online"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_presence_user_handler_offline_with_auth() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::util::ServiceExt;

        let registry = SubscriptionRegistry::new();
        let jwt_keys = JwtKeys::from_secret("test_secret");
        let token = make_test_token("test_secret");
        let router = realtime_router(registry, jwt_keys);

        let req = Request::builder()
            .method("GET")
            .uri(format!("/presence/user_xyz?token={token}"))
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 10_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["user_id"], "user_xyz");
        assert_eq!(json["online"], false);
        assert!(json["presence"].is_null());
    }

    #[tokio::test]
    async fn test_presence_user_handler_online_with_auth() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::util::ServiceExt;

        let registry = SubscriptionRegistry::new();
        registry.set_presence("alice", "conn_1", serde_json::json!({"device": "mobile"}));

        let jwt_keys = JwtKeys::from_secret("test_secret");
        let token = make_test_token("test_secret");
        let router = realtime_router(registry, jwt_keys);

        let req = Request::builder()
            .method("GET")
            .uri(format!("/presence/alice?token={token}"))
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 10_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["user_id"], "alice");
        assert_eq!(json["online"], true);
    }

    #[tokio::test]
    async fn test_presence_handler_with_users_with_auth() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::util::ServiceExt;

        let registry = SubscriptionRegistry::new();
        registry.set_presence("user_1", "conn_1", serde_json::json!({}));
        registry.set_presence("user_2", "conn_2", serde_json::json!({}));

        let jwt_keys = JwtKeys::from_secret("test_secret");
        let token = make_test_token("test_secret");
        let router = realtime_router(registry, jwt_keys);

        let req = Request::builder()
            .method("GET")
            .uri(format!("/presence?token={token}"))
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 10_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["count"], 2);
        assert_eq!(json["online"].as_array().unwrap().len(), 2);
    }

    /// Test that extract_ws_token returns None when no token is provided —
    /// which causes ws_handler to reject with 401.
    #[test]
    fn test_ws_handler_rejects_missing_token() {
        let query = WsQuery { token: None };
        let headers = axum::http::HeaderMap::new();
        // No token → extract returns None → ws_handler would return 401
        assert!(extract_ws_token(&query, &headers).is_none());
    }

    /// Test that an invalid JWT fails verify_token —
    /// which causes ws_handler to reject with 401.
    #[test]
    fn test_ws_handler_rejects_invalid_token() {
        let keys = JwtKeys::from_secret("test_secret");
        let result = verify_token("invalid-jwt", &keys);
        // Invalid token → verify fails → ws_handler would return 401
        assert!(result.is_err());
    }

    // ── ServerMessage Debug tests ──

    #[test]
    fn test_client_message_debug() {
        let msg = ClientMessage::Ping;
        let debug = format!("{:?}", msg);
        assert!(debug.contains("Ping"));
    }

    #[test]
    fn test_server_message_debug() {
        let msg = ServerMessage::Pong;
        let debug = format!("{:?}", msg);
        assert!(debug.contains("Pong"));
    }

    #[test]
    fn test_server_message_change_debug() {
        let event = crate::registry::ChangeEvent {
            action: crate::registry::ChangeAction::Update,
            collection: "orders".to_string(),
            document_id: "ord_1".to_string(),
            data: serde_json::json!({"status": "shipped"}),
            before_data: Some(serde_json::json!({"status": "confirmed"})),
            after_data: Some(serde_json::json!({"status": "shipped"})),
            timestamp: "2024-06-01T00:00:00Z".to_string(),
        };
        let msg = ServerMessage::Change {
            subscription_id: "sub1".to_string(),
            event: Box::new(event),
        };
        let debug = format!("{:?}", msg);
        assert!(debug.contains("Change"));
    }

    #[test]
    fn test_serialize_change_with_before_after_data() {
        let event = crate::registry::ChangeEvent {
            action: crate::registry::ChangeAction::Update,
            collection: "orders".to_string(),
            document_id: "ord_1".to_string(),
            data: serde_json::json!({"status": "shipped"}),
            before_data: Some(serde_json::json!({"status": "confirmed"})),
            after_data: Some(serde_json::json!({"status": "shipped"})),
            timestamp: "2024-06-01T00:00:00Z".to_string(),
        };
        let msg = ServerMessage::Change {
            subscription_id: "sub1".to_string(),
            event: Box::new(event),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"]["action"], "update");
        assert_eq!(parsed["event"]["before_data"][fields::STATUS], "confirmed");
        assert_eq!(parsed["event"]["after_data"][fields::STATUS], "shipped");
    }

    #[test]
    fn test_serialize_change_with_delete_action() {
        let event = crate::registry::ChangeEvent {
            action: crate::registry::ChangeAction::Delete,
            collection: "products".to_string(),
            document_id: "prod_1".to_string(),
            data: serde_json::json!({}),
            before_data: None,
            after_data: None,
            timestamp: "2024-06-01T00:00:00Z".to_string(),
        };
        let msg = ServerMessage::Change {
            subscription_id: "sub_del".to_string(),
            event: Box::new(event),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"]["action"], "delete");
    }
}
