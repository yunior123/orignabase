use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use ob_database::DatabaseClient;
use ob_notifications::{NotificationsState, notifications_router};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, MutexGuard};
use tower::util::ServiceExt;

static TEST_SUFFIX: AtomicU64 = AtomicU64::new(1);
static ROUTES_TEST_LOCK: Mutex<()> = Mutex::const_new(());

struct TestState {
    state: NotificationsState,
    _guard: MutexGuard<'static, ()>,
}

async fn test_state() -> TestState {
    let guard = ROUTES_TEST_LOCK.lock().await;
    TestState {
        state: NotificationsState::new(
            DatabaseClient::new_mem().await,
            Some("integration-project".into()),
            None,
            reqwest::Client::new(),
        ),
        _guard: guard,
    }
}

async fn parse_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn unique_test_id(prefix: &str) -> String {
    let suffix = TEST_SUFFIX.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{suffix}")
}

#[tokio::test]
async fn register_then_send_to_user_persists_pending_notification() {
    let state = test_state().await;
    let db = state.state.db.clone();
    let app = notifications_router(state.state);
    let user_id = format!("users:{}", unique_test_id("buyer"));
    let token = unique_test_id("device-token");
    let title = unique_test_id("Order update");
    let order_id = format!("orders:{}", unique_test_id("order"));

    let register_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST") // ignore-magic
                .uri("/push/register")
                .header("content-type", "application/json") // ignore-magic
                .body(Body::from(
                    json!({ // ignore-magic
                        "user_id": user_id,
                        "token": token, // ignore-magic
                        "platform": "android"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(register_response.status(), StatusCode::OK);

    let send_response = app
        .oneshot(
            Request::builder()
                .method("POST") // ignore-magic
                .uri("/push/send")
                .header("content-type", "application/json") // ignore-magic
                .body(Body::from(
                    json!({ // ignore-magic
                        "to": user_id,
                        "target_type": "user", // ignore-magic
                        "title": title, // ignore-magic
                        "body": "Your order has shipped",
                        "data": { "order_id": order_id }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send_response.status(), StatusCode::OK);

    let payload = parse_json(send_response).await;
    assert_eq!(payload["sent"], 1); // ignore-magic
    assert_eq!(payload["failed"], 0); // ignore-magic
    assert_eq!(payload["total_devices"], 1); // ignore-magic

    let pending = db
        .list_documents("_pending_notifications", None, None)
        .await
        .unwrap();
    let matching: Vec<_> = pending
        .into_iter()
        .filter(|doc| doc["token"] == token && doc["title"] == title)
        .collect();
    assert_eq!(matching.len(), 1);
    assert_eq!(matching[0]["data"]["order_id"], order_id); // ignore-magic
}

#[tokio::test]
async fn subscribe_send_and_unsubscribe_topic_uses_real_router_flow() {
    let state = test_state().await;
    let db = state.state.db.clone();
    let app = notifications_router(state.state);
    let token = unique_test_id("device-topic");
    let topic = unique_test_id("flash-sales");
    let title = unique_test_id("Big sale");

    let subscribe_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST") // ignore-magic
                .uri("/push/subscribe")
                .header("content-type", "application/json") // ignore-magic
                .body(Body::from(
                    json!({ // ignore-magic
                        "token": token, // ignore-magic
                        "topic": topic
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(subscribe_response.status(), StatusCode::OK);

    let first_send_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST") // ignore-magic
                .uri("/push/send")
                .header("content-type", "application/json") // ignore-magic
                .body(Body::from(
                    json!({ // ignore-magic
                        "to": topic,
                        "target_type": "topic",
                        "title": title, // ignore-magic
                        "body": "Starts now"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_send_response.status(), StatusCode::OK);
    let first_send_payload = parse_json(first_send_response).await;
    assert_eq!(first_send_payload["sent"], 1); // ignore-magic
    assert_eq!(first_send_payload["total_devices"], 1); // ignore-magic

    let unsubscribe_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE") // ignore-magic
                .uri("/push/subscribe")
                .header("content-type", "application/json") // ignore-magic
                .body(Body::from(
                    json!({ // ignore-magic
                        "token": token, // ignore-magic
                        "topic": topic
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unsubscribe_response.status(), StatusCode::OK);

    let second_send_response = app
        .oneshot(
            Request::builder()
                .method("POST") // ignore-magic
                .uri("/push/send")
                .header("content-type", "application/json") // ignore-magic
                .body(Body::from(
                    json!({ // ignore-magic
                        "to": topic,
                        "target_type": "topic",
                        "title": title, // ignore-magic
                        "body": "Starts now"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_send_response.status(), StatusCode::OK);
    let second_send_payload = parse_json(second_send_response).await;
    assert_eq!(second_send_payload["sent"], 0); // ignore-magic
    assert_eq!(second_send_payload["message"], "No devices found"); // ignore-magic

    let pending = db
        .list_documents("_pending_notifications", None, None)
        .await
        .unwrap();
    let matching: Vec<_> = pending
        .into_iter()
        .filter(|doc| doc["token"] == token && doc["title"] == title)
        .collect();
    assert_eq!(matching.len(), 1);
}

#[tokio::test]
async fn unregistering_a_token_removes_it_from_user_fanout() {
    let state = test_state().await;
    let router = notifications_router(state.state);
    let user_id = format!("users:{}", unique_test_id("buyer"));
    let token = unique_test_id("device-token");
    let title = unique_test_id("Order update");

    let register_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST") // ignore-magic
                .uri("/push/register")
                .header("content-type", "application/json") // ignore-magic
                .body(Body::from(
                    json!({ // ignore-magic
                        "user_id": user_id,
                        "token": token, // ignore-magic
                        "platform": "ios"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(register_response.status(), StatusCode::OK);

    let unregister_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE") // ignore-magic
                .uri("/push/register")
                .header("content-type", "application/json") // ignore-magic
                .body(Body::from(json!({ "token": token }).to_string())) // ignore-magic
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unregister_response.status(), StatusCode::OK);

    let send_response = router
        .oneshot(
            Request::builder()
                .method("POST") // ignore-magic
                .uri("/push/send")
                .header("content-type", "application/json") // ignore-magic
                .body(Body::from(
                    json!({ // ignore-magic
                        "to": user_id,
                        "target_type": "user", // ignore-magic
                        "title": title, // ignore-magic
                        "body": "No device should receive this"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send_response.status(), StatusCode::OK);
    let payload = parse_json(send_response).await;
    assert_eq!(payload["sent"], 0); // ignore-magic
    assert_eq!(payload["message"], "No devices found"); // ignore-magic
}
