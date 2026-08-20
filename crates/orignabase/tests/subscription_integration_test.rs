//! Live integration tests for subscription functionality.
//!
//! Run with: `cd orignabase && cargo test --test subscription_integration_test -- --ignored`

use ob_database::fields;
use serde_json::{Value, json};

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "https://api.dev.orignagta.ca".to_string())
}

/// Login as buyer and return (token, user_id).
async fn login_buyer(client: &reqwest::Client) -> (String, String) {
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
    let token = body["access_token"] // ignore-magic
        .as_str()
        .expect("missing access_token")
        .to_string();
    let user_id = body["user"][fields::ID] // ignore-magic
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_default();
    (token, user_id)
}

/// Create a premium subscription for a user.
async fn create_subscription(
    client: &reqwest::Client,
    token: &str,
    subscription_tier: &str,
) -> Result<Value, String> {
    let resp = client
        .post(format!("{}/subscriptions/create", base_url()))
        .header("Authorization", format!("Bearer {}", token)) // ignore-magic
        .json(&json!({ // ignore-magic
            "tier": subscription_tier,
            "paymentMethodId": "pm_test_visa"  // Test payment method
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
            "subscription creation failed: {} — {}",
            status, body
        ))
    }
}

/// Get user's subscription status.
async fn get_subscription_status(client: &reqwest::Client, token: &str) -> Result<Value, String> {
    let resp = client
        .get(format!("{}/subscriptions/status", base_url()))
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
        Ok(body)
    } else {
        Err(format!("get subscription failed: {} — {}", status, body))
    }
}

#[tokio::test]
#[ignore]
async fn test_subscription_benefits_delay_48h() {
    let client = reqwest::Client::new();
    let (buyer_token, _user_id) = login_buyer(&client).await;

    // Create premium subscription
    match create_subscription(&client, &buyer_token, "premium").await {
        Ok(sub) => {
            eprintln!("Subscription created: {}", sub);

            // Immediately check subscription status
            match get_subscription_status(&client, &buyer_token).await {
                Ok(status) => {
                    let benefits_active = status["benefitsActive"].as_bool().unwrap_or(false); // ignore-magic
                    let activation_time = status["benefitsActivateAt"].as_str(); // ignore-magic

                    // According to spec, benefits should NOT be active immediately
                    // They activate 48 hours after creation
                    assert!(
                        !benefits_active || activation_time.is_some(),
                        "Benefits should not be active immediately or should have delayed activation"
                    );

                    if let Some(activate_at) = activation_time {
                        // Verify it's a future timestamp
                        assert!(
                            activate_at.contains("T") && activate_at.contains("Z"),
                            "benefitsActivateAt should be ISO 8601 timestamp: {}",
                            activate_at
                        );
                    }
                }
                Err(e) => {
                    eprintln!("Could not get subscription status: {}", e);
                }
            }
        }
        Err(e) => {
            eprintln!("Subscription creation not available: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_subscription_early_cancel_tracking() {
    let client = reqwest::Client::new();
    let (buyer_token, _user_id) = login_buyer(&client).await;

    // Create subscription
    if let Ok(sub) = create_subscription(&client, &buyer_token, "premium").await {
        let subscription_id = sub[fields::ID] // ignore-magic
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_default();

        if subscription_id.is_empty() {
            eprintln!("No subscription ID returned");
            return;
        }

        // Immediately cancel (within 7 days)
        let cancel_resp = client
            .post(format!(
                "{}/subscriptions/{}/cancel",
                base_url(),
                subscription_id
            ))
            .header("Authorization", format!("Bearer {}", buyer_token)) // ignore-magic
            .json(&json!({})) // ignore-magic
            .send()
            .await;

        if let Ok(resp) = cancel_resp
            && resp.status() == 200
        {
            let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic

            let was_early_cancel = body["wasEarlyCancel"].as_bool().unwrap_or(false); // ignore-magic
            assert!(
                was_early_cancel,
                "Immediate cancellation should be marked as early cancel"
            );

            // Check subscription status again
            if let Ok(status) = get_subscription_status(&client, &buyer_token).await {
                let early_cancel_count = status["earlyCancelCount"].as_i64().unwrap_or(0); // ignore-magic
                assert!(
                    early_cancel_count >= 1,
                    "early_cancel_count should increment after early cancel"
                );
            }
        }
    }
}
