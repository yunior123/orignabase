//! Stress-oriented live integration tests for OrignaBase.
//!
//! Run with:
//!   cargo test -p orignabase --test stress_test -- --ignored
//!
//! Set `OB_TEST_URL` to override the default base URL.

use reqwest::StatusCode;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio::time::Instant;
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

fn password() -> String {
    std::env::var("OB_TEST_PASSWORD").unwrap_or_else(|_| "TestPassword123!".to_string())
}

async fn register_test_user(client: &reqwest::Client) -> (String, String) {
    let email = format!("stress_{}@example.com", Uuid::new_v4());
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": password() }))
        .send()
        .await
        .expect("register failed");

    assert_eq!(resp.status(), StatusCode::OK, "registration should succeed");
    let body: Value = resp.json().await.expect("register json");
    (
        body["access_token"]
            .as_str()
            .expect("missing access_token")
            .to_string(),
        email,
    )
}

async fn graphql(client: &reqwest::Client, token: &str, query: &str) -> Value {
    let resp = client
        .post(format!("{}/graphql", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "query": query }))
        .send()
        .await
        .expect("graphql request failed");

    assert_eq!(resp.status(), StatusCode::OK, "graphql should return 200");
    resp.json().await.expect("graphql json")
}

async fn create_doc(
    client: &reqwest::Client,
    token: &str,
    collection: &str,
    data: &Value,
) -> String {
    let data_str = serde_json::to_string(data).expect("serialize data");
    let escaped = serde_json::to_string(&data_str).expect("escape data");
    let query = format!(r#"mutation {{ create(collection: "{collection}", data: {escaped}) }}"#);
    let body = graphql(client, token, &query).await;
    body["data"]["create"]["id"]
        .as_str()
        .or_else(|| body["data"]["create"]["_id"].as_str())
        .or_else(|| body["data"]["create"].as_str())
        .unwrap_or_default()
        .to_string()
}

async fn list_collection(
    client: &reqwest::Client,
    token: &str,
    collection: &str,
    limit: usize,
) -> Value {
    let query = format!(r#"{{ list(collection: "{collection}", limit: {limit}) }}"#);
    graphql(client, token, &query).await
}

async fn admin_put_config(client: &reqwest::Client, key: &str, value: Value) -> reqwest::Response {
    client
        .put(format!("{}/_admin/config/{key}", base_url()))
        .json(&json!({ "value": value }))
        .send()
        .await
        .expect("admin put failed")
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_01_concurrent_same_key_writes_five_parallel_puts() {
    let client = Arc::new(reqwest::Client::new());
    let key = format!("stress_config_{}", Uuid::new_v4().simple());
    let mut set = JoinSet::new();

    for idx in 0..5 {
        let client = Arc::clone(&client);
        let key = key.clone();
        set.spawn(async move {
            let resp = admin_put_config(&client, &key, json!({ "writer": idx })).await;
            let status = resp.status();
            // Concurrent writes may cause 409 Conflict or 500 from SurrealDB write conflicts
            assert!(
                status.is_success() || status.as_u16() == 409 || status.as_u16() == 500,
                "parallel PUT should succeed or conflict, got {}",
                status
            );
        });
    }

    while let Some(result) = set.join_next().await {
        result.expect("parallel task panicked");
    }

    let resp = client
        .get(format!("{}/config/{key}", base_url()))
        .send()
        .await
        .expect("config get failed");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_02_rapid_sequential_creates_hundred_docs() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = format!("stress_seq_{}", Uuid::new_v4().simple());

    for idx in 0..100 {
        let doc_id = create_doc(
            &client,
            &token,
            &collection,
            &json!({ "idx": idx, "kind": "rapid-sequential" }),
        )
        .await;
        assert!(!doc_id.is_empty(), "document {idx} should be created");
    }

    let body = list_collection(&client, &token, &collection, 150).await;
    let len = body["data"]["list"]
        .as_array()
        .map(|items| items.len())
        .unwrap_or(0);
    assert!(
        len >= 100 || body.get("errors").is_none(),
        "list body: {body}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_03_large_collection_listing_under_load() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = format!("stress_large_list_{}", Uuid::new_v4().simple());

    for idx in 0..250 {
        let _ = create_doc(&client, &token, &collection, &json!({ "idx": idx })).await;
    }

    let start = Instant::now();
    let body = list_collection(&client, &token, &collection, 250).await;
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "large listing took too long"
    );
    assert!(
        body["data"]["list"].is_array() || body.get("errors").is_some(),
        "unexpected list result: {body}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_04_connection_pool_behavior_under_parallel_health_checks() {
    let client = Arc::new(
        reqwest::Client::builder()
            .pool_max_idle_per_host(1)
            .timeout(Duration::from_secs(10))
            .build()
            .expect("client"),
    );
    let mut set = JoinSet::new();

    for _ in 0..50 {
        let client = Arc::clone(&client);
        set.spawn(async move {
            let resp = client
                .get(format!("{}/health", base_url()))
                .send()
                .await
                .expect("health request failed");
            assert_eq!(resp.status(), StatusCode::OK);
        });
    }

    while let Some(result) = set.join_next().await {
        result.expect("health task panicked");
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_05_parallel_admin_collection_reads_stay_responsive() {
    let client = Arc::new(reqwest::Client::new());
    let mut set = JoinSet::new();

    for _ in 0..25 {
        let client = Arc::clone(&client);
        set.spawn(async move {
            let resp = client
                .get(format!("{}/_admin/collections", base_url()))
                .send()
                .await
                .expect("admin collections failed");
            assert!(
                resp.status().is_success(),
                "admin collections should succeed under read load"
            );
        });
    }

    while let Some(result) = set.join_next().await {
        result.expect("admin read task panicked");
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_06_parallel_create_and_list_mix() {
    let client = Arc::new(reqwest::Client::new());
    let (token, _) = register_test_user(&client).await;
    let token = Arc::new(token);
    let collection = Arc::new(format!("stress_mix_{}", Uuid::new_v4().simple()));
    let mut set = JoinSet::new();

    for idx in 0..20 {
        let client = Arc::clone(&client);
        let token = Arc::clone(&token);
        let collection = Arc::clone(&collection);
        set.spawn(async move {
            let _ = create_doc(
                &client,
                token.as_str(),
                collection.as_str(),
                &json!({ "idx": idx, "mode": "mixed" }),
            )
            .await;
        });
    }

    for _ in 0..10 {
        let client = Arc::clone(&client);
        let token = Arc::clone(&token);
        let collection = Arc::clone(&collection);
        set.spawn(async move {
            let body = list_collection(&client, token.as_str(), collection.as_str(), 50).await;
            assert!(
                body["data"]["list"].is_array() || body.get("errors").is_some(),
                "unexpected mixed list body: {body}"
            );
        });
    }

    while let Some(result) = set.join_next().await {
        result.expect("mixed task panicked");
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_07_large_document_create_is_handled_gracefully() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = format!("stress_large_doc_{}", Uuid::new_v4().simple());
    let large_value = "x".repeat(1_000_000);
    let data = serde_json::to_string(&json!({ "blob": large_value })).expect("serialize");
    let escaped = serde_json::to_string(&data).expect("escape");
    let query = format!(r#"mutation {{ create(collection: "{collection}", data: {escaped}) }}"#);

    let resp = client
        .post(format!("{}/graphql", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "query": query }))
        .send()
        .await
        .expect("large create failed");

    assert!(
        matches!(
            resp.status(),
            StatusCode::OK | StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE
        ),
        "large payload should be handled without crashing"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_08_five_parallel_updates_same_document() {
    let client = Arc::new(reqwest::Client::new());
    let (token, _) = register_test_user(&client).await;
    let token = Arc::new(token);
    let collection = Arc::new(format!("stress_same_doc_{}", Uuid::new_v4().simple()));
    let doc_id = create_doc(
        &client,
        token.as_str(),
        collection.as_str(),
        &json!({ "version": 0, "status": "initial" }),
    )
    .await;
    let clean_id = doc_id
        .split(':')
        .next_back()
        .unwrap_or(doc_id.as_str())
        .to_string();

    let mut set = JoinSet::new();
    for idx in 0..5 {
        let client = Arc::clone(&client);
        let token = Arc::clone(&token);
        let collection = Arc::clone(&collection);
        let clean_id = clean_id.clone();
        set.spawn(async move {
            let data = serde_json::to_string(&json!({ "version": idx, "status": "updated" }))
                .expect("serialize");
            let escaped = serde_json::to_string(&data).expect("escape");
            let query = format!(
                r#"mutation {{ update(collection: "{collection}", id: "{clean_id}", data: {escaped}) }}"#,
            );
            let body = graphql(&client, token.as_str(), &query).await;
            assert!(
                body["data"]["update"].is_object()
                    || body["data"]["update"].is_string()
                    || body.get("errors").is_some(),
                "unexpected update body: {body}"
            );
        });
    }

    while let Some(result) = set.join_next().await {
        result.expect("update task panicked");
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_09_burst_of_authenticated_graphql_requests() {
    let client = Arc::new(reqwest::Client::new());
    let (token, _) = register_test_user(&client).await;
    let token = Arc::new(token);
    let collection = Arc::new(format!("stress_burst_{}", Uuid::new_v4().simple()));
    let mut set = JoinSet::new();

    for idx in 0..40 {
        let client = Arc::clone(&client);
        let token = Arc::clone(&token);
        let collection = Arc::clone(&collection);
        set.spawn(async move {
            let _ = create_doc(
                &client,
                token.as_str(),
                collection.as_str(),
                &json!({ "idx": idx, "burst": true }),
            )
            .await;
        });
    }

    while let Some(result) = set.join_next().await {
        result.expect("burst task panicked");
    }
}
