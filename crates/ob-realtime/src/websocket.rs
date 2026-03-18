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

/// State shared with WebSocket handler.
#[derive(Clone)]
pub struct RealtimeState {
    pub registry: Arc<SubscriptionRegistry>,
    pub jwt_keys: JwtKeys,
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
            if srv_tx_bridge.send(server_msg).await.is_err() {
                break;
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
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Subscribe {
                        id,
                        collection,
                        filter_hash,
                        document_id,
                    }) => {
                        // HIGH FIX: Enforce subscription limit per connection
                        let sub_count = state.registry.connection_subscription_count(&connection_id);
                        if sub_count >= MAX_SUBS_PER_CONNECTION {
                            let _ = srv_tx.send(ServerMessage::Error {
                                message: format!("Max subscriptions ({}) reached", MAX_SUBS_PER_CONNECTION),
                            }).await;
                            continue;
                        }

                        let sub = Subscription {
                            id: id.clone(),
                            collection,
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
                        // MEDIUM FIX: Use authenticated user_id from JWT, NOT from client message
                        state.registry.set_presence(&user_id, &connection_id, metadata);
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
    tracing::info!(conn_id = %connection_id, "WebSocket disconnected");
}

/// GET /presence — Get all online users.
async fn presence_handler(State(state): State<RealtimeState>) -> axum::Json<serde_json::Value> {
    let online = state.registry.get_online_users();
    axum::Json(serde_json::json!({
        "online": online,
        "count": state.registry.online_count(),
    }))
}

/// GET /presence/:user_id — Check if a specific user is online.
async fn presence_user_handler(
    State(state): State<RealtimeState>,
    axum::extract::Path(user_id): axum::extract::Path<String>,
) -> axum::Json<serde_json::Value> {
    let info = state.registry.get_presence(&user_id);
    axum::Json(serde_json::json!({
        "user_id": user_id,
        "online": info.is_some(),
        "presence": info,
    }))
}

/// Build the realtime WebSocket router.
pub fn realtime_router(registry: Arc<SubscriptionRegistry>, jwt_keys: JwtKeys) -> axum::Router {
    let state = RealtimeState { registry, jwt_keys };
    axum::Router::new()
        .route("/realtime", axum::routing::get(ws_handler))
        .route("/presence", axum::routing::get(presence_handler))
        .route(
            "/presence/{user_id}",
            axum::routing::get(presence_user_handler),
        )
        .with_state(state)
}
