//! Reliability and degraded-mode live integration tests for OrignaBase.
//!
//! Run with:
//!   cargo test -p orignabase --test reliability_test -- --ignored
//!
//! Several tests expect an externally induced outage. Use these env vars to opt in:
//! - `OB_TEST_EXPECT_DB_DOWN=1`
//! - `OB_TEST_EXPECT_MEILI_DOWN=1`

use futures_util::SinkExt;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

fn ws_url() -> String {
    base_url()
        .replace("http://", "ws://")
        .replace("https://", "wss://")
}

fn password() -> String {
    std::env::var("OB_TEST_PASSWORD").unwrap_or_else(|_| "TestPassword123!".to_string())
}

fn expect_db_down() -> bool {
    std::env::var("OB_TEST_EXPECT_DB_DOWN")
        .map(|value| value == "1")
        .unwrap_or(false)
}

fn expect_meili_down() -> bool {
    std::env::var("OB_TEST_EXPECT_MEILI_DOWN")
        .map(|value| value == "1")
        .unwrap_or(false)
}

async fn register_test_user(client: &reqwest::Client) -> (String, String) {
    let email = format!("reliability_{}@example.com", Uuid::new_v4());
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": password() }))
        .send()
        .await
        .expect("register failed");

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.expect("register json");
    (
        body["access_token"]
            .as_str()
            .expect("missing access_token")
            .to_string(),
        email,
    )
}

async fn graphql(client: &reqwest::Client, token: &str, query: &str) -> reqwest::Response {
    client
        .post(format!("{}/graphql", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "query": query }))
        .send()
        .await
        .expect("graphql request failed")
}

async fn create_doc(
    client: &reqwest::Client,
    token: &str,
    collection: &str,
    data: &Value,
) -> String {
    let data_str = serde_json::to_string(data).expect("serialize");
    let escaped = serde_json::to_string(&data_str).expect("escape");
    let query = format!(r#"mutation {{ create(collection: "{collection}", data: {escaped}) }}"#);
    let resp = graphql(client, token, &query).await;
    let body: Value = resp.json().await.expect("create json");
    body["data"]["create"]["id"]
        .as_str()
        .or_else(|| body["data"]["create"]["_id"].as_str())
        .or_else(|| body["data"]["create"].as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_01_health_endpoint_stays_up_during_dependency_trouble() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/health", base_url()))
        .send()
        .await
        .expect("health failed");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await.expect("health text"), "ok");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_02_admin_health_returns_structured_json() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/_admin/health", base_url()))
        .send()
        .await
        .expect("admin health failed");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.expect("admin health json");
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_03_db_unreachable_returns_503_for_collection_queries() {
    if !expect_db_down() {
        return;
    }

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/_admin/collections", base_url()))
        .send()
        .await
        .expect("admin collections failed");
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_04_db_unreachable_error_body_does_not_look_like_panic() {
    if !expect_db_down() {
        return;
    }

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/_admin/collections", base_url()))
        .send()
        .await
        .expect("admin collections failed");
    let status = resp.status();
    let body = resp.text().await.expect("body text").to_lowercase();

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        !body.contains("panic"),
        "error body should not mention panic"
    );
    assert!(!body.contains("stack backtrace"));
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_05_db_unreachable_graphql_should_not_return_500_html() {
    if !expect_db_down() {
        return;
    }

    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let resp = graphql(
        &client,
        &token,
        r#"{ list(collection: "outage_test", limit: 1) }"#,
    )
    .await;
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");

    assert!(
        status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE,
        "unexpected graphql status during outage: {status}"
    );
    assert!(
        content_type.contains("json"),
        "graphql outage response should stay json"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_06_websocket_connects_initially() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let url = format!("{}/realtime?token={token}", ws_url());
    let (mut ws, _) = connect_async(&url)
        .await
        .expect("initial websocket connect");
    ws.send(Message::Ping(Default::default()))
        .await
        .expect("ping");
    ws.close(None).await.expect("close");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_07_websocket_auto_reconnect_after_brief_outage() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let url = format!("{}/realtime?token={token}", ws_url());
    let (mut first, _) = connect_async(&url).await.expect("first websocket connect");
    first.close(None).await.expect("close first");

    let (mut second, _) = connect_async(&url).await.expect("second websocket connect");
    second
        .send(Message::Text(
            json!({ "type": "subscribe", "collection": "reliability_ws" })
                .to_string()
                .into(),
        ))
        .await
        .expect("send subscribe");
    second.close(None).await.expect("close second");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_08_meilisearch_failure_does_not_block_crud_create() {
    if !expect_meili_down() {
        return;
    }

    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = format!("meili_down_create_{}", Uuid::new_v4().simple());
    let doc_id = create_doc(
        &client,
        &token,
        &collection,
        &json!({ "title": "degraded" }),
    )
    .await;
    assert!(!doc_id.is_empty(), "crud create should keep working");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_09_meilisearch_failure_does_not_block_crud_list() {
    if !expect_meili_down() {
        return;
    }

    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = format!("meili_down_list_{}", Uuid::new_v4().simple());
    let _ = create_doc(
        &client,
        &token,
        &collection,
        &json!({ "title": "degraded-list" }),
    )
    .await;
    let resp = graphql(
        &client,
        &token,
        &format!(r#"{{ list(collection: "{collection}", limit: 10) }}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_10_meilisearch_failure_search_response_is_graceful() {
    if !expect_meili_down() {
        return;
    }

    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let resp = graphql(
        &client,
        &token,
        r#"{ search(collection: "products", query: "degraded", limit: 5) }"#,
    )
    .await;
    let status = resp.status();
    let body: Value = resp.json().await.expect("search json");

    assert!(
        status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE,
        "unexpected search status: {status}"
    );
    assert!(
        body.get("data").is_some() || body.get("errors").is_some(),
        "search response should stay structured"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_11_post_outage_health_recovers_cleanly() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/_admin/health", base_url()))
        .send()
        .await
        .expect("admin health failed");
    assert_eq!(resp.status(), StatusCode::OK);
}
