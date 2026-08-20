use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use ob_auth::jwt::{JwtKeys, issue_access_token};
use ob_realtime::{SubscriptionRegistry, websocket::realtime_router};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tower::util::ServiceExt;

fn make_access_token(secret: &str, user_id: &str) -> String {
    let keys = JwtKeys::from_secret(secret);
    issue_access_token(user_id, &[], &keys, 3600, true).unwrap()
}

async fn spawn_realtime_server(secret: &str) -> (String, Arc<SubscriptionRegistry>) {
    let registry = SubscriptionRegistry::new();
    let router = realtime_router(registry.clone(), JwtKeys::from_secret(secret));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("ws://{address}"), registry)
}

async fn next_json_message(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Value {
    use futures_util::StreamExt;

    loop {
        match socket.next().await.unwrap().unwrap() {
            Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected websocket message: {other:?}"),
        }
    }
}

#[tokio::test]
async fn websocket_ping_and_presence_flow_work_over_real_socket() {
    use futures_util::SinkExt;

    let secret = "integration-secret";
    let token = make_access_token(secret, "users:socket-user");
    let (base_ws, registry) = spawn_realtime_server(secret).await;
    let (mut socket, _) = connect_async(format!("{base_ws}/realtime?token={token}"))
        .await
        .unwrap();

    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "presence",
                "metadata": { "device": "ios" }
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let presence_update = next_json_message(&mut socket).await;
    assert_eq!(presence_update["type"], "presence_update");
    assert_eq!(presence_update["online"][0]["user_id"], "users:socket-user");
    assert_eq!(presence_update["online"][0]["metadata"]["device"], "ios");

    socket
        .send(Message::Text(
            serde_json::json!({ "type": "ping" }).to_string(),
        ))
        .await
        .unwrap();
    let pong = next_json_message(&mut socket).await;
    assert_eq!(pong["type"], "pong");
    assert!(registry.is_online("users:socket-user"));
}

#[tokio::test]
async fn websocket_rejects_disallowed_collections_and_oversized_presence_metadata() {
    use futures_util::SinkExt;

    let secret = "integration-secret";
    let token = make_access_token(secret, "users:socket-user");
    let (base_ws, _) = spawn_realtime_server(secret).await;
    let (mut socket, _) = connect_async(format!("{base_ws}/realtime?token={token}"))
        .await
        .unwrap();

    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "subscribe",
                "id": "sub-1",
                "collection": "_internal",
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let disallowed = next_json_message(&mut socket).await;
    assert_eq!(disallowed["type"], "error");
    assert!(
        disallowed["message"]
            .as_str()
            .unwrap()
            .contains("not allowed")
    );

    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "presence",
                "metadata": { "blob": "x".repeat(5000) }
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let oversized = next_json_message(&mut socket).await;
    assert_eq!(oversized["type"], "error");
    assert!(oversized["message"].as_str().unwrap().contains("too large"));
}

#[tokio::test]
async fn websocket_presence_http_endpoints_reflect_active_socket_presence() {
    use futures_util::SinkExt;

    let secret = "integration-secret";
    let token = make_access_token(secret, "users:presence-user");
    let registry = SubscriptionRegistry::new();
    let router = realtime_router(registry.clone(), JwtKeys::from_secret(secret));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let (mut socket, _) = connect_async(format!("ws://{address}/realtime?token={token}"))
        .await
        .unwrap();
    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "presence",
                "metadata": { "device": "web" }
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let _ = next_json_message(&mut socket).await;

    let app = realtime_router(registry, JwtKeys::from_secret(secret));
    let presence_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/presence?token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(presence_response.status(), StatusCode::OK);
    let body = to_bytes(presence_response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["count"], 1);
    assert_eq!(payload["online"][0]["user_id"], "users:presence-user");

    let user_presence_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/presence/users:presence-user?token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(user_presence_response.status(), StatusCode::OK);
    let body = to_bytes(user_presence_response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["online"], true);
    assert_eq!(payload["presence"]["metadata"]["device"], "web");
}
