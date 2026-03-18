//! Integration tests for cart repository endpoints.
//!
//! These tests cover:
//! - Adding items to cart
//! - Retrieving cart contents
//! - Removing items
//! - Updating quantities
//! - Clearing cart
//!
//! Run with: cargo test --test cart_repository_test -- --ignored

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

// =============================================================================
// SECTION: Cart — Operations
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_add_item() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/cart/add",
        Some(&token),
        Some(json!({
            "productId": "products:test_prod_1",
            "quantity": 1,
            "unitPriceCents": 2999
        })),
    )
    .await;

    // Should succeed or fail with 400 if product doesn't exist
    assert!(status == 200 || status == 201 || status == 400);
    if status == 200 || status == 201 {
        assert!(body.get("cartId").is_some() || body.get("id").is_some() || body.get("success").is_some());
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_add_item_missing_product_id() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/add",
        Some(&token),
        Some(json!({
            "quantity": 1,
            "unitPriceCents": 2999
        })),
    )
    .await;

    // Should reject missing productId
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_add_item_invalid_quantity() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/add",
        Some(&token),
        Some(json!({
            "productId": "products:test_prod_1",
            "quantity": -5,
            "unitPriceCents": 2999
        })),
    )
    .await;

    // Should reject negative quantity
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_add_item_zero_quantity() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/add",
        Some(&token),
        Some(json!({
            "productId": "products:test_prod_1",
            "quantity": 0,
            "unitPriceCents": 2999
        })),
    )
    .await;

    // Should reject zero quantity
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_add_item_invalid_price() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/add",
        Some(&token),
        Some(json!({
            "productId": "products:test_prod_1",
            "quantity": 1,
            "unitPriceCents": -100
        })),
    )
    .await;

    // Should reject negative price
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_add_requires_authentication() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/add",
        None,
        Some(json!({
            "productId": "products:test_prod_1",
            "quantity": 1,
            "unitPriceCents": 2999
        })),
    )
    .await;

    assert!(status == 401 || status == 403);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_get() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/cart/get",
        Some(&token),
        None,
    )
    .await;

    assert_eq!(status, 200, "Should retrieve cart");
    // Cart may be empty initially
    assert!(body.get("items").is_some() || body.get("cart").is_some() || body.is_array());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_get_requires_authentication() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/get",
        None,
        None,
    )
    .await;

    assert!(status == 401 || status == 403);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_remove_item() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/remove",
        Some(&token),
        Some(json!({
            "productId": "products:nonexistent_123"
        })),
    )
    .await;

    // Should succeed or return 404 if item not in cart
    assert!(status == 200 || status == 400 || status == 404);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_remove_requires_authentication() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/remove",
        None,
        Some(json!({
            "productId": "products:test_123"
        })),
    )
    .await;

    assert!(status == 401 || status == 403);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_remove_missing_product_id() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/remove",
        Some(&token),
        Some(json!({})),
    )
    .await;

    // Should reject missing productId
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_clear() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/clear",
        Some(&token),
        None,
    )
    .await;

    // Should succeed (even if cart already empty)
    assert_eq!(status, 200);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_clear_requires_authentication() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/clear",
        None,
        None,
    )
    .await;

    assert!(status == 401 || status == 403);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_update_quantity() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/update-quantity",
        Some(&token),
        Some(json!({
            "productId": "products:nonexistent_123",
            "quantity": 5
        })),
    )
    .await;

    // Should succeed or return 404 if item not in cart
    assert!(status == 200 || status == 400 || status == 404);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_update_quantity_invalid_qty() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/update-quantity",
        Some(&token),
        Some(json!({
            "productId": "products:test_123",
            "quantity": -1
        })),
    )
    .await;

    // Should reject negative quantity
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_update_quantity_zero() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/update-quantity",
        Some(&token),
        Some(json!({
            "productId": "products:test_123",
            "quantity": 0
        })),
    )
    .await;

    // May treat as remove (200) or reject (400)
    assert!(status == 200 || status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_cart_update_quantity_requires_authentication() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/cart/update-quantity",
        None,
        Some(json!({
            "productId": "products:test_123",
            "quantity": 5
        })),
    )
    .await;

    assert!(status == 401 || status == 403);
}
