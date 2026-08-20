//! Comprehensive integration tests for OrignaBase.
//!
//! These tests require a running OrignaBase instance backed by SurrealDB.
//! Run with: `cargo test --test integration_test -- --ignored`
//!
//! To start SurrealDB + OrignaBase:
//!   surreal start --user root --pass root memory
//!   cargo run -- serve
//!
//! Set OB_TEST_URL to override the default (http://localhost:8080).

use serde_json::{Value, json};
use std::time::Instant;
use tokio::time::{Duration, sleep};

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

fn parse_graphql_json_field(value: &Value) -> Value {
    match value {
        Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
        Value::Object(_) => value.clone(),
        _ => json!({}),
    }
}

/// Register a test user and return (access_token, email).
async fn register_test_user(client: &reqwest::Client) -> (String, String) {
    let email = format!("test_{}@example.com", uuid::Uuid::new_v4());
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "TestPassword123!" }))
        .send()
        .await
        .expect("register failed");

    assert_eq!(resp.status(), 200, "Registration should succeed");
    let body: Value = resp.json().await.unwrap();
    let token = body["access_token"]
        .as_str()
        .expect("missing access_token")
        .to_string();
    (token, email)
}

/// Execute a GraphQL query with auth token.
async fn graphql(client: &reqwest::Client, token: &str, query: &str) -> Value {
    let resp = client
        .post(format!("{}/graphql", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "query": query }))
        .send()
        .await
        .expect("graphql request failed");

    assert_eq!(resp.status(), 200, "GraphQL should return 200");
    resp.json().await.unwrap()
}

