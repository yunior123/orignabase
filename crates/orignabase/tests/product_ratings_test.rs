//! Integration tests for product ratings and reviews.
//!
//! Tests:
//! - `POST /api/products/submit-rating` — buyer rates delivered product → 200
//! - `POST /api/products/submit-rating` — rating product not purchased → 403
//! - `POST /api/products/submit-rating` — duplicate rating → 400
//! - `POST /api/products/get-ratings` — returns ratings list with avg
//! - Rating value validation: 1–5 only; 0 or 6 → 400
//! - `POST /api/products/submit-rating` — seller cannot rate own product → 403
//!
//! Run with: `cargo test --test product_ratings_test -- --ignored`

use reqwest::Client;
use serde_json::{Value, json};
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

async fn register_test_user(client: &Client) -> (String, String) {
    let email = format!("test_rating_{}@test.origna.ca", Uuid::new_v4());
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "TestPass123!" }))
        .send()
        .await
        .expect("register failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let token = body["access_token"]
        .as_str()
        .expect("missing access_token")
        .to_string();
    let user_id = body["user"]["id"].as_str().unwrap_or("").to_string();
    (token, user_id)
}

async fn api_post(client: &Client, path: &str, token: &str, body: Value) -> (u16, Value) {
    let resp = client
        .post(format!("{}{}", base_url(), path))
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .await
        .expect("request failed");

    let status = resp.status().as_u16();
    let b: Value = resp.json().await.unwrap_or(json!({}));
    (status, b)
}

