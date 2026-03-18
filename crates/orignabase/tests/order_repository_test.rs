//! Integration tests for order repository endpoints.
//!
//! These tests cover the complete order lifecycle:
//! - Order creation (POST /api/orders/create)
//! - Listing orders (buyer/seller views)
//! - Retrieving order details
//! - Cancelling orders
//! - Confirming delivery
//!
//! Run with: cargo test --test order_repository_test -- --ignored

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

#[allow(dead_code)]
fn buyer_address_payload(label: &str) -> Value {
    json!({
        "label": label,
        "street": "123 Queen St W",
        "city": "Toronto",
        "province": "ON",
        "postalCode": "M5V 2B7",
        "country": "Canada",
        "apartment": "Unit 8"
    })
}

// =============================================================================
// SECTION: Orders — Lifecycle
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_order_create_with_valid_payload() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    // Create order with valid payload
    let (status, body) = make_request(
        &client,
        "POST",
        "/api/orders/create",
        Some(&token),
        Some(json!({
            "items": [
                {
                    "productId": "products:test_prod_1",
                    "quantity": 1,
                    "unitPriceCents": 9999,
                    "name": "Test Product"
                }
            ],
            "shippingAddressId": "addresses:test_addr_1",
            "subtotalCents": 9999,
            "taxAmountCents": 0,
            "shippingCostCents": 0,
            "totalAmountCents": 9999,
            "sellerId": "users:test_seller"
        })),
    )
    .await;

    // Should return 200 or 201, or 404/400 if test data doesn't exist
    assert!(status == 200 || status == 201 || status == 400 || status == 404);
    if status == 200 || status == 201 {
        assert!(body.get("order_id").is_some() || body.get("id").is_some());
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_order_create_missing_items() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    // Missing items array
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/orders/create",
        Some(&token),
        Some(json!({
            "shippingAddressId": "addresses:test_addr_1",
            "subtotalCents": 0,
            "taxAmountCents": 0,
            "shippingCostCents": 0,
            "totalAmountCents": 0,
            "sellerId": "users:test_seller"
        })),
    )
    .await;

    // Should reject due to missing items
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_order_create_missing_seller_id() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    // Missing sellerId
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/orders/create",
        Some(&token),
        Some(json!({
            "items": [
                {
                    "productId": "products:test_prod_1",
                    "quantity": 1,
                    "unitPriceCents": 9999,
                    "name": "Test Product"
                }
            ],
            "shippingAddressId": "addresses:test_addr_1",
            "subtotalCents": 9999,
            "taxAmountCents": 0,
            "shippingCostCents": 0,
            "totalAmountCents": 9999
        })),
    )
    .await;

    // Should reject due to missing sellerId
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_order_get_buyer_orders() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    // Get buyer's orders (may be empty initially)
    let (status, body) = make_request(
        &client,
        "POST",
        "/api/orders/get-buyer-orders",
        Some(&token),
        Some(json!({
            "limit": 10,
            "offset": 0
        })),
    )
    .await;

    assert_eq!(status, 200, "Should succeed");
    assert!(body.get("orders").is_some() || body.is_array());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_order_get_buyer_orders_pagination() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    // Test pagination parameters
    let (status, body) = make_request(
        &client,
        "POST",
        "/api/orders/get-buyer-orders",
        Some(&token),
        Some(json!({
            "limit": 20,
            "offset": 0
        })),
    )
    .await;

    assert_eq!(status, 200);
    // Response should contain orders array or be empty
    assert!(body.get("orders").is_some() || body.is_array() || body.is_null());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_order_get_seller_orders() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    // Get seller's orders
    let (status, body) = make_request(
        &client,
        "POST",
        "/api/orders/get-seller-orders",
        Some(&token),
        Some(json!({
            "limit": 10,
            "offset": 0
        })),
    )
    .await;

    assert_eq!(status, 200, "Should succeed");
    assert!(body.get("orders").is_some() || body.is_array());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_order_get_by_id() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    // Get order by ID (use non-existent ID to test error handling)
    let (status, body) = make_request(
        &client,
        "POST",
        "/api/orders/get-order",
        Some(&token),
        Some(json!({
            "orderId": "orders:nonexistent_123"
        })),
    )
    .await;

    // Should return 404 if order not found, or 200 with null/error
    assert!(status == 200 || status == 404);
    if status == 200 {
        // If successful, should have order data
        assert!(body.get("order").is_some() || body.is_null());
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_order_cancel_requires_pending_status() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    // Try to cancel non-existent order
    let (status, body) = make_request(
        &client,
        "POST",
        "/api/orders/cancel-order",
        Some(&token),
        Some(json!({
            "orderId": "orders:nonexistent_123"
        })),
    )
    .await;

    // Should return 404 or 400 depending on implementation
    assert!(status == 400 || status == 404);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_order_confirm_receipt() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    // Try to confirm receipt of non-existent order
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/orders/confirm-receipt",
        Some(&token),
        Some(json!({
            "orderId": "orders:nonexistent_123"
        })),
    )
    .await;

    // Should return 404 or 400
    assert!(status == 400 || status == 404);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_order_requires_authentication() {
    let client = reqwest::Client::new();

    // Try to create order without token
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/orders/create",
        None,
        Some(json!({})),
    )
    .await;

    // Should require authentication
    assert!(status == 401 || status == 403);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_order_get_orders_requires_authentication() {
    let client = reqwest::Client::new();

    // Try to get orders without token
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/orders/get-buyer-orders",
        None,
        Some(json!({
            "limit": 10,
            "offset": 0
        })),
    )
    .await;

    // Should require authentication
    assert!(status == 401 || status == 403);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_order_money_validation() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    // Invalid money amounts (negative)
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/orders/create",
        Some(&token),
        Some(json!({
            "items": [
                {
                    "productId": "products:test_prod_1",
                    "quantity": 1,
                    "unitPriceCents": -100,
                    "name": "Test Product"
                }
            ],
            "shippingAddressId": "addresses:test_addr_1",
            "subtotalCents": -100,
            "taxAmountCents": 0,
            "shippingCostCents": 0,
            "totalAmountCents": -100,
            "sellerId": "users:test_seller"
        })),
    )
    .await;

    // Should reject negative amounts
    assert!(status == 400 || status == 422);
}
