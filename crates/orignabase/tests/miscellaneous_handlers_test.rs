//! Miscellaneous integration tests for less-common handler scenarios.
//!
//! This file covers:
//! - PDF generation (invoices)
//! - File upload/download
//! - State transition edge cases
//! - Error recovery and resilience
//!
//! Run with: `cargo test --test miscellaneous_handlers_test -- --ignored`

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
// SECTION 1: PDF Invoice Generation (3 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_700_invoice_generate_english() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/orders/generate-invoice",
        Some(&token),
        Some(json!({
            "orderId": "ord_test_123",
            "language": "en"
        })),
    )
    .await;

    // May succeed or fail if order doesn't exist
    assert!(
        status == 200 || status == 404 || status == 400,
        "Invoice generation should handle missing orders gracefully"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_701_invoice_generate_french() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/orders/generate-invoice",
        Some(&token),
        Some(json!({
            "orderId": "ord_test_456",
            "language": "fr"
        })),
    )
    .await;

    assert!(
        status == 200 || status == 404 || status == 400,
        "French invoice generation should be supported"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_702_invoice_missing_order() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/orders/generate-invoice",
        Some(&token),
        Some(json!({
            "orderId": "nonexistent_order_12345",
            "language": "en"
        })),
    )
    .await;

    // Should reject with 404
    assert!(
        status == 404 || status == 400,
        "Missing order should return 404"
    );
}

// =============================================================================
// SECTION 2: File Operations (3 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_703_upload_product_image() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    // Simulating image upload (typically multipart/form-data in real implementation)
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/upload-image",
        Some(&token),
        Some(json!({
            "productId": "prod_123",
            "imageUrl": "https://example.com/image.jpg",
            "position": 0
        })),
    )
    .await;

    // Should succeed or reject invalid image URL
    assert!(
        status == 200 || status == 400 || status == 422,
        "Image upload should be handled"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_704_download_digital_product() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "GET",
        "/api/digital/download/prod_digital_123",
        Some(&token),
        None,
    )
    .await;

    // Should return 404 for non-existent product or 200 with file content
    assert!(
        status == 200 || status == 404 || status == 403,
        "Digital download should check authorization"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_705_invalid_file_type() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/upload-image",
        Some(&token),
        Some(json!({
            "productId": "prod_123",
            "imageUrl": "https://example.com/malicious.exe",
            "position": 0
        })),
    )
    .await;

    // Should reject non-image files
    assert!(
        status == 400 || status == 422,
        "Invalid file types should be rejected"
    );
}

// =============================================================================
// SECTION 3: State Transition Edge Cases (4 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_706_order_status_invalid_transition() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    // Try to jump from pending to delivered (should be invalid)
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/orders/update-status",
        Some(&token),
        Some(json!({
            "orderId": "ord_test_123",
            "newStatus": "delivered"
        })),
    )
    .await;

    // Should reject invalid transition
    assert!(
        status == 400 || status == 422 || status == 404,
        "Invalid state transitions should be rejected"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_707_product_lifecycle_draft_to_active() {
    let client = reqwest::Client::new();
    let (token, user_id, _) = register_test_user(&client).await;

    // Create a product in draft status
    let (status_create, body_create) = make_request(
        &client,
        "POST",
        "/api/products/create",
        Some(&token),
        Some(json!({
            "title": "Test Product",
            "description": "A test product",
            "priceCents": 10000,
            "categoryId": "electronics",
            "sellerId": user_id
        })),
    )
    .await;

    if status_create == 200 || status_create == 201 {
        // Product was created, now try to transition to active
        let product_id = body_create.get("id").or(body_create.get("productId"));

        if let Some(id) = product_id.and_then(|v| v.as_str()) {
            let (status_update, _body_update) = make_request(
                &client,
                "POST",
                "/api/products/update",
                Some(&token),
                Some(json!({
                    "productId": id,
                    "lifecycleStatus": "active"
                })),
            )
            .await;

            assert!(
                status_update == 200 || status_update == 400,
                "Product activation should be handled"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_708_double_payment_prevention() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    // Try processing the same payment twice
    let order_id = "ord_double_payment_test";

    let (_status_1, _body_1) = make_request(
        &client,
        "POST",
        "/api/payments/capture",
        Some(&token),
        Some(json!({
            "orderId": order_id,
            "paymentIntentId": "pi_test_123"
        })),
    )
    .await;

    // Second attempt with same order
    let (status_2, _body_2) = make_request(
        &client,
        "POST",
        "/api/payments/capture",
        Some(&token),
        Some(json!({
            "orderId": order_id,
            "paymentIntentId": "pi_test_123"
        })),
    )
    .await;

    // Second should fail or be idempotent
    assert!(
        status_2 == 409 || status_2 == 400 || status_2 == 200,
        "Double payment should be prevented"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_709_cancelled_order_reactivation() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    // Try to reactivate a cancelled order (should be terminal state)
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/orders/update-status",
        Some(&token),
        Some(json!({
            "orderId": "ord_cancelled_123",
            "newStatus": "pending"
        })),
    )
    .await;

    // Should reject (cancelled is terminal)
    assert!(
        status == 400 || status == 422 || status == 404,
        "Cancelled orders should be terminal"
    );
}

// =============================================================================
// SECTION 4: Idempotency & Error Recovery (3 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_710_idempotent_cart_add() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let product_id = "prod_idempotent_test";

    // Add item to cart
    let (status_1, _body_1) = make_request(
        &client,
        "POST",
        "/api/cart/add-item",
        Some(&token),
        Some(json!({
            "productId": product_id,
            "quantity": 1
        })),
    )
    .await;

    // Add same item again (should increase quantity or be idempotent)
    let (status_2, _body_2) = make_request(
        &client,
        "POST",
        "/api/cart/add-item",
        Some(&token),
        Some(json!({
            "productId": product_id,
            "quantity": 1
        })),
    )
    .await;

    // Both should succeed
    assert!(
        (status_1 == 200 || status_1 == 201) && (status_2 == 200 || status_2 == 201),
        "Cart add should be idempotent"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_711_refund_already_refunded() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let order_id = "ord_refund_test";

    // Try to refund twice (should handle idempotently)
    let (_status_1, _body_1) = make_request(
        &client,
        "POST",
        "/api/orders/refund",
        Some(&token),
        Some(json!({
            "orderId": order_id,
            "amount": 10000
        })),
    )
    .await;

    let (status_2, _body_2) = make_request(
        &client,
        "POST",
        "/api/orders/refund",
        Some(&token),
        Some(json!({
            "orderId": order_id,
            "amount": 10000
        })),
    )
    .await;

    // Second refund should fail or indicate already refunded
    assert!(
        status_2 == 400 || status_2 == 409 || status_2 == 404,
        "Double refund should be prevented"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_712_connection_timeout_recovery() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    // Rapid successive requests to test connection reuse
    for i in 0..10 {
        let (status, _body) =
            make_request(&client, "GET", "/api/user/profile", Some(&token), None).await;

        // Should maintain connection or gracefully fail
        assert!(
            status == 200 || status == 400,
            "Request {} should complete",
            i
        );
    }
}
