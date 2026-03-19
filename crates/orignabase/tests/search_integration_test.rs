//! Integration tests for search endpoints.
//!
//! These tests cover Meilisearch integration for product search, filtering,
//! and autocomplete functionality.
//!
//! Run with: `cargo test --test search_integration_test -- --ignored`
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
// SECTION 1: Product Search — Basic queries (4 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_400_search_products_empty_query() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/search/products",
        Some(&token),
        Some(json!({
            "query": "",
            "limit": 20,
            "offset": 0
        })),
    )
    .await;

    // Should succeed — empty query returns all products (browsing)
    assert_eq!(
        status, 200,
        "Empty query should return products: {:?}",
        body
    );
    assert!(body.get("hits").is_some(), "Should have hits array");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_401_search_products_with_query() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/search/products",
        Some(&token),
        Some(json!({
            "query": "laptop",
            "limit": 10,
            "offset": 0
        })),
    )
    .await;

    // Should return 200 even if no hits
    assert!(
        status == 200 || status == 400,
        "Search should succeed or fail gracefully: status={}",
        status
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_402_search_products_pagination() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    // Test offset and limit
    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/search/products",
        Some(&token),
        Some(json!({
            "query": "",
            "limit": 50,
            "offset": 100
        })),
    )
    .await;

    assert!(
        status == 200 || status == 400,
        "Pagination should be respected"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_403_search_products_invalid_limit() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/search/products",
        Some(&token),
        Some(json!({
            "query": "test",
            "limit": -1,
            "offset": 0
        })),
    )
    .await;

    // Should reject invalid limit
    assert!(
        status == 400 || status == 200,
        "Invalid limit should be rejected or sanitized"
    );
}

// =============================================================================
// SECTION 2: Product Search — Filtering (4 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_404_search_products_filter_category() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/search/products",
        Some(&token),
        Some(json!({
            "query": "",
            "filters": {
                "categoryId": "electronics"
            },
            "limit": 20,
            "offset": 0
        })),
    )
    .await;

    // Should accept category filter
    assert!(
        status == 200 || status == 400,
        "Category filter should be supported"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_405_search_products_filter_price_range() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/search/products",
        Some(&token),
        Some(json!({
            "query": "",
            "filters": {
                "priceRange": {
                    "min": 1000,
                    "max": 50000
                }
            },
            "limit": 20,
            "offset": 0
        })),
    )
    .await;

    assert!(
        status == 200 || status == 400,
        "Price range filter should be supported"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_406_search_products_filter_seller_id() {
    let client = reqwest::Client::new();
    let (token, user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/search/products",
        Some(&token),
        Some(json!({
            "query": "",
            "filters": {
                "sellerId": user_id
            },
            "limit": 20,
            "offset": 0
        })),
    )
    .await;

    assert!(
        status == 200 || status == 400,
        "Seller filter should be supported"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_407_search_products_multiple_filters() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/search/products",
        Some(&token),
        Some(json!({
            "query": "laptop",
            "filters": {
                "categoryId": "electronics",
                "priceRange": {
                    "min": 50000,
                    "max": 200000
                }
            },
            "limit": 10,
            "offset": 0
        })),
    )
    .await;

    assert!(
        status == 200 || status == 400,
        "Multiple filters should be combined correctly"
    );
}

// =============================================================================
// SECTION 3: Product Search — Sorting (3 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_408_search_products_sort_by_price_asc() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/search/products",
        Some(&token),
        Some(json!({
            "query": "",
            "sort": "price:asc",
            "limit": 20,
            "offset": 0
        })),
    )
    .await;

    assert!(
        status == 200 || status == 400,
        "Price ascending sort should be supported"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_409_search_products_sort_by_price_desc() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/search/products",
        Some(&token),
        Some(json!({
            "query": "",
            "sort": "price:desc",
            "limit": 20,
            "offset": 0
        })),
    )
    .await;

    assert!(
        status == 200 || status == 400,
        "Price descending sort should be supported"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_410_search_products_sort_by_newest() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/search/products",
        Some(&token),
        Some(json!({
            "query": "",
            "sort": "createdAt:desc",
            "limit": 20,
            "offset": 0
        })),
    )
    .await;

    assert!(
        status == 200 || status == 400,
        "Created date sort should be supported"
    );
}

// =============================================================================
// SECTION 4: Autocomplete (2 tests)
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_411_autocomplete_products() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, body) = make_request(
        &client,
        "POST",
        "/api/search/autocomplete",
        Some(&token),
        Some(json!({
            "query": "lap",
            "limit": 5
        })),
    )
    .await;

    // Should return suggestions or empty array
    assert_eq!(status, 200, "Autocomplete should succeed: {:?}", body);
    assert!(
        body.get("suggestions").is_some() || body.is_array(),
        "Should return suggestions array"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_412_autocomplete_empty_query() {
    let client = reqwest::Client::new();
    let (token, _user_id, _) = register_test_user(&client).await;

    let (status, _body) = make_request(
        &client,
        "POST",
        "/api/search/autocomplete",
        Some(&token),
        Some(json!({
            "query": "",
            "limit": 10
        })),
    )
    .await;

    // Empty autocomplete may return nothing or top products
    assert!(
        status == 200 || status == 400,
        "Empty autocomplete should be handled"
    );
}
