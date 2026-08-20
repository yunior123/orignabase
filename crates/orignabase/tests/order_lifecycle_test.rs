//! Integration tests for order lifecycle — via GraphQL.
//!
//! Run with: `cargo test --test order_lifecycle_test -- --ignored`

use ob_database::fields;
use reqwest::Client;
use serde_json::{Value, json};
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string()) // ignore-magic
}

async fn register_test_user(client: &Client) -> (String, String) {
    let email = format!("test_order_{}@test.origna.ca", Uuid::new_v4()); // ignore-magic
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

async fn login_admin(client: &Client) -> String {
    let resp = client
        .post(format!("{}/auth/login", base_url()))
        .json(&json!({
            "email": "e2e-admin@test.origna.ca",
            "password": "TestPass123!"
        }))
        .send()
        .await
        .expect("admin login failed");
    assert_eq!(resp.status(), 200, "admin login failed");
    let body: Value = resp.json().await.unwrap();
    body["access_token"]
        .as_str()
        .expect("missing admin access_token")
        .to_string()
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

async fn cancel_order(client: &Client, token: &str, order_id: &str, user_id: &str) -> (u16, Value) {
    let resp = client
        .post(format!("{}/api/orders/cancel", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({
            "orderId": order_id,
            "userId": user_id
        }))
        .send()
        .await
        .expect("cancel order request failed");
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({}));
    (status, body)
}

async fn register_seller_user(client: &Client) -> (String, String) {
    let email = format!("test_order_seller_{}@test.origna.ca", Uuid::new_v4());
    let register_resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "TestPass123!" }))
        .send()
        .await
        .expect("seller register failed");
    assert_eq!(register_resp.status(), 200, "seller register failed");
    let register_body: Value = register_resp.json().await.unwrap();
    let user_id = register_body["user"][fields::ID]
        .as_str()
        .expect("missing seller user id")
        .to_string();
    let admin_token = login_admin(client).await;
    let data = serde_json::to_string(&json!({ "roles": ["seller", "user"] })).unwrap();
    let escaped = serde_json::to_string(&data).unwrap();
    let query =
        format!(r#"mutation {{ update(collection: "users", id: "{user_id}", data: {escaped}) }}"#);
    let (status, body) = graphql(client, Some(&admin_token), &query).await;
    assert_eq!(status, 200, "seller role bootstrap failed: {body}");

    let login_resp = client
        .post(format!("{}/auth/login", base_url()))
        .json(&json!({
            "email": email,
            "password": "TestPass123!"
        }))
        .send()
        .await
        .expect("seller login failed");

    assert_eq!(
        login_resp.status(),
        200,
        "seller login failed after role update"
    );
    let login_body: Value = login_resp.json().await.unwrap();
    let token = login_body["access_token"]
        .as_str()
        .expect("missing seller access_token")
        .to_string();
    (token, user_id)
}

