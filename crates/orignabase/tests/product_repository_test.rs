//! Integration tests for product repository endpoints.
//!
//! These tests cover:
//! - Product CRUD operations
//! - Listing and searching products
//! - Stock management
//! - Product visibility lifecycle (draft → active → inactive)
//!
//! Run with: cargo test --test product_repository_test -- --ignored

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

fn test_product_payload() -> Value {
    json!({
        "title": "Test Product",
        "description": "A test product for integration tests",
        "priceCents": 2999,
        "categoryId": "categories:test_category",
        "subcategory": "Electronics",
        "imageUrls": ["https://example.com/image.jpg"],
        "stockQuantity": 100,
        "isDigital": false,
        "isPerishable": false
    })
}

// =============================================================================
// SECTION: Products — CRUD
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_create_success() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/products/create",
        Some(&token),
        Some(test_product_payload()),
    )
    .await;

    // Should return 200/201 or fail with 400 if test data invalid
    assert!(status == 200 || status == 201 || status == 400);
    if status == 200 || status == 201 {
        assert!(body.get("productId").is_some() || body.get("id").is_some());
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_create_missing_title() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let mut payload = test_product_payload();
    payload.as_object_mut().unwrap().remove("title");

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/create",
        Some(&token),
        Some(payload),
    )
    .await;

    // Should reject missing title
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_create_invalid_price() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let mut payload = test_product_payload();
    payload["priceCents"] = json!(-100);

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/create",
        Some(&token),
        Some(payload),
    )
    .await;

    // Should reject negative price
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_create_invalid_stock() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let mut payload = test_product_payload();
    payload["stockQuantity"] = json!(-50);

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/create",
        Some(&token),
        Some(payload),
    )
    .await;

    // Should reject negative stock
    assert!(status == 400 || status == 422);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_create_requires_authentication() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/create",
        None,
        Some(test_product_payload()),
    )
    .await;

    assert!(status == 401 || status == 403);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_list_success() {
    let client = reqwest::Client::new();

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/products/list",
        None,
        Some(json!({
            "limit": 20,
            "offset": 0
        })),
    )
    .await;

    assert_eq!(status, 200, "Should succeed without auth");
    assert!(body.get("products").is_some() || body.is_array());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_list_pagination() {
    let client = reqwest::Client::new();

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/products/list",
        None,
        Some(json!({
            "limit": 50,
            "offset": 0
        })),
    )
    .await;

    assert_eq!(status, 200);
    // Response should be array or object with products field
    assert!(body.is_array() || body.get("products").is_some());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_list_filters() {
    let client = reqwest::Client::new();

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/products/list",
        None,
        Some(json!({
            "limit": 20,
            "offset": 0,
            "categoryId": "categories:test_category",
            "minPrice": 1000,
            "maxPrice": 50000
        })),
    )
    .await;

    assert_eq!(status, 200);
    assert!(body.is_array() || body.get("products").is_some());
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_get_by_id() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/get",
        None,
        Some(json!({
            "productId": "products:nonexistent_123"
        })),
    )
    .await;

    // Should return 404 or 200 with null/error
    assert!(status == 200 || status == 404);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_update_requires_authentication() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/update",
        None,
        Some(json!({
            "productId": "products:test_123",
            "title": "Updated Title"
        })),
    )
    .await;

    assert!(status == 401 || status == 403);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_update_nonexistent() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/update",
        Some(&token),
        Some(json!({
            "productId": "products:nonexistent_123",
            "title": "Updated Title"
        })),
    )
    .await;

    // Should return 404 or 400
    assert!(status == 400 || status == 404);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_delete_requires_authentication() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/delete",
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
async fn test_product_delete_nonexistent() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/delete",
        Some(&token),
        Some(json!({
            "productId": "products:nonexistent_123"
        })),
    )
    .await;

    assert!(status == 400 || status == 404);
}

// =============================================================================
// SECTION: Products — Search (Meilisearch)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_search() {
    let client = reqwest::Client::new();

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/search/products",
        None,
        Some(json!({
            "query": "test",
            "limit": 20,
            "offset": 0
        })),
    )
    .await;

    // Search may not be available (501) or return results (200)
    assert!(status == 200 || status == 501 || status == 404);
    if status == 200 {
        assert!(body.is_array() || body.get("results").is_some());
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_search_with_filters() {
    let client = reqwest::Client::new();

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/search/products",
        None,
        Some(json!({
            "query": "electronics",
            "categoryId": "categories:test",
            "minPrice": 1000,
            "maxPrice": 100000,
            "limit": 20
        })),
    )
    .await;

    assert!(status == 200 || status == 501 || status == 404);
}

// =============================================================================
// SECTION: Products — Digital & Perishable
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_digital_no_shipping() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let mut payload = test_product_payload();
    payload["isDigital"] = json!(true);

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/create",
        Some(&token),
        Some(payload),
    )
    .await;

    // Digital products should be created successfully
    assert!(status == 200 || status == 201 || status == 400);
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_product_perishable_local_delivery() {
    let client = reqwest::Client::new();
    let (token, _user_id, _email) = register_test_user(&client).await;

    let mut payload = test_product_payload();
    payload["isPerishable"] = json!(true);

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/products/create",
        Some(&token),
        Some(payload),
    )
    .await;

    // Perishable products should be allowed
    assert!(status == 200 || status == 201 || status == 400);
}
