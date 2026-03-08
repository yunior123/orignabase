//! Integration tests for OrignaBase.
//!
//! These tests require a running SurrealDB instance at ws://localhost:8000.
//! Run with: `cargo test --test integration_test -- --ignored`
//!
//! To start SurrealDB:
//!   surreal start --user root --pass root memory

use serde_json::json;

/// Helper to build a test HTTP client pointing at a running orignabase instance.
fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_health_endpoint() {
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
async fn test_auth_register_and_login() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", uuid::Uuid::new_v4());

    // Register
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({
            "email": email,
            "password": "TestPassword123!"
        }))
        .send()
        .await
        .expect("register failed");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["access_token"].is_string());
    assert!(body["refresh_token"].is_string());

    // Login with same credentials
    let resp = client
        .post(format!("{}/auth/login", base_url()))
        .json(&json!({
            "email": email,
            "password": "TestPassword123!"
        }))
        .send()
        .await
        .expect("login failed");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["access_token"].is_string());

    // Login with wrong password
    let resp = client
        .post(format!("{}/auth/login", base_url()))
        .json(&json!({
            "email": email,
            "password": "WrongPassword"
        }))
        .send()
        .await
        .expect("bad login request failed");

    assert_ne!(resp.status(), 200);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_refresh_token() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", uuid::Uuid::new_v4());

    // Register to get tokens
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({
            "email": email,
            "password": "TestPassword123!"
        }))
        .send()
        .await
        .unwrap();

    let body: serde_json::Value = resp.json().await.unwrap();
    let refresh_token = body["refresh_token"].as_str().unwrap();

    // Use refresh token
    let resp = client
        .post(format!("{}/auth/refresh", base_url()))
        .json(&json!({ "refresh_token": refresh_token }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["access_token"].is_string());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_graphql_crud() {
    let client = reqwest::Client::new();

    // Register and get token
    let email = format!("test_{}@example.com", uuid::Uuid::new_v4());
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({
            "email": email,
            "password": "TestPassword123!"
        }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["access_token"].as_str().unwrap().to_string();

    // Create a document via GraphQL
    let resp = client
        .post(format!("{}/graphql", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({
            "query": r#"mutation { create(collection: "test_items", data: "{\"title\":\"Hello\",\"price\":42}") }"#
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_admin_health() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/_admin/health", base_url()))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_analytics_event_ingestion() {
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
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["event_id"].is_string());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_functions_list_empty() {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/functions", base_url()))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_array());
}
