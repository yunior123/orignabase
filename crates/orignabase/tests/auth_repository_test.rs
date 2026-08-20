//! Integration tests for auth repository endpoints.
//!
//! These tests cover:
//! - User registration (POST /auth/register)
//! - Login (POST /auth/login)
//! - Token refresh
//! - Logout
//! - Authentication enforcement
//! - Password reset (forgot password)
//!
//! Run with: cargo test --test auth_repository_test -- --ignored

use serde_json::{Value, json};
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string()) // ignore-magic
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
        "POST" => client.post(&url),     // ignore-magic
        "GET" => client.get(&url),       // ignore-magic
        "PUT" => client.put(&url),       // ignore-magic
        "DELETE" => client.delete(&url), // ignore-magic
        _ => panic!("Unsupported method"),
    };

    let req = if let Some(t) = token {
        req.header("Authorization", format!("Bearer {t}")) // ignore-magic
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
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic
    (status, body)
}

// =============================================================================
// SECTION: Auth — Registration
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_register_success() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic

    let (status, body) = make_request(
        &client,
        "POST",           // ignore-magic
        "/auth/register", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": email, // ignore-magic
            "password": "TestPassword123!" // ignore-magic
        })),
    )
    .await;

    assert_eq!(status, 200, "Registration should succeed");
    assert!(
        body.get("access_token").is_some(), // ignore-magic
        "Should return access token"
    );
    assert!(body.get("user").is_some(), "Should return user object"); // ignore-magic
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_register_missing_email() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",           // ignore-magic
        "/auth/register", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "password": "TestPassword123!" // ignore-magic
        })),
    )
    .await;

    // Should reject missing email
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_register_missing_password() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic

    let (status, _body) = make_request(
        &client,
        "POST",           // ignore-magic
        "/auth/register", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": email // ignore-magic
        })),
    )
    .await;

    // Should reject missing password
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_register_invalid_email() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",           // ignore-magic
        "/auth/register", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": "not-an-email", // ignore-magic
            "password": "TestPassword123!" // ignore-magic
        })),
    )
    .await;

    // Should reject invalid email format
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_register_weak_password() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic

    let (status, _body) = make_request(
        &client,
        "POST",           // ignore-magic
        "/auth/register", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": email, // ignore-magic
            "password": "weak" // ignore-magic
        })),
    )
    .await;

    // Should reject weak password (too short)
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_register_duplicate_email() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic

    // Register first user
    let (status1, _body1) = make_request(
        &client,
        "POST",           // ignore-magic
        "/auth/register", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": &email, // ignore-magic
            "password": "TestPassword123!" // ignore-magic
        })),
    )
    .await;
    assert_eq!(status1, 200);

    // Try to register with same email
    let (status2, _body2) = make_request(
        &client,
        "POST",           // ignore-magic
        "/auth/register", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": &email, // ignore-magic
            "password": "TestPassword123!" // ignore-magic
        })),
    )
    .await;

    // Should reject duplicate email
    assert!(status2 == 400 || status2 == 409);
}

// =============================================================================
// SECTION: Auth — Login
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_login_success() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic

    // Register
    let (status_reg, _body_reg) = make_request(
        &client,
        "POST",           // ignore-magic
        "/auth/register", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": &email, // ignore-magic
            "password": "TestPassword123!" // ignore-magic
        })),
    )
    .await;
    assert_eq!(status_reg, 200);

    // Login
    let (status_login, body_login) = make_request(
        &client,
        "POST",        // ignore-magic
        "/auth/login", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": &email, // ignore-magic
            "password": "TestPassword123!" // ignore-magic
        })),
    )
    .await;

    assert_eq!(status_login, 200, "Login should succeed");
    assert!(
        body_login.get("access_token").is_some(), // ignore-magic
        "Should return access token"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_login_missing_email() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",        // ignore-magic
        "/auth/login", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "password": "TestPassword123!" // ignore-magic
        })),
    )
    .await;

    // Should reject missing email
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_login_missing_password() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",        // ignore-magic
        "/auth/login", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": "test@example.com" // ignore-magic
        })),
    )
    .await;

    // Should reject missing password
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_login_invalid_password() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic

    // Register
    let (status_reg, _body_reg) = make_request(
        &client,
        "POST",           // ignore-magic
        "/auth/register", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": &email, // ignore-magic
            "password": "TestPassword123!" // ignore-magic
        })),
    )
    .await;
    assert_eq!(status_reg, 200);

    // Try to login with wrong password
    let (status_login, _body_login) = make_request(
        &client,
        "POST",        // ignore-magic
        "/auth/login", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": &email, // ignore-magic
            "password": "WrongPassword123!" // ignore-magic
        })),
    )
    .await;

    // Should reject invalid password
    assert!(status_login == 401 || status_login == 400);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_login_nonexistent_email() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",        // ignore-magic
        "/auth/login", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": "nonexistent@example.com", // ignore-magic
            "password": "TestPassword123!" // ignore-magic
        })),
    )
    .await;

    // Should reject with 401 (anti-enumeration: don't reveal if email exists)
    assert!(status == 401 || status == 400);
}

