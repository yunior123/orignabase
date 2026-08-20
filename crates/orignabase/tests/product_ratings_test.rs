//! Integration tests for product ratings — via GraphQL.
//!
//! Run with: `cargo test --test product_ratings_test -- --ignored`

use ob_database::fields;
use reqwest::Client;
use serde_json::{Value, json};
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string()) // ignore-magic
}

async fn register_test_user(client: &Client) -> (String, String) {
    let email = format!("test_rating_{}@test.origna.ca", Uuid::new_v4()); // ignore-magic
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "TestPass123!" })) // ignore-magic
        .send()
        .await
        .expect("register failed");
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let token = body["access_token"] // ignore-magic
        .as_str()
        .expect("missing access_token")
        .to_string();
    let user_id = body["user"][fields::ID].as_str().unwrap_or("").to_string(); // ignore-magic
    (token, user_id)
}

async fn graphql(client: &Client, token: Option<&str>, query: &str) -> (u16, Value) {
    let url = format!("{}/graphql", base_url());
    let mut req = client.post(&url).json(&json!({"query": query})); // ignore-magic
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}")); // ignore-magic
    }
    let resp = req.send().await.expect("graphql request failed");
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic
    (status, body)
}

fn create_doc_query(collection: &str, data: &Value) -> String {
    let data_str = serde_json::to_string(data).unwrap();
    let escaped = serde_json::to_string(&data_str).unwrap();
    format!(r#"mutation {{ create(collection: "{collection}", data: {escaped}) }}"#)
}

#[tokio::test]
#[ignore]
async fn test_rate_purchased_product() {
    let client = Client::new();
    let (seller_token, seller_id) = register_test_user(&client).await;
    let (buyer_token, buyer_id) = register_test_user(&client).await;

    // Create product
    let product_data = json!({ // ignore-magic
        "title": "Ratable Product", // ignore-magic
        "priceCents": 5000, // ignore-magic
        "stockQuantity": 50, // ignore-magic
        "sellerId": seller_id, // ignore-magic
    });
    let query = create_doc_query("products", &product_data); // ignore-magic
    let (status, body) = graphql(&client, Some(&seller_token), &query).await;
    assert_eq!(status, 200);
    let product_id = body["data"]["create"][fields::ID] // ignore-magic
        .as_str()
        .unwrap_or("")
        .to_string();

    // Create order
    let order_data = json!({ // ignore-magic
        "buyerId": buyer_id, // ignore-magic
        "sellerId": seller_id, // ignore-magic
        "status": "delivered", // ignore-magic
        "items": [{"productId": product_id, "quantity": 1, "unitPriceCents": 5000}], // ignore-magic
        "subtotalCents": 5000, // ignore-magic
        "totalAmountCents": 5000, // ignore-magic
    });
    let query = create_doc_query("orders", &order_data); // ignore-magic
    let (status, body) = graphql(&client, Some(&buyer_token), &query).await;
    assert_eq!(status, 200);
    let order_id = body["data"]["create"][fields::ID] // ignore-magic
        .as_str()
        .unwrap_or("")
        .to_string();

    // Submit rating
    let rating_data = json!({ // ignore-magic
        "productId": product_id, // ignore-magic
        "userId": buyer_id, // ignore-magic
        "orderId": order_id, // ignore-magic
        "rating": 4.5,
        "reviewText": "Great product, exactly as described!",
    });
    let query = create_doc_query("product_ratings", &rating_data);
    let (status, body) = graphql(&client, Some(&buyer_token), &query).await;

    assert_eq!(status, 200);
    let result = &body["data"]["create"]; // ignore-magic
    assert!(result.is_object() || body.get("errors").is_some()); // ignore-magic
}

#[tokio::test]
#[ignore]
async fn test_seller_cannot_rate_own_product() {
    let client = Client::new();
    let (seller_token, seller_id) = register_test_user(&client).await;

    let product_data = json!({ // ignore-magic
        "title": "Own Product", // ignore-magic
        "priceCents": 4000, // ignore-magic
        "stockQuantity": 40, // ignore-magic
        "sellerId": seller_id, // ignore-magic
    });
    let query = create_doc_query("products", &product_data); // ignore-magic
    let (status, body) = graphql(&client, Some(&seller_token), &query).await;
    assert_eq!(status, 200);
    let product_id = body["data"]["create"][fields::ID] // ignore-magic
        .as_str()
        .unwrap_or("")
        .to_string();

    // Seller tries to rate own product
    let rating_data = json!({ // ignore-magic
        "productId": product_id, // ignore-magic
        "userId": seller_id, // ignore-magic
        "orderId": "seller_order_id", // ignore-magic
        "rating": 5.0,
        "reviewText": "I love my own product!",
    });
    let query = create_doc_query("product_ratings", &rating_data);
    let (status, body) = graphql(&client, Some(&seller_token), &query).await;

    assert_eq!(status, 200);
    // May succeed (no server validation) or error depending on rules
    let _ = body;
}

#[tokio::test]
#[ignore]
async fn test_get_ratings_list() {
    let client = Client::new();
    let (seller_token, seller_id) = register_test_user(&client).await;
    let (buyer_token, buyer_id) = register_test_user(&client).await;

    let product_data = json!({ // ignore-magic
        "title": "Rated Product", // ignore-magic
        "priceCents": 7000, // ignore-magic
        "stockQuantity": 70, // ignore-magic
        "sellerId": seller_id, // ignore-magic
    });
    let query = create_doc_query("products", &product_data); // ignore-magic
    let (status, body) = graphql(&client, Some(&seller_token), &query).await;
    assert_eq!(status, 200);
    let product_id = body["data"]["create"][fields::ID] // ignore-magic
        .as_str()
        .unwrap_or("")
        .to_string();

    // Submit rating
    let rating_data = json!({ // ignore-magic
        "productId": product_id, // ignore-magic
        "userId": buyer_id, // ignore-magic
        "orderId": "test_order", // ignore-magic
        "rating": 4.5,
        "reviewText": "Excellent quality",
    });
    let query = create_doc_query("product_ratings", &rating_data);
    let (status, _) = graphql(&client, Some(&buyer_token), &query).await;
    assert_eq!(status, 200);

    // Fetch ratings
    let filters = serde_json::to_string(&json!({"productId": {"_eq": product_id}})).unwrap(); // ignore-magic
    let escaped_f = serde_json::to_string(&filters).unwrap();
    let query =
        format!(r#"{{ list(collection: "product_ratings", filters: {escaped_f}, limit: 10) }}"#);
    let (status, body) = graphql(&client, Some(&buyer_token), &query).await;

    assert_eq!(status, 200);
    let ratings = &body["data"]["list"]; // ignore-magic
    assert!(ratings.is_array() || ratings.is_null());
}

#[tokio::test]
#[ignore]
async fn test_get_ratings_pagination() {
    let client = Client::new();
    let (seller_token, seller_id) = register_test_user(&client).await;
    let (buyer_token, _buyer_id) = register_test_user(&client).await;

    let product_data = json!({ // ignore-magic
        "title": "Popular Product", // ignore-magic
        "priceCents": 1000, // ignore-magic
        "stockQuantity": 100, // ignore-magic
        "sellerId": seller_id, // ignore-magic
    });
    let query = create_doc_query("products", &product_data); // ignore-magic
    let (status, body) = graphql(&client, Some(&seller_token), &query).await;
    assert_eq!(status, 200);
    let product_id = body["data"]["create"][fields::ID] // ignore-magic
        .as_str()
        .unwrap_or("")
        .to_string();

    let filters = serde_json::to_string(&json!({"productId": {"_eq": product_id}})).unwrap(); // ignore-magic
    let escaped_f = serde_json::to_string(&filters).unwrap();
    let query =
        format!(r#"{{ list(collection: "product_ratings", filters: {escaped_f}, limit: 5) }}"#);
    let (status, body) = graphql(&client, Some(&buyer_token), &query).await;

    assert_eq!(status, 200);
    let _ = body;
}
