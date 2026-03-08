use crate::registry::{RealtimeMessage, Subscription, SubscriptionRegistry};
use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;

/// State shared with WebSocket handler.
#[derive(Clone)]
pub struct RealtimeState {
    pub registry: Arc<SubscriptionRegistry>,
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
    },
    Unsubscribe {
        id: String,
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
        event: crate::registry::ChangeEvent,
    },
    Pong,
    Error {
        message: String,
    },
}

/// Axum handler for WebSocket upgrade.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<RealtimeState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: RealtimeState) {
    let connection_id = uuid::Uuid::new_v4().to_string();
    tracing::info!(conn_id = %connection_id, "WebSocket connected");

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (msg_tx, mut msg_rx) = mpsc::channel::<RealtimeMessage>(256);

    // Task: forward subscription messages to WebSocket
    let conn_id_clone = connection_id.clone();
    let send_task = tokio::spawn(async move {
        use futures_util::SinkExt;

        // Forward realtime messages
        while let Some(msg) = msg_rx.recv().await {
            let server_msg = ServerMessage::Change {
                subscription_id: msg.subscription_id,
                event: msg.event,
            };
            if let Ok(json) = serde_json::to_string(&server_msg)
                && ws_sender.send(Message::Text(json.into())).await.is_err()
            {
                break;
            }
        }
        tracing::debug!(conn_id = %conn_id_clone, "WebSocket send task ended");
    });

    // Task: receive client messages
    use futures_util::StreamExt;
    while let Some(Ok(msg)) = ws_receiver.next().await {
        match msg {
            Message::Text(text) => {
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Subscribe {
                        id,
                        collection,
                        filter_hash,
                    }) => {
                        let sub = Subscription {
                            id: id.clone(),
                            collection,
                            filter_hash,
                            user_id: None,
                            sender: msg_tx.clone(),
                        };
                        state.registry.subscribe(&connection_id, sub);

                        let resp = ServerMessage::Subscribed { id };
                        if let Ok(json) = serde_json::to_string(&resp) {
                            // Send confirmation back through the msg channel would be
                            // wrong — we need the ws_sender. For now, log it.
                            tracing::debug!("Subscription confirmed: {json}");
                        }
                    }
                    Ok(ClientMessage::Unsubscribe { id }) => {
                        state.registry.unsubscribe(&id);
                        tracing::debug!(sub_id = %id, "Unsubscribed");
                    }
                    Ok(ClientMessage::Ping) => {
                        // Pong handled by axum automatically for protocol pings
                        tracing::trace!("Ping received");
                    }
                    Err(e) => {
                        tracing::warn!("Invalid WebSocket message: {e}");
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Cleanup on disconnect
    state.registry.disconnect(&connection_id);
    send_task.abort();
    tracing::info!(conn_id = %connection_id, "WebSocket disconnected");
}

/// Build the realtime WebSocket router.
pub fn realtime_router(registry: Arc<SubscriptionRegistry>) -> axum::Router {
    let state = RealtimeState { registry };
    axum::Router::new()
        .route("/realtime", axum::routing::get(ws_handler))
        .with_state(state)
}
