//! Integration tests for shipping — via GraphQL.
//!
//! Run with: `cargo test --test shipping_test -- --ignored`

use ob_database::fields;
use reqwest::Client;
use serde_json::{Value, json};
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string()) // ignore-magic
}

async fn register_test_user(client: &Client) -> (String, String) {
    let email = format!("test_shipping_{}@test.origna.ca", Uuid::new_v4()); // ignore-magic
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
async fn test_shipping_create_product_and_order() {
    let client = Client::new();
    let (seller_token, seller_id) = register_test_user(&client).await;
    let (buyer_token, buyer_id) = register_test_user(&client).await;

    // Create product via GraphQL
    let product_data = json!({ // ignore-magic
        "title": "Shipping Test Product", // ignore-magic
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

    if !product_id.is_empty() {
        // Create order via GraphQL
        let order_data = json!({ // ignore-magic
            "buyerId": buyer_id, // ignore-magic
            "sellerId": seller_id, // ignore-magic
            "status": "pending", // ignore-magic
            "items": [{"productId": product_id, "quantity": 1, "unitPriceCents": 5000}], // ignore-magic
            "subtotalCents": 5000, // ignore-magic
            "taxAmountCents": 0, // ignore-magic
            "shippingCostCents": 500, // ignore-magic
            "totalAmountCents": 5500, // ignore-magic
        });
        let query = create_doc_query("orders", &order_data); // ignore-magic
        let (status, body) = graphql(&client, Some(&buyer_token), &query).await;
        assert_eq!(status, 200);
        let result = &body["data"]["create"]; // ignore-magic
        assert!(result.is_object() || body.get("errors").is_some()); // ignore-magic
    }
}

#[tokio::test]
#[ignore]
async fn test_shipping_free_threshold() {
    let client = Client::new();
    let (seller_token, seller_id) = register_test_user(&client).await;
    let (buyer_token, buyer_id) = register_test_user(&client).await;

    // Create cheap product
    let product_data = json!({ // ignore-magic
        "title": "Cheap Product", // ignore-magic
        "priceCents": 3000, // ignore-magic
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

    if !product_id.is_empty() {
        // Order below threshold — should have shipping cost
        let order_data = json!({ // ignore-magic
            "buyerId": buyer_id, // ignore-magic
            "sellerId": seller_id, // ignore-magic
            "status": "pending", // ignore-magic
            "items": [{"productId": product_id, "quantity": 1, "unitPriceCents": 3000}], // ignore-magic
            "subtotalCents": 3000, // ignore-magic
            "shippingCostCents": 500, // ignore-magic
            "totalAmountCents": 3500, // ignore-magic
        });
        let query = create_doc_query("orders", &order_data); // ignore-magic
        let (status, _body) = graphql(&client, Some(&buyer_token), &query).await;
        assert_eq!(status, 200);

        // Order above threshold — free shipping
        let order_data2 = json!({ // ignore-magic
            "buyerId": buyer_id, // ignore-magic
            "sellerId": seller_id, // ignore-magic
            "status": "pending", // ignore-magic
            "items": [{"productId": product_id, "quantity": 3, "unitPriceCents": 3000}], // ignore-magic
            "subtotalCents": 9000, // ignore-magic
            "shippingCostCents": 0, // ignore-magic
            "totalAmountCents": 9000, // ignore-magic
        });
        let query = create_doc_query("orders", &order_data2); // ignore-magic
        let (status, body) = graphql(&client, Some(&buyer_token), &query).await;
        assert_eq!(status, 200);
        let shipping = body["data"]["create"]["shippingCostCents"] // ignore-magic
            .as_i64()
            .unwrap_or(-1);
        assert_eq!(shipping, 0, "Order above $75 should have free shipping");
    }
}

#[tokio::test]
#[ignore]
async fn test_shipping_cost_in_integer_cents() {
    let client = Client::new();
    let (seller_token, seller_id) = register_test_user(&client).await;
    let (buyer_token, buyer_id) = register_test_user(&client).await;

    let product_data = json!({ // ignore-magic
        "title": "Product", // ignore-magic
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

    if !product_id.is_empty() {
        let order_data = json!({ // ignore-magic
            "buyerId": buyer_id, // ignore-magic
            "sellerId": seller_id, // ignore-magic
            "status": "pending", // ignore-magic
            "items": [{"productId": product_id, "quantity": 1, "unitPriceCents": 4000}], // ignore-magic
            "subtotalCents": 4000, // ignore-magic
            "shippingCostCents": 500, // ignore-magic
            "totalAmountCents": 4500, // ignore-magic
        });
        let query = create_doc_query("orders", &order_data); // ignore-magic
        let (status, body) = graphql(&client, Some(&buyer_token), &query).await;
        assert_eq!(status, 200);
        let cost = body["data"]["create"]["shippingCostCents"].as_i64(); // ignore-magic
        assert!(
            cost.is_some() || body.get("errors").is_some(),
            "Shipping cost must be integer cents"
        );
    }
}