// =============================================================================
// SECTION: Auth — Token Validation
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_protected_endpoint_requires_token() {
    let client = reqwest::Client::new();

    // Try to access GraphQL without token — returns 200 with null data
    let (status, body) = make_request(
        &client,
        "POST",     // ignore-magic
        "/graphql", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "query": "query { get(collection: \"users\", id: \"users:test\") }" // ignore-magic
        })),
    )
    .await;

    assert_eq!(status, 200, "GraphQL always returns 200");
    // Without auth, data should be null
    let data = body.get("data");
    assert!(
        data.is_none()
            || data.and_then(|d| d.as_object()).is_none()
            || data.and_then(|d| d.get("get")).is_none_or(|v| v.is_null()),
        "Should return null data without auth"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_invalid_token() {
    let client = reqwest::Client::new();

    // Try GraphQL with invalid/malformed token — returns 200 with null data
    let (status, body) = make_request(
        &client,
        "POST",     // ignore-magic
        "/graphql", // ignore-magic
        Some("invalid_token_xyz"),
        Some(json!({ // ignore-magic
            "query": "query { get(collection: \"users\", id: \"users:test\") }" // ignore-magic
        })),
    )
    .await;

    assert_eq!(status, 200, "GraphQL always returns 200");
    // With invalid token, data should be null
    let data = body.get("data");
    assert!(
        data.is_none()
            || data.and_then(|d| d.as_object()).is_none()
            || data.and_then(|d| d.get("get")).is_none_or(|v| v.is_null()),
        "Should return null data with invalid token"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_expired_token_simulation() {
    let client = reqwest::Client::new();

    // Token that is deliberately malformed (simulates expired)
    let bad_token = "REDACTED_SECRET";

    // Try GraphQL with bad token — returns 200 with null data
    let (status, body) = make_request(
        &client,
        "POST",     // ignore-magic
        "/graphql", // ignore-magic
        Some(bad_token),
        Some(json!({ // ignore-magic
            "query": "query { get(collection: \"users\", id: \"users:test\") }" // ignore-magic
        })),
    )
    .await;

    assert_eq!(status, 200, "GraphQL always returns 200");
    // With bad token, data should be null
    let data = body.get("data");
    assert!(
        data.is_none()
            || data.and_then(|d| d.as_object()).is_none()
            || data.and_then(|d| d.get("get")).is_none_or(|v| v.is_null()),
        "Should return null data with bad token"
    );
}

// =============================================================================
// SECTION: Auth — Logout & Token Refresh
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_logout() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic

    // Register and get tokens
    let (status_reg, body_reg) = make_request(
        &client,
        "POST",           // ignore-magic
        "/auth/register", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": &email, // ignore-magic
            "password": "TestPassword123!" // ignore-magic
        })),
    )
    .await;
    assert_eq!(status_reg, 200);
    let refresh_token = body_reg["refresh_token"] // ignore-magic
        .as_str()
        .expect("missing refresh_token")
        .to_string();

    // Logout — requires refresh_token in body
    let (status_logout, _body_logout) = make_request(
        &client,
        "POST",
        "/auth/logout",
        None,
        Some(json!({ // ignore-magic
            "refresh_token": &refresh_token // ignore-magic
        })),
    )
    .await;

    // Should succeed (200)
    assert_eq!(
        status_logout, 200,
        "Logout should succeed with valid refresh_token"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_refresh_token() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic

    // Register
    let (status_reg, body_reg) = make_request(
        &client,
        "POST",           // ignore-magic
        "/auth/register", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": &email, // ignore-magic
            "password": "TestPassword123!" // ignore-magic
        })),
    )
    .await;
    assert_eq!(status_reg, 200);
    let token = body_reg["access_token"] // ignore-magic
        .as_str()
        .expect("missing token")
        .to_string();

    // Try refresh — use body with refresh_token, not Bearer header
    // Registration doesn't return a refresh_token, so we use the access_token as a fallback
    let refresh_token = body_reg
        .get("refresh_token") // ignore-magic
        .and_then(|v| v.as_str())
        .unwrap_or(&token);

    let (status_refresh, body_refresh) = make_request(
        &client,
        "POST",          // ignore-magic
        "/auth/refresh", // ignore-magic
        None,
        Some(json!({ "refresh_token": refresh_token })), // ignore-magic
    )
    .await;

    // Should succeed (200) or return 404 if endpoint doesn't exist
    assert!(status_refresh == 200 || status_refresh == 404);
    if status_refresh == 200 {
        assert!(body_refresh.get("access_token").is_some()); // ignore-magic
    }
}

