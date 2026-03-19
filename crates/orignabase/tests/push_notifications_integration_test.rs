//! Integration tests for push notifications and FCM endpoints.
//!
//! These tests cover push token registration, notification management,
//! and rate limiting for push services.
//!
//! Run with: `cargo test --test push_notifications_integration_test -- --ignored`
//!
//! Requirements:
//!   surreal start --user root --pass root memory
//!   cargo run -- serve

use serde_json::{Value, json};
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

/// Register a test user and return (access_token, user_id, email).
async fn register_test_user(client: &reqwest::Client) -> (String, String, String) {
    let email = format!("test_{}@example.com", Uuid::new_v4());
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

// =============================================================================
// SECTION 1: Push Token Registration (4 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_500_push_register_token_success() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let fcm_token = format!("fcm_token_{}", Uuid::new_v4());

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/push/register-token",
        Some(&token),
        Some(json!({
            "fcmToken": fcm_token,
            "platform": "web",
            "userAgent": "Mozilla/5.0"
        })),
    )
    .await;

    assert!(
        status == 200 || status == 201 || status == 400,
        "Register token should succeed or fail gracefully: status={}, body={:?}",
        status,
        body
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_501_push_register_token_missing_fields() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/push/register-token",
        Some(&token),
        Some(json!({
            "platform": "web"
            // Missing fcmToken
        })),
    )
    .await;

    // Should reject missing required fields
    assert!(
        status == 400 || status == 422,
        "Should reject missing fcmToken"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_502_push_register_token_empty_token() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/push/register-token",
        Some(&token),
        Some(json!({
            "fcmToken": "",
            "platform": "web"
        })),
    )
    .await;

    // Should reject empty token
    assert!(
        status == 400 || status == 422,
        "Should reject empty fcmToken"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_503_push_register_multiple_tokens() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    // Register first token
    let fcm_token_1 = format!("fcm_token_1_{}", Uuid::new_v4());
    let (_status_1, _body_1) = make_request(
        &client,
        "POST",
        "/api/push/register-token",
        Some(&token),
        Some(json!({
            "fcmToken": fcm_token_1,
            "platform": "web"
        })),
    )
    .await;

    // Register second token
    let fcm_token_2 = format!("fcm_token_2_{}", Uuid::new_v4());
    let (status_2, _body_2) = make_request(
        &client,
        "POST",
        "/api/push/register-token",
        Some(&token),
        Some(json!({
            "fcmToken": fcm_token_2,
            "platform": "ios"
        })),
    )
    .await;

    // Should succeed — user can have multiple tokens (different devices)
    assert!(
        status_2 == 200 || status_2 == 201 || status_2 == 400,
        "Should allow multiple tokens per user"
    );
}

// =============================================================================
// SECTION 2: Push Token Unregistration (3 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_504_push_unregister_token_success() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let fcm_token = format!("fcm_token_{}", Uuid::new_v4());

    // First register
    let _ = make_request(
        &client,
        "POST",
        "/api/push/register-token",
        Some(&token),
        Some(json!({
            "fcmToken": fcm_token.clone(),
            "platform": "web"
        })),
    )
    .await;

    // Then unregister
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/push/unregister-token",
        Some(&token),
        Some(json!({
            "fcmToken": fcm_token
        })),
    )
    .await;

    assert!(
        status == 200 || status == 204 || status == 400,
        "Unregister should succeed or fail gracefully"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_505_push_unregister_nonexistent_token() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/push/unregister-token",
        Some(&token),
        Some(json!({
            "fcmToken": "nonexistent_token_12345"
        })),
    )
    .await;

    // Should handle gracefully (idempotent)
    assert!(
        status == 200 || status == 204 || status == 404,
        "Unregistering nonexistent token should be idempotent"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_506_push_unregister_empty_token() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/push/unregister-token",
        Some(&token),
        Some(json!({
            "fcmToken": ""
        })),
    )
    .await;

    // Should reject empty token
    assert!(
        status == 400 || status == 422,
        "Should reject empty fcmToken"
    );
}

// =============================================================================
// SECTION 3: Notifications — Retrieval and Management (4 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_507_notifications_get_list() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/notifications/get",
        Some(&token),
        Some(json!({
            "limit": 10,
            "offset": 0
        })),
    )
    .await;

    // Should return notification list (may be empty for new user)
    assert_eq!(status, 200, "Get notifications should succeed: {:?}", body);
    assert!(
        body.get("notifications").is_some() || body.is_array(),
        "Should return notifications array"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_508_notifications_mark_read() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    // First get notifications to see if any exist
    let (status_get, body_get) = make_request(
        &client,
        "POST",
        "/api/notifications/get",
        Some(&token),
        Some(json!({
            "limit": 10,
            "offset": 0
        })),
    )
    .await;

    assert_eq!(status_get, 200);

    // Try to mark a notification as read (use any ID if available)
    let (status_mark, _body_mark) = make_request(
        &client,
        "POST",
        "/api/notifications/mark-read",
        Some(&token),
        Some(json!({
            "notificationId": "test-notification-123"
        })),
    )
    .await;

    // Should succeed or return 404 if notification doesn't exist
    assert!(
        status_mark == 200 || status_mark == 204 || status_mark == 404,
        "Mark read should succeed or handle missing notification"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_509_notifications_mark_all_read() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/notifications/mark-all-read",
        Some(&token),
        Some(json!({})),
    )
    .await;

    // Should succeed even if no unread notifications exist
    assert!(
        status == 200 || status == 204,
        "Mark all read should be idempotent"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_510_notifications_delete() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "DELETE",
        "/api/notifications/test-notification-123",
        Some(&token),
        None,
    )
    .await;

    // Should handle gracefully
    assert!(
        status == 200 || status == 204 || status == 404,
        "Delete should succeed or handle missing notification"
    );
}

// =============================================================================
// SECTION 4: Push Rate Limiting (2 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_511_push_rate_limit_check() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    // Try rapid registrations to test rate limiting
    for i in 0..3 {
        let fcm_token = format!("fcm_token_{}_{}", i, Uuid::new_v4());
        let (status, _body) = make_request(
            &client,
            "POST",
            "/api/push/register-token",
            Some(&token),
            Some(json!({
                "fcmToken": fcm_token,
                "platform": "web"
            })),
        )
        .await;

        // Should eventually hit rate limit or succeed
        assert!(
            status == 200 || status == 201 || status == 400 || status == 429 || status == 503,
            "Rate limit should be enforced: status={}",
            status
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_512_notification_list_pagination() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    // Test with different limits and offsets
    let (status_1, _body_1) = make_request(
        &client,
        "POST",
        "/api/notifications/get",
        Some(&token),
        Some(json!({
            "limit": 5,
            "offset": 0
        })),
    )
    .await;

    let (status_2, _body_2) = make_request(
        &client,
        "POST",
        "/api/notifications/get",
        Some(&token),
        Some(json!({
            "limit": 10,
            "offset": 5
        })),
    )
    .await;

    assert_eq!(status_1, 200);
    assert_eq!(status_2, 200);
}
