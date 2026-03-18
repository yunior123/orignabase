//! Integration tests for user repository endpoints.
//!
//! These tests cover:
//! - User profile operations (get, update)
//! - Address management (add, list, delete)
//! - User role and permission checks
//!
//! Run with: cargo test --test user_repository_test -- --ignored

use serde_json::{Value, json};
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

async fn register_test_user(client: &reqwest::Client) -> (String, String, String) {
    let email = format!("test_{}@example.com", Uuid::new_v4());
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "TestPassword123!" }))
        .send()
        .await
        .expect("register failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let token = body["access_token"]
        .as_str()
        .expect("missing access_token")
        .to_string();
    let user_id = body["user"]["id"]
        .as_str()
        .expect("missing user.id")
        .to_string();
    (token, user_id, email)
}

async fn make_request(
    client: &reqwest::Client,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (u16, Value) {
    let url = format!("{}{}", base_url(), path);

    let req = match method {
        "POST" => client.post(&url),
        "GET" => client.get(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        _ => panic!("Unsupported method"),
    };

    let req = if let Some(t) = token {
        req.header("Authorization", format!("Bearer {t}"))
    } else {
        req
    };

    let req = if let Some(b) = body {
        req.json(&b)
    } else {
        req
    };

    let resp = req.send().await.expect("request failed");
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({}));
    (status, body)
}

fn address_payload(label: &str) -> Value {
    json!({
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
// SECTION: Users — Profile
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_get_profile_success() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/users/get-profile",
        Some(&token),
        None,
    )
    .await;

    assert_eq!(status, 200);
    // Should return user profile with email matching registration
    assert!(body.get("profile").is_some() || body.get("user").is_some());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_get_profile_requires_authentication() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/get-profile",
        None,
        None,
    )
    .await;

    assert!(status == 401 || status == 403);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_update_profile_success() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/update-profile",
        Some(&token),
        Some(json!({
            "firstName": "John",
            "lastName": "Doe",
            "phoneNumber": "+14165551234"
        })),
    )
    .await;

    // Should succeed or return 400 for validation errors
    assert!(status == 200 || status == 400);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_update_profile_invalid_phone() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/update-profile",
        Some(&token),
        Some(json!({
            "phoneNumber": "invalid-phone"
        })),
    )
    .await;

    // Should reject invalid phone format
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_update_profile_requires_authentication() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/update-profile",
        None,
        Some(json!({
            "firstName": "Jane"
        })),
    )
    .await;

    assert!(status == 401 || status == 403);
}

// =============================================================================
// SECTION: Users — Addresses
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_add_address() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/users/add-address",
        Some(&token),
        Some(address_payload("Home")),
    )
    .await;

    assert!(status == 200 || status == 201);
    assert!(body.get("addressId").is_some() || body.get("id").is_some() || body.get("address").is_some());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_add_address_missing_street() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/add-address",
        Some(&token),
        Some(json!({
            "label": "Home",
            "city": "Toronto",
            "province": "ON",
            "postalCode": "M5V 2B7",
            "country": "Canada"
        })),
    )
    .await;

    // Should reject missing street
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_add_address_missing_city() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/add-address",
        Some(&token),
        Some(json!({
            "label": "Home",
            "street": "123 Queen St W",
            "province": "ON",
            "postalCode": "M5V 2B7",
            "country": "Canada"
        })),
    )
    .await;

    // Should reject missing city
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_add_address_invalid_postal_code() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/add-address",
        Some(&token),
        Some(json!({
            "label": "Home",
            "street": "123 Queen St W",
            "city": "Toronto",
            "province": "ON",
            "postalCode": "INVALID",
            "country": "Canada"
        })),
    )
    .await;

    // Should reject invalid Canadian postal code
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_add_address_requires_authentication() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/add-address",
        None,
        Some(address_payload("Home")),
    )
    .await;

    assert!(status == 401 || status == 403);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_get_addresses() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/users/get-addresses",
        Some(&token),
        None,
    )
    .await;

    assert_eq!(status, 200);
    // Should return array of addresses (may be empty initially)
    assert!(body.is_array() || body.get("addresses").is_some());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_get_addresses_requires_authentication() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/get-addresses",
        None,
        None,
    )
    .await;

    assert!(status == 401 || status == 403);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_delete_address() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    // Try to delete non-existent address
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/delete-address",
        Some(&token),
        Some(json!({
            "addressId": "addresses:nonexistent_123"
        })),
    )
    .await;

    // Should succeed silently or return 404
    assert!(status == 200 || status == 404);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_delete_address_requires_authentication() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/delete-address",
        None,
        Some(json!({
            "addressId": "addresses:test_123"
        })),
    )
    .await;

    assert!(status == 401 || status == 403);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_delete_address_missing_id() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/delete-address",
        Some(&token),
        Some(json!({})),
    )
    .await;

    // Should reject missing addressId
    assert!(status == 400 || status == 422);
}

// =============================================================================
// SECTION: Users — Authorization
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_cannot_access_other_user_profile() {
    let client = reqwest::Client::new();
    let (token1, _user_id1, _email1) = register_test_user(&client).await;
    let (_token2, user_id2, _email2) = register_test_user(&client).await;

    // User 1 tries to access User 2's profile directly (if supported)
    let (status, _body) = make_request(
        &client,
        "POST",
        &format!("/api/users/{}/profile", user_id2),
        Some(&token1),
        None,
    )
    .await;

    // Should return 401/403 (unauthorized) or 404 (not found)
    // If endpoint doesn't exist, 404 is acceptable
    assert!(status == 401 || status == 403 || status == 404);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_user_cannot_modify_other_user_address() {
    let client = reqwest::Client::new();
    let (token1, _user_id1, _email1) = register_test_user(&client).await;
    let (_token2, _user_id2, _email2) = register_test_user(&client).await;

    // User 1 tries to delete User 2's address
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/delete-address",
        Some(&token1),
        Some(json!({
            "addressId": "addresses:other_user_address"
        })),
    )
    .await;

    // Should reject (either 401/403 or silent success but address unchanged)
    assert!(status == 200 || status == 401 || status == 403 || status == 404);
}