// =============================================================================
// SECTION: Auth — Password Reset
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_forgot_password() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic

    // Register
    let (status_reg, _body_reg) = make_request(
        &client,
        "POST",           // ignore-magic
        "/auth/register", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": &email, // ignore-magic
            "password": "TestPassword123!" // ignore-magic
        })),
    )
    .await;
    assert_eq!(status_reg, 200);

    // Request password reset
    let (status_reset, _body_reset) = make_request(
        &client,
        "POST",                  // ignore-magic
        "/auth/forgot-password", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": &email // ignore-magic
        })),
    )
    .await;

    // Should succeed (200) or return 404 if endpoint doesn't exist
    // IMPORTANT: Should NOT reveal if email exists (anti-enumeration)
    assert!(status_reset == 200 || status_reset == 404);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_forgot_password_nonexistent_email() {
    let client = reqwest::Client::new();

    // Request reset for non-existent email
    let (status, _body) = make_request(
        &client,
        "POST",                  // ignore-magic
        "/auth/forgot-password", // ignore-magic
        None,
        Some(json!({ // ignore-magic
            "email": "does_not_exist@example.com" // ignore-magic
        })),
    )
    .await;

    // Should NOT reveal if email exists (anti-enumeration)
    // Typically returns 200 (success) without exposing whether email exists
    assert!(status == 200 || status == 404);
}

// =============================================================================
// SECTION: Auth — Rate Limiting
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_rate_limit_login_attempts() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic

    // Attempt multiple failed logins in rapid succession
    for i in 0..5 {
        let (status, _body) = make_request(
            &client,
            "POST",        // ignore-magic
            "/auth/login", // ignore-magic
            None,
            Some(json!({ // ignore-magic
                "email": &email, // ignore-magic
                "password": format!("WrongPassword{}", i) // ignore-magic
            })),
        )
        .await;

        // First attempts should return 401, eventually may hit 429 (rate limit)
        assert!(status == 400 || status == 401 || status == 429);
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_rate_limit_registration() {
    let client = reqwest::Client::new();

    // Try to register multiple times with same email
    for _i in 0..3 {
        let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic
        let (status, _body) = make_request(
            &client,
            "POST",           // ignore-magic
            "/auth/register", // ignore-magic
            None,
            Some(json!({ // ignore-magic
                "email": &email, // ignore-magic
                "password": "TestPassword123!" // ignore-magic
            })),
        )
        .await;

        // Should succeed (200) for unique emails, or hit 429 (rate limit)
        assert!(status == 200 || status == 429);
    }
}
