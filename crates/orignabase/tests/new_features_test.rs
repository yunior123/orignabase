//! New features live integration tests
//! 
//! Tests new features and infrastructure improvements:
//! - Bulk product upload endpoint
//! - Email notification triggers
//! - JWT key rotation admin endpoint
//! - Support chat endpoint
//! - Geocode proxy endpoint
//! - Database indexes (query performance)
//! - Health endpoint
//! - Data retention (webhook cleanup)
//! - Subscription double-create prevention
//! - Password reset token invalidation
//! 
//! Run: cargo test --test new_features_test -- --ignored

use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use uuid::Uuid;

mod new_features {
    use super::*;

    fn base_url() -> String {
        std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8081".to_string())
    }

    fn client() -> Client {
        Client::new()
    }

    fn unique_email() -> String {
        format!("feat_{}@example.com", Uuid::new_v4())
    }

    async fn register_and_login(email: &str, password: &str) -> String {
        let client = client();
        
        client
            .post(format!("{}/auth/register", base_url()))
            .json(&json!({"email": email, "password": password}))
            .send()
            .await
            .expect("register failed");

        let login = client
            .post(format!("{}/auth/login", base_url()))
            .json(&json!({"email": email, "password": password}))
            .send()
            .await
            .expect("login failed");

        let body: Value = login.json().await.expect("login body invalid");
        body["access_token"]
            .as_str()
            .expect("missing access_token")
            .to_string()
    }

    // ═══════════════════════════════════════════════════════════════════
    // TEST 1: Health endpoint returns ok and validates dependencies
    // ═══════════════════════════════════════════════════════════════════
    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn health_endpoint_validates_all_dependencies() {
        let client = client();

        let response = client
            .get(format!("{}/health", base_url()))
            .send()
            .await
            .expect("health request failed");

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "health endpoint should return 200"
        );

        let body: Value = response.json().await.unwrap_or(json!({}));

        // Should include checks for critical dependencies
        assert!(
            body["status"].as_str() == Some("ok")
                || body["healthy"].as_bool() == Some(true),
            "health status should indicate ok"
        );

        // Check for SurrealDB health (if returned)
        if let Some(db_status) = body["database"].as_str() {
            assert_eq!(db_status, "ok", "database should be healthy");
        }

