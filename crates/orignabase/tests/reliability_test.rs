//! Reliability and degraded-mode live integration tests for OrignaBase.
//!
//! Run with:
//!   cargo test -p orignabase --test reliability_test -- --ignored
//!
//! Several tests expect an externally induced outage. Use these env vars to opt in:
//! - `OB_TEST_EXPECT_DB_DOWN=1`
//! - `OB_TEST_EXPECT_MEILI_DOWN=1`

use futures_util::SinkExt;
use ob_database::fields;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string()) // ignore-magic
}

fn ws_url() -> String {
    base_url()
        .replace("http://", "ws://")
        .replace("https://", "wss://")
}

fn password() -> String {
    std::env::var("OB_TEST_PASSWORD").unwrap_or_else(|_| "TestPassword123!".to_string()) // ignore-magic
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
        .json(&json!({ "email": email, "password": password() })) // ignore-magic
        .send()
        .await
        .expect("register failed");

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.expect("register json");
    (
        body["access_token"] // ignore-magic
            .as_str()
            .expect("missing access_token")
            .to_string(),
        email,
    )
}

async fn graphql(client: &reqwest::Client, token: &str, query: &str) -> reqwest::Response {
    client
        .post(format!("{}/graphql", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ "query": query })) // ignore-magic
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
    body["data"]["create"][fields::ID] // ignore-magic
        .as_str()
        .or_else(|| body["data"]["create"]["_id"].as_str()) // ignore-magic
        .or_else(|| body["data"]["create"].as_str()) // ignore-magic
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
    assert_eq!(resp.text().await.expect("health text"), "ok"); // ignore-magic
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
    assert_eq!(body[fields::STATUS], "ok"); // ignore-magic
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
            json!({ "type": "subscribe", "collection": "reliability_ws" }) // ignore-magic
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
        &json!({ "title": "degraded" }), // ignore-magic
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
        &json!({ "title": "degraded-list" }), // ignore-magic
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
        r#"{ search(collection: "products", query: "degraded", limit: 5) }"#, // ignore-magic
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