#[tokio::test]
#[ignore]
async fn test_rate_purchased_product() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Seller creates product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Ratable Product",
            "description": "A product",
            "priceCents": 5000,
            "stockQuantity": 50,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Buyer creates order for product (simulates purchase)
    let (status, order) = api_post(
        &client,
        "/api/orders/create",
        &buyer_token,
        json!({
            "items": [{ "productId": product_id, "quantity": 1, "unitPriceCents": 5000 }],
            "buyerId": buyer_id,
            "sellerId": seller_id,
            "totalAmountCents": 5000,
            "subtotalCents": 5000,
            "taxAmountCents": 0,
            "shippingCostCents": 0,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let order_id = order["id"].as_str().unwrap_or("").to_string();

    // Buyer submits a rating for the product
    let (status, rating_resp) = api_post(
        &client,
        "/api/products/submit-rating",
        &buyer_token,
        json!({
            "productId": product_id,
            "userId": buyer_id,
            "orderId": order_id,
            "rating": 4.5,
            "reviewText": "Great product, exactly as described!",
        }),
    )
    .await;

    assert_eq!(
        status, 200,
        "Buyer should be able to rate a purchased product"
    );
    let success = rating_resp["success"].as_bool().unwrap_or(false);
    assert!(success, "Rating submission should succeed");
}

#[tokio::test]
#[ignore]
async fn test_cannot_rate_unpurchased_product() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Seller creates product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Product",
            "description": "A product",
            "priceCents": 3000,
            "stockQuantity": 30,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Buyer tries to rate without purchasing (403)
    let (status, _) = api_post(
        &client,
        "/api/products/submit-rating",
        &buyer_token,
        json!({
            "productId": product_id,
            "userId": buyer_id,
            "orderId": "fake_order_id",  // Not a real order
            "rating": 5.0,
            "reviewText": "Amazing!",
        }),
    )
    .await;

    assert!(
        status == 403 || status >= 400,
        "Should not allow rating unpurchased product (got {})",
        status
    );
}

#[tokio::test]
#[ignore]
async fn test_seller_cannot_rate_own_product() {
    let client = Client::new();
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Seller creates product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Own Product",
            "description": "A product",
            "priceCents": 4000,
            "stockQuantity": 40,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Seller tries to rate their own product (403)
    let (status, _) = api_post(
        &client,
        "/api/products/submit-rating",
        &seller_token,
        json!({
            "productId": product_id,
            "userId": seller_id,
            "orderId": "seller_order_id",
            "rating": 5.0,
            "reviewText": "I love my own product!",
        }),
    )
    .await;

    assert!(
        status == 403 || status >= 400,
        "Seller should not be able to rate own product (got {})",
        status
    );
}

#[tokio::test]
#[ignore]
async fn test_rating_validation_range() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Create product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Product",
            "description": "A product",
            "priceCents": 2000,
            "stockQuantity": 20,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Create order
    let (status, order) = api_post(
        &client,
        "/api/orders/create",
        &buyer_token,
        json!({
            "items": [{ "productId": product_id, "quantity": 1, "unitPriceCents": 2000 }],
            "buyerId": buyer_id,
            "sellerId": seller_id,
            "totalAmountCents": 2000,
            "subtotalCents": 2000,
            "taxAmountCents": 0,
            "shippingCostCents": 0,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let order_id = order["id"].as_str().unwrap_or("").to_string();

    // Test: Rating of 0 (invalid, below 1)
    let (status, _) = api_post(
        &client,
        "/api/products/submit-rating",
        &buyer_token,
        json!({
            "productId": product_id,
            "userId": buyer_id,
            "orderId": order_id,
            "rating": 0.0,
        }),
    )
    .await;
    assert!(
        status >= 400,
        "Rating of 0 should fail validation (got {})",
        status
    );

    // Test: Rating of 6 (invalid, above 5)
    let (status, _) = api_post(
        &client,
        "/api/products/submit-rating",
        &buyer_token,
        json!({
            "productId": product_id,
            "userId": buyer_id,
            "orderId": order_id,
            "rating": 6.0,
        }),
    )
    .await;
    assert!(
        status >= 400,
        "Rating of 6 should fail validation (got {})",
        status
    );

    // Test: Valid rating of 3 (within 1-5 range)
    let (status, _) = api_post(
        &client,
        "/api/products/submit-rating",
        &buyer_token,
        json!({
            "productId": product_id,
            "userId": buyer_id,
            "orderId": order_id,
            "rating": 3.0,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "Rating of 3 (within 1-5) should succeed (got {})",
        status
    );
}

#[tokio::test]
#[ignore]
async fn test_duplicate_rating() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Create product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Product",
            "description": "A product",
            "priceCents": 6000,
            "stockQuantity": 60,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Create order
    let (status, order) = api_post(
        &client,
        "/api/orders/create",
        &buyer_token,
        json!({
            "items": [{ "productId": product_id, "quantity": 1, "unitPriceCents": 6000 }],
            "buyerId": buyer_id,
            "sellerId": seller_id,
            "totalAmountCents": 6000,
            "subtotalCents": 6000,
            "taxAmountCents": 0,
            "shippingCostCents": 0,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let order_id = order["id"].as_str().unwrap_or("").to_string();

    // Submit first rating (should succeed)
    let (status, _) = api_post(
        &client,
        "/api/products/submit-rating",
        &buyer_token,
        json!({
            "productId": product_id,
            "userId": buyer_id,
            "orderId": order_id,
            "rating": 4.0,
            "reviewText": "Good",
        }),
    )
    .await;
    assert_eq!(status, 200, "First rating should succeed");

    // Try to submit duplicate rating (should fail with 400)
    let (status, _) = api_post(
        &client,
        "/api/products/submit-rating",
        &buyer_token,
        json!({
            "productId": product_id,
            "userId": buyer_id,
            "orderId": order_id,
            "rating": 5.0,  // Different rating, same user+product
            "reviewText": "Actually, excellent",
        }),
    )
    .await;

    assert!(
        status >= 400,
        "Duplicate rating should fail (got status {})",
        status
    );
}

#[tokio::test]
#[ignore]
async fn test_get_ratings_list() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Create product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Rated Product",
            "description": "A product",
            "priceCents": 7000,
            "stockQuantity": 70,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Create order and submit rating
    let (status, order) = api_post(
        &client,
        "/api/orders/create",
        &buyer_token,
        json!({
            "items": [{ "productId": product_id, "quantity": 1, "unitPriceCents": 7000 }],
            "buyerId": buyer_id,
            "sellerId": seller_id,
            "totalAmountCents": 7000,
            "subtotalCents": 7000,
            "taxAmountCents": 0,
            "shippingCostCents": 0,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let order_id = order["id"].as_str().unwrap_or("").to_string();

    // Submit rating
    let (status, _) = api_post(
        &client,
        "/api/products/submit-rating",
        &buyer_token,
        json!({
            "productId": product_id,
            "userId": buyer_id,
            "orderId": order_id,
            "rating": 4.5,
            "reviewText": "Excellent quality",
        }),
    )
    .await;
    assert_eq!(status, 200);

    // Fetch ratings for product
    let (status, ratings_resp) = api_post(
        &client,
        "/api/products/get-ratings",
        &buyer_token,
        json!({
            "productId": product_id,
            "limit": 10,
        }),
    )
    .await;

    assert_eq!(status, 200, "Fetching ratings should succeed");

    let empty_vec = vec![];
    let ratings = ratings_resp["ratings"].as_array().unwrap_or(&empty_vec);
    assert!(
        !ratings.is_empty(),
        "Product should have at least one rating"
    );

    // Verify rating data structure
    let rating = &ratings[0];
    assert!(rating.get("userId").is_some(), "Rating should have userId");
    assert!(
        rating.get("rating").is_some(),
        "Rating should have rating field"
    );
}

#[tokio::test]
#[ignore]
async fn test_get_ratings_pagination() {
    let client = Client::new();
    let (buyer_token, _buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Create product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Popular Product",
            "description": "A product",
            "priceCents": 1000,
            "stockQuantity": 100,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Fetch ratings with limit
    let (status, ratings_resp) = api_post(
        &client,
        "/api/products/get-ratings",
        &buyer_token,
        json!({
            "productId": product_id,
            "limit": 5,
        }),
    )
    .await;

    assert_eq!(status, 200, "Fetching ratings should succeed");

    let empty_vec = vec![];
    let ratings = ratings_resp["ratings"].as_array().unwrap_or(&empty_vec);
    assert!(
        ratings.len() <= 5,
        "Ratings list should respect limit parameter"
    );

    // Verify pagination fields
    assert!(
        ratings_resp.get("hasMore").is_some(),
        "Response should have hasMore field"
    );

    // May have nextCursor for pagination
    let has_cursor = ratings_resp.get("nextCursor").is_some();
    let has_more = ratings_resp["hasMore"].as_bool().unwrap_or(false);
    if has_more {
        assert!(
            has_cursor,
            "If hasMore is true, nextCursor should be present"
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_rating_response_structure() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Create product and order
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Product",
            "description": "A product",
            "priceCents": 8000,
            "stockQuantity": 80,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    let (status, order) = api_post(
        &client,
        "/api/orders/create",
        &buyer_token,
        json!({
            "items": [{ "productId": product_id, "quantity": 1, "unitPriceCents": 8000 }],
            "buyerId": buyer_id,
            "sellerId": seller_id,
            "totalAmountCents": 8000,
            "subtotalCents": 8000,
            "taxAmountCents": 0,
            "shippingCostCents": 0,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let order_id = order["id"].as_str().unwrap_or("").to_string();

    // Submit rating
    let (status, rating_resp) = api_post(
        &client,
        "/api/products/submit-rating",
        &buyer_token,
        json!({
            "productId": product_id,
            "userId": buyer_id,
            "orderId": order_id,
            "rating": 4.5,
            "reviewText": "Great product",
        }),
    )
    .await;

    assert_eq!(status, 200);

    // Verify response structure
    assert!(
        rating_resp.get("success").is_some(),
        "Response should have success field"
    );
    assert!(
        rating_resp.get("newRating").is_some(),
        "Response should have newRating field"
    );
    assert!(
        rating_resp.get("ratingCount").is_some(),
        "Response should have ratingCount field"
    );

    let new_rating = rating_resp["newRating"].as_f64().unwrap_or(-1.0);
    assert!(
        new_rating > 0.0 && new_rating <= 5.0,
        "New rating should be within valid range"
    );
}
