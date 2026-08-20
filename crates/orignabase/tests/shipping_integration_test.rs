//! Live integration tests for shipping calculation.
//!
//! Run with: `cd orignabase && cargo test --test shipping_integration_test -- --ignored`

use ob_database::fields;
use serde_json::{Value, json};
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "https://api.dev.orignagta.ca".to_string())
}

async fn register_test_user(client: &reqwest::Client, prefix: &str) -> (String, String) {
    let email = format!("{prefix}_{}@test.origna.ca", Uuid::new_v4());
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({
            "email": email,
            "password": "TestPass123!"
        }))
        .send()
        .await
        .expect("register failed");

    assert_eq!(resp.status(), 200, "register failed for {prefix}");
    let body: Value = resp.json().await.expect("parse register response");
    let token = body["access_token"]
        .as_str()
        .expect("missing access_token")
        .to_string();
    let user_id = body["user"][fields::ID]
        .as_str()
        .expect("missing user.id")
        .to_string();
    (token, user_id)
}

async fn login_admin(client: &reqwest::Client) -> String {
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
    let body: Value = resp.json().await.expect("parse admin login response");
    body["access_token"]
        .as_str()
        .expect("missing admin access_token")
        .to_string()
}

async fn graphql(client: &reqwest::Client, token: &str, query: &str) -> (u16, Value) {
    let resp = client
        .post(format!("{}/graphql", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "query": query }))
        .send()
        .await
        .expect("graphql request failed");

    let status = resp.status().as_u16();
    let body = resp.json::<Value>().await.unwrap_or_else(|_| json!({}));
    (status, body)
}

async fn provision_seller(client: &reqwest::Client) -> String {
    let (_seller_token, seller_id) = register_test_user(client, "shipping_seller").await;
    let admin_token = login_admin(client).await;
    let data = serde_json::to_string(&json!({
        "warehouseAddress": {
            "state": "ON",
            "latitude": 43.6532,
            "longitude": -79.3832
        }
    }))
    .expect("serialize seller update");
    let escaped = serde_json::to_string(&data).expect("escape seller update");
    let query = format!(
        r#"mutation {{ update(collection: "users", id: "{seller_id}", data: {escaped}) }}"#
    );
    let (status, body) = graphql(client, &admin_token, &query).await;
    assert_eq!(status, 200, "seller warehouse update failed: {body}");

    let verify_query = format!(r#"{{ get(collection: "users", id: "{seller_id}") }}"#);
    let (verify_status, verify_body) = graphql(client, &admin_token, &verify_query).await;
    assert_eq!(
        verify_status, 200,
        "seller verification failed: {verify_body}"
    );
    assert_eq!(
        verify_body["data"]["get"]["warehouseAddress"]["state"].as_str(),
        Some("ON"),
        "seller warehouse address was not persisted: {verify_body}"
    );
    seller_id
}

async fn calculate_shipping(
    client: &reqwest::Client,
    token: &str,
    items: Vec<Value>,
    buyer_address: Value,
    subtotal_cents: i64,
) -> Result<Value, String> {
    let resp = client
        .post(format!("{}/api/shipping/calculate", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({
            "items": items,
            "buyerAddress": buyer_address,
            "subtotalCents": subtotal_cents
        }))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let status = resp.status();
    let body_text = resp
        .text()
        .await
        .map_err(|e| format!("read response failed: {e}"))?;

    if status == 200 {
        serde_json::from_str::<Value>(&body_text)
            .map_err(|e| format!("parse success response failed: {e} — {body_text}"))
    } else {
        Err(format!("shipping calc failed: {status} — {body_text}"))
    }
}

#[tokio::test]
#[ignore]
async fn test_shipping_calculation_standard_delivery() {
    let client = reqwest::Client::new();
    let (buyer_token, _buyer_id) = register_test_user(&client, "shipping_buyer").await;
    let seller_id = provision_seller(&client).await;

    let items = vec![json!({
        "productId": "test-product-1",
        "sellerId": seller_id,
        "quantity": 1,
        "weightKg": 1.5,
        "isPerishable": false,
        "sellerAddress": {
            "state": "ON",
            "latitude": 43.6532,
            "longitude": -79.3832
        },
        "shipFromProvince": "ON"
    })];

    let buyer_address = json!({
        "state": "QC",
        "latitude": 45.5017,
        "longitude": -73.5673
    });

    let result = calculate_shipping(&client, &buyer_token, items, buyer_address, 5000)
        .await
        .expect("shipping calculation should succeed");

    assert_eq!(result["success"].as_bool(), Some(true));
    let shipping_cost_cents = result["totalCostCents"]
        .as_i64()
        .expect("missing totalCostCents");
    assert!(
        shipping_cost_cents > 0,
        "Cross-province shipping should have a cost"
    );
}

#[tokio::test]
#[ignore]
async fn test_perishable_rejects_cross_province() {
    let client = reqwest::Client::new();
    let (buyer_token, _buyer_id) = register_test_user(&client, "shipping_buyer").await;
    let seller_id = provision_seller(&client).await;

    let items = vec![json!({
        "productId": "test-perishable-1",
        "sellerId": seller_id,
        "quantity": 1,
        "weightKg": 0.5,
        "isPerishable": true,
        "sellerAddress": {
            "state": "ON",
            "latitude": 43.6532,
            "longitude": -79.3832
        },
        "shipFromProvince": "ON"
    })];

    let buyer_address = json!({
        "state": "QC",
        "latitude": 45.5017,
        "longitude": -73.5673
    });

    let error = calculate_shipping(&client, &buyer_token, items, buyer_address, 5000)
        .await
        .expect_err("cross-province perishables should be rejected");
    assert!(
        error.contains("422")
            || error.contains("Perishable items cannot be shipped across provinces"),
        "Should reject cross-province perishables: {error}"
    );
}

#[tokio::test]
#[ignore]
async fn test_free_shipping_threshold_75_cad() {
    let client = reqwest::Client::new();
    let (buyer_token, _buyer_id) = register_test_user(&client, "shipping_buyer").await;
    let seller_id = provision_seller(&client).await;

    let items = vec![json!({
        "productId": "test-product-expensive",
        "sellerId": seller_id,
        "quantity": 1,
        "weightKg": 2.0,
        "isPerishable": false,
        "sellerAddress": {
            "state": "ON",
            "latitude": 43.6532,
            "longitude": -79.3832
        },
        "shipFromProvince": "ON"
    })];

    let buyer_address = json!({
        "state": "ON",
        "latitude": 43.7001,
        "longitude": -79.4163
    });

    let result = calculate_shipping(&client, &buyer_token, items, buyer_address, 8000)
        .await
        .expect("free shipping request should succeed");

    let shipping_cost = result["totalCostCents"]
        .as_i64()
        .expect("missing totalCostCents");
    assert_eq!(
        shipping_cost, 0,
        "Shipping should be free for orders >= $75 CAD"
    );
}