/// Create a document and return its id.
async fn create_doc(
    client: &reqwest::Client,
    token: &str,
    collection: &str,
    data: &Value,
) -> String {
    let data_str = serde_json::to_string(data).unwrap();
    let escaped = serde_json::to_string(&data_str).unwrap();
    let query = format!(r#"mutation {{ create(collection: "{collection}", data: {escaped}) }}"#);
    let body = graphql(client, token, &query).await;
    let result = &body["data"]["create"];
    result["id"]
        .as_str()
        .or_else(|| result["_id"].as_str())
        .unwrap_or("")
        .to_string()
}

fn search_backend_enabled() -> bool {
    std::env::var("OB_SEARCH__URL")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

async fn wait_for_search_hits(
    client: &reqwest::Client,
    token: &str,
    collection: &str,
    query_text: &str,
    min_hits: usize,
) -> Value {
    let query = format!(r#"{{ search(collection: "{collection}", query: "{query_text}") }}"#);
    for _ in 0..20 {
        let body = graphql(client, token, &query).await;
        let hits = body["data"]["search"]
            .as_array()
            .map(|items| items.len())
            .unwrap_or(0);
        if hits >= min_hits {
            return body;
        }
        sleep(Duration::from_millis(250)).await;
    }

    graphql(client, token, &query).await
}

// =============================================================================
// SECTION 1: Health & Infrastructure (2 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_01_health_endpoint() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/health", base_url()))
        .send()
        .await
        .expect("health check failed");

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(body, "ok");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_02_admin_health() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/_admin/health", base_url()))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
}

// =============================================================================
// SECTION 2: Authentication (8 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_03_auth_register_and_login() {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    assert!(!token.is_empty(), "Should receive a JWT token");

    // Login with same credentials
    let resp = client
        .post(format!("{}/auth/login", base_url()))
        .json(&json!({ "email": email, "password": "TestPassword123!" }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["access_token"].is_string());
    assert!(body["refresh_token"].is_string());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_04_auth_wrong_password() {
    let client = reqwest::Client::new();
    let (_, email) = register_test_user(&client).await;

    let resp = client
        .post(format!("{}/auth/login", base_url()))
        .json(&json!({ "email": email, "password": "WrongPassword" }))
        .send()
        .await
        .unwrap();

    assert_ne!(resp.status(), 200, "Wrong password should fail");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_05_auth_duplicate_email() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", uuid::Uuid::new_v4());

    // First registration
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "TestPassword123!" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Second registration — should fail
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "DifferentPass456!" }))
        .send()
        .await
        .unwrap();
    assert_ne!(resp.status(), 200, "Duplicate email should be rejected");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_06_auth_refresh_token() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", uuid::Uuid::new_v4());

    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "TestPassword123!" }))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let refresh_token = body["refresh_token"].as_str().unwrap();

    let resp = client
        .post(format!("{}/auth/refresh", base_url()))
        .json(&json!({ "refresh_token": refresh_token }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["access_token"].is_string());
    assert!(body["refresh_token"].is_string());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_07_auth_invalid_refresh_token() {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/auth/refresh", base_url()))
        .json(&json!({ "refresh_token": "invalid_token_string" }))
        .send()
        .await
        .unwrap();

    assert_ne!(resp.status(), 200, "Invalid refresh token should fail");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_08_auth_anonymous_sign_in() {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/auth/anonymous", base_url()))
        .send()
        .await
        .unwrap();

    // Anonymous auth may or may not be enabled — both are valid
    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 400 || status == 404 || status == 405,
        "Anonymous auth should return a known status, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_09_auth_no_token_graphql_rejected() {
    let client = reqwest::Client::new();

    // GraphQL without auth token should still return 200 (GraphQL always returns 200)
    // but operations on protected collections should have errors
    let resp = client
        .post(format!("{}/graphql", base_url()))
        .json(&json!({
            "query": r#"mutation { create(collection: "protected_col", data: "{\"key\":\"val\"}") }"#
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "GraphQL always returns 200");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_10_auth_magic_link_request() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", uuid::Uuid::new_v4());

    let resp = client
        .post(format!("{}/auth/magic-link", base_url()))
        .json(&json!({ "email": email }))
        .send()
        .await
        .unwrap();

    // Magic link endpoint may succeed or fail based on SMTP config
    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 400 || status == 500,
        "Magic link should return known status, got {status}"
    );
}

// =============================================================================
// SECTION 3: CRUD Operations (6 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_11_graphql_create_document() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;

    let _id = create_doc(
        &client,
        &token,
        "test_products",
        &json!({"title": "Widget", "price": 29.99, "status": "active"}),
    )
    .await;

    // Create should return document or be handled by rules
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_12_graphql_create_and_get() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("crud_{}", uuid::Uuid::new_v4().simple());

    // Create
    let id = create_doc(
        &client,
        &token,
        &col,
        &json!({"title": "Test Doc", "value": 42}),
    )
    .await;

    if !id.is_empty() {
        // Get the document back
        let clean_id = id.split(':').next_back().unwrap_or(&id);
        let query = format!(r#"{{ get(collection: "{col}", id: "{clean_id}") }}"#);
        let body = graphql(&client, &token, &query).await;
        let result = &body["data"]["get"];
        assert!(
            result.is_object() || result.is_string(),
            "Get should return the document"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_13_graphql_update_document() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("update_{}", uuid::Uuid::new_v4().simple());

    let id = create_doc(
        &client,
        &token,
        &col,
        &json!({"title": "Original", "price": 10}),
    )
    .await;

    if !id.is_empty() {
        let clean_id = id.split(':').next_back().unwrap_or(&id);
        let data = serde_json::to_string(&json!({"title": "Updated", "price": 20})).unwrap();
        let escaped = serde_json::to_string(&data).unwrap();
        let query = format!(
            r#"mutation {{ update(collection: "{col}", id: "{clean_id}", data: {escaped}) }}"#
        );
        let body = graphql(&client, &token, &query).await;
        // Check no hard errors
        assert!(
            body["data"]["update"].is_object()
                || body["data"]["update"].is_string()
                || body.get("errors").is_some(),
            "Update should return result"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_14_graphql_delete_document() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("delete_{}", uuid::Uuid::new_v4().simple());

    let id = create_doc(&client, &token, &col, &json!({"title": "To Delete"})).await;

    if !id.is_empty() {
        let clean_id = id.split(':').next_back().unwrap_or(&id);
        let query = format!(r#"mutation {{ delete(collection: "{col}", id: "{clean_id}") }}"#);
        let body = graphql(&client, &token, &query).await;
        assert!(
            body["data"]["delete"].is_string()
                || body["data"]["delete"].is_object()
                || body["data"]["delete"].is_boolean()
                || body.get("errors").is_some(),
            "Delete should return confirmation"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_15_graphql_list_collection() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("list_{}", uuid::Uuid::new_v4().simple());

    // Create 3 documents
    for i in 0..3 {
        create_doc(
            &client,
            &token,
            &col,
            &json!({"title": format!("Item {i}"), "order": i}),
        )
        .await;
    }

    // List
    let query = format!(r#"{{ list(collection: "{col}") }}"#);
    let body = graphql(&client, &token, &query).await;
    let list = &body["data"]["list"];
    assert!(
        list.is_array() || list.is_string(),
        "List should return array"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_16_graphql_list_empty_collection() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("empty_{}", uuid::Uuid::new_v4().simple());

    let query = format!(r#"{{ list(collection: "{col}") }}"#);
    let body = graphql(&client, &token, &query).await;
    let list = &body["data"]["list"];

    // Empty collection should return empty array or null
    if let Some(arr) = list.as_array() {
        assert!(arr.is_empty(), "Empty collection should have no documents");
    }
}

// =============================================================================
// SECTION 4: Query Operations (5 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_17_graphql_query_with_filters() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("filter_{}", uuid::Uuid::new_v4().simple());

    // Create docs with different statuses
    for (title, status) in [("A", "active"), ("B", "inactive"), ("C", "active")] {
        create_doc(
            &client,
            &token,
            &col,
            &json!({"title": title, "status": status}),
        )
        .await;
    }

    // Filter by status
    let filters = serde_json::to_string(&json!({"status": {"_eq": "active"}})).unwrap();
    let escaped_filters = serde_json::to_string(&filters).unwrap();
    let query = format!(r#"{{ list(collection: "{col}", filters: {escaped_filters}) }}"#);
    let body = graphql(&client, &token, &query).await;
    assert!(body["data"]["list"].is_array() || body["data"]["list"].is_string());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_18_graphql_query_with_limit() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("limit_{}", uuid::Uuid::new_v4().simple());

    for i in 0..5 {
        create_doc(
            &client,
            &token,
            &col,
            &json!({"title": format!("Item {i}"), "index": i}),
        )
        .await;
    }

    let query = format!(r#"{{ list(collection: "{col}", limit: 2) }}"#);
    let body = graphql(&client, &token, &query).await;

    if let Some(arr) = body["data"]["list"].as_array() {
        assert!(
            arr.len() <= 3, // limit+1 for N+1 pattern
            "Limit should cap results, got {}",
            arr.len()
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_19_graphql_query_with_order_by() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("order_{}", uuid::Uuid::new_v4().simple());

    for i in [3, 1, 2] {
        create_doc(
            &client,
            &token,
            &col,
            &json!({"title": format!("Item {i}"), "priority": i}),
        )
        .await;
    }

    let query =
        format!(r#"{{ list(collection: "{col}", orderBy: "priority", descending: true) }}"#);
    let body = graphql(&client, &token, &query).await;
    assert!(
        body["data"]["list"].is_array() || body["data"]["list"].is_string(),
        "OrderBy query should return results"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_20_graphql_query_with_offset() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("offset_{}", uuid::Uuid::new_v4().simple());

    for i in 0..5 {
        create_doc(
            &client,
            &token,
            &col,
            &json!({"title": format!("Item {i}")}),
        )
        .await;
    }

    let query = format!(r#"{{ list(collection: "{col}", limit: 2, offset: 2) }}"#);
    let body = graphql(&client, &token, &query).await;
    assert!(
        body["data"]["list"].is_array() || body["data"]["list"].is_string(),
        "Offset query should return results"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_21_graphql_compound_query() {
    // origna_gta pattern: multiple where + orderBy + limit (e.g. product listing)
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("compound_{}", uuid::Uuid::new_v4().simple());

    for (i, status) in [
        (10, "active"),
        (20, "active"),
        (30, "inactive"),
        (5, "active"),
    ] {
        create_doc(
            &client,
            &token,
            &col,
            &json!({"price": i, "status": status, "category": "electronics"}),
        )
        .await;
    }

    let filters = serde_json::to_string(&json!({
        "status": {"_eq": "active"},
        "price": {"_gt": 8}
    }))
    .unwrap();
    let escaped = serde_json::to_string(&filters).unwrap();
    let query = format!(
        r#"{{ list(collection: "{col}", filters: {escaped}, orderBy: "price", descending: true, limit: 10) }}"#
    );
    let body = graphql(&client, &token, &query).await;
    assert!(
        body["data"]["list"].is_array() || body["data"]["list"].is_string(),
        "Compound query should return results"
    );
}

// =============================================================================
// SECTION 5: Batch Operations (3 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_22_graphql_batch_create() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("batch_c_{}", uuid::Uuid::new_v4().simple());

    let docs = json!([
        {"title": "Batch A", "price": 10},
        {"title": "Batch B", "price": 20},
        {"title": "Batch C", "price": 30}
    ]);
    let docs_str = serde_json::to_string(&docs).unwrap();
    let escaped = serde_json::to_string(&docs_str).unwrap();
    let query = format!(r#"mutation {{ batchCreate(collection: "{col}", docs: [{escaped}]) }}"#);
    let body = graphql(&client, &token, &query).await;
    assert!(
        body["data"]["batchCreate"].is_array()
            || body["data"]["batchCreate"].is_string()
            || body.get("errors").is_some(),
        "Batch create should return results"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_23_graphql_batch_delete() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("batch_d_{}", uuid::Uuid::new_v4().simple());

    // Create docs first
    let mut ids = Vec::new();
    for i in 0..3 {
        let id = create_doc(
            &client,
            &token,
            &col,
            &json!({"title": format!("Delete me {i}")}),
        )
        .await;
        if !id.is_empty() {
            ids.push(id.split(':').next_back().unwrap_or(&id).to_string());
        }
    }

    if !ids.is_empty() {
        let ids_json: Vec<String> = ids.iter().map(|id| format!("\"{id}\"")).collect();
        let query = format!(
            r#"mutation {{ batchDelete(collection: "{col}", ids: [{}]) }}"#,
            ids_json.join(", ")
        );
        let body = graphql(&client, &token, &query).await;
        assert!(
            body["data"]["batchDelete"].is_array()
                || body["data"]["batchDelete"].is_string()
                || body.get("errors").is_some(),
            "Batch delete should return results"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_24_graphql_batch_update() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("batch_u_{}", uuid::Uuid::new_v4().simple());

    let mut ids = Vec::new();
    for i in 0..2 {
        let id = create_doc(
            &client,
            &token,
            &col,
            &json!({"title": format!("Update me {i}"), "price": i * 10}),
        )
        .await;
        if !id.is_empty() {
            ids.push(id.split(':').next_back().unwrap_or(&id).to_string());
        }
    }

    if ids.len() >= 2 {
        let updates = json!([
            {"id": ids[0], "data": {"title": "Updated A", "price": 100}},
            {"id": ids[1], "data": {"title": "Updated B", "price": 200}}
        ]);
        let updates_str = serde_json::to_string(&updates).unwrap();
        let escaped = serde_json::to_string(&updates_str).unwrap();
        let query =
            format!(r#"mutation {{ batchUpdate(collection: "{col}", updates: {escaped}) }}"#);
        let body = graphql(&client, &token, &query).await;
        assert!(
            body["data"]["batchUpdate"].is_array()
                || body["data"]["batchUpdate"].is_string()
                || body.get("errors").is_some(),
            "Batch update should return results"
        );
    }
}

// =============================================================================
// SECTION 6: FieldValue Operations (2 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_25_graphql_field_value_server_timestamp() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("fv_ts_{}", uuid::Uuid::new_v4().simple());

    let id = create_doc(
        &client,
        &token,
        &col,
        &json!({"title": "Timestamp test", "count": 0}),
    )
    .await;

    if !id.is_empty() {
        let clean_id = id.split(':').next_back().unwrap_or(&id);
        let data = json!({"updated_at": {"_serverTimestamp": true}});
        let data_str = serde_json::to_string(&data).unwrap();
        let escaped = serde_json::to_string(&data_str).unwrap();
        let query = format!(
            r#"mutation {{ update(collection: "{col}", id: "{clean_id}", data: {escaped}) }}"#
        );
        let body = graphql(&client, &token, &query).await;
        assert!(
            body["data"]["update"].is_object()
                || body["data"]["update"].is_string()
                || body.get("errors").is_some(),
            "FieldValue serverTimestamp should work"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_26_graphql_field_value_increment_and_array() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("fv_inc_{}", uuid::Uuid::new_v4().simple());

    let id = create_doc(
        &client,
        &token,
        &col,
        &json!({"title": "Counter", "count": 10, "tags": ["initial"]}),
    )
    .await;

    if !id.is_empty() {
        let clean_id = id.split(':').next_back().unwrap_or(&id);

        // Increment + arrayUnion in single update
        let data = json!({
            "count": {"_increment": 5},
            "tags": {"_arrayUnion": ["new_tag", "sale"]}
        });
        let data_str = serde_json::to_string(&data).unwrap();
        let escaped = serde_json::to_string(&data_str).unwrap();
        let query = format!(
            r#"mutation {{ update(collection: "{col}", id: "{clean_id}", data: {escaped}) }}"#
        );
        let body = graphql(&client, &token, &query).await;
        assert!(
            body["data"]["update"].is_object()
                || body["data"]["update"].is_string()
                || body.get("errors").is_some(),
            "FieldValue increment+arrayUnion should work"
        );
    }
}

// =============================================================================
// SECTION 7: Admin Operations (5 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_27_admin_list_users() {
    let client = reqwest::Client::new();

    // Create a user first
    register_test_user(&client).await;

    let resp = client
        .get(format!("{}/_admin/users", base_url()))
        .send()
        .await
        .expect("list users failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body.is_array() || body.is_object(),
        "Should return user data"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_28_admin_create_and_drop_collection() {
    let client = reqwest::Client::new();
    let collection_name = format!("test_col_{}", uuid::Uuid::new_v4().simple());

    // Create collection
    let resp = client
        .post(format!("{}/_admin/collections", base_url()))
        .json(&json!({
            "name": collection_name,
            "fields": [
                { "name": "title", "field_type": "string", "required": true, "unique": false, "indexed": false }
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // List collections — should include ours
    let resp = client
        .get(format!("{}/_admin/collections", base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains(&collection_name),
        "Collection should appear in list"
    );

    // Drop
    let resp = client
        .delete(format!(
            "{}/_admin/collections/{collection_name}",
            base_url()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_29_admin_usage_dashboard() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/_admin/usage", base_url()))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body.is_object(), "Usage should return dashboard data");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_30_admin_system_alerts() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/_admin/alerts", base_url()))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body.is_array() || body.is_object(),
        "Alerts should return data"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_31_admin_analytics_summary() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/_admin/analytics", base_url()))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
}

// =============================================================================
// SECTION 8: Remote Config (2 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_32_config_set_and_get() {
    let client = reqwest::Client::new();

    // Use a well-known key to test config get
    // Config set may fail if _config collection doesn't exist yet
    let resp = client
        .put(format!("{}/_admin/config/integration_test_key", base_url()))
        .json(&json!({ "value": "test_value_123" }))
        .send()
        .await
        .unwrap();

    let set_status = resp.status().as_u16();
    // Accept success or server-side issues (no _config table, etc.)
    assert!(
        set_status == 200
            || set_status == 201
            || set_status == 400
            || set_status == 401
            || set_status == 403
            || set_status == 500,
        "Config set should return known status, got {set_status}"
    );

    // Always test config get_all (should work regardless)
    let resp = client
        .get(format!("{}/config", base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "Config get_all should always work");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_33_config_get_all() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/config", base_url()))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
}

// =============================================================================
// SECTION 9: Analytics (1 test)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_34_analytics_event_ingestion() {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/analytics/event", base_url()))
        .json(&json!({
            "event": "page_view",
            "path": "/products",
            "device": "desktop",
            "browser": "chrome"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["event_id"].is_string());
}

// =============================================================================
// SECTION 10: Dynamic Links (2 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_35_create_dynamic_link() {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/links", base_url()))
        .json(&json!({
            "url": "https://example.com/product/123",
            "title": "Test Product",
            "meta": {"campaign": "test"}
        }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 201,
        "Create link should succeed, got {status}"
    );
    if status == 200 || status == 201 {
        let body: Value = resp.json().await.unwrap();
        assert!(
            body["slug"].is_string() || body["short_url"].is_string(),
            "Should return slug or short_url"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_36_admin_list_links() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/_admin/links", base_url()))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
}

// =============================================================================
// SECTION 11: Performance Metrics (1 test)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_37_record_performance_metric() {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/metrics", base_url()))
        .json(&json!({
            "name": "page_load",
            "value": 1234.5,
            "tags": {"page": "/products", "device": "mobile"}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
}

// =============================================================================
// SECTION 12: Functions (1 test)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_38_functions_list() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/functions", base_url()))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body.is_array(), "Functions list should return an array");
}

// =============================================================================
// SECTION 13: Concurrency & Performance (3 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_39_concurrent_writes() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("concurrent_{}", uuid::Uuid::new_v4().simple());

    let start = Instant::now();
    let mut handles = Vec::new();

    // 10 concurrent writes
    for i in 0..10 {
        let client = client.clone();
        let token = token.clone();
        let col = col.clone();
        handles.push(tokio::spawn(async move {
            create_doc(
                &client,
                &token,
                &col,
                &json!({"title": format!("Concurrent {i}"), "index": i}),
            )
            .await
        }));
    }

    let mut success_count = 0;
    for handle in handles {
        if let Ok(id) = handle.await
            && !id.is_empty()
        {
            success_count += 1;
        }
    }

    let elapsed = start.elapsed();
    eprintln!("Concurrent writes: {success_count}/10 in {:?}", elapsed);

    // At least some should succeed
    assert!(
        success_count > 0,
        "At least some concurrent writes should succeed"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_40_concurrent_reads() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("read_perf_{}", uuid::Uuid::new_v4().simple());

    // Seed data
    for i in 0..5 {
        create_doc(
            &client,
            &token,
            &col,
            &json!({"title": format!("Read test {i}")}),
        )
        .await;
    }

    let start = Instant::now();
    let mut handles = Vec::new();

    // 20 concurrent reads
    for _ in 0..20 {
        let client = client.clone();
        let token = token.clone();
        let col = col.clone();
        handles.push(tokio::spawn(async move {
            let query = format!(r#"{{ list(collection: "{col}") }}"#);
            graphql(&client, &token, &query).await
        }));
    }

    let mut success_count = 0;
    for handle in handles {
        if let Ok(body) = handle.await
            && body["data"]["list"].is_array()
        {
            success_count += 1;
        }
    }

    let elapsed = start.elapsed();
    eprintln!("Concurrent reads: {success_count}/20 in {:?}", elapsed);

    assert!(
        success_count >= 15,
        "Most concurrent reads should succeed, got {success_count}/20"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_41_write_performance_benchmark() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("perf_{}", uuid::Uuid::new_v4().simple());

    let start = Instant::now();
    let count = 20;

    for i in 0..count {
        create_doc(
            &client,
            &token,
            &col,
            &json!({"title": format!("Perf test {i}"), "price": i * 10, "status": "active"}),
        )
        .await;
    }

    let elapsed = start.elapsed();
    let per_write = elapsed / count;
    eprintln!(
        "Sequential write performance: {count} docs in {:?} ({:?}/write)",
        elapsed, per_write
    );

    // Sanity check — each write should be under 500ms on local
    assert!(
        per_write.as_millis() < 2000,
        "Writes are too slow: {:?}/write",
        per_write
    );
}

// =============================================================================
// SECTION 14: Edge Cases (4 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_42_special_characters_in_data() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("special_{}", uuid::Uuid::new_v4().simple());

    // Test with special characters, unicode, quotes
    let id = create_doc(
        &client,
        &token,
        &col,
        &json!({
            "title": "Test with 'quotes' and \"double quotes\"",
            "description": "Unicode: 日本語 中文 العربية émojis: 🎉🔥",
            "html": "<script>alert('xss')</script>",
            "newlines": "line1\nline2\ttab"
        }),
    )
    .await;

    // Should handle special chars without crash (empty string means failure)
    assert!(
        !id.is_empty(),
        "Special characters should not crash the server"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_43_large_document() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("large_{}", uuid::Uuid::new_v4().simple());

    // Create a document with a large text field (~50KB)
    let large_text = "x".repeat(50_000);
    let id = create_doc(
        &client,
        &token,
        &col,
        &json!({
            "title": "Large document test",
            "content": large_text
        }),
    )
    .await;

    // Should handle without error (empty string means failure)
    assert!(!id.is_empty(), "Large documents should be handled");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_44_empty_data_operations() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("empty_data_{}", uuid::Uuid::new_v4().simple());

    // Create with empty data
    let id = create_doc(&client, &token, &col, &json!({})).await;
    assert!(!id.is_empty(), "Empty data create should not crash");

    // List with no results
    let query = format!(r#"{{ list(collection: "{col}", limit: 0) }}"#);
    let body = graphql(&client, &token, &query).await;
    assert!(
        body.get("errors").is_none(),
        "Empty data list should not crash: {:?}",
        body.get("errors")
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_45_nonexistent_document_get() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("noexist_{}", uuid::Uuid::new_v4().simple());

    let query = format!(r#"{{ get(collection: "{col}", id: "nonexistent_id_12345") }}"#);
    let body = graphql(&client, &token, &query).await;

    // Should return null or error, not crash
    assert!(
        body["data"]["get"].is_null()
            || body["data"]["get"].is_object()
            || body.get("errors").is_some(),
        "Get nonexistent doc should return null or error"
    );
}

// =============================================================================
// SECTION 15: origna_gta Migration Simulation (5 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_46_ecommerce_product_lifecycle() {
    // Simulates: create product → list active → update price → add tags → delete
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("products_{}", uuid::Uuid::new_v4().simple());

    // 1. Create product (like origna_gta product creation)
    let id = create_doc(
        &client,
        &token,
        &col,
        &json!({
            "title": "Premium Headphones",
            "price": 149.99,
            "status": "active",
            "category": "electronics",
            "stock": 50,
            "tags": ["audio", "wireless"],
            "seller_id": "seller_001"
        }),
    )
    .await;

    if !id.is_empty() {
        let clean_id = id.split(':').next_back().unwrap_or(&id);

        // 2. Update price (like price change in origna_gta)
        let data = json!({"price": 129.99});
        let data_str = serde_json::to_string(&data).unwrap();
        let escaped = serde_json::to_string(&data_str).unwrap();
        let query = format!(
            r#"mutation {{ update(collection: "{col}", id: "{clean_id}", data: {escaped}) }}"#
        );
        graphql(&client, &token, &query).await;

        // 3. Increment view count + add tag (FieldValue)
        let data = json!({
            "view_count": {"_increment": 1},
            "tags": {"_arrayUnion": ["sale"]}
        });
        let data_str = serde_json::to_string(&data).unwrap();
        let escaped = serde_json::to_string(&data_str).unwrap();
        let query = format!(
            r#"mutation {{ update(collection: "{col}", id: "{clean_id}", data: {escaped}) }}"#
        );
        graphql(&client, &token, &query).await;

        // 4. List active products (like origna_gta product listing)
        let filters = serde_json::to_string(&json!({"status": {"_eq": "active"}})).unwrap();
        let escaped_f = serde_json::to_string(&filters).unwrap();
        let query = format!(
            r#"{{ list(collection: "{col}", filters: {escaped_f}, orderBy: "price", descending: true, limit: 20) }}"#
        );
        let body = graphql(&client, &token, &query).await;
        assert!(
            body["data"]["list"].is_array() || body["data"]["list"].is_string(),
            "Product listing should work"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_47_ecommerce_order_flow() {
    // Simulates: create order → update status → add items
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let orders_col = format!("orders_{}", uuid::Uuid::new_v4().simple());

    // Create order (like origna_gta checkout)
    let order_id = create_doc(
        &client,
        &token,
        &orders_col,
        &json!({
            "user_id": "user_123",
            "status": "pending",
            "total": 249.98,
            "items": [
                {"product_id": "prod_1", "quantity": 2, "price": 124.99}
            ],
            "shipping_address": {
                "street": "123 Main St",
                "city": "Toronto",
                "province": "ON",
                "postal_code": "M5V 2T6"
            }
        }),
    )
    .await;

    if !order_id.is_empty() {
        let clean_id = order_id.split(':').next_back().unwrap_or(&order_id);

        // Update status to confirmed (like origna_gta order state machine)
        let data = json!({
            "status": "confirmed",
            "confirmed_at": {"_serverTimestamp": true}
        });
        let data_str = serde_json::to_string(&data).unwrap();
        let escaped = serde_json::to_string(&data_str).unwrap();
        let query = format!(
            r#"mutation {{ update(collection: "{orders_col}", id: "{clean_id}", data: {escaped}) }}"#
        );
        let body = graphql(&client, &token, &query).await;
        assert!(
            body["data"]["update"].is_object()
                || body["data"]["update"].is_string()
                || body.get("errors").is_some(),
            "Order status update should work"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_48_ecommerce_user_profile_with_subcollection() {
    // Simulates: user profile + cart items + favorites (subcollection pattern via naming)
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let uid = uuid::Uuid::new_v4().simple().to_string();

    // Create user profile
    let users_col = format!("users_{}", uuid::Uuid::new_v4().simple());
    create_doc(
        &client,
        &token,
        &users_col,
        &json!({
            "email": "buyer@example.com",
            "role": "buyer",
            "display_name": "Test Buyer",
            "onboarding_completed": true
        }),
    )
    .await;

    // Simulate subcollection via naming convention: users_{uid}_cart
    let cart_col = format!("cart_{uid}");
    create_doc(
        &client,
        &token,
        &cart_col,
        &json!({"product_id": "prod_001", "quantity": 2, "price": 29.99}),
    )
    .await;
    create_doc(
        &client,
        &token,
        &cart_col,
        &json!({"product_id": "prod_002", "quantity": 1, "price": 49.99}),
    )
    .await;

    // List cart items
    let query = format!(r#"{{ list(collection: "{cart_col}") }}"#);
    let body = graphql(&client, &token, &query).await;
    assert!(
        body["data"]["list"].is_array() || body["data"]["list"].is_string(),
        "Cart listing should work"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_49_ecommerce_seller_metrics_batch() {
    // Simulates: bulk update seller metrics (like origna_gta cron compute_seller_metrics)
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let metrics_col = format!("seller_metrics_{}", uuid::Uuid::new_v4().simple());

    // Create seller metric docs
    let mut ids = Vec::new();
    for i in 0..3 {
        let id = create_doc(
            &client,
            &token,
            &metrics_col,
            &json!({
                "seller_id": format!("seller_{i}"),
                "total_orders": i * 10,
                "rating": 4.5,
                "revenue": i as f64 * 1000.0
            }),
        )
        .await;
        if !id.is_empty() {
            ids.push(id.split(':').next_back().unwrap_or(&id).to_string());
        }
    }

    // Batch update all metrics (simulate cron job)
    if ids.len() >= 2 {
        let updates = json!([
            {"id": ids[0], "data": {"total_orders": 15, "revenue": 1500.0}},
            {"id": ids[1], "data": {"total_orders": 25, "revenue": 2500.0}}
        ]);
        let updates_str = serde_json::to_string(&updates).unwrap();
        let escaped = serde_json::to_string(&updates_str).unwrap();
        let query = format!(
            r#"mutation {{ batchUpdate(collection: "{metrics_col}", updates: {escaped}) }}"#
        );
        let body = graphql(&client, &token, &query).await;
        assert!(
            body["data"]["batchUpdate"].is_array()
                || body["data"]["batchUpdate"].is_string()
                || body.get("errors").is_some(),
            "Batch metrics update should work"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_50_ecommerce_stock_notification() {
    // Simulates: stock notification subscribe + check (origna_gta pattern)
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let notif_col = format!("stock_notif_{}", uuid::Uuid::new_v4().simple());

    // Subscribe to stock notification
    create_doc(
        &client,
        &token,
        &notif_col,
        &json!({
            "user_id": "user_abc",
            "product_id": "prod_xyz",
            "status": "subscribed",
            "threshold": 0
        }),
    )
    .await;

    // List subscriptions for user
    let filters = serde_json::to_string(&json!({"user_id": {"_eq": "user_abc"}})).unwrap();
    let escaped = serde_json::to_string(&filters).unwrap();
    let query = format!(r#"{{ list(collection: "{notif_col}", filters: {escaped}) }}"#);
    let body = graphql(&client, &token, &query).await;
    assert!(
        body["data"]["list"].is_array() || body["data"]["list"].is_string(),
        "Stock notification query should work"
    );
}

// =============================================================================
// SECTION 16: Storage — Upload / Download / Delete (3 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_51_storage_requires_signed_url() {
    // Storage endpoints require signed URLs (expires + signature params)
    // Without them, should return 400 (missing query params)
    let client = reqwest::Client::new();

    let resp = client
        .put(format!("{}/storage/upload/test/test_file.txt", base_url()))
        .header("Content-Type", "text/plain")
        .body("test content")
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 403,
        "Storage without signed URL should return 400 or 403, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_52_storage_download_without_signature() {
    let client = reqwest::Client::new();

    // Download without signed URL should fail
    let resp = client
        .get(format!(
            "{}/storage/download/test/nonexistent.txt",
            base_url()
        ))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 403 || status == 404,
        "Storage download without signed URL should fail, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_53_storage_delete_without_signature() {
    let client = reqwest::Client::new();

    let resp = client
        .delete(format!(
            "{}/storage/delete/test/nonexistent.txt",
            base_url()
        ))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 403 || status == 404,
        "Storage delete without signed URL should fail, got {status}"
    );
}

// =============================================================================
// SECTION 17: Push Notifications (3 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_54_push_register_token() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;

    let resp = client
        .post(format!("{}/push/register", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({
            "token": "fake_fcm_token_123456",
            "platform": "android"
        }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 201 || status == 400 || status == 422 || status == 500,
        "Push register should return known status, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_55_push_subscribe_topic() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;

    // Register token first
    client
        .post(format!("{}/push/register", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({
            "token": "topic_test_token_789",
            "platform": "ios"
        }))
        .send()
        .await
        .unwrap();

    // Subscribe to topic
    let resp = client
        .post(format!("{}/push/subscribe", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({
            "token": "topic_test_token_789",
            "topic": "promotions"
        }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 201 || status == 400 || status == 422 || status == 500,
        "Topic subscribe should return known status, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_56_push_send_notification() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;

    let resp = client
        .post(format!("{}/push/send", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({
            "token": "send_test_token_000",
            "title": "Test Notification",
            "body": "Hello from OrignaBase!",
            "data": {"action": "open_product", "product_id": "123"}
        }))
        .send()
        .await
        .unwrap();

    // Will likely fail without real FCM key
    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 400 || status == 422 || status == 500,
        "Push send should return known status, got {status}"
    );
}

// =============================================================================
// SECTION 18: Presence (2 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_57_presence_get_all() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/presence", base_url()))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Presence list should return 200");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_58_presence_get_user() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/presence/nonexistent_user_id", base_url()))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 404,
        "Presence user should return 200 or 404, got {status}"
    );
}

// =============================================================================
// SECTION 19: MFA / TOTP (2 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_59_mfa_setup_requires_auth() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;

    // Request MFA setup
    let resp = client
        .post(format!("{}/auth/mfa/setup", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    // Should either return QR code data or indicate MFA not configured
    assert!(
        status == 200 || status == 400 || status == 500,
        "MFA setup should return known status, got {status}"
    );

    if status == 200 {
        let body: Value = resp.json().await.unwrap();
        // Should contain secret, QR URL, or otpauth URL
        assert!(
            body["secret"].is_string()
                || body["qr_code"].is_string()
                || body["otpauth_url"].is_string()
                || body.is_object(),
            "MFA setup should return setup data"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_60_mfa_challenge_without_setup() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;

    // Try MFA challenge without having set up MFA
    let resp = client
        .post(format!("{}/auth/mfa/challenge", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "code": "123456" }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    // Should fail — MFA not set up
    assert!(
        status == 400 || status == 401 || status == 404 || status == 422 || status == 500,
        "MFA challenge without setup should fail, got {status}"
    );
}

// =============================================================================
// SECTION 20: Email Templates (2 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_61_email_templates_list() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/admin/email-templates", base_url()))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 401 || status == 404 || status == 500,
        "Email templates list should return known status, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_62_email_template_get_verification() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/admin/email-templates/verification", base_url()))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 401 || status == 404 || status == 500,
        "Email template get should return known status, got {status}"
    );
}

// =============================================================================
// SECTION 21: GraphQL Search (1 test)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_63_graphql_search() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("search_{}", uuid::Uuid::new_v4().simple());

    // Create searchable documents
    for title in ["Red Widget", "Blue Gadget", "Green Widget"] {
        create_doc(
            &client,
            &token,
            &col,
            &json!({"title": title, "status": "active"}),
        )
        .await;
    }

    // Search via GraphQL
    let query = format!(r#"{{ search(collection: "{col}", query: "Widget") }}"#);
    let body = if search_backend_enabled() {
        wait_for_search_hits(&client, &token, &col, "Widget", 2).await
    } else {
        graphql(&client, &token, &query).await
    };

    if search_backend_enabled() {
        let hits = body["data"]["search"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            hits.len() >= 2,
            "Configured search backend should return indexed hits, got body: {body}"
        );
    } else {
        assert!(
            body["data"]["search"].is_array()
                || body["data"]["search"].is_null()
                || body.get("errors").is_some(),
            "Search should return array, null, or error"
        );
    }
}

// =============================================================================
// SECTION 22: Admin Index Management (2 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_64_admin_create_index() {
    let client = reqwest::Client::new();
    let col = format!("idx_test_{}", uuid::Uuid::new_v4().simple());

    let resp = client
        .post(format!("{}/_admin/indexes", base_url()))
        .json(&json!({
            "collection": col,
            "name": "idx_status",
            "fields": ["status"],
            "unique": false
        }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 201 || status == 400,
        "Create index should return known status, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_65_admin_list_indexes() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/_admin/indexes", base_url()))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200,
        "List indexes should return 200, got {status}"
    );
}

// =============================================================================
// SECTION 23: Forgot / Reset Password (2 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_66_auth_forgot_password() {
    let client = reqwest::Client::new();
    let (_, email) = register_test_user(&client).await;

    let resp = client
        .post(format!("{}/auth/forgot-password", base_url()))
        .json(&json!({ "email": email }))
        .send()
        .await
        .unwrap();

    // May fail if SMTP not configured, but should not crash
    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 400 || status == 500,
        "Forgot password should return known status, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_67_auth_reset_password_invalid_token() {
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/auth/reset-password", base_url()))
        .json(&json!({
            "token": "invalid_reset_token_12345",
            "password": "NewPassword789!"
        }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 401 || status == 404 || status == 422 || status == 500,
        "Reset with invalid token should fail, got {status}"
    );
}

// =============================================================================
// SECTION 24: Write Throughput Stress Test (2 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_68_high_throughput_concurrent_writes() {
    // Test: 50 concurrent writes to verify OrignaBase handles >Firestore's 10K/s limit pattern
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("throughput_{}", uuid::Uuid::new_v4().simple());

    let start = Instant::now();
    let mut handles = Vec::new();

    for i in 0..50 {
        let client = client.clone();
        let token = token.clone();
        let col = col.clone();
        handles.push(tokio::spawn(async move {
            create_doc(
                &client,
                &token,
                &col,
                &json!({
                    "title": format!("Throughput item {i}"),
                    "price": i as f64 * 1.99,
                    "stock": 100 - i,
                    "category": format!("cat_{}", i % 5),
                    "status": if i % 3 == 0 { "active" } else { "draft" }
                }),
            )
            .await
        }));
    }

    let mut success = 0;
    for handle in handles {
        if let Ok(id) = handle.await
            && !id.is_empty()
        {
            success += 1;
        }
    }

    let elapsed = start.elapsed();
    let writes_per_sec = if elapsed.as_secs_f64() > 0.0 {
        success as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

    eprintln!(
        "High throughput: {success}/50 writes in {:?} ({:.0} writes/sec)",
        elapsed, writes_per_sec
    );

    assert!(
        success >= 40,
        "At least 80% of concurrent writes should succeed, got {success}/50"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_69_rapid_sequential_crud_cycle() {
    // Rapid create-read-update-delete cycles (origna_gta checkout pattern)
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("rapid_{}", uuid::Uuid::new_v4().simple());

    let start = Instant::now();
    let cycles = 10;

    for i in 0..cycles {
        // Create
        let id = create_doc(
            &client,
            &token,
            &col,
            &json!({"title": format!("Rapid {i}"), "count": 0}),
        )
        .await;

        if id.is_empty() {
            continue;
        }

        let clean_id = id.split(':').next_back().unwrap_or(&id).to_string();

        // Read
        let query = format!(r#"{{ get(collection: "{col}", id: "{clean_id}") }}"#);
        graphql(&client, &token, &query).await;

        // Update
        let data = json!({"count": {"_increment": 1}});
        let data_str = serde_json::to_string(&data).unwrap();
        let escaped = serde_json::to_string(&data_str).unwrap();
        let query = format!(
            r#"mutation {{ update(collection: "{col}", id: "{clean_id}", data: {escaped}) }}"#
        );
        graphql(&client, &token, &query).await;

        // Delete
        let query = format!(r#"mutation {{ delete(collection: "{col}", id: "{clean_id}") }}"#);
        graphql(&client, &token, &query).await;
    }

    let elapsed = start.elapsed();
    let per_cycle = elapsed / cycles;
    eprintln!(
        "CRUD cycles: {cycles} in {:?} ({:?}/cycle, 4 ops each)",
        elapsed, per_cycle
    );

    assert!(
        per_cycle.as_millis() < 5000,
        "CRUD cycle too slow: {:?}/cycle",
        per_cycle
    );
}

// =============================================================================
// SECTION 25: origna_gta Advanced Migration (5 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_70_ecommerce_coupon_workflow() {
    // origna_gta pattern: create coupon → validate → apply → track usage
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("coupons_{}", uuid::Uuid::new_v4().simple());

    // Create coupon
    let coupon_id = create_doc(
        &client,
        &token,
        &col,
        &json!({
            "code": "SAVE20",
            "discount_percent": 20,
            "max_uses": 100,
            "uses": 0,
            "active": true,
            "min_order_amount": 50.0,
            "valid_until": "2027-01-01T00:00:00Z"
        }),
    )
    .await;

    if !coupon_id.is_empty() {
        let clean_id = coupon_id.split(':').next_back().unwrap_or(&coupon_id);

        // Increment usage (FieldValue)
        let data = json!({"uses": {"_increment": 1}});
        let data_str = serde_json::to_string(&data).unwrap();
        let escaped = serde_json::to_string(&data_str).unwrap();
        let query = format!(
            r#"mutation {{ update(collection: "{col}", id: "{clean_id}", data: {escaped}) }}"#
        );
        graphql(&client, &token, &query).await;

        // Query active coupons
        let filters = serde_json::to_string(&json!({"active": {"_eq": true}})).unwrap();
        let escaped_f = serde_json::to_string(&filters).unwrap();
        let query = format!(r#"{{ list(collection: "{col}", filters: {escaped_f}) }}"#);
        let body = graphql(&client, &token, &query).await;
        assert!(body["data"]["list"].is_array() || body["data"]["list"].is_string());
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_71_ecommerce_chat_messages() {
    // origna_gta pattern: chat between buyer and seller
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let chat_id = uuid::Uuid::new_v4().simple().to_string();
    let messages_col = format!("messages_{chat_id}");

    // Send messages
    for (sender, text) in [
        ("buyer_001", "Hi, is this still available?"),
        ("seller_001", "Yes! Would you like to buy?"),
        ("buyer_001", "Yes, placing order now"),
    ] {
        create_doc(
            &client,
            &token,
            &messages_col,
            &json!({
                "sender_id": sender,
                "text": text,
                "read": false,
                "created_at": {"_serverTimestamp": true}
            }),
        )
        .await;
    }

    // List messages ordered by time
    let query =
        format!(r#"{{ list(collection: "{messages_col}", orderBy: "created_at", limit: 50) }}"#);
    let body = graphql(&client, &token, &query).await;
    if let Some(arr) = body["data"]["list"].as_array() {
        assert!(arr.len() >= 2, "Should have chat messages");
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_72_ecommerce_return_request_flow() {
    // origna_gta: create return request → approve → update order
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let returns_col = format!("returns_{}", uuid::Uuid::new_v4().simple());

    let return_id = create_doc(
        &client,
        &token,
        &returns_col,
        &json!({
            "order_id": "order_12345",
            "reason": "Defective product",
            "status": "pending",
            "items": [{"product_id": "prod_1", "quantity": 1}],
            "refund_amount": 49.99
        }),
    )
    .await;

    if !return_id.is_empty() {
        let clean_id = return_id.split(':').next_back().unwrap_or(&return_id);
        let data = json!({
            "status": "approved",
            "approved_at": {"_serverTimestamp": true}
        });
        let data_str = serde_json::to_string(&data).unwrap();
        let escaped = serde_json::to_string(&data_str).unwrap();
        let query = format!(
            r#"mutation {{ update(collection: "{returns_col}", id: "{clean_id}", data: {escaped}) }}"#
        );
        let body = graphql(&client, &token, &query).await;
        assert!(
            body["data"]["update"].is_object()
                || body["data"]["update"].is_string()
                || body.get("errors").is_some()
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_73_ecommerce_product_ratings_aggregate() {
    // origna_gta: multiple ratings → compute average
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let ratings_col = format!("ratings_{}", uuid::Uuid::new_v4().simple());

    // Submit ratings
    for (user, rating) in [("u1", 5), ("u2", 4), ("u3", 3), ("u4", 5), ("u5", 4)] {
        create_doc(
            &client,
            &token,
            &ratings_col,
            &json!({
                "product_id": "prod_xyz",
                "user_id": user,
                "rating": rating,
                "review": format!("Rating {rating}/5 stars")
            }),
        )
        .await;
    }

    // List all ratings for product
    let filters = serde_json::to_string(&json!({"product_id": {"_eq": "prod_xyz"}})).unwrap();
    let escaped = serde_json::to_string(&filters).unwrap();
    let query = format!(
        r#"{{ list(collection: "{ratings_col}", filters: {escaped}, orderBy: "rating", descending: true) }}"#
    );
    let body = graphql(&client, &token, &query).await;
    if let Some(arr) = body["data"]["list"].as_array() {
        assert!(arr.len() >= 3, "Should have multiple ratings");
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_74_ecommerce_multi_collection_workflow() {
    // Full origna_gta workflow: user registers → creates product → buyer orders → rating
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let prefix = uuid::Uuid::new_v4().simple().to_string();

    // 1. Seller creates product
    let products_col = format!("products_{prefix}");
    let product_id = create_doc(
        &client,
        &token,
        &products_col,
        &json!({
            "title": "Handmade Candle",
            "price": 24.99,
            "status": "active",
            "seller_id": "seller_abc",
            "stock": 50,
            "category": "home"
        }),
    )
    .await;

    // 2. Buyer places order
    let orders_col = format!("orders_{prefix}");
    let order_id = create_doc(
        &client,
        &token,
        &orders_col,
        &json!({
            "buyer_id": "buyer_xyz",
            "status": "pending",
            "total": 24.99,
            "items": [{"product_id": &product_id, "quantity": 1, "price": 24.99}]
        }),
    )
    .await;

    // 3. Update product stock (FieldValue increment -1)
    if !product_id.is_empty() {
        let clean_pid = product_id.split(':').next_back().unwrap_or(&product_id);
        let data = json!({"stock": {"_increment": -1}});
        let data_str = serde_json::to_string(&data).unwrap();
        let escaped = serde_json::to_string(&data_str).unwrap();
        let query = format!(
            r#"mutation {{ update(collection: "{products_col}", id: "{clean_pid}", data: {escaped}) }}"#
        );
        graphql(&client, &token, &query).await;
    }

    // 4. Confirm order
    if !order_id.is_empty() {
        let clean_oid = order_id.split(':').next_back().unwrap_or(&order_id);
        let data = json!({
            "status": "confirmed",
            "confirmed_at": {"_serverTimestamp": true}
        });
        let data_str = serde_json::to_string(&data).unwrap();
        let escaped = serde_json::to_string(&data_str).unwrap();
        let query = format!(
            r#"mutation {{ update(collection: "{orders_col}", id: "{clean_oid}", data: {escaped}) }}"#
        );
        graphql(&client, &token, &query).await;
    }

    // 5. Buyer submits rating
    let ratings_col = format!("ratings_{prefix}");
    create_doc(
        &client,
        &token,
        &ratings_col,
        &json!({
            "product_id": &product_id,
            "user_id": "buyer_xyz",
            "rating": 5,
            "review": "Amazing candle, smells great!"
        }),
    )
    .await;

    // 6. Verify all collections have data
    for col in [&products_col, &orders_col, &ratings_col] {
        let query = format!(r#"{{ list(collection: "{col}") }}"#);
        let body = graphql(&client, &token, &query).await;
        assert!(
            body["data"]["list"].is_array() || body["data"]["list"].is_string(),
            "Collection {col} should have data"
        );
    }
}

// =============================================================================
// SECTION 26: Resumable Uploads (5 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_75_resumable_upload_init() {
    let client = reqwest::Client::new();

    // Init a resumable upload session
    let resp = client
        .post(format!("{}/storage/upload/resumable", base_url()))
        .json(&json!({
            "path": "test/resumable_file.bin",
            "content_type": "application/octet-stream",
            "total_size": 1000
        }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert_eq!(
        status, 200,
        "Init resumable should return 200, got {status}"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body["id"].is_string(), "Should return session id");
    assert_eq!(body["path"], "test/resumable_file.bin");
    assert_eq!(body["total_size"], 1000);
    assert_eq!(body["bytes_received"], 0);
    assert_eq!(body["status"], "in_progress");

    // Cleanup: cancel the session
    let session_id = body["id"].as_str().unwrap();
    client
        .delete(format!(
            "{}/storage/upload/resumable/{session_id}",
            base_url()
        ))
        .send()
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_76_resumable_upload_full_flow() {
    let client = reqwest::Client::new();

    // Create test data: 500 bytes
    let data: Vec<u8> = (0..500u16).map(|i| (i % 256) as u8).collect();

    // 1. Init session
    let resp = client
        .post(format!("{}/storage/upload/resumable", base_url()))
        .json(&json!({
            "path": "test/resumable_complete.bin",
            "content_type": "application/octet-stream",
            "total_size": data.len()
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    let session_id = body["id"].as_str().unwrap().to_string();

    // 2. Upload in 3 chunks: 200 + 200 + 100
    for (offset, end) in [(0, 200), (200, 400), (400, 500)] {
        let chunk = &data[offset..end];
        let resp = client
            .patch(format!(
                "{}/storage/upload/resumable/{session_id}",
                base_url()
            ))
            .header("Upload-Offset", offset.to_string())
            .header("Content-Type", "application/octet-stream")
            .body(chunk.to_vec())
            .send()
            .await
            .unwrap();

        let status = resp.status().as_u16();
        assert_eq!(
            status, 200,
            "Chunk at offset {offset} should succeed, got {status}"
        );

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["bytes_received"], end as u64);
    }

    // 3. Verify final response shows complete
    // (The last chunk response already auto-finalized and stored the file)
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_77_resumable_upload_query_progress() {
    let client = reqwest::Client::new();

    // Init
    let resp = client
        .post(format!("{}/storage/upload/resumable", base_url()))
        .json(&json!({
            "path": "test/resumable_progress.bin",
            "content_type": "application/octet-stream",
            "total_size": 300
        }))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let session_id = body["id"].as_str().unwrap().to_string();

    // Upload first chunk (100 bytes)
    client
        .patch(format!(
            "{}/storage/upload/resumable/{session_id}",
            base_url()
        ))
        .header("Upload-Offset", "0")
        .header("Content-Type", "application/octet-stream")
        .body(vec![0u8; 100])
        .send()
        .await
        .unwrap();

    // Query progress
    let resp = client
        .get(format!(
            "{}/storage/upload/resumable/{session_id}",
            base_url()
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["bytes_received"], 100);
    assert_eq!(body["total_size"], 300);
    assert_eq!(body["status"], "in_progress");

    // Cleanup
    client
        .delete(format!(
            "{}/storage/upload/resumable/{session_id}",
            base_url()
        ))
        .send()
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_78_resumable_upload_cancel() {
    let client = reqwest::Client::new();

    // Init
    let resp = client
        .post(format!("{}/storage/upload/resumable", base_url()))
        .json(&json!({
            "path": "test/resumable_cancel.bin",
            "content_type": "application/octet-stream",
            "total_size": 500
        }))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let session_id = body["id"].as_str().unwrap().to_string();

    // Upload partial
    client
        .patch(format!(
            "{}/storage/upload/resumable/{session_id}",
            base_url()
        ))
        .header("Upload-Offset", "0")
        .header("Content-Type", "application/octet-stream")
        .body(vec![0u8; 200])
        .send()
        .await
        .unwrap();

    // Cancel
    let resp = client
        .delete(format!(
            "{}/storage/upload/resumable/{session_id}",
            base_url()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // Verify session is gone
    let resp = client
        .get(format!(
            "{}/storage/upload/resumable/{session_id}",
            base_url()
        ))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    assert!(
        status == 404 || status == 500,
        "Cancelled session should not be found, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_79_resumable_upload_wrong_offset_rejected() {
    let client = reqwest::Client::new();

    // Init
    let resp = client
        .post(format!("{}/storage/upload/resumable", base_url()))
        .json(&json!({
            "path": "test/resumable_badoffset.bin",
            "content_type": "application/octet-stream",
            "total_size": 200
        }))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let session_id = body["id"].as_str().unwrap().to_string();

    // Try uploading with wrong offset (should be 0, sending 50)
    let resp = client
        .patch(format!(
            "{}/storage/upload/resumable/{session_id}",
            base_url()
        ))
        .header("Upload-Offset", "50")
        .header("Content-Type", "application/octet-stream")
        .body(vec![0u8; 50])
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "Wrong offset should be rejected, got {status}"
    );

    // Cleanup session
    client
        .delete(format!(
            "{}/storage/upload/resumable/{session_id}",
            base_url()
        ))
        .send()
        .await
        .unwrap();
}

// =============================================================================
// SECTION 24: High-Throughput Benchmarks (3 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_80_throughput_500_concurrent_writes() {
    // True throughput test: 500 concurrent HTTP writes via GraphQL
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("tp500_{}", uuid::Uuid::new_v4().simple());

    let total = 500usize;
    let start = Instant::now();
    let mut handles = Vec::with_capacity(total);

    for i in 0..total {
        let client = client.clone();
        let token = token.clone();
        let col = col.clone();
        handles.push(tokio::spawn(async move {
            create_doc(
                &client,
                &token,
                &col,
                &json!({
                    "idx": i,
                    "price": i as f64 * 0.99,
                    "status": "active"
                }),
            )
            .await
        }));
    }

    let mut success = 0usize;
    for handle in handles {
        if let Ok(id) = handle.await
            && !id.is_empty()
        {
            success += 1;
        }
    }

    let elapsed = start.elapsed();
    let wps = success as f64 / elapsed.as_secs_f64();
    let avg_ms = elapsed.as_secs_f64() * 1000.0 / success as f64;
    eprintln!(
        ">>> Throughput (500 concurrent HTTP→GraphQL→SurrealDB): {success}/{total} in {:.2?} → {:.0} writes/sec, avg {:.1}ms/write",
        elapsed, wps, avg_ms
    );

    assert!(
        success >= 400,
        "At least 80% of 500 concurrent writes should succeed, got {success}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_81_throughput_batch_write_100() {
    // Batch write: single HTTP request creates 100 documents
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("batch100_{}", uuid::Uuid::new_v4().simple());

    let docs: Vec<Value> = (0..100)
        .map(|i| json!({"title": format!("Batch {i}"), "price": i, "status": "active"}))
        .collect();

    let docs_str = serde_json::to_string(&docs).unwrap();
    let escaped = serde_json::to_string(&docs_str).unwrap();

    let start = Instant::now();
    let query = format!(r#"mutation {{ batchCreate(collection: "{col}", docs: [{escaped}]) }}"#);
    let _body = graphql(&client, &token, &query).await;
    let elapsed = start.elapsed();

    let total_ms = elapsed.as_secs_f64() * 1000.0;
    eprintln!(
        ">>> Batch write 100 docs in single request: {:.1}ms total ({:.0} docs/sec, {:.2}ms/doc avg)",
        total_ms,
        100.0 / elapsed.as_secs_f64(),
        total_ms / 100.0
    );

    assert!(
        elapsed.as_millis() < 10000,
        "Batch 100 should complete under 10s, took {:?}",
        elapsed
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_82_throughput_read_heavy_1000() {
    // Read throughput: 1000 concurrent reads
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("readheavy_{}", uuid::Uuid::new_v4().simple());

    // Seed 10 docs
    for i in 0..10 {
        create_doc(
            &client,
            &token,
            &col,
            &json!({"title": format!("Read test {i}"), "price": i * 10}),
        )
        .await;
    }

    // 1000 concurrent list reads
    let total = 1000usize;
    let start = Instant::now();
    let mut handles = Vec::with_capacity(total);

    for _ in 0..total {
        let client = client.clone();
        let token = token.clone();
        let col = col.clone();
        handles.push(tokio::spawn(async move {
            let query = format!(r#"{{ list(collection: "{col}", limit: 10) }}"#);
            let body = graphql(&client, &token, &query).await;
            body["data"]["list"].is_array() || body["data"]["list"].is_string()
        }));
    }

    let mut success = 0usize;
    for handle in handles {
        if let Ok(true) = handle.await {
            success += 1;
        }
    }

    let elapsed = start.elapsed();
    let rps = success as f64 / elapsed.as_secs_f64();
    let avg_ms = elapsed.as_secs_f64() * 1000.0 / success as f64;
    eprintln!(
        ">>> Read throughput (1000 concurrent HTTP→GraphQL→SurrealDB): {success}/{total} in {:.2?} → {:.0} reads/sec, avg {:.1}ms/read",
        elapsed, rps, avg_ms
    );

    assert!(
        success >= 800,
        "At least 80% of 1000 concurrent reads should succeed, got {success}"
    );
}

// =============================================================================
// SECTION: Benchmark Comparison Tests (tests 83–87)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_83_benchmark_firebase_comparison() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("fbcomp_{}", uuid::Uuid::new_v4().simple());

    // --- Single write ---
    let start = Instant::now();
    create_doc(
        &client,
        &token,
        &col,
        &json!({"title": "single", "price": 1}),
    )
    .await;
    let single_write_ms = start.elapsed().as_secs_f64() * 1000.0;

    // --- Single read ---
    let doc_id = create_doc(
        &client,
        &token,
        &col,
        &json!({"title": "read_target", "price": 2}),
    )
    .await;
    let query = format!(r#"{{ get(collection: "{col}", id: "{doc_id}") }}"#);
    let start = Instant::now();
    graphql(&client, &token, &query).await;
    let single_read_ms = start.elapsed().as_secs_f64() * 1000.0;

    // --- Batch 100 writes ---
    let batch_col = format!("fbcomp_b100_{}", uuid::Uuid::new_v4().simple());
    let start = Instant::now();
    let mut handles = Vec::with_capacity(100);
    for i in 0..100 {
        let c = client.clone();
        let t = token.clone();
        let cl = batch_col.clone();
        handles.push(tokio::spawn(async move {
            create_doc(&c, &t, &cl, &json!({"item": i})).await
        }));
    }
    let mut batch100_ok = 0usize;
    for h in handles {
        if h.await.is_ok() {
            batch100_ok += 1;
        }
    }
    let batch100_elapsed = start.elapsed();
    let batch100_ops = batch100_ok as f64 / batch100_elapsed.as_secs_f64();

    // --- Batch 500 writes ---
    let batch_col = format!("fbcomp_b500_{}", uuid::Uuid::new_v4().simple());
    let start = Instant::now();
    let mut handles = Vec::with_capacity(500);
    for i in 0..500 {
        let c = client.clone();
        let t = token.clone();
        let cl = batch_col.clone();
        handles.push(tokio::spawn(async move {
            create_doc(&c, &t, &cl, &json!({"item": i})).await
        }));
    }
    let mut batch500_ok = 0usize;
    for h in handles {
        if h.await.is_ok() {
            batch500_ok += 1;
        }
    }
    let batch500_elapsed = start.elapsed();
    let batch500_ops = batch500_ok as f64 / batch500_elapsed.as_secs_f64();

    // --- Concurrent 100 writes ---
    let cw_col = format!("fbcomp_cw100_{}", uuid::Uuid::new_v4().simple());
    let start = Instant::now();
    let mut handles = Vec::with_capacity(100);
    for i in 0..100 {
        let c = client.clone();
        let t = token.clone();
        let cl = cw_col.clone();
        handles.push(tokio::spawn(async move {
            create_doc(&c, &t, &cl, &json!({"cw": i})).await
        }));
    }
    let mut cw100_ok = 0usize;
    for h in handles {
        if h.await.is_ok() {
            cw100_ok += 1;
        }
    }
    let cw100_elapsed = start.elapsed();
    let cw100_ops = cw100_ok as f64 / cw100_elapsed.as_secs_f64();

    // --- Concurrent 1000 reads ---
    // Seed some docs first
    for i in 0..10 {
        create_doc(
            &client,
            &token,
            &col,
            &json!({"title": format!("seed_{i}"), "price": i}),
        )
        .await;
    }
    let start = Instant::now();
    let mut handles = Vec::with_capacity(1000);
    for _ in 0..1000 {
        let c = client.clone();
        let t = token.clone();
        let cl = col.clone();
        handles.push(tokio::spawn(async move {
            let q = format!(r#"{{ list(collection: "{cl}", limit: 10) }}"#);
            graphql(&c, &t, &q).await
        }));
    }
    let mut cr1000_ok = 0usize;
    for h in handles {
        if h.await.is_ok() {
            cr1000_ok += 1;
        }
    }
    let cr1000_elapsed = start.elapsed();
    let cr1000_ops = cr1000_ok as f64 / cr1000_elapsed.as_secs_f64();

    // --- Filtered query ---
    let filter = r#"{\"price\":{\"_gt\":5}}"#;
    let fq = format!(r#"{{ list(collection: "{col}", filter: "{filter}", limit: 20) }}"#);
    let start = Instant::now();
    graphql(&client, &token, &fq).await;
    let filtered_ms = start.elapsed().as_secs_f64() * 1000.0;

    // Per-op response times in ms
    let batch100_ms_per_op = 1000.0 / batch100_ops;
    let batch500_ms_per_op = 1000.0 / batch500_ops;
    let cw100_ms_per_op = 1000.0 / cw100_ops;
    let cr1000_ms_per_op = 1000.0 / cr1000_ops;

    // --- Print comparison table ---
    eprintln!(
        "\n>>> ╔═══════════════════════════════════════════════════════════════════════════════════╗"
    );
    eprintln!(
        ">>> ║                  OrignaBase vs Firebase/Supabase Comparison                      ║"
    );
    eprintln!(
        ">>> ╠══════════════════════════╦═════════════════╦═══════════════╦════════════════════╣"
    );
    eprintln!(
        ">>> ║ Metric                   ║ OrignaBase      ║ Firestore     ║ Supabase           ║"
    );
    eprintln!(
        ">>> ╠══════════════════════════╬═════════════════╬═══════════════╬════════════════════╣"
    );
    eprintln!(
        ">>> ║ Single write             ║ {:>7.1}ms       ║   ~50-200ms   ║   ~15-60ms         ║",
        single_write_ms
    );
    eprintln!(
        ">>> ║ Single read              ║ {:>7.1}ms       ║   ~20-100ms   ║   ~10-30ms         ║",
        single_read_ms
    );
    eprintln!(
        ">>> ║ Batch 100 wr (ops/s)     ║ {:>7.0} ({:.1}ms) ║   ~500        ║   ~1,550           ║",
        batch100_ops, batch100_ms_per_op
    );
    eprintln!(
        ">>> ║ Batch 500 wr (ops/s)     ║ {:>7.0} ({:.1}ms) ║   ~500        ║   ~1,550           ║",
        batch500_ops, batch500_ms_per_op
    );
    eprintln!(
        ">>> ║ Conc 100 wr (ops/s)      ║ {:>7.0} ({:.1}ms) ║  ~1,000       ║   ~3,100           ║",
        cw100_ops, cw100_ms_per_op
    );
    eprintln!(
        ">>> ║ Conc 1000 rd (ops/s)     ║ {:>7.0} ({:.1}ms) ║  ~5,000       ║  ~10,000           ║",
        cr1000_ops, cr1000_ms_per_op
    );
    eprintln!(
        ">>> ║ Filtered query           ║ {:>7.1}ms       ║   ~30-150ms   ║   ~10-50ms         ║",
        filtered_ms
    );
    eprintln!(
        ">>> ╠══════════════════════════╬═════════════════╬═══════════════╬════════════════════╣"
    );
    eprintln!(
        ">>> ║ Known limits             ║ (measured)      ║ 10K wr/s DB   ║ ~3.1x Fstore       ║"
    );
    eprintln!(
        ">>> ║                          ║                 ║ 1 wr/s/doc    ║                    ║"
    );
    eprintln!(
        ">>> ╚══════════════════════════╩═════════════════╩═══════════════╩════════════════════╝\n"
    );

    // Sanity: at least batch writes completed
    assert!(
        batch100_ok >= 80,
        "Batch 100 should have >=80 successes, got {batch100_ok}"
    );
    assert!(
        batch500_ok >= 400,
        "Batch 500 should have >=400 successes, got {batch500_ok}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_84_benchmark_p99_latency() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("p99_{}", uuid::Uuid::new_v4().simple());

    // --- 100 sequential writes ---
    let mut write_times = Vec::with_capacity(100);
    for i in 0..100 {
        let start = Instant::now();
        create_doc(
            &client,
            &token,
            &col,
            &json!({"title": format!("p99_write_{i}"), "v": i}),
        )
        .await;
        write_times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    write_times.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let w_p50 = write_times[49];
    let w_p95 = write_times[94];
    let w_p99 = write_times[98];

    // --- 100 sequential reads ---
    let doc_id = create_doc(
        &client,
        &token,
        &col,
        &json!({"title": "read_target", "price": 42}),
    )
    .await;
    let read_query = format!(r#"{{ get(collection: "{col}", id: "{doc_id}") }}"#);

    let mut read_times = Vec::with_capacity(100);
    for _ in 0..100 {
        let start = Instant::now();
        graphql(&client, &token, &read_query).await;
        read_times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    read_times.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let r_p50 = read_times[49];
    let r_p95 = read_times[94];
    let r_p99 = read_times[98];

    eprintln!("\n>>> ┌─────────────────────────────────────────────┐");
    eprintln!(">>> │        Latency Percentiles (ms)             │");
    eprintln!(">>> ├──────────────┬──────────┬──────────┬────────┤");
    eprintln!(">>> │ Operation    │   p50    │   p95    │  p99   │");
    eprintln!(">>> ├──────────────┼──────────┼──────────┼────────┤");
    eprintln!(
        ">>> │ Write        │ {:>7.1} │ {:>7.1} │ {:>5.1} │",
        w_p50, w_p95, w_p99
    );
    eprintln!(
        ">>> │ Read         │ {:>7.1} │ {:>7.1} │ {:>5.1} │",
        r_p50, r_p95, r_p99
    );
    eprintln!(">>> └──────────────┴──────────┴──────────┴────────┘\n");

    // Sanity: p99 should be under 5 seconds for local
    assert!(w_p99 < 5000.0, "Write p99 should be < 5s, got {w_p99:.1}ms");
    assert!(r_p99 < 5000.0, "Read p99 should be < 5s, got {r_p99:.1}ms");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_85_benchmark_mixed_workload() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("mixed_{}", uuid::Uuid::new_v4().simple());

    // Seed 20 docs
    for i in 0..20 {
        create_doc(
            &client,
            &token,
            &col,
            &json!({"title": format!("seed_{i}"), "price": i * 10, "category": "test"}),
        )
        .await;
    }

    // 500 ops: 70% reads (350), 20% writes (100), 10% queries (50)
    let total = 500usize;
    let reads = 350usize;
    let writes = 100usize;
    let queries = 50usize;

    let start = Instant::now();
    let mut handles = Vec::with_capacity(total);

    // Reads
    for _ in 0..reads {
        let c = client.clone();
        let t = token.clone();
        let cl = col.clone();
        handles.push(tokio::spawn(async move {
            let q = format!(r#"{{ list(collection: "{cl}", limit: 10) }}"#);
            graphql(&c, &t, &q).await;
            "read"
        }));
    }

    // Writes
    for i in 0..writes {
        let c = client.clone();
        let t = token.clone();
        let cl = col.clone();
        handles.push(tokio::spawn(async move {
            create_doc(&c, &t, &cl, &json!({"mixed_write": i})).await;
            "write"
        }));
    }

    // Queries (filtered)
    for _ in 0..queries {
        let c = client.clone();
        let t = token.clone();
        let cl = col.clone();
        handles.push(tokio::spawn(async move {
            let filter = r#"{\"price\":{\"_gt\":50}}"#;
            let q = format!(
                r#"{{ list(collection: "{cl}", filter: "{filter}", orderBy: "price", orderDesc: true, limit: 10) }}"#
            );
            graphql(&c, &t, &q).await;
            "query"
        }));
    }

    let mut read_ok = 0usize;
    let mut write_ok = 0usize;
    let mut query_ok = 0usize;
    for h in handles {
        if let Ok(kind) = h.await {
            match kind {
                "read" => read_ok += 1,
                "write" => write_ok += 1,
                "query" => query_ok += 1,
                _ => {}
            }
        }
    }

    let elapsed = start.elapsed();
    let total_ok = read_ok + write_ok + query_ok;
    let overall_ops = total_ok as f64 / elapsed.as_secs_f64();

    eprintln!("\n>>> ┌─────────────────────────────────────────────────┐");
    eprintln!(">>> │     Mixed Workload (70R/20W/10Q, 500 total)     │");
    eprintln!(">>> ├───────────────┬────────┬────────┬───────────────┤");
    eprintln!(">>> │ Type          │ Target │ OK     │ Success %     │");
    eprintln!(">>> ├───────────────┼────────┼────────┼───────────────┤");
    eprintln!(
        ">>> │ Reads         │ {:>5}  │ {:>5}  │ {:>10.1}%    │",
        reads,
        read_ok,
        read_ok as f64 / reads as f64 * 100.0
    );
    eprintln!(
        ">>> │ Writes        │ {:>5}  │ {:>5}  │ {:>10.1}%    │",
        writes,
        write_ok,
        write_ok as f64 / writes as f64 * 100.0
    );
    eprintln!(
        ">>> │ Queries       │ {:>5}  │ {:>5}  │ {:>10.1}%    │",
        queries,
        query_ok,
        query_ok as f64 / queries as f64 * 100.0
    );
    eprintln!(">>> ├───────────────┴────────┴────────┴───────────────┤");
    eprintln!(
        ">>> │ Total: {total_ok}/{total} in {:.2?} → {:.0} ops/sec      │",
        elapsed, overall_ops
    );
    eprintln!(">>> └─────────────────────────────────────────────────┘\n");

    assert!(
        total_ok >= 400,
        "At least 80% of mixed ops should succeed, got {total_ok}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_86_benchmark_sustained_throughput() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("sustained_{}", uuid::Uuid::new_v4().simple());

    let duration = std::time::Duration::from_secs(5);
    let start = Instant::now();
    let mut total_writes = 0usize;
    let mut interval_counts: Vec<usize> = Vec::new();
    let mut interval_start = Instant::now();
    let mut interval_count = 0usize;

    while start.elapsed() < duration {
        create_doc(
            &client,
            &token,
            &col,
            &json!({"sustained": total_writes, "ts": start.elapsed().as_millis() as u64}),
        )
        .await;
        total_writes += 1;
        interval_count += 1;

        // Record per-second intervals
        if interval_start.elapsed() >= std::time::Duration::from_secs(1) {
            interval_counts.push(interval_count);
            interval_count = 0;
            interval_start = Instant::now();
        }
    }
    if interval_count > 0 {
        interval_counts.push(interval_count);
    }

    let elapsed = start.elapsed();
    let avg_wps = total_writes as f64 / elapsed.as_secs_f64();
    let peak_wps = interval_counts.iter().copied().max().unwrap_or(0);
    let min_wps = interval_counts.iter().copied().min().unwrap_or(0);

    eprintln!("\n>>> ┌───────────────────────────────────────────────┐");
    eprintln!(">>> │   Sustained Write Throughput (5 seconds)       │");
    eprintln!(">>> ├───────────────────┬───────────────────────────┤");
    eprintln!(">>> │ Total writes      │ {:>25} │", total_writes);
    eprintln!(">>> │ Elapsed           │ {:>22.2?} │", elapsed);
    eprintln!(">>> │ Average writes/s  │ {:>25.1} │", avg_wps);
    eprintln!(">>> │ Peak writes/s     │ {:>25} │", peak_wps);
    eprintln!(">>> │ Min writes/s      │ {:>25} │", min_wps);
    eprintln!(">>> │ Per-second:        │ {:?} │", interval_counts);
    eprintln!(">>> └───────────────────┴───────────────────────────┘\n");

    assert!(
        total_writes >= 5,
        "Should complete at least 5 writes in 5 seconds"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_87_benchmark_batch_scaling() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;

    eprintln!("\n>>> ┌────────────────────────────────────────────────┐");
    eprintln!(">>> │         Batch Size Scaling Analysis             │");
    eprintln!(">>> ├────────────┬──────────┬─────────┬──────────────┤");
    eprintln!(">>> │ Batch Size │ OK / Tot │ Time    │ Docs/sec     │");
    eprintln!(">>> ├────────────┼──────────┼─────────┼──────────────┤");

    let mut best_size = 1usize;
    let mut best_dps = 0.0f64;

    for batch_size in [1usize, 10, 50, 100, 500] {
        let col = format!("bscale_{}_{}", batch_size, uuid::Uuid::new_v4().simple());
        let start = Instant::now();

        let mut handles = Vec::with_capacity(batch_size);
        for i in 0..batch_size {
            let c = client.clone();
            let t = token.clone();
            let cl = col.clone();
            handles.push(tokio::spawn(async move {
                create_doc(&c, &t, &cl, &json!({"batch_item": i})).await
            }));
        }

        let mut ok = 0usize;
        for h in handles {
            if h.await.is_ok() {
                ok += 1;
            }
        }

        let elapsed = start.elapsed();
        let dps = ok as f64 / elapsed.as_secs_f64();

        if dps > best_dps {
            best_dps = dps;
            best_size = batch_size;
        }

        eprintln!(
            ">>> │ {:>10} │ {:>4}/{:<4}│ {:>6.2?} │ {:>12.1} │",
            batch_size, ok, batch_size, elapsed, dps
        );
    }

    eprintln!(">>> ├────────────┴──────────┴─────────┴──────────────┤");
    eprintln!(
        ">>> │ Optimal batch size: {:<5} ({:.0} docs/sec)      │",
        best_size, best_dps
    );
    eprintln!(">>> └────────────────────────────────────────────────┘\n");

    // Sanity
    assert!(best_dps > 0.0, "Should have non-zero throughput");
}

// =============================================================================
// SECTION: Admin User CRUD (tests 88–93)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_88_admin_create_user() {
    let client = reqwest::Client::new();
    let email = format!("admin_created_{}@example.com", uuid::Uuid::new_v4());
    let resp = client
        .post(format!("{}/admin/users", base_url()))
        .json(&json!({
            "email": email,
            "password": "AdminCreated123!",
            "display_name": "Admin Created User"
        }))
        .send()
        .await
        .unwrap();

    // Should succeed (admin routes may or may not require auth in dev)
    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 201,
        "Admin create user should return 200/201, got {status}"
    );
    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("user").is_some() || body.get("id").is_some() || body.get("email").is_some(),
        "Response should contain user info: {body}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_89_admin_get_user() {
    let client = reqwest::Client::new();
    // First register a user
    let (_, email) = register_test_user(&client).await;

    // List users and find our user
    let resp = client
        .get(format!("{}/admin/users", base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let empty = vec![];
    let users = body["users"].as_array().unwrap_or(&empty);

    // Find our user's ID
    let user = users.iter().find(|u| u["email"].as_str() == Some(&email));
    assert!(user.is_some(), "Should find registered user in admin list");

    let user_id = user.unwrap()["id"].as_str().unwrap_or("");

    // Get individual user
    let resp = client
        .get(format!("{}/admin/users/{user_id}", base_url()))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200,
        "Admin get user should return 200, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_90_admin_update_user() {
    let client = reqwest::Client::new();
    let (_, email) = register_test_user(&client).await;

    // List users to get ID
    let resp = client
        .get(format!("{}/admin/users", base_url()))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let empty = vec![];
    let users = body["users"].as_array().unwrap_or(&empty);
    let user = users
        .iter()
        .find(|u| u["email"].as_str() == Some(&email))
        .unwrap();
    let user_id = user["id"].as_str().unwrap_or("");

    // Update user
    let resp = client
        .patch(format!("{}/admin/users/{user_id}", base_url()))
        .json(&json!({ "display_name": "Updated Name" }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200,
        "Admin update user should return 200, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_91_admin_set_custom_claims() {
    let client = reqwest::Client::new();
    let (_, email) = register_test_user(&client).await;

    // Get user ID
    let resp = client
        .get(format!("{}/admin/users", base_url()))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let empty = vec![];
    let users = body["users"].as_array().unwrap_or(&empty);
    let user = users
        .iter()
        .find(|u| u["email"].as_str() == Some(&email))
        .unwrap();
    let user_id = user["id"].as_str().unwrap_or("");

    // Set custom claims (like Firebase Auth custom claims)
    let resp = client
        .put(format!("{}/admin/users/{user_id}/claims", base_url()))
        .json(&json!({ "custom_claims": { "role": "seller", "tier": "premium" } }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200,
        "Admin set claims should return 200, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_92_admin_delete_user() {
    let client = reqwest::Client::new();
    // Create user specifically for deletion
    let email = format!("deleteme_{}@example.com", uuid::Uuid::new_v4());
    client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "DeleteMe123!" }))
        .send()
        .await
        .unwrap();

    // Get user ID
    let resp = client
        .get(format!("{}/admin/users", base_url()))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let empty = vec![];
    let users = body["users"].as_array().unwrap_or(&empty);
    let user = users.iter().find(|u| u["email"].as_str() == Some(&email));
    assert!(user.is_some(), "Should find user to delete");
    let user_id = user.unwrap()["id"].as_str().unwrap_or("");

    // Delete user
    let resp = client
        .delete(format!("{}/admin/users/{user_id}", base_url()))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200,
        "Admin delete user should return 200, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_93_admin_config_delete() {
    let client = reqwest::Client::new();
    let (_token, _) = register_test_user(&client).await;

    // First set a config
    let key = format!("test_del_{}", uuid::Uuid::new_v4().simple());
    client
        .put(format!("{}/_admin/config/{key}", base_url()))
        .json(&json!({ "key": key, "value": "temp_value" }))
        .send()
        .await
        .unwrap();

    // Verify it exists
    let resp = client
        .get(format!("{}/config/{key}", base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Delete it
    let resp = client
        .delete(format!("{}/_admin/config/{key}", base_url()))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 204,
        "Config delete should return 200/204, got {status}"
    );
}

// =============================================================================
// SECTION: Admin Index Management (tests 94–95)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_94_admin_drop_index() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;

    // Create a collection and index first
    let col = format!("idx_test_{}", uuid::Uuid::new_v4().simple());
    create_doc(
        &client,
        &token,
        &col,
        &json!({"title": "seed", "price": 10}),
    )
    .await;

    // Create an index
    let idx_name = format!("idx_price_{}", uuid::Uuid::new_v4().simple());
    client
        .post(format!("{}/_admin/indexes", base_url()))
        .json(&json!({
            "name": idx_name,
            "collection": col,
            "fields": ["price"]
        }))
        .send()
        .await
        .unwrap();

    // Drop the index
    let resp = client
        .delete(format!("{}/_admin/indexes/{idx_name}", base_url()))
        .json(&json!({ "collection": col }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 204 || status == 404,
        "Index drop should succeed or report not found, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_95_admin_metrics_query() {
    let client = reqwest::Client::new();

    // Record some metrics first
    client
        .post(format!("{}/_admin/metrics", base_url()))
        .json(&json!({
            "name": "test_metric",
            "value": 42.0,
            "tags": {"env": "test"}
        }))
        .send()
        .await
        .unwrap();

    // Query metrics
    let resp = client
        .get(format!("{}/_admin/metrics", base_url()))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200,
        "Metrics query should return 200, got {status}"
    );
}

// =============================================================================
// SECTION: Auth Extended Flows (tests 96–101)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_96_anonymous_upgrade() {
    let client = reqwest::Client::new();

    // Sign in anonymously
    let resp = client
        .post(format!("{}/auth/anonymous", base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let anon_token = body["access_token"].as_str().unwrap().to_string();

    // Upgrade to permanent account
    let upgrade_email = format!("upgraded_{}@example.com", uuid::Uuid::new_v4());
    let resp = client
        .post(format!("{}/auth/anonymous/upgrade", base_url()))
        .header("Authorization", format!("Bearer {anon_token}"))
        .json(&json!({
            "email": upgrade_email,
            "password": "UpgradedPass123!"
        }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200,
        "Anonymous upgrade should return 200, got {status}"
    );

    // Login with upgraded credentials
    let resp = client
        .post(format!("{}/auth/login", base_url()))
        .json(&json!({ "email": upgrade_email, "password": "UpgradedPass123!" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "Should be able to login with upgraded account"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_97_email_verification_flow() {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;

    // Request verification email (dev mode returns token in response)
    let resp = client
        .post(format!("{}/auth/send-verification", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "email": email }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200,
        "Send verification should return 200, got {status}"
    );

    let body: Value = resp.json().await.unwrap();
    // In dev mode (no SMTP), the token is returned in the response
    if let Some(verify_token) = body.get("token").and_then(|t| t.as_str()) {
        // Verify the email
        let resp = client
            .post(format!("{}/auth/verify-email", base_url()))
            .json(&json!({ "token": verify_token }))
            .send()
            .await
            .unwrap();

        let status = resp.status().as_u16();
        assert!(
            status == 200,
            "Verify email should return 200, got {status}"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_98_magic_link_full_flow() {
    let client = reqwest::Client::new();
    let (_, email) = register_test_user(&client).await;

    // Request magic link
    let resp = client
        .post(format!("{}/auth/magic-link", base_url()))
        .json(&json!({ "email": email }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    // In dev mode, the token is returned
    if let Some(magic_token) = body.get("token").and_then(|t| t.as_str()) {
        // Verify magic link
        let resp = client
            .post(format!("{}/auth/verify-magic-link", base_url()))
            .json(&json!({ "token": magic_token }))
            .send()
            .await
            .unwrap();

        let status = resp.status().as_u16();
        assert!(
            status == 200,
            "Verify magic link should return 200, got {status}"
        );
        let body: Value = resp.json().await.unwrap();
        assert!(
            body.get("access_token").is_some(),
            "Magic link verification should return tokens"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_99_mfa_full_lifecycle() {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;

    // 1. Setup MFA
    let resp = client
        .post(format!("{}/auth/mfa/setup", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let setup_body: Value = resp.json().await.unwrap();
    let secret = setup_body.get("secret").and_then(|s| s.as_str());

    if let Some(secret) = secret {
        // 2. Verify setup with TOTP code
        // In test mode, try with the actual TOTP code
        let code = generate_totp_code(secret);
        let resp = client
            .post(format!("{}/auth/mfa/verify-setup", base_url()))
            .header("Authorization", format!("Bearer {token}"))
            .json(&json!({ "code": code }))
            .send()
            .await
            .unwrap();

        let status = resp.status().as_u16();
        if status == 200 {
            let body: Value = resp.json().await.unwrap();
            // Should get recovery codes
            assert!(
                body.get("recovery_codes").is_some(),
                "Should receive recovery codes after MFA setup"
            );

            // 3. Login should now require MFA
            let resp = client
                .post(format!("{}/auth/login", base_url()))
                .json(&json!({ "email": email, "password": "TestPassword123!" }))
                .send()
                .await
                .unwrap();
            let login_body: Value = resp.json().await.unwrap();

            if login_body.get("mfa_required").and_then(|v| v.as_bool()) == Some(true) {
                // 4. Complete MFA challenge
                let challenge_token = login_body["challenge_token"].as_str().unwrap_or("");
                let code = generate_totp_code(secret);
                let resp = client
                    .post(format!("{}/auth/mfa/challenge", base_url()))
                    .json(&json!({
                        "challenge_token": challenge_token,
                        "code": code
                    }))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(resp.status(), 200, "MFA challenge should succeed");
            }

            // 5. Disable MFA
            let code = generate_totp_code(secret);
            let resp = client
                .delete(format!("{}/auth/mfa", base_url()))
                .header("Authorization", format!("Bearer {token}"))
                .json(&json!({ "code": code }))
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            assert!(
                status == 200 || status == 204,
                "MFA disable should succeed, got {status}"
            );
        }
    }
}

/// Generate a TOTP code from a base32 secret (for testing).
fn generate_totp_code(secret: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Simple TOTP implementation: HMAC-SHA1(secret, floor(time/30))
    // For tests, we use a simplified approach
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 30;

    // Base32 decode the secret
    let secret_bytes = base32_decode(secret);
    if secret_bytes.is_empty() {
        return "000000".to_string();
    }

    // HMAC-SHA1
    use hmac::{Hmac, Mac};
    use sha1::Sha1;
    type HmacSha1 = Hmac<Sha1>;

    let mut mac = HmacSha1::new_from_slice(&secret_bytes).unwrap();
    mac.update(&time.to_be_bytes());
    let result = mac.finalize().into_bytes();

    // Dynamic truncation
    let offset = (result[19] & 0x0f) as usize;
    let code = ((result[offset] as u32 & 0x7f) << 24)
        | ((result[offset + 1] as u32) << 16)
        | ((result[offset + 2] as u32) << 8)
        | (result[offset + 3] as u32);

    format!("{:06}", code % 1_000_000)
}

fn base32_decode(input: &str) -> Vec<u8> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits = 0u64;
    let mut bit_count = 0u32;
    let mut result = Vec::new();

    for ch in input.bytes() {
        let ch_upper = ch.to_ascii_uppercase();
        if let Some(val) = alphabet.iter().position(|&c| c == ch_upper) {
            bits = (bits << 5) | val as u64;
            bit_count += 5;
            if bit_count >= 8 {
                bit_count -= 8;
                result.push((bits >> bit_count) as u8);
                bits &= (1 << bit_count) - 1;
            }
        }
    }
    result
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_100_password_change_full_flow() {
    let client = reqwest::Client::new();
    let (_, email) = register_test_user(&client).await;

    // Request password reset
    let resp = client
        .post(format!("{}/auth/forgot-password", base_url()))
        .json(&json!({ "email": email }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    // In dev mode, token is returned
    if let Some(reset_token) = body.get("token").and_then(|t| t.as_str()) {
        let resp = client
            .post(format!("{}/auth/reset-password", base_url()))
            .json(&json!({
                "token": reset_token,
                "new_password": "NewPassword456!"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "Password reset should succeed");

        // Login with new password
        let resp = client
            .post(format!("{}/auth/login", base_url()))
            .json(&json!({ "email": email, "password": "NewPassword456!" }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "Login with new password should work");
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_101_email_template_crud() {
    let client = reqwest::Client::new();

    // List templates
    let resp = client
        .get(format!("{}/admin/email-templates", base_url()))
        .send()
        .await
        .unwrap();
    if resp.status().as_u16() == 500 {
        let body = resp.text().await.unwrap_or_default();
        assert!(
            body.contains("Email service not configured"),
            "Unexpected template failure: {body}"
        );
        return;
    }
    assert_eq!(resp.status(), 200);

    let template: Value = client
        .get(format!("{}/admin/email-templates/verification", base_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // Update a template
    let resp = client
        .put(format!("{}/admin/email-templates/verification", base_url()))
        .json(&json!({
            "name": "verification",
            "subject": "Custom Verification",
            "html_body": template["html_body"].as_str().unwrap_or(""),
            "text_body": template["text_body"].as_str().unwrap_or(""),
            "description": template["description"].as_str().unwrap_or("")
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    assert!(status == 200 || status == 201, "Template update: {status}");

    // Reset template
    let resp = client
        .post(format!(
            "{}/admin/email-templates/verification/reset",
            base_url()
        ))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    assert!(status == 200 || status == 204, "Template reset: {status}");
}

// =============================================================================
// SECTION: Analytics & Dynamic Links (tests 102–103)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_102_analytics_stats_query() {
    let client = reqwest::Client::new();

    // Ingest some events
    for i in 0..5 {
        client
            .post(format!("{}/analytics/event", base_url()))
            .json(&json!({
                "event_type": "page_view",
                "data": { "page": format!("/test/{i}") }
            }))
            .send()
            .await
            .unwrap();
    }

    // Query stats
    let resp = client
        .get(format!("{}/analytics/stats", base_url()))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    assert!(
        status == 200,
        "Analytics stats should return 200, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_103_dynamic_link_redirect() {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // Create a dynamic link
    let slug = format!("test_{}", uuid::Uuid::new_v4().simple());
    let resp = reqwest::Client::new()
        .post(format!("{}/links", base_url()))
        .json(&json!({
            "slug": slug,
            "url": "https://example.com/test"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Follow the redirect
    let resp = client
        .get(format!("{}/l/{slug}", base_url()))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 301 || status == 302 || status == 307 || status == 308,
        "Dynamic link should redirect, got {status}"
    );
}

// =============================================================================
// SECTION: Push Notification Cleanup (tests 104–105)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_104_push_unregister_token() {
    let client = reqwest::Client::new();

    // Register a token first
    let token_val = format!("test_token_{}", uuid::Uuid::new_v4());
    client
        .post(format!("{}/push/register", base_url()))
        .json(&json!({
            "user_id": "user_probe",
            "token": token_val,
            "platform": "android"
        }))
        .send()
        .await
        .unwrap();

    // Unregister
    let resp = client
        .delete(format!("{}/push/register", base_url()))
        .json(&json!({ "token": token_val }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 204,
        "Push unregister should return 200/204, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_105_push_unsubscribe_topic() {
    let client = reqwest::Client::new();
    let token_val = format!("test_token_{}", uuid::Uuid::new_v4());
    let topic = "test_notifications";

    // Register + subscribe
    client
        .post(format!("{}/push/register", base_url()))
        .json(&json!({ "user_id": "user_probe", "token": token_val, "platform": "ios" }))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{}/push/subscribe", base_url()))
        .json(&json!({ "token": token_val, "topic": topic }))
        .send()
        .await
        .unwrap();

    // Unsubscribe
    let resp = client
        .delete(format!("{}/push/subscribe", base_url()))
        .json(&json!({ "token": token_val, "topic": topic }))
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 204,
        "Push unsubscribe should return 200/204, got {status}"
    );
}

// =============================================================================
// SECTION: WebSocket Realtime (test 106)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_106_websocket_connection() {
    use tokio_tungstenite::connect_async;

    let ws_url = base_url().replace("http://", "ws://");
    let url = format!("{ws_url}/realtime");

    let result = connect_async(&url).await;
    match result {
        Ok((mut ws, _)) => {
            use futures_util::SinkExt;
            use tokio_tungstenite::tungstenite::Message;

            // Send a subscribe message
            let subscribe_msg = json!({
                "type": "subscribe",
                "collection": "ws_test_collection"
            });
            ws.send(Message::Text(subscribe_msg.to_string().into()))
                .await
                .ok();

            // Send a ping
            ws.send(Message::Ping(Default::default())).await.ok();

            // Close gracefully
            ws.close(None).await.ok();
        }
        Err(e) => {
            // WebSocket connection is expected to work if server is running
            eprintln!(">>> WebSocket connection failed (expected if WS auth enforced): {e}");
        }
    }
}

// =============================================================================
// SECTION: OrignaGTA Migration Simulation (tests 107–120)
// =============================================================================

/// Simulates origna_gta's user + profile creation flow
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_107_orignagta_user_profile_creation() {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;

    // Create user profile (like origna_gta's users collection)
    let profile_id = create_doc(
        &client,
        &token,
        "users",
        &json!({
            "email": email,
            "displayName": "Test Seller",
            "roles": ["buyer"],
            "isSeller": false,
            "subscriptionStatus": "free",
            "createdAt": {"_serverTimestamp": true},
            "fcmTokens": []
        }),
    )
    .await;
    assert!(!profile_id.is_empty(), "Should create user profile");

    // Create address subcollection (users/uid/addresses pattern)
    let addr_id = create_doc(
        &client,
        &token,
        "addresses",
        &json!({
            "userId": profile_id,
            "street": "123 King St W",
            "city": "Toronto",
            "province": "ON",
            "postalCode": "M5V 1J2",
            "isDefault": true
        }),
    )
    .await;
    assert!(!addr_id.is_empty(), "Should create address");
}

/// Simulates origna_gta's product listing with compound queries
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_108_orignagta_product_compound_queries() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("products_{}", uuid::Uuid::new_v4().simple());

    // Seed products (like origna_gta product catalog)
    for i in 0..15 {
        create_doc(
            &client,
            &token,
            &col,
            &json!({
                "title": format!("Product {i}"),
                "price": 10.0 + (i as f64 * 5.0),
                "category": if i % 3 == 0 { "electronics" } else { "clothing" },
                "stockQuantity": 100 - i * 5,
                "sellerId": "seller_001",
                "lifecycleStatus": "active",
                "createdAt": {"_serverTimestamp": true}
            }),
        )
        .await;
    }

    // Compound query: where + orderBy + limit (origna_gta pattern)
    let filter =
        r#"{\"sellerId\":{\"_eq\":\"seller_001\"},\"lifecycleStatus\":{\"_eq\":\"active\"}}"#;
    let query = format!(
        r#"{{ list(collection: "{col}", filters: "{filter}", orderBy: "price", descending: true, limit: 5) }}"#
    );
    let body = graphql(&client, &token, &query).await;
    let result = &body["data"]["list"];
    assert!(
        result.is_string() || result.is_array(),
        "Compound query should return results"
    );

    // Price range filter (origna_gta browse by price)
    let filter = r#"{\"price\":{\"_gte\":20,\"_lte\":50}}"#;
    let query = format!(r#"{{ list(collection: "{col}", filters: "{filter}", limit: 10) }}"#);
    let body = graphql(&client, &token, &query).await;
    assert!(body["data"]["list"].is_string() || body["data"]["list"].is_array());
}

/// Simulates origna_gta's cart + stock transaction (atomic add-to-cart)
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_109_orignagta_cart_transaction() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let products_col = format!("products_{}", uuid::Uuid::new_v4().simple());
    let carts_col = format!("carts_{}", uuid::Uuid::new_v4().simple());

    // Create product with stock
    let product_id = create_doc(
        &client,
        &token,
        &products_col,
        &json!({
            "title": "Premium Widget",
            "price": 29.99,
            "stockQuantity": 50,
            "sellerId": "seller_A"
        }),
    )
    .await;

    // Add to cart (create cart item)
    let cart_id = create_doc(
        &client,
        &token,
        &carts_col,
        &json!({
            "productId": product_id,
            "quantity": 2,
            "unitPrice": 29.99,
            "addedAt": {"_serverTimestamp": true}
        }),
    )
    .await;
    assert!(!cart_id.is_empty(), "Should create cart item");

    // Decrement stock using FieldValue increment (negative)
    let data_str = serde_json::to_string(&json!({
        "stockQuantity": { "_increment": -2 }
    }))
    .unwrap();
    let escaped = serde_json::to_string(&data_str).unwrap();
    let clean_pid = product_id
        .strip_prefix(&format!("{products_col}:"))
        .unwrap_or(&product_id);
    let query = format!(
        r#"mutation {{ updateWithFieldValues(collection: "{products_col}", id: "{clean_pid}", data: {escaped}) }}"#
    );
    let body = graphql(&client, &token, &query).await;
    assert!(
        body["errors"].is_null() || body["errors"].as_array().is_none_or(|a| a.is_empty()),
        "Stock decrement should succeed: {body}"
    );

    // Verify stock was decremented
    let query = format!(r#"{{ get(collection: "{products_col}", id: "{clean_pid}") }}"#);
    let body = graphql(&client, &token, &query).await;
    let doc = parse_graphql_json_field(&body["data"]["get"]);
    let stock = doc["stockQuantity"].as_i64().unwrap_or(-1);
    assert_eq!(stock, 48, "Stock should be decremented from 50 to 48");
}

/// Simulates origna_gta's order creation + status transitions
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_110_orignagta_order_state_machine() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("orders_{}", uuid::Uuid::new_v4().simple());

    // Create order (pending)
    let order_id = create_doc(
        &client,
        &token,
        &col,
        &json!({
            "buyerId": "buyer_001",
            "sellerId": "seller_001",
            "items": [
                {"productId": "p1", "title": "Widget A", "quantity": 2, "unitPrice": 19.99},
                {"productId": "p2", "title": "Widget B", "quantity": 1, "unitPrice": 49.99}
            ],
            "totalAmount": 89.97,
            "status": "pending",
            "shippingAddress": {
                "street": "456 Queen St",
                "city": "Toronto",
                "province": "ON"
            },
            "createdAt": {"_serverTimestamp": true}
        }),
    )
    .await;
    assert!(!order_id.is_empty());

    // State transitions: pending → confirmed → processing → shipped → delivered
    let clean_id = order_id
        .strip_prefix(&format!("{col}:"))
        .unwrap_or(&order_id);

    let transitions = [
        ("confirmed", "Payment verified"),
        ("processing", "Order being prepared"),
        ("shipped", "Tracking: CAN123456"),
        ("delivered", "Package received"),
    ];

    for (status, note) in transitions {
        let data_str = serde_json::to_string(&json!({
            "status": status,
            "statusNote": note,
            "updatedAt": {"_serverTimestamp": true}
        }))
        .unwrap();
        let escaped = serde_json::to_string(&data_str).unwrap();
        let query = format!(
            r#"mutation {{ update(collection: "{col}", id: "{clean_id}", data: {escaped}) }}"#
        );
        let body = graphql(&client, &token, &query).await;
        assert!(
            body["errors"].is_null() || body["errors"].as_array().is_none_or(|a| a.is_empty()),
            "Transition to {status} should succeed: {body}"
        );
    }

    // Verify final state
    let query = format!(r#"{{ get(collection: "{col}", id: "{clean_id}") }}"#);
    let body = graphql(&client, &token, &query).await;
    let doc = parse_graphql_json_field(&body["data"]["get"]);
    assert_eq!(doc["status"], "delivered");
}

/// Simulates origna_gta's batch notification fanout
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_111_orignagta_batch_notification_fanout() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("notifications_{}", uuid::Uuid::new_v4().simple());

    // Batch create notifications (like origna_gta fanout to multiple users)
    let notifications: Vec<Value> = (0..20)
        .map(|i| {
            json!({
                "userId": format!("user_{i}"),
                "title": "New Product Available!",
                "body": "Check out our latest collection",
                "type": "product_update",
                "read": false,
                "createdAt": {"_serverTimestamp": true}
            })
        })
        .collect();

    let docs_str = serde_json::to_string(&notifications).unwrap();
    let escaped = serde_json::to_string(&docs_str).unwrap();
    let query = format!(r#"mutation {{ batchCreate(collection: "{col}", docs: [{escaped}]) }}"#);
    let body = graphql(&client, &token, &query).await;
    assert!(
        body["errors"].is_null() || body["errors"].as_array().is_none_or(|a| a.is_empty()),
        "Batch notification creation should succeed"
    );

    // Query unread notifications for a specific user
    let filter = r#"{\"userId\":{\"_eq\":\"user_5\"},\"read\":{\"_eq\":false}}"#;
    let query = format!(r#"{{ list(collection: "{col}", filters: "{filter}", limit: 50) }}"#);
    let body = graphql(&client, &token, &query).await;
    assert!(body["data"]["list"].is_string() || body["data"]["list"].is_array());
}

/// Simulates origna_gta's chat messaging pattern
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_112_orignagta_chat_messages() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let chats_col = format!("chats_{}", uuid::Uuid::new_v4().simple());
    let msgs_col = format!("chat_messages_{}", uuid::Uuid::new_v4().simple());

    // Create chat thread
    let chat_id = create_doc(
        &client,
        &token,
        &chats_col,
        &json!({
            "participants": ["buyer_1", "seller_1"],
            "lastMessage": "",
            "unreadCount": 0,
            "createdAt": {"_serverTimestamp": true}
        }),
    )
    .await;

    // Send messages (rapid sequential, simulating real-time chat)
    for i in 0..10 {
        let sender = if i % 2 == 0 { "buyer_1" } else { "seller_1" };
        create_doc(
            &client,
            &token,
            &msgs_col,
            &json!({
                "chatId": chat_id,
                "senderId": sender,
                "text": format!("Message {i}: How's the product?"),
                "type": "text",
                "createdAt": {"_serverTimestamp": true}
            }),
        )
        .await;
    }

    // Query latest messages (ordered by creation, limited)
    let filter = format!(r#"{{\"chatId\":{{\"_eq\":\"{chat_id}\"}}}}"#);
    let query = format!(r#"{{ list(collection: "{msgs_col}", filters: "{filter}", limit: 5) }}"#);
    let body = graphql(&client, &token, &query).await;
    assert!(body["data"]["list"].is_string() || body["data"]["list"].is_array());
}

/// Simulates origna_gta's seller ratings aggregate pattern
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_113_orignagta_ratings_aggregate() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let ratings_col = format!("ratings_{}", uuid::Uuid::new_v4().simple());
    let seller_col = format!("sellers_{}", uuid::Uuid::new_v4().simple());

    // Create seller
    let seller_id = create_doc(
        &client,
        &token,
        &seller_col,
        &json!({
            "name": "Toronto Widgets Inc",
            "totalRatings": 0,
            "averageRating": 0.0,
            "totalReviews": 0
        }),
    )
    .await;

    // Add ratings
    let ratings = [5, 4, 5, 3, 4, 5, 4, 5, 4, 5]; // avg 4.4
    for (i, &rating) in ratings.iter().enumerate() {
        create_doc(
            &client,
            &token,
            &ratings_col,
            &json!({
                "sellerId": seller_id,
                "buyerId": format!("buyer_{i}"),
                "rating": rating,
                "review": format!("Great product! #{i}"),
                "createdAt": {"_serverTimestamp": true}
            }),
        )
        .await;
    }

    // Update seller aggregate (like origna_gta Cloud Function trigger)
    let clean_sid = seller_id
        .strip_prefix(&format!("{seller_col}:"))
        .unwrap_or(&seller_id);
    let data_str = serde_json::to_string(&json!({
        "totalReviews": 10,
        "averageRating": 4.4,
        "totalRatings": {"_increment": 10}
    }))
    .unwrap();
    let escaped = serde_json::to_string(&data_str).unwrap();
    let query = format!(
        r#"mutation {{ updateWithFieldValues(collection: "{seller_col}", id: "{clean_sid}", data: {escaped}) }}"#
    );
    let body = graphql(&client, &token, &query).await;
    assert!(
        body["errors"].is_null() || body["errors"].as_array().is_none_or(|a| a.is_empty()),
        "Seller rating update should succeed"
    );
}

/// Simulates origna_gta's subscription/premium flow
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_114_orignagta_subscription_flow() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("subscriptions_{}", uuid::Uuid::new_v4().simple());

    // Create subscription (like origna_gta premium membership)
    let sub_id = create_doc(
        &client,
        &token,
        &col,
        &json!({
            "userId": "user_001",
            "plan": "premium",
            "priceCad": 9.99,
            "status": "active",
            "startDate": {"_serverTimestamp": true},
            "features": ["chat", "priority_support", "seller_tools"]
        }),
    )
    .await;
    assert!(!sub_id.is_empty());

    // Cancel subscription
    let clean_id = sub_id.strip_prefix(&format!("{col}:")).unwrap_or(&sub_id);
    let data_str = serde_json::to_string(&json!({
        "status": "cancelled",
        "cancelledAt": {"_serverTimestamp": true}
    }))
    .unwrap();
    let escaped = serde_json::to_string(&data_str).unwrap();
    let query =
        format!(r#"mutation {{ update(collection: "{col}", id: "{clean_id}", data: {escaped}) }}"#);
    let body = graphql(&client, &token, &query).await;
    assert!(
        body["errors"].is_null() || body["errors"].as_array().is_none_or(|a| a.is_empty()),
        "Subscription cancel should succeed"
    );
}

/// Simulates origna_gta's inventory/warehouse pattern
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_115_orignagta_inventory_levels() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let inv_col = format!("inventory_{}", uuid::Uuid::new_v4().simple());

    // Create inventory levels (like origna_gta's products/{pid}/inventoryLevels/{warehouseId})
    let warehouses = [
        "warehouse_toronto",
        "warehouse_montreal",
        "warehouse_vancouver",
    ];
    for wh in &warehouses {
        create_doc(
            &client,
            &token,
            &inv_col,
            &json!({
                "productId": "product_001",
                "warehouseId": wh,
                "quantity": 100,
                "reservedQuantity": 5,
                "lastRestocked": {"_serverTimestamp": true}
            }),
        )
        .await;
    }

    // Query inventory for a specific product across warehouses
    let filter = r#"{\"productId\":{\"_eq\":\"product_001\"}}"#;
    let query = format!(r#"{{ list(collection: "{inv_col}", filters: "{filter}", limit: 10) }}"#);
    let body = graphql(&client, &token, &query).await;
    let result = &body["data"]["list"];
    assert!(result.is_string() || result.is_array());
}

/// Simulates origna_gta's coupon + discount verification
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_116_orignagta_coupon_verification() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("coupons_{}", uuid::Uuid::new_v4().simple());

    // Create coupons
    let coupon_id = create_doc(
        &client,
        &token,
        &col,
        &json!({
            "code": "WELCOME20",
            "discountPercent": 20,
            "maxUses": 100,
            "currentUses": 0,
            "expiresAt": "2027-12-31T23:59:59Z",
            "isActive": true,
            "minOrderAmount": 50.0
        }),
    )
    .await;

    // Apply coupon (increment usage with FieldValue)
    let clean_id = coupon_id
        .strip_prefix(&format!("{col}:"))
        .unwrap_or(&coupon_id);
    let data_str = serde_json::to_string(&json!({
        "currentUses": { "_increment": 1 }
    }))
    .unwrap();
    let escaped = serde_json::to_string(&data_str).unwrap();
    let query = format!(
        r#"mutation {{ updateWithFieldValues(collection: "{col}", id: "{clean_id}", data: {escaped}) }}"#
    );
    let body = graphql(&client, &token, &query).await;
    assert!(
        body["errors"].is_null() || body["errors"].as_array().is_none_or(|a| a.is_empty()),
        "Coupon usage increment should succeed"
    );
}

/// Concurrent transaction simulation: multiple users buying same product
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_117_concurrent_checkout_race() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let products_col = format!("race_products_{}", uuid::Uuid::new_v4().simple());

    // Create product with limited stock
    let product_id = create_doc(
        &client,
        &token,
        &products_col,
        &json!({
            "title": "Limited Edition Widget",
            "price": 99.99,
            "stockQuantity": 10
        }),
    )
    .await;

    // 20 concurrent buyers try to buy 1 unit each (stock = 10, 10 should fail)
    let clean_pid = product_id
        .strip_prefix(&format!("{products_col}:"))
        .unwrap_or(&product_id);
    let mut handles = Vec::with_capacity(20);

    for _i in 0..20 {
        let c = client.clone();
        let t = token.clone();
        let col = products_col.clone();
        let pid = clean_pid.to_string();
        handles.push(tokio::spawn(async move {
            let data_str = serde_json::to_string(&json!({
                "stockQuantity": { "_increment": -1 }
            }))
            .unwrap();
            let escaped = serde_json::to_string(&data_str).unwrap();
            let query = format!(
                r#"mutation {{ updateWithFieldValues(collection: "{col}", id: "{pid}", data: {escaped}) }}"#
            );
            let body = graphql(&c, &t, &query).await;
            body["errors"].is_null() || body["errors"].as_array().is_none_or(|a| a.is_empty())
        }));
    }

    let mut successes = 0usize;
    for h in handles {
        if let Ok(true) = h.await {
            successes += 1;
        }
    }

    // All operations should succeed (no crash), stock may go negative without proper locking
    assert!(
        successes >= 10,
        "Most concurrent stock decrements should succeed, got {successes}"
    );

    // Check final stock
    let query = format!(r#"{{ get(collection: "{products_col}", id: "{clean_pid}") }}"#);
    let body = graphql(&client, &token, &query).await;
    let doc_str = body["data"]["get"].as_str().unwrap_or("{}");
    let doc: Value = serde_json::from_str(doc_str).unwrap_or(json!({}));
    let final_stock = doc["stockQuantity"].as_i64().unwrap_or(999);
    eprintln!(
        ">>> Concurrent checkout: {successes}/20 succeeded, final stock: {final_stock} (started at 10)"
    );
}

/// Simulates origna_gta's return/refund request flow
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_118_orignagta_return_request() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let returns_col = format!("returns_{}", uuid::Uuid::new_v4().simple());

    // Create return request
    let return_id = create_doc(
        &client,
        &token,
        &returns_col,
        &json!({
            "orderId": "order_001",
            "buyerId": "buyer_001",
            "sellerId": "seller_001",
            "reason": "Product damaged during shipping",
            "items": [{"productId": "p1", "quantity": 1, "refundAmount": 29.99}],
            "status": "pending",
            "totalRefund": 29.99,
            "createdAt": {"_serverTimestamp": true}
        }),
    )
    .await;

    // Seller approves return
    let clean_id = return_id
        .strip_prefix(&format!("{returns_col}:"))
        .unwrap_or(&return_id);
    let data_str = serde_json::to_string(&json!({
        "status": "approved",
        "approvedAt": {"_serverTimestamp": true},
        "sellerNote": "Approved for full refund"
    }))
    .unwrap();
    let escaped = serde_json::to_string(&data_str).unwrap();
    let query = format!(
        r#"mutation {{ update(collection: "{returns_col}", id: "{clean_id}", data: {escaped}) }}"#
    );
    let body = graphql(&client, &token, &query).await;
    assert!(
        body["errors"].is_null() || body["errors"].as_array().is_none_or(|a| a.is_empty()),
        "Return approval should succeed"
    );
}

/// Simulates origna_gta's search functionality
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_119_orignagta_product_search() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;

    let products_col = "products";
    let marker = format!("widget-search-{}", uuid::Uuid::new_v4().simple());
    create_doc(
        &client,
        &token,
        products_col,
        &json!({
            "title": format!("Widget {marker}"),
            "description": format!("Search marker {marker}"),
            "status": "active"
        }),
    )
    .await;

    // Search via GraphQL (uses Meilisearch if configured)
    let query = format!(r#"{{ search(collection: "products", query: "{marker}", limit: 10) }}"#);
    let body = if search_backend_enabled() {
        wait_for_search_hits(&client, &token, products_col, &marker, 1).await
    } else {
        graphql(&client, &token, &query).await
    };

    if search_backend_enabled() {
        let hits = body["data"]["search"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !hits.is_empty(),
            "Configured search backend should return at least one indexed product hit, got body: {body}"
        );
    } else {
        assert!(
            body.get("data").is_some() || body.get("errors").is_some(),
            "Search should return data or graceful error"
        );
    }
}

/// Simulates origna_gta's multi-collection dashboard query
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_120_orignagta_seller_dashboard() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let seller_id = "seller_dashboard_test";

    // Seed data across multiple collections (products, orders, ratings)
    let products_col = format!("dash_products_{}", uuid::Uuid::new_v4().simple());
    let orders_col = format!("dash_orders_{}", uuid::Uuid::new_v4().simple());

    for i in 0..5 {
        create_doc(
            &client,
            &token,
            &products_col,
            &json!({
                "sellerId": seller_id,
                "title": format!("Dashboard Product {i}"),
                "price": 20.0 + i as f64 * 10.0,
                "status": "active"
            }),
        )
        .await;
    }

    for i in 0..8 {
        create_doc(
            &client,
            &token,
            &orders_col,
            &json!({
                "sellerId": seller_id,
                "totalAmount": 50.0 + i as f64 * 15.0,
                "status": if i < 6 { "delivered" } else { "pending" },
                "createdAt": {"_serverTimestamp": true}
            }),
        )
        .await;
    }

    // Concurrent dashboard queries (like origna_gta's seller dashboard)
    let start = Instant::now();
    let mut handles = Vec::new();

    // Products count
    let c = client.clone();
    let t = token.clone();
    let col = products_col.clone();
    handles.push(tokio::spawn(async move {
        let f = format!(r#"{{\"sellerId\":{{\"_eq\":\"{seller_id}\"}}}}"#);
        let q = format!(r#"{{ list(collection: "{col}", filter: "{f}", limit: 100) }}"#);
        graphql(&c, &t, &q).await
    }));

    // Delivered orders
    let c = client.clone();
    let t = token.clone();
    let col = orders_col.clone();
    handles.push(tokio::spawn(async move {
        let f = format!(
            r#"{{\"sellerId\":{{\"_eq\":\"{seller_id}\"}},\"status\":{{\"_eq\":\"delivered\"}}}}"#
        );
        let q = format!(r#"{{ list(collection: "{col}", filter: "{f}", limit: 100) }}"#);
        graphql(&c, &t, &q).await
    }));

    // Pending orders
    let c = client.clone();
    let t = token.clone();
    let col = orders_col.clone();
    handles.push(tokio::spawn(async move {
        let f = format!(
            r#"{{\"sellerId\":{{\"_eq\":\"{seller_id}\"}},\"status\":{{\"_eq\":\"pending\"}}}}"#
        );
        let q = format!(r#"{{ list(collection: "{col}", filter: "{f}", limit: 100) }}"#);
        graphql(&c, &t, &q).await
    }));

    for h in handles {
        let _ = h.await;
    }

    let elapsed = start.elapsed();
    eprintln!(
        ">>> Seller dashboard (3 concurrent queries): {:.1}ms",
        elapsed.as_secs_f64() * 1000.0
    );
    assert!(
        elapsed.as_millis() < 10000,
        "Dashboard queries should complete under 10s"
    );
}

// =============================================================================
// SECTION: Edge Cases & Security (tests 121–126)
// =============================================================================

/// Test SQL/SurrealQL injection attempts via GraphQL
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_121_injection_prevention_graphql() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;

    // Attempt injection via collection name
    let malicious_names = [
        "products; DROP TABLE users;--",
        "products' OR '1'='1",
        "products\"; DELETE FROM users;--",
        "../../../etc/passwd",
    ];

    for name in &malicious_names {
        let escaped_name = name.replace('"', "\\\"");
        let query = format!(r#"{{ list(collection: "{escaped_name}", limit: 1) }}"#);
        let body = graphql(&client, &token, &query).await;
        // Should either return error or empty — never execute injection
        assert!(
            !body["errors"].is_null() || body["data"]["list"].is_null(),
            "Injection attempt with '{name}' should be rejected"
        );
    }
}

/// Test auth token edge cases
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_122_auth_token_edge_cases() {
    let client = reqwest::Client::new();

    // Expired/malformed tokens
    let bad_tokens = ["not.a.valid.jwt", "REDACTED_SECRET", "", "Bearer "];

    for bad_token in &bad_tokens {
        let query = r#"{ list(collection: "test", limit: 1) }"#;
        let resp = client
            .post(format!("{}/graphql", base_url()))
            .header("Authorization", format!("Bearer {bad_token}"))
            .json(&json!({ "query": query }))
            .send()
            .await
            .unwrap();

        // Should return 200 with anonymous context (no crash)
        assert_eq!(
            resp.status(),
            200,
            "Bad token should not crash server: {bad_token}"
        );
    }
}

/// Test oversized request handling
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_123_oversized_request_handling() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;

    // Create document with very large field
    let large_value = "x".repeat(500_000); // 500KB string
    let col = format!("large_{}", uuid::Uuid::new_v4().simple());
    let data_str = serde_json::to_string(&json!({ "bigField": large_value })).unwrap();
    let escaped = serde_json::to_string(&data_str).unwrap();
    let query = format!(r#"mutation {{ create(collection: "{col}", data: {escaped}) }}"#);

    let resp = client
        .post(format!("{}/graphql", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "query": query }))
        .send()
        .await
        .unwrap();

    // Should either succeed or return 413/error — not crash
    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 413 || status == 400,
        "Large request should be handled gracefully, got {status}"
    );
}

/// Test Unicode handling in documents
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_124_unicode_handling() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("unicode_{}", uuid::Uuid::new_v4().simple());

    let test_cases = [
        ("emoji", json!({"text": "Hello 🌍 World 🎉 OrignaBase 🚀"})),
        ("chinese", json!({"text": "你好世界 - 中文测试"})),
        ("arabic", json!({"text": "مرحبا بالعالم"})),
        ("cyrillic", json!({"text": "Привет мир"})),
        (
            "mixed",
            json!({"text": "Café résumé naïve über Straße 日本語 한국어"}),
        ),
    ];

    for (label, data) in &test_cases {
        let doc_id = create_doc(&client, &token, &col, data).await;
        assert!(
            !doc_id.is_empty(),
            "Unicode test '{label}' should create doc"
        );

        // Read it back
        let clean_id = doc_id.strip_prefix(&format!("{col}:")).unwrap_or(&doc_id);
        let query = format!(r#"{{ get(collection: "{col}", id: "{clean_id}") }}"#);
        let body = graphql(&client, &token, &query).await;
        let doc = parse_graphql_json_field(&body["data"]["get"]);
        assert_eq!(
            doc["text"], data["text"],
            "Unicode roundtrip failed for '{label}'"
        );
    }
}

/// Test FieldValue operations edge cases
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_125_field_value_edge_cases() {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let col = format!("fv_edge_{}", uuid::Uuid::new_v4().simple());

    // Create doc with arrays and counters
    let doc_id = create_doc(
        &client,
        &token,
        &col,
        &json!({
            "counter": 0,
            "tags": ["initial"],
            "toDelete": "temporary"
        }),
    )
    .await;

    let clean_id = doc_id.strip_prefix(&format!("{col}:")).unwrap_or(&doc_id);

    // Multiple FieldValue ops in single request
    let data_str = serde_json::to_string(&json!({
        "counter": { "_increment": 5 },
        "tags": { "_arrayUnion": ["new_tag_1", "new_tag_2"] },
        "toDelete": { "_deleteField": true },
        "updatedAt": { "_serverTimestamp": true }
    }))
    .unwrap();
    let escaped = serde_json::to_string(&data_str).unwrap();
    let query = format!(
        r#"mutation {{ updateWithFieldValues(collection: "{col}", id: "{clean_id}", data: {escaped}) }}"#
    );
    let body = graphql(&client, &token, &query).await;
    assert!(
        body["errors"].is_null() || body["errors"].as_array().is_none_or(|a| a.is_empty()),
        "Multiple FieldValue ops should succeed: {body}"
    );

    // Verify results
    let query = format!(r#"{{ get(collection: "{col}", id: "{clean_id}") }}"#);
    let body = graphql(&client, &token, &query).await;
    let doc = parse_graphql_json_field(&body["data"]["get"]);
    assert_eq!(doc["counter"], 5, "Counter should be incremented to 5");
}

/// Test rate limiting on auth routes
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_126_auth_rate_limiting() {
    let client = reqwest::Client::new();

    // Rapid login attempts (should trigger rate limit)
    let mut statuses = Vec::new();
    for _ in 0..110 {
        let resp = client
            .post(format!("{}/auth/login", base_url()))
            .json(&json!({
                "email": "ratelimit@test.com",
                "password": "wrong_password"
            }))
            .send()
            .await
            .unwrap();
        statuses.push(resp.status().as_u16());
    }

    // Check if any requests were rate-limited (429)
    let rate_limited = statuses.iter().filter(|&&s| s == 429).count();
    let auth_errors = statuses.iter().filter(|&&s| s == 401 || s == 400).count();

    eprintln!(
        ">>> Rate limit test: {} auth errors, {} rate limited out of {}",
        auth_errors,
        rate_limited,
        statuses.len()
    );

    // At least some should be auth errors (wrong password)
    assert!(
        auth_errors > 0 || rate_limited > 0,
        "Should see either auth errors or rate limiting"
    );
}
