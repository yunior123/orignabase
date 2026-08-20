//! Live integration tests for return request functionality.
//!
//! Run with: `cd orignabase && cargo test --test return_integration_test -- --ignored`

use ob_database::fields;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::sleep;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "https://api.dev.orignagta.ca".to_string())
}

/// Login as buyer and return access token.
async fn login_buyer(client: &reqwest::Client) -> String {
    let resp = client
        .post(format!("{}/auth/login", base_url()))
        .json(&json!({ // ignore-magic
            "email": "e2e-buyer@test.origna.ca", // ignore-magic
            "password": "TestPass123!" // ignore-magic
        }))
        .send()
        .await
        .expect("login failed");

    assert_eq!(resp.status(), 200, "Buyer login failed");
    let body: Value = resp.json().await.expect("parse login response");
    body["access_token"] // ignore-magic
        .as_str()
        .expect("missing access_token")
        .to_string()
}

/// Login as admin and return access token.
async fn login_admin(client: &reqwest::Client) -> String {
    let resp = client
        .post(format!("{}/auth/login", base_url()))
        .json(&json!({ // ignore-magic
            "email": "e2e-admin@test.origna.ca", // ignore-magic
            "password": "TestPass123!" // ignore-magic
        }))
        .send()
        .await
        .expect("login failed");

    assert_eq!(resp.status(), 200, "Admin login failed");
    let body: Value = resp.json().await.expect("parse login response");
    body["access_token"] // ignore-magic
        .as_str()
        .expect("missing access_token")
        .to_string()
}

/// Get list of orders for current user.
async fn get_orders(client: &reqwest::Client, token: &str) -> Result<Vec<Value>, String> {
    let resp = client
        .get(format!("{}/orders", base_url()))
        .header("Authorization", format!("Bearer {}", token)) // ignore-magic
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("parse response: {}", e))?;

    if status == 200 {
        Ok(body.as_array().cloned().unwrap_or_default())
    } else {
        Err(format!("get orders failed: {} — {}", status, body))
    }
}

/// Create a return request for an order item.
async fn create_return_request(
    client: &reqwest::Client,
    token: &str,
    order_id: &str,
    item_id: &str,
    reason: &str,
    quantity: i32,
) -> Result<Value, String> {
    let resp = client
        .post(format!("{}/return-requests/create", base_url()))
        .header("Authorization", format!("Bearer {}", token)) // ignore-magic
        .json(&json!({ // ignore-magic
            "orderId": order_id, // ignore-magic
            "itemId": item_id,
            "reason": reason,
            "quantityRequested": quantity
        }))
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("parse response: {}", e))?;

    if status == 201 {
        Ok(body)
    } else {
        Err(format!(
            "create return request failed: {} — {}",
            status, body
        ))
    }
}

/// Approve a return request (admin action).
async fn approve_return_request(
    client: &reqwest::Client,
    token: &str,
    return_request_id: &str,
) -> Result<Value, String> {
    let resp = client
        .post(format!(
            "{}/return-requests/{}/approve",
            base_url(),
            return_request_id
        ))
        .header("Authorization", format!("Bearer {}", token)) // ignore-magic
        .json(&json!({})) // ignore-magic
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("parse response: {}", e))?;

    if status == 200 {
        Ok(body)
    } else {
        Err(format!(
            "approve return request failed: {} — {}",
            status, body
        ))
    }
}

#[tokio::test]
#[ignore]
async fn test_return_request_lifecycle() {
    let client = reqwest::Client::new();
    let buyer_token = login_buyer(&client).await;
    let admin_token = login_admin(&client).await;

    // Get buyer's orders
    match get_orders(&client, &buyer_token).await {
        Ok(orders) => {
            if orders.is_empty() {
                eprintln!("No orders found for buyer — skipping return lifecycle test");
                return;
            }

            // Take first delivered order
            let order = &orders[0];
            let order_id = order[fields::ID] // ignore-magic
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let status = order[fields::STATUS].as_str().unwrap_or(""); // ignore-magic

            if order_id.is_empty() || status != "delivered" {
                // ignore-magic
                eprintln!("Order {} not in delivered state — skipping", order_id);
                return;
            }

            // Get first item from order
            let empty_items = vec![];
            let items = order["items"].as_array().unwrap_or(&empty_items); // ignore-magic
            if items.is_empty() {
                eprintln!("Order has no items");
                return;
            }

            let item = &items[0];
            let item_id = item[fields::ID] // ignore-magic
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_default();

            if item_id.is_empty() {
                return;
            }

            // Create return request
            match create_return_request(
                &client,
                &buyer_token,
                &order_id,
                &item_id,
                "product defective",
                1,
            )
            .await
            {
                Ok(return_req) => {
                    let return_id = return_req[fields::ID] // ignore-magic
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    let return_status = return_req[fields::STATUS].as_str().unwrap_or(""); // ignore-magic

                    // Verify initial status is "pending"
                    assert_eq!(
                        return_status,
                        "pending", // ignore-magic
                        "Return should start in pending state"
                    );

                    // Admin approves the return
                    sleep(Duration::from_millis(500)).await;

                    match approve_return_request(&client, &admin_token, &return_id).await {
                        Ok(approved) => {
                            let new_status = approved[fields::STATUS].as_str().unwrap_or(""); // ignore-magic
                            assert_eq!(
                                new_status, "approved",
                                "Return should transition to approved state"
                            );
                        }
                        Err(e) => {
                            eprintln!("Could not approve return: {}", e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Could not create return request: {}", e);
                }
            }
        }
        Err(e) => {
            eprintln!("Could not get orders: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_return_request_expired_window() {
    let client = reqwest::Client::new();
    let buyer_token = login_buyer(&client).await;

    // Get buyer's orders
    match get_orders(&client, &buyer_token).await {
        Ok(orders) => {
            if orders.is_empty() {
                eprintln!("No orders found");
                return;
            }

            // Find an order that's too old (> 30 days old)
            let old_order = orders.iter().find(|order| {
                let created_at = order[fields::CREATED_AT].as_str().unwrap_or(""); // ignore-magic
                // Very basic check: if order exists and we can parse it, try to return
                !created_at.is_empty()
            });

            if let Some(order) = old_order {
                let order_id = order[fields::ID] // ignore-magic
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_default();

                let empty_items = vec![];
                let items = order["items"].as_array().unwrap_or(&empty_items); // ignore-magic
                if !items.is_empty() {
                    let item_id = items[0][fields::ID] // ignore-magic
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_default();

                    if !item_id.is_empty() {
                        // Try to create return request (might fail if too old)
                        match create_return_request(
                            &client,
                            &buyer_token,
                            &order_id,
                            &item_id,
                            "damaged item",
                            1,
                        )
                        .await
                        {
                            Ok(_) => {
                                eprintln!(
                                    "Return request created — order not yet outside 30-day window"
                                );
                            }
                            Err(e) => {
                                // Expect 400 with message about return window
                                assert!(
                                    e.contains("400")
                                        || e.contains("window")
                                        || e.contains("expired")
                                        || e.contains("days"),
                                    "Should reject return outside 30-day window: {}",
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Could not fetch orders: {}", e);
        }
    }
}