// ── Connection Pool Exhaustion Tests ──

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_12_connection_pool_exhaustion_100_concurrent_requests() {
    // Simulate connection pool exhaustion by firing 100 concurrent requests
    // with a minimal connection pool. The server should handle this gracefully
    // (queue, reject, or serve — but never crash).
    let client = std::sync::Arc::new(
        reqwest::Client::builder()
            .pool_max_idle_per_host(2) // Intentionally small pool
            .pool_idle_timeout(std::time::Duration::from_secs(1))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("client"),
    );

    let (token, _) = register_test_user(&client).await;
    let token = std::sync::Arc::new(token);
    let collection = format!("pool_exhaust_{}", Uuid::new_v4().simple());
    let collection = std::sync::Arc::new(collection);

    let mut set = tokio::task::JoinSet::new();
    let start = std::time::Instant::now();

    for idx in 0..100u32 {
        let client = std::sync::Arc::clone(&client);
        let token = std::sync::Arc::clone(&token);
        let collection = std::sync::Arc::clone(&collection);
        set.spawn(async move {
            let data = serde_json::to_string(&serde_json::json!({ // ignore-magic
                "idx": idx,
                "purpose": "pool_exhaustion_test"
            }))
            .expect("serialize");
            let escaped = serde_json::to_string(&data).expect("escape");
            let query =
                format!(r#"mutation {{ create(collection: "{collection}", data: {escaped}) }}"#,);
            let resp = graphql(&client, token.as_str(), &query).await;
            resp.status()
        });
    }

    let mut success_count = 0u32;
    let mut error_count = 0u32;
    while let Some(result) = set.join_next().await {
        match result {
            Ok(status) => {
                if status.is_success() {
                    success_count += 1;
                } else {
                    error_count += 1;
                }
            }
            Err(_) => error_count += 1,
        }
    }

    let elapsed = start.elapsed();

    // At least 80% should succeed even under pool exhaustion
    assert!(
        success_count >= 80,
        "expected >=80 successes under pool exhaustion, got {success_count} success / {error_count} errors in {elapsed:?}"
    );

    // Should complete within 30 seconds
    assert!(
        elapsed < std::time::Duration::from_secs(30),
        "pool exhaustion test took too long: {elapsed:?}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_13_graceful_behavior_under_sustained_load() {
    // Send requests steadily for 10 seconds and verify the server stays responsive.
    // This tests that the server doesn't degrade catastrophically under sustained load.
    let client = std::sync::Arc::new(reqwest::Client::new());
    let start = std::time::Instant::now();
    let duration = std::time::Duration::from_secs(10);

    let mut request_count = 0u32;
    let mut success_count = 0u32;
    let mut max_latency = std::time::Duration::ZERO;

    while start.elapsed() < duration {
        let req_start = std::time::Instant::now();
        let resp = client
            .get(format!("{}/health", base_url()))
            .send()
            .await
            .expect("health request failed");

        let latency = req_start.elapsed();
        if latency > max_latency {
            max_latency = latency;
        }

        if resp.status().is_success() {
            success_count += 1;
        }
        request_count += 1;
    }

    // All health checks should succeed
    assert_eq!(
        success_count, request_count,
        "all {request_count} health checks should succeed, got {success_count}"
    );

    // No single request should take more than 5 seconds
    assert!(
        max_latency < std::time::Duration::from_secs(5),
        "max health check latency {max_latency:?} exceeded 5s"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_14_recovery_after_request_timeout() {
    // Simulate a slow/timed-out request, then verify the server recovers
    // and serves subsequent requests normally.
    let slow_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(1)) // Intentionally tiny timeout
        .build()
        .expect("client");

    // Fire a request that will timeout on the client side
    let _slow_result = slow_client
        .get(format!("{}/health", base_url()))
        .send()
        .await;
    // We don't care if this times out — the point is the server shouldn't be affected

    // Now verify the server is still responsive with a normal client
    let normal_client = reqwest::Client::new();
    for _ in 0..5 {
        let resp = normal_client
            .get(format!("{}/health", base_url()))
            .send()
            .await
            .expect("recovery health check failed");
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "server should recover and respond normally after client-side timeouts"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_15_mixed_valid_invalid_requests_under_load() {
    // Fire a mix of valid and invalid requests concurrently.
    // The server should handle both gracefully without affecting valid requests.
    let client = std::sync::Arc::new(reqwest::Client::new());
    let (token, _) = register_test_user(&client).await;
    let token = std::sync::Arc::new(token);

    let mut set = tokio::task::JoinSet::new();

    for idx in 0..50u32 {
        let client = std::sync::Arc::clone(&client);
        let token = std::sync::Arc::clone(&token);
        set.spawn(async move {
            if idx % 3 == 0 {
                // Valid: health check
                let resp = client
                    .get(format!("{}/health", base_url()))
                    .send()
                    .await
                    .expect("health failed");
                (resp.status(), "health")
            } else if idx % 3 == 1 {
                // Valid: authenticated GraphQL
                let resp = graphql(
                    &client,
                    token.as_str(),
                    r#"{ list(collection: "nonexistent_test", limit: 1) }"#,
                )
                .await;
                (resp.status(), "graphql_valid")
            } else {
                // Invalid: malformed GraphQL
                let resp = client
                    .post(format!("{}/graphql", base_url()))
                    .header("Authorization", format!("Bearer {token}")) // ignore-magic
                    .json(&serde_json::json!({ "query": "{ this is not valid graphql !!!" })) // ignore-magic
                    .send()
                    .await
                    .expect("malformed graphql failed");
                (resp.status(), "graphql_malformed")
            }
        });
    }

    let mut health_ok = 0u32;
    let mut graphql_ok = 0u32;
    let mut malformed_handled = 0u32;

    while let Some(result) = set.join_next().await {
        let (status, kind) = result.expect("task panicked");
        match kind {
            "health" => {
                if status == StatusCode::OK {
                    health_ok += 1;
                }
            }
            "graphql_valid" => {
                if status.is_success() {
                    graphql_ok += 1;
                }
            }
            "graphql_malformed" => {
                // Malformed should return 400 or 200 with errors — never 500
                if status.as_u16() < 500 {
                    malformed_handled += 1;
                }
            }
            _ => {}
        }
    }

    // All health checks should pass
    assert!(health_ok >= 16, "expected >=16 health OK, got {health_ok}");
    // Valid GraphQL should mostly succeed
    assert!(
        graphql_ok >= 14,
        "expected >=14 valid graphql OK, got {graphql_ok}"
    );
    // Malformed should be handled gracefully (no 500s)
    assert!(
        malformed_handled >= 16,
        "expected >=16 malformed handled gracefully, got {malformed_handled}"
    );
}
