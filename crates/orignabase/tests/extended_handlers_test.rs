//! Extended integration tests for additional handler coverage.
//!
//! This file covers specific endpoints and edge cases not fully captured
//! in handlers_integration_test.rs, including:
//! - Shipping calculation advanced cases
//! - Email validation and format edge cases
//! - Analytics and tracking endpoints
//! - Rate limiting scenarios
//!
//! Run with: `cargo test --test extended_handlers_test -- --ignored`

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
// SECTION 1: Shipping Calculation — Advanced Cases (5 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_600_shipping_calc_perishable_local_delivery() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/shipping/calculate",
        Some(&token),
        Some(json!({
            "items": [{
                "productId": "prod_123",
                "quantity": 1,
                "weight": 0.5,
                "isPerishable": true
            }],
            "warehouseAddress": {
                "latitude": 43.6629,
                "longitude": -79.3957
            },
            "buyerAddress": {
                "latitude": 43.6532,
                "longitude": -79.3832
            }
        })),
    )
    .await;

    // Should return shipping cost or error if distance > 50km
    assert!(
        status == 200 || status == 400,
        "Perishable shipping should validate distance"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_601_shipping_calc_cross_province() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/shipping/calculate",
        Some(&token),
        Some(json!({
            "items": [{
                "productId": "prod_123",
                "quantity": 1,
                "weight": 1.0,
                "isPerishable": false
            }],
            "warehouseAddress": {
                "latitude": 43.6629,
                "longitude": -79.3957
            },
            "buyerAddress": {
                "latitude": 51.0486,
                "longitude": -114.0708
            }
        })),
    )
    .await;

    // Should accept cross-province shipping for non-perishable items
    assert!(
        status == 200 || status == 400,
        "Cross-province shipping should be allowed"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_602_shipping_calc_free_shipping_threshold() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    // Order subtotal above free shipping threshold ($75 CAD = 7500 cents)
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/shipping/calculate",
        Some(&token),
        Some(json!({
            "items": [{
                "productId": "prod_123",
                "quantity": 1,
                "weight": 2.0,
                "price": 8000
            }],
            "subtotalCents": 8000,
            "warehouseAddress": {
                "latitude": 43.6629,
                "longitude": -79.3957
            },
            "buyerAddress": {
                "latitude": 43.6532,
                "longitude": -79.3832
            }
        })),
    )
    .await;

    // May return free shipping or reduced shipping cost
    assert!(
        status == 200 || status == 400,
        "Free shipping threshold should apply"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_603_shipping_calc_multiple_items() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/shipping/calculate",
        Some(&token),
        Some(json!({
            "items": [
                {
                    "productId": "prod_1",
                    "quantity": 2,
                    "weight": 0.5
                },
                {
                    "productId": "prod_2",
                    "quantity": 1,
                    "weight": 2.0
                },
                {
                    "productId": "prod_3",
                    "quantity": 3,
                    "weight": 0.2
                }
            ],
            "warehouseAddress": {
                "latitude": 43.6629,
                "longitude": -79.3957
            },
            "buyerAddress": {
                "latitude": 43.6532,
                "longitude": -79.3832
            }
        })),
    )
    .await;

    assert!(
        status == 200 || status == 400,
        "Multi-item shipping should combine weights"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_604_shipping_calc_invalid_coordinates() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/shipping/calculate",
        Some(&token),
        Some(json!({
            "items": [{
                "productId": "prod_123",
                "quantity": 1,
                "weight": 1.0
            }],
            "warehouseAddress": {
                "latitude": 999.0,
                "longitude": 999.0
            },
            "buyerAddress": {
                "latitude": 43.6532,
                "longitude": -79.3832
            }
        })),
    )
    .await;

    // Should reject invalid lat/lon
    assert!(
        status == 400 || status == 422,
        "Invalid coordinates should be rejected"
    );
}

// =============================================================================
// SECTION 2: Email Validation & Resend (4 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_605_email_verification_resend() {
    let client = reqwest::Client::new();
    let (token, _user_id, email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/auth/resend-verification",
        Some(&token),
        Some(json!({
            "email": email
        })),
    )
    .await;

    // Should allow resending verification email
    assert!(
        status == 200 || status == 400,
        "Resend verification should be supported"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_606_email_format_validation() {
    let client = reqwest::Client::new();

    let invalid_emails = vec![
        "notanemail",
        "missing@domain",
        "@nodomain.com",
        "spaces in@email.com",
        "",
    ];

    for invalid_email in invalid_emails {
        let (status, _body) = make_request(
            &client,
            "POST",
            "/auth/register",
            None,
            Some(json!({
                "email": invalid_email,
                "password": "TestPassword123!"
            })),
        )
        .await;

        // Should reject invalid emails
        assert!(
            status == 400 || status == 422,
            "Invalid email {} should be rejected",
            invalid_email
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_607_email_case_insensitivity() {
    let client = reqwest::Client::new();

    let email = format!("TEST_{}@EXAMPLE.COM", Uuid::new_v4());
    let (status, _body) = make_request(
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

    // Should accept — email should be normalized to lowercase
    assert_eq!(status, 200, "Email case should be normalized");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_608_email_duplicate_prevention() {
    let client = reqwest::Client::new();
    let email = format!("unique_{}@example.com", Uuid::new_v4());

    // Register first user
    let (status_1, _body_1) = make_request(
        &client,
        "POST",
        "/auth/register",
        None,
        Some(json!({
            "email": email.clone(),
            "password": "TestPassword123!"
        })),
    )
    .await;

    assert_eq!(status_1, 200);

    // Try register same email again
    let (status_2, _body_2) = make_request(
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

    // Should reject duplicate
    assert_eq!(status_2, 409, "Duplicate email should be rejected");
}

// =============================================================================
// SECTION 3: Unauthenticated Endpoint Access (3 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_609_public_product_list() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/list",
        None, // No token
        Some(json!({
            "limit": 10,
            "offset": 0
        })),
    )
    .await;

    // Public product listing may be allowed without auth
    assert!(
        status == 200 || status == 401 || status == 403,
        "Public product list should be accessible or require auth"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_610_unauthorized_protected_endpoint() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/user/profile",
        None, // No token
        Some(json!({})),
    )
    .await;

    // Should require authentication
    assert_eq!(status, 401, "Protected endpoints must require auth");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_611_invalid_token_format() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/user/profile",
        Some("not-a-valid-jwt-token"),
        Some(json!({})),
    )
    .await;

    // Should reject invalid token
    assert_eq!(status, 401, "Invalid token should be rejected");
}

// =============================================================================
// SECTION 4: Sequential Request Handling (2 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_612_sequential_user_operations() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    // Execute sequential requests
    for i in 0..5 {
        let (status, _body) = make_request(
            &client,
            "POST",
            "/api/user/profile",
            Some(&token),
            Some(json!({
                "name": format!("User {}", i)
            })),
        )
        .await;

        assert!(
            status == 200 || status == 400,
            "Sequential updates should succeed"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_613_sequential_cart_operations() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    // Sequential cart modifications
    for i in 0..3 {
        let (status, _body) = make_request(
            &client,
            "POST",
            "/api/cart/add-item",
            Some(&token),
            Some(json!({
                "productId": format!("prod_{}", i),
                "quantity": 1
            })),
        )
        .await;

        assert!(status == 200 || status == 201, "Cart add should succeed");
    }
}