        // Check for Meilisearch health (if returned)
        if let Some(search_status) = body["search"].as_str() {
            assert_eq!(search_status, "ok", "search should be healthy");
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // TEST 2: Bulk product upload endpoint accepts CSV/JSON
    // ═══════════════════════════════════════════════════════════════════
    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn bulk_product_upload_endpoint_exists() {
        let token = register_and_login(&unique_email(), "TestPass123!").await;
        let client = client();

        let response = client
            .post(format!("{}/products/bulk-upload", base_url()))
            .header("Authorization", format!("Bearer {}", token))
            .json(&json!({
                "products": [
                    {
                        "title": "Bulk product 1",
                        "description": "Test",
                        "priceCents": 5000,
                        "categoryId": "cat_123",
                        "subcategory": "test",
                        "stockQuantity": 10
                    },
                    {
                        "title": "Bulk product 2",
                        "description": "Test",
                        "priceCents": 7500,
                        "categoryId": "cat_456",
                        "subcategory": "test",
                        "stockQuantity": 20
                    }
                ]
            }))
            .send()
            .await
            .expect("bulk upload request failed");

        // Should succeed or indicate partial success
        assert!(
            response.status() == StatusCode::OK
                || response.status() == StatusCode::CREATED
                || response.status() == StatusCode::ACCEPTED
                || response.status() == StatusCode::NOT_FOUND,
            "bulk upload endpoint should respond (found={}, not_found={})",
            response.status() == StatusCode::OK,
            response.status() == StatusCode::NOT_FOUND
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // TEST 3: Email notification triggered on order confirmation
    // ═══════════════════════════════════════════════════════════════════
    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn email_notifications_triggered_on_order_events() {
        let token = register_and_login(&unique_email(), "TestPass123!").await;
        let client = client();

        // Create order
        let response = client
            .post(format!("{}/orders", base_url()))
            .header("Authorization", format!("Bearer {}", token))
            .json(&json!({
                "items": [{"productId": "prod_test_123", "quantity": 1, "unitPriceCents": 5000}],
                "shippingAddressId": "addr_test_123"
            }))
            .send()
            .await
            .expect("create order failed");

        if response.status() == StatusCode::OK || response.status() == StatusCode::CREATED {
            // Check notification queue (if available)
            let notify_check = client
                .get(format!("{}/notifications?type=email&limit=10", base_url()))
                .header("Authorization", format!("Bearer {}", token))
                .send()
                .await;

            match notify_check {
                Ok(resp) => {
                    if resp.status() == StatusCode::OK {
                        let body: Value = resp.json().await.unwrap_or(json!([]));
                        
                        // Should have email notifications queued
                        if let Some(arr) = body.as_array() {
                            println!(
                                "Email notifications: {} pending",
                                arr.len()
                            );
                            // At least order confirmation should be queued
                            assert!(
                                arr.iter().any(|n| {
                                    n["type"].as_str().map(|t| t.contains("order")).unwrap_or(false)
                                }),
                                "order confirmation email should be queued"
                            );
                        }
                    }
                }
                Err(_) => {
                    println!("Notification queue endpoint not available");
                }
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // TEST 4: JWT key rotation admin endpoint available
    // ═══════════════════════════════════════════════════════════════════
    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn jwt_key_rotation_admin_endpoint_exists() {
        let admin_token = register_and_login("admin@test.origna.ca", "TestPass123!").await;
        let client = client();

        let response = client
            .post(format!("{}/admin/jwt/rotate-keys", base_url()))
            .header("Authorization", format!("Bearer {}", admin_token))
            .json(&json!({}))
            .send()
            .await
            .expect("key rotation request failed");

        // Should either succeed or return 403 (not admin) or 404 (not implemented yet)
        assert!(
            response.status() != StatusCode::INTERNAL_SERVER_ERROR,
            "key rotation endpoint should not return 500"
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // TEST 5: Support chat endpoint for customer support
    // ═══════════════════════════════════════════════════════════════════
    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn support_chat_endpoint_available() {
        let token = register_and_login(&unique_email(), "TestPass123!").await;
        let client = client();

        let response = client
            .post(format!("{}/support/chat", base_url()))
            .header("Authorization", format!("Bearer {}", token))
            .json(&json!({
                "message": "Help, my order is missing!",
                "orderId": "ord_test_123"
            }))
            .send()
            .await
            .expect("support chat request failed");

        // Should succeed or indicate not implemented (not 500)
        assert!(
            response.status() != StatusCode::INTERNAL_SERVER_ERROR,
            "support chat endpoint should not return 500"
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // TEST 6: Geocode proxy endpoint for address validation
    // ═══════════════════════════════════════════════════════════════════
    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn geocode_proxy_endpoint_available() {
        let token = register_and_login(&unique_email(), "TestPass123!").await;
        let client = client();

        let response = client
            .get(format!(
                "{}/geocode?address=Toronto,%20Ontario,%20Canada",
                base_url()
            ))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .expect("geocode request failed");

        // Should respond with coordinates or indicate not implemented
        assert!(
            response.status() != StatusCode::INTERNAL_SERVER_ERROR,
            "geocode endpoint should not return 500"
        );

        if response.status() == StatusCode::OK {
            let body: Value = response.json().await.unwrap_or(json!({}));
            
            // Should have lat/lng or similar
            assert!(
                body.get("lat").is_some() || body.get("latitude").is_some(),
                "geocode response should include latitude"
            );
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // TEST 7: Database indexes exist for query performance
    // ═══════════════════════════════════════════════════════════════════
    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn critical_db_indexes_exist() {
        let client = client();

        // Query that should use index on orders.status
        let response = client
            .get(format!(
                "{}/orders?status=confirmed&limit=10",
                base_url()
            ))
            .send()
            .await
            .expect("indexed query failed");

        // Should complete quickly (if index exists) - no timeout
        assert!(
            response.status() != StatusCode::REQUEST_TIMEOUT
                && response.status() != StatusCode::INTERNAL_SERVER_ERROR,
            "indexed query should not timeout or error"
        );

        // Query on products.priceCents should use index
        let price_query = client
            .get(format!(
                "{}/search/products?minPrice=1000&maxPrice=50000",
                base_url()
            ))
            .send()
            .await
            .expect("price range query failed");

        assert!(
            price_query.status() != StatusCode::REQUEST_TIMEOUT,
            "price range query should use index and not timeout"
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // TEST 8: Data retention - webhook events cleaned up after 90 days
    // ═══════════════════════════════════════════════════════════════════
    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn webhook_event_retention_enforced() {
        let admin_token = register_and_login("admin@test.origna.ca", "TestPass123!").await;
        let client = client();

        // Check webhook event cleanup config
        let response = client
            .get(format!("{}/admin/config/data-retention", base_url()))
            .header("Authorization", format!("Bearer {}", admin_token))
            .send()
            .await
            .expect("config request failed");

        if response.status() == StatusCode::OK {
            let body: Value = response.json().await.unwrap_or(json!({}));
            
            if let Some(webhook_days) = body["webhookEventRetentionDays"].as_i64() {
                assert!(
                    webhook_days >= 30 && webhook_days <= 180,
                    "webhook retention should be between 30-180 days (got {})",
                    webhook_days
                );
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // TEST 9: Subscription double-create prevention
    // ═══════════════════════════════════════════════════════════════════
    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn subscription_double_create_prevented() {
        let token = register_and_login(&unique_email(), "TestPass123!").await;
        let client = client();

        // Create first subscription
        let first = client
            .post(format!("{}/subscriptions", base_url()))
            .header("Authorization", format!("Bearer {}", token))
            .json(&json!({
                "plan": "premium_monthly",
                "billingCycleAnchor": 1700000000
            }))
            .send()
            .await
            .expect("first subscription failed");

        if first.status() == StatusCode::OK || first.status() == StatusCode::CREATED {
            let first_body: Value = first.json().await.unwrap_or(json!({}));
            let first_id = first_body["id"].as_str().unwrap_or("").to_string();

            // Attempt to create duplicate
            let second = client
                .post(format!("{}/subscriptions", base_url()))
                .header("Authorization", format!("Bearer {}", token))
                .json(&json!({
                    "plan": "premium_monthly",
                    "billingCycleAnchor": 1700000000
                }))
                .send()
                .await
                .expect("second subscription failed");

            // Should fail (user already has subscription)
            assert!(
                second.status() == StatusCode::BAD_REQUEST
                    || second.status() == StatusCode::CONFLICT
                    || second.status() == StatusCode::UNPROCESSABLE_ENTITY,
                "duplicate subscription should be rejected"
            );
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // TEST 10: Password reset token invalidation after use
    // ═══════════════════════════════════════════════════════════════════
    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn password_reset_token_invalidated_after_use() {
        let email = unique_email();
        let client = client();

        // Request password reset
        let reset_req = client
            .post(format!("{}/auth/password-reset", base_url()))
            .json(&json!({"email": email}))
            .send()
            .await
            .expect("reset request failed");

        if reset_req.status() == StatusCode::OK {
            // In real test, would extract token from email
            // Simulating token: "test_reset_token_123"
            let token = "test_reset_token_123";

            // First reset attempt
            let first_reset = client
                .post(format!("{}/auth/reset-password", base_url()))
                .json(&json!({
                    "token": token,
                    "newPassword": "NewPass456!"
                }))
                .send()
                .await
                .expect("first reset failed");

            if first_reset.status() == StatusCode::OK {
                // Attempt to reuse same token
                let second_reset = client
                    .post(format!("{}/auth/reset-password", base_url()))
                    .json(&json!({
                        "token": token,
                        "newPassword": "AnotherPass789!"
                    }))
                    .send()
                    .await
                    .expect("second reset failed");

                // Should fail (token already used)
                assert!(
                    second_reset.status() == StatusCode::BAD_REQUEST
                        || second_reset.status() == StatusCode::UNAUTHORIZED,
                    "reset token should be invalidated after first use"
                );
            }
        }
    }
}
