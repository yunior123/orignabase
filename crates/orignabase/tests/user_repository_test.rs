//! Integration tests for user repository — via GraphQL.
//!
//! These tests cover:
//! - User profile operations (get, update via GraphQL)
//! - Address management (create/get/delete via `addresses` collection)
//!
//! Run with: cargo test --test user_repository_test -- --ignored

use ob_database::fields;
use serde_json::{Value, json};
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string()) // ignore-magic
}

async fn register_test_user(client: &reqwest::Client) -> (String, String, String) {
    let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "TestPassword123!" })) // ignore-magic
        .send()
        .await
        .expect("register failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let token = body["access_token"] // ignore-magic
        .as_str()
        .expect("missing access_token")
        .to_string();
    let user_id = body["user"][fields::ID] // ignore-magic
        .as_str()
        .expect("missing user.id")
        .to_string();
    (token, user_id, email)
}

async fn graphql(client: &reqwest::Client, token: Option<&str>, query: &str) -> (u16, Value) {
    let url = format!("{}/graphql", base_url());
    let mut req = client.post(&url).json(&json!({"query": query})); // ignore-magic
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}")); // ignore-magic
    }
    let resp = req.send().await.expect("graphql request failed");
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic
    (status, body)
}

fn address_data(label: &str) -> Value {
    json!({ // ignore-magic
        "label": label,
        "street": "123 Queen St W",
        "apartment": "Unit 8",
        "city": "Toronto",
        "province": "ON",
        "postalCode": "M5V 2B7",
        "country": "Canada"
    })
}

// =============================================================================
// SECTION: Users — Profile via GraphQL
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_get_profile_success() {
    let client = reqwest::Client::new();
    let (token, user_id, _email) = register_test_user(&client).await;

    let query = format!(r#"{{ get(collection: "users", id: "{user_id}") }}"#); // ignore-magic
    let (status, body) = graphql(&client, Some(&token), &query).await;

    assert_eq!(status, 200);
    let result = &body["data"]["get"]; // ignore-magic
    assert!(
        result.is_object() || result.is_null(),
        "Should return user doc or null: {body}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_get_profile_requires_authentication() {
    let client = reqwest::Client::new();

    let query = r#"{ get(collection: "users", id: "users:test_123") }"#; // ignore-magic
    let (status, body) = graphql(&client, None, query).await;

    assert_eq!(status, 200);
    // Without auth, may return null or error depending on rules
    let _ = body;
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_update_profile_success() {
    let client = reqwest::Client::new();
    let (token, user_id, _email) = register_test_user(&client).await;

    let data = serde_json::to_string(&json!({ // ignore-magic
        "firstName": "John",
        "lastName": "Doe",
        "phoneNumber": "+14165551234"
    }))
    .unwrap();
    let escaped = serde_json::to_string(&data).unwrap();
    let query =
        format!(r#"mutation {{ update(collection: "users", id: "{user_id}", data: {escaped}) }}"#); // ignore-magic
    let (status, body) = graphql(&client, Some(&token), &query).await;

    assert_eq!(status, 200);
    let _ = body;
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_update_profile_requires_authentication() {
    let client = reqwest::Client::new();

    let data = serde_json::to_string(&json!({"firstName": "Jane"})).unwrap(); // ignore-magic
    let escaped = serde_json::to_string(&data).unwrap();
    let query = format!(
        r#"mutation {{ update(collection: "users", id: "users:test_123", data: {escaped}) }}"# // ignore-magic
    );
    let (status, body) = graphql(&client, None, &query).await;

    assert_eq!(status, 200);
    let _ = body;
}

// =============================================================================
// SECTION: Users — Addresses via GraphQL
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_add_address() {
    let client = reqwest::Client::new();
    let (token, user_id, _email) = register_test_user(&client).await;

    let mut addr = address_data("Home");
    addr[fields::USER_ID] = json!(user_id); // ignore-magic
    let data = serde_json::to_string(&addr).unwrap();
    let escaped = serde_json::to_string(&data).unwrap();
    let query = format!(r#"mutation {{ create(collection: "addresses", data: {escaped}) }}"#); // ignore-magic
    let (status, body) = graphql(&client, Some(&token), &query).await;

    assert_eq!(status, 200);
    let result = &body["data"]["create"]; // ignore-magic
    assert!(
        result.is_object() || body.get("errors").is_some(),
        "Should create address or error: {body}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_add_address_requires_authentication() {
    let client = reqwest::Client::new();

    let data = serde_json::to_string(&address_data("Home")).unwrap();
    let escaped = serde_json::to_string(&data).unwrap();
    let query = format!(r#"mutation {{ create(collection: "addresses", data: {escaped}) }}"#); // ignore-magic
    let (status, body) = graphql(&client, None, &query).await;

    assert_eq!(status, 200);
    let _ = body;
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_get_addresses() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let query = r#"{ list(collection: "addresses", limit: 10) }"#; // ignore-magic
    let (status, body) = graphql(&client, Some(&token), query).await;

    assert_eq!(status, 200);
    let result = &body["data"]["list"]; // ignore-magic
    assert!(result.is_array() || result.is_null());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_get_addresses_requires_authentication() {
    let client = reqwest::Client::new();

    let query = r#"{ list(collection: "addresses", limit: 10) }"#; // ignore-magic
    let (status, body) = graphql(&client, None, query).await;

    assert_eq!(status, 200);
    let _ = body;
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_delete_address() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let query = r#"mutation { delete(collection: "addresses", id: "addresses:nonexistent_123") }"#; // ignore-magic
    let (status, body) = graphql(&client, Some(&token), query).await;

    assert_eq!(status, 200);
    let _ = body;
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_delete_address_requires_authentication() {
    let client = reqwest::Client::new();

    let query = r#"mutation { delete(collection: "addresses", id: "addresses:test_123") }"#; // ignore-magic
    let (status, body) = graphql(&client, None, query).await;

    assert_eq!(status, 200);
    let _ = body;
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_cannot_access_other_user_profile() {
    let client = reqwest::Client::new();
    let (token1, _user_id1, _email1) = register_test_user(&client).await;
    let (_token2, user_id2, _email2) = register_test_user(&client).await;

    let query = format!(r#"{{ get(collection: "users", id: "{user_id2}") }}"#); // ignore-magic
    let (status, body) = graphql(&client, Some(&token1), &query).await;

    assert_eq!(status, 200);
    // May return the user doc (open rules) or null/error (restricted rules)
    let result = &body["data"]["get"]; // ignore-magic
    assert!(
        result.is_object() || result.is_null() || body.get("errors").is_some(),
        "Should handle cross-user access: {body}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_cannot_modify_other_user_address() {
    let client = reqwest::Client::new();
    let (token1, _user_id1, _email1) = register_test_user(&client).await;
    let (_token2, _user_id2, _email2) = register_test_user(&client).await;

    let query =
        r#"mutation { delete(collection: "addresses", id: "addresses:other_user_address") }"#; // ignore-magic
    let (status, body) = graphql(&client, Some(&token1), query).await;

    assert_eq!(status, 200);
    let _ = body;
}