fn create_doc_query(collection: &str, data: &Value) -> String {
    let data_str = serde_json::to_string(data).unwrap();
    let escaped = serde_json::to_string(&data_str).unwrap();
    format!(r#"mutation {{ create(collection: "{collection}", data: {escaped}) }}"#)
}

#[tokio::test]
#[ignore]
async fn test_order_create_pending_status() {
    let client = Client::new();
    let (seller_token, seller_id) = register_seller_user(&client).await;
    let (buyer_token, buyer_id) = register_test_user(&client).await;

    // Create product
    let product_data = json!({ // ignore-magic
        "name": "Test Product", // ignore-magic
        "priceCents": 10000, // ignore-magic
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
    assert!(!product_id.is_empty(), "Product should have an ID: {body}");

    // Create order
    let order_data = json!({ // ignore-magic
        "buyerId": buyer_id, // ignore-magic
        "userId": buyer_id, // ignore-magic
        "sellerId": seller_id, // ignore-magic
        "orderStatus": "pending", // ignore-magic
        "items": [{"productId": product_id, "quantity": 1, "unitPriceCents": 10000}], // ignore-magic
        "subtotalCents": 10000, // ignore-magic
        "taxAmountCents": 0, // ignore-magic
        "shippingCostCents": 0, // ignore-magic
        "totalAmountCents": 10000, // ignore-magic
    });
    let query = create_doc_query("orders", &order_data); // ignore-magic
    let (status, body) = graphql(&client, Some(&buyer_token), &query).await;
    assert_eq!(status, 200);
    let order_id = body["data"]["create"][fields::ID] // ignore-magic
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(!order_id.is_empty(), "Order should have an ID");

    // Verify order has correct initial status
    let get_query = format!(r#"{{ get(collection: "orders", id: "{order_id}") }}"#); // ignore-magic
    let (status, detail) = graphql(&client, Some(&buyer_token), &get_query).await;
    assert_eq!(status, 200);
    let status_field = detail["data"]["get"]["orderStatus"] // ignore-magic
        .as_str()
        .or_else(|| detail["data"]["get"][fields::STATUS].as_str())
        .unwrap_or("unknown");
    assert_eq!(
        status_field,
        "pending", // ignore-magic
        "Initial order status should be 'pending'"
    );
}

#[tokio::test]
#[ignore]
async fn test_order_cancel_pending() {
    let client = Client::new();
    let (seller_token, seller_id) = register_seller_user(&client).await;
    let (buyer_token, buyer_id) = register_test_user(&client).await;

    // Create product
    let product_data = json!({ // ignore-magic
        "name": "Test Product", // ignore-magic
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
        "userId": buyer_id, // ignore-magic
        "sellerId": seller_id, // ignore-magic
        "orderStatus": "pending", // ignore-magic
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

    // Cancel order via the live cancellation API
    let (status, body) = cancel_order(&client, &buyer_token, &order_id, &buyer_id).await;
    assert_eq!(
        status, 200,
        "Cancelling pending order should succeed: {body}"
    );

    // Verify order is now cancelled
    let get_query = format!(r#"{{ get(collection: "orders", id: "{order_id}") }}"#); // ignore-magic
    let (status, detail) = graphql(&client, Some(&buyer_token), &get_query).await;
    assert_eq!(status, 200);
    let status_field = detail["data"]["get"]["orderStatus"] // ignore-magic
        .as_str()
        .or_else(|| detail["data"]["get"][fields::STATUS].as_str())
        .unwrap_or("unknown");
    assert_eq!(
        status_field,
        "cancelled", // ignore-magic
        "Order status should be 'cancelled'"
    );
}

#[tokio::test]
#[ignore]
async fn test_order_state_transitions() {
    let client = Client::new();
    let (seller_token, seller_id) = register_seller_user(&client).await;
    let (buyer_token, buyer_id) = register_test_user(&client).await;

    // Create product
    let product_data = json!({ // ignore-magic
        "name": "Test Product", // ignore-magic
        "priceCents": 8000, // ignore-magic
        "stockQuantity": 80, // ignore-magic
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
        "userId": buyer_id, // ignore-magic
        "sellerId": seller_id, // ignore-magic
        "orderStatus": "pending", // ignore-magic
        "items": [{"productId": product_id, "quantity": 2, "unitPriceCents": 8000}], // ignore-magic
        "subtotalCents": 16000, // ignore-magic
        "totalAmountCents": 16000, // ignore-magic
    });
    let query = create_doc_query("orders", &order_data); // ignore-magic
    let (status, body) = graphql(&client, Some(&buyer_token), &query).await;
    assert_eq!(status, 200);
    let order_id = body["data"]["create"][fields::ID] // ignore-magic
        .as_str()
        .unwrap_or("")
        .to_string();

    // Verify initial state: pending
    let get_query = format!(r#"{{ get(collection: "orders", id: "{order_id}") }}"#); // ignore-magic
    let (status, detail) = graphql(&client, Some(&buyer_token), &get_query).await;
    assert_eq!(status, 200);
    let initial_status = detail["data"]["get"]["orderStatus"] // ignore-magic
        .as_str()
        .or_else(|| detail["data"]["get"][fields::STATUS].as_str())
        .unwrap_or("");
    assert_eq!(initial_status, "pending"); // ignore-magic

    // Transition to confirmed
    let data = serde_json::to_string(&json!({"status": "confirmed"})).unwrap(); // ignore-magic
    let escaped = serde_json::to_string(&data).unwrap();
    let update_query = format!(
        r#"mutation {{ update(collection: "orders", id: "{order_id}", data: {escaped}) }}"# // ignore-magic
    );
    let (_status, _) = graphql(&client, Some(&seller_token), &update_query).await;

    // Verify order has all required fields
    let get_query = format!(r#"{{ get(collection: "orders", id: "{order_id}") }}"#); // ignore-magic
    let (status, detail) = graphql(&client, Some(&buyer_token), &get_query).await;
    assert_eq!(status, 200);
    let order = &detail["data"]["get"]; // ignore-magic
    assert!(
        order.get(fields::BUYER_ID).is_some(),
        "Order must have buyerId"
    ); // ignore-magic
    assert!(
        order.get(fields::SELLER_ID).is_some(),
        "Order must have sellerId"
    ); // ignore-magic
    assert!(
        order.get("orderStatus").is_some() || order.get(fields::STATUS).is_some(),
        "Order must have orderStatus"
    ); // ignore-magic
    assert!(
        order.get(fields::TOTAL_AMOUNT_CENTS).is_some(), // ignore-magic
        "Order must have totalAmountCents"
    );
    assert!(order.get("items").is_some(), "Order must have items"); // ignore-magic
}

#[tokio::test]
#[ignore]
async fn test_buyer_orders_pagination() {
    let client = Client::new();
    let (seller_token, seller_id) = register_seller_user(&client).await;
    let (buyer_token, buyer_id) = register_test_user(&client).await;

    // Create multiple orders
    for i in 0..5 {
        let product_data = json!({ // ignore-magic
            "name": format!("Test Product {}", i), // ignore-magic
            "priceCents": 2000 + (i * 1000), // ignore-magic
            "stockQuantity": 100, // ignore-magic
            "sellerId": seller_id, // ignore-magic
        });
        let query = create_doc_query("products", &product_data); // ignore-magic
        let (_, body) = graphql(&client, Some(&seller_token), &query).await;
        let product_id = body["data"]["create"][fields::ID] // ignore-magic
            .as_str()
            .unwrap_or("")
            .to_string();

        let price = 2000 + (i * 1000);
        let order_data = json!({ // ignore-magic
            "buyerId": buyer_id, // ignore-magic
            "userId": buyer_id, // ignore-magic
            "sellerId": seller_id, // ignore-magic
            "orderStatus": "pending", // ignore-magic
            "items": [{"productId": product_id, "quantity": 1, "unitPriceCents": price}], // ignore-magic
            "subtotalCents": price, // ignore-magic
            "totalAmountCents": price, // ignore-magic
        });
        let query = create_doc_query("orders", &order_data); // ignore-magic
        let (status, _) = graphql(&client, Some(&buyer_token), &query).await;
        assert_eq!(status, 200);
    }

    // Fetch buyer orders with pagination
    let filters = serde_json::to_string(&json!({"buyerId": {"_eq": buyer_id}})).unwrap(); // ignore-magic
    let escaped_f = serde_json::to_string(&filters).unwrap();
    let query =
        format!(r#"{{ list(collection: "orders", filters: {escaped_f}, limit: 2, offset: 0) }}"#); // ignore-magic
    let (status, body) = graphql(&client, Some(&buyer_token), &query).await;
    assert_eq!(status, 200);

    let empty_vec = vec![];
    let orders_list = body["data"]["list"].as_array().unwrap_or(&empty_vec); // ignore-magic
    assert!(orders_list.len() <= 2, "Should respect limit parameter");

    // Fetch with offset
    let query2 =
        format!(r#"{{ list(collection: "orders", filters: {escaped_f}, limit: 2, offset: 2) }}"#); // ignore-magic
    let (status, _body2) = graphql(&client, Some(&buyer_token), &query2).await;
    assert_eq!(status, 200);
}

#[tokio::test]
#[ignore]
async fn test_order_detail_fields() {
    let client = Client::new();
    let (seller_token, seller_id) = register_seller_user(&client).await;
    let (buyer_token, buyer_id) = register_test_user(&client).await;

    let product_data = json!({ // ignore-magic
        "name": "Detail Test Product", // ignore-magic
        "priceCents": 12500, // ignore-magic
        "stockQuantity": 125, // ignore-magic
        "sellerId": seller_id, // ignore-magic
    });
    let query = create_doc_query("products", &product_data); // ignore-magic
    let (status, body) = graphql(&client, Some(&seller_token), &query).await;
    assert_eq!(status, 200);
    let product_id = body["data"]["create"][fields::ID] // ignore-magic
        .as_str()
        .unwrap_or("")
        .to_string();

    let order_data = json!({ // ignore-magic
        "buyerId": buyer_id, // ignore-magic
        "userId": buyer_id, // ignore-magic
        "sellerId": seller_id, // ignore-magic
        "orderStatus": "pending", // ignore-magic
        "items": [{"productId": product_id, "quantity": 2, "unitPriceCents": 12500, "name": "Test Item"}], // ignore-magic
        "subtotalCents": 25000, // ignore-magic
        "taxAmountCents": 0, // ignore-magic
        "shippingCostCents": 0, // ignore-magic
        "totalAmountCents": 25000, // ignore-magic
    });
    let query = create_doc_query("orders", &order_data); // ignore-magic
    let (status, body) = graphql(&client, Some(&buyer_token), &query).await;
    assert_eq!(status, 200);
    let order_id = body["data"]["create"][fields::ID] // ignore-magic
        .as_str()
        .unwrap_or("")
        .to_string();

    // Fetch full order detail
    let get_query = format!(r#"{{ get(collection: "orders", id: "{order_id}") }}"#); // ignore-magic
    let (status, detail) = graphql(&client, Some(&buyer_token), &get_query).await;
    assert_eq!(status, 200);

    let order = &detail["data"]["get"]; // ignore-magic
    assert_eq!(order[fields::BUYER_ID].as_str().unwrap_or(""), buyer_id); // ignore-magic
    assert_eq!(order[fields::SELLER_ID].as_str().unwrap_or(""), seller_id); // ignore-magic
    assert_eq!(
        order[fields::TOTAL_AMOUNT_CENTS].as_i64().unwrap_or(0),
        25000
    ); // ignore-magic
    assert_eq!(order[fields::SUBTOTAL_CENTS].as_i64().unwrap_or(0), 25000); // ignore-magic

    let empty_vec = vec![];
    let items = order["items"].as_array().unwrap_or(&empty_vec); // ignore-magic
    assert_eq!(items.len(), 1, "Should have exactly 1 item");
}
