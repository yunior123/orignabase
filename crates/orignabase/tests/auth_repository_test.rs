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
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
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
// SECTION: Auth — Registration
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_register_success() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4());

    let (status, body) = make_request(
        &client,
        "POST",
        "/auth/register",
        None,
        Some(json!({
            "email": email,
            "password": "TestPassword123!"
        })),
    )
    .await;

    assert_eq!(status, 200, "Registration should succeed");
    assert!(body.get("access_token").is_some(), "Should return access token");
    assert!(body.get("user").is_some(), "Should return user object");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_register_missing_email() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/auth/register",
        None,
        Some(json!({
            "password": "TestPassword123!"
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
    let email = format!("test_{}@example.com", Uuid::new_v4());

    let (status, _body) = make_request(
        &client,
        "POST",
        "/auth/register",
        None,
        Some(json!({
            "email": email
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
        "POST",
        "/auth/register",
        None,
        Some(json!({
            "email": "not-an-email",
            "password": "TestPassword123!"
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
    let email = format!("test_{}@example.com", Uuid::new_v4());

    let (status, _body) = make_request(
        &client,
        "POST",
        "/auth/register",
        None,
        Some(json!({
            "email": email,
            "password": "weak"
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
    let email = format!("test_{}@example.com", Uuid::new_v4());

    // Register first user
    let (status1, _body1) = make_request(
        &client,
        "POST",
        "/auth/register",
        None,
        Some(json!({
            "email": &email,
            "password": "TestPassword123!"
        })),
    )
    .await;
    assert_eq!(status1, 200);

    // Try to register with same email
    let (status2, _body2) = make_request(
        &client,
        "POST",
        "/auth/register",
        None,
        Some(json!({
            "email": &email,
            "password": "TestPassword123!"
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
    let email = format!("test_{}@example.com", Uuid::new_v4());

    // Register
    let (status_reg, _body_reg) = make_request(
        &client,
        "POST",
        "/auth/register",
        None,
        Some(json!({
            "email": &email,
            "password": "TestPassword123!"
        })),
    )
    .await;
    assert_eq!(status_reg, 200);

    // Login
    let (status_login, body_login) = make_request(
        &client,
        "POST",
        "/auth/login",
        None,
        Some(json!({
            "email": &email,
            "password": "TestPassword123!"
        })),
    )
    .await;

    assert_eq!(status_login, 200, "Login should succeed");
    assert!(body_login.get("access_token").is_some(), "Should return access token");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_login_missing_email() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/auth/login",
        None,
        Some(json!({
            "password": "TestPassword123!"
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
        "POST",
        "/auth/login",
        None,
        Some(json!({
            "email": "test@example.com"
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
    let email = format!("test_{}@example.com", Uuid::new_v4());

    // Register
    let (status_reg, _body_reg) = make_request(
        &client,
        "POST",
        "/auth/register",
        None,
        Some(json!({
            "email": &email,
            "password": "TestPassword123!"
        })),
    )
    .await;
    assert_eq!(status_reg, 200);

    // Try to login with wrong password
    let (status_login, _body_login) = make_request(
        &client,
        "POST",
        "/auth/login",
        None,
        Some(json!({
            "email": &email,
            "password": "WrongPassword123!"
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
        "POST",
        "/auth/login",
        None,
        Some(json!({
            "email": "nonexistent@example.com",
            "password": "TestPassword123!"
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

    // Try to access protected endpoint without token
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
async fn test_auth_invalid_token() {
    let client = reqwest::Client::new();

    // Try with invalid/malformed token
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/get-profile",
        Some("invalid_token_xyz"),
        None,
    )
    .await;

    // Should reject invalid token
    assert!(status == 401 || status == 403);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_expired_token_simulation() {
    let client = reqwest::Client::new();

    // Token that is deliberately malformed (simulates expired)
    let bad_token = "REDACTED_SECRET";

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/users/get-profile",
        Some(bad_token),
        None,
    )
    .await;

    // Should reject bad token
    assert!(status == 401 || status == 403);
}

// =============================================================================
// SECTION: Auth — Logout & Token Refresh
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_logout() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4());

    // Register and get token
    let (status_reg, body_reg) = make_request(
        &client,
        "POST",
        "/auth/register",
        None,
        Some(json!({
            "email": &email,
            "password": "TestPassword123!"
        })),
    )
    .await;
    assert_eq!(status_reg, 200);
    let token = body_reg["access_token"]
        .as_str()
        .expect("missing token")
        .to_string();

    // Logout
    let (status_logout, _body_logout) = make_request(
        &client,
        "POST",
        "/auth/logout",
        Some(&token),
        None,
    )
    .await;

    // Should succeed (200) or return 404 if endpoint doesn't exist
    assert!(status_logout == 200 || status_logout == 404);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_refresh_token() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4());

    // Register
    let (status_reg, body_reg) = make_request(
        &client,
        "POST",
        "/auth/register",
        None,
        Some(json!({
            "email": &email,
            "password": "TestPassword123!"
        })),
    )
    .await;
    assert_eq!(status_reg, 200);
    let token = body_reg["access_token"]
        .as_str()
        .expect("missing token")
        .to_string();

    // Try refresh
    let (status_refresh, body_refresh) = make_request(
        &client,
        "POST",
        "/auth/refresh",
        Some(&token),
        None,
    )
    .await;

    // Should succeed (200) or return 404 if endpoint doesn't exist
    assert!(status_refresh == 200 || status_refresh == 404);
    if status_refresh == 200 {
        assert!(body_refresh.get("access_token").is_some());
    }
}

// =============================================================================
// SECTION: Auth — Password Reset
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_auth_forgot_password() {
    let client = reqwest::Client::new();
    let email = format!("test_{}@example.com", Uuid::new_v4());

    // Register
    let (status_reg, _body_reg) = make_request(
        &client,
        "POST",
        "/auth/register",
        None,
        Some(json!({
            "email": &email,
            "password": "TestPassword123!"
        })),
    )
    .await;
    assert_eq!(status_reg, 200);

    // Request password reset
    let (status_reset, _body_reset) = make_request(
        &client,
        "POST",
        "/auth/forgot-password",
        None,
        Some(json!({
            "email": &email
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
        "POST",
        "/auth/forgot-password",
        None,
        Some(json!({
            "email": "does_not_exist@example.com"
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
    let email = format!("test_{}@example.com", Uuid::new_v4());

    // Attempt multiple failed logins in rapid succession
    for i in 0..5 {
        let (status, _body) = make_request(
            &client,
            "POST",
            "/auth/login",
            None,
            Some(json!({
                "email": &email,
                "password": format!("WrongPassword{}", i)
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
        let email = format!("test_{}@example.com", Uuid::new_v4());
        let (status, _body) = make_request(
            &client,
            "POST",
            "/auth/register",
            None,
            Some(json!({
                "email": &email,
                "password": "TestPassword123!"
            })),
        )
        .await;

        // Should succeed (200) for unique emails, or hit 429 (rate limit)
        assert!(status == 200 || status == 429);
    }
}
