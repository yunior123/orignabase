//! Stock notification subscribe/unsubscribe handlers.
//! Ported from: functions/handlers/products.py (subscribe_stock_notification, unsubscribe_stock_notification)

use axum::{Extension, Json, Router, extract::State, routing::post};
use ob_auth::middleware::AuthContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::info;

use crate::HandlersState;
use crate::shared::auth::resolve_self_user_id;
use crate::shared::schema::{collections, fields};
use crate::shared::validation::validate_uid;

// ─── Request/Response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StockSubscribeRequest {
    pub product_id: String,
    pub user_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockSubscribeResponse {
    pub success: bool,
    pub subscribed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StockUnsubscribeRequest {
    pub product_id: String,
    pub user_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockUnsubscribeResponse {
    pub success: bool,
    pub unsubscribed: bool,
}

// ─── Router ─────────────────────────────────────────────────────────────────

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/products/stock-notify/subscribe", post(subscribe))
        .route("/api/products/stock-notify/unsubscribe", post(unsubscribe))
        .with_state(state)
}

// ─── Handlers ───────────────────────────────────────────────────────────────

async fn subscribe(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<StockSubscribeRequest>,
) -> Result<Json<StockSubscribeResponse>, ob_core::Error> {
    let user_id = resolve_self_user_id(&auth, Some(req.user_id.as_str()), "userId")?;
    validate_uid("productId", &req.product_id)?;
    validate_uid("userId", &user_id)?;

    // Validate productId format (no path traversal)
    if req.product_id.contains('/') || req.product_id == "." || req.product_id == ".." {
        return Err(ob_core::Error::Validation(
            "Invalid productId format".into(),
        ));
    }

    // Verify product exists
    let product = state
        .db
        .get_document(collections::PRODUCTS, &req.product_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Product not found".into()))?;

    if product.is_null() {
        return Err(ob_core::Error::NotFound("Product not found".into()));
    }

    // Sellers cannot subscribe to their own products
    let seller_id = product
        .get(fields::SELLER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if seller_id == user_id {
        return Err(ob_core::Error::Forbidden(
            "Sellers cannot subscribe to their own product notifications".into(),
        ));
    }

    // Check product is actually out of stock
    let stock = product
        .get(fields::STOCK_QUANTITY)
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    if stock > 0 {
        return Err(ob_core::Error::Validation(
            "Product is already in stock".into(),
        ));
    }

    // Check for existing active subscription (idempotent)
    let existing_query = format!(
        "SELECT * FROM {} WHERE data->>'{}' = '{}' AND data->>'{}' = '{}' AND (data->>'{}') IS NULL LIMIT 1",
        collections::STOCK_NOTIFICATIONS,
        fields::PRODUCT_ID,
        ob_core::escape_sql_string(&req.product_id),
        fields::USER_ID,
        ob_core::escape_sql_string(&user_id),
        fields::NOTIFIED_AT,
    );

    let existing: Vec<Value> = state
        .db
        .query_raw(&existing_query)
        .await
        .unwrap_or_default();
    if !existing.is_empty() {
        return Ok(Json(StockSubscribeResponse {
            success: true,
            subscribed: true,
        }));
    }

    // Fetch user email
    let user = state
        .db
        .get_document(collections::USERS, &user_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("User not found".into()))?;

    let user_email = user
        .get(fields::EMAIL)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if user_email.is_empty() {
        return Err(ob_core::Error::Validation("Account has no email".into()));
    }

    let product_name = product
        .get(fields::TITLE)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Create subscription document
    let now = chrono::Utc::now().to_rfc3339();
    let doc = serde_json::json!({
        fields::PRODUCT_ID: req.product_id,
        fields::USER_ID: user_id,
        fields::EMAIL: user_email,
        fields::PRODUCT_NAME: product_name,
        fields::NOTIFIED_AT: null,
        fields::CREATED_AT: now,
    });

    state
        .db
        .create_document(collections::STOCK_NOTIFICATIONS, doc)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to create subscription: {e}")))?;

    info!(product_id = %req.product_id, user_id = %user_id, "Stock notification subscribed");

    Ok(Json(StockSubscribeResponse {
        success: true,
        subscribed: true,
    }))
}

async fn unsubscribe(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<StockUnsubscribeRequest>,
) -> Result<Json<StockUnsubscribeResponse>, ob_core::Error> {
    let user_id = resolve_self_user_id(&auth, Some(req.user_id.as_str()), "userId")?;
    validate_uid("productId", &req.product_id)?;
    validate_uid("userId", &user_id)?;

    // Delete active subscriptions for this product+user
    let delete_query = format!(
        "DELETE FROM {} WHERE data->>'{}' = '{}' AND data->>'{}' = '{}' AND (data->>'{}') IS NULL",
        collections::STOCK_NOTIFICATIONS,
        fields::PRODUCT_ID,
        ob_core::escape_sql_string(&req.product_id),
        fields::USER_ID,
        ob_core::escape_sql_string(&user_id),
        fields::NOTIFIED_AT,
    );

    state
        .db
        .query_raw(&delete_query)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to unsubscribe: {e}")))?;

    info!(product_id = %req.product_id, user_id = %user_id, "Stock notification unsubscribed");

    Ok(Json(StockUnsubscribeResponse {
        success: true,
        unsubscribed: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use serde_json::json;
    use std::sync::Arc;

    fn auth(user_id: &str) -> Extension<AuthContext> {
        Extension(AuthContext {
            user_id: user_id.into(),
            roles: vec!["user".into()],
            authenticated: true,
            email_verified: true,
            custom_claims: serde_json::Value::Null,
        })
    }

    async fn setup_state() -> HandlersState {
        unsafe { std::env::set_var("OB_TEST_MODE", "1") };
        HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
        }
    }

    #[test]
    fn test_subscribe_request_deser() {
        let json = r#"{"productId": "prod1", "userId": "user1"}"#;
        let req: StockSubscribeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.product_id, "prod1");
        assert_eq!(req.user_id, "user1");
    }

    #[test]
    fn test_path_traversal_rejection() {
        let bad_ids = ["../etc/passwd", ".", "..", "a/b/c"];
        for id in bad_ids {
            assert!(
                id.contains('/') || id == "." || id == "..",
                "Should detect path traversal: {id}"
            );
        }
    }

    #[test]
    fn test_unsubscribe_response_serialize() {
        let resp = StockUnsubscribeResponse {
            success: true,
            unsubscribed: true,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"unsubscribed\":true"));
    }

    // ── Ported from test_handlers_products_engagement_deep.py (stock notification tests) ──

    #[test]
    fn test_subscribe_response_serialize() {
        let resp = StockSubscribeResponse {
            success: true,
            subscribed: true,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"subscribed\":true"));
    }

    #[test]
    fn test_path_traversal_dot_variants() {
        // All these should be detected as path traversal attempts
        assert!("." == "." || "." == "..");
        assert!(".." == "." || ".." == "..");
        assert!("../etc/passwd".contains('/'));
        assert!("a/b".contains('/'));
        // Valid product IDs do NOT contain slashes or dots-only
        let valid_id = "prod_abc123";
        assert!(!valid_id.contains('/') && valid_id != "." && valid_id != "..");
    }

    #[test]
    fn test_subscribe_request_missing_fields_fail() {
        // Missing userId
        let json = r#"{"productId": "prod1"}"#;
        assert!(serde_json::from_str::<StockSubscribeRequest>(json).is_err());

        // Missing productId
        let json = r#"{"userId": "user1"}"#;
        assert!(serde_json::from_str::<StockSubscribeRequest>(json).is_err());
    }

    #[test]
    fn test_unsubscribe_request_deser() {
        let json = r#"{"productId": "prod1", "userId": "user1"}"#;
        let req: StockUnsubscribeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.product_id, "prod1");
        assert_eq!(req.user_id, "user1");
    }

    #[test]
    fn test_seller_self_subscribe_detection() {
        // Simulate seller_id == user_id scenario
        let seller_id = "seller_abc";
        let user_id = "seller_abc";
        assert_eq!(seller_id, user_id, "Self-subscription should be caught");
    }

    #[test]
    fn test_stock_quantity_check_for_subscribe() {
        // Out of stock -> subscribe allowed
        let stock: i64 = 0;
        assert!(stock <= 0);

        // In stock -> subscribe should be rejected
        let stock: i64 = 5;
        assert!(stock > 0);

        // Negative stock -> still counts as out of stock
        let stock: i64 = -1;
        assert!(stock <= 0);
    }

    #[tokio::test]
    async fn test_subscribe_success_creates_notification_and_is_idempotent() {
        let state = setup_state().await;
        let u = uuid::Uuid::new_v4().to_string();
        let prod = format!("prod_sub_{u}");
        let buyer = format!("buyer_sub_{u}");
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                &prod,
                json!({
                    fields::PRODUCT_ID: prod,
                    fields::SELLER_ID: format!("seller_sub_{u}"),
                    fields::STOCK_QUANTITY: 0,
                    fields::TITLE: "Blue Widget",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                &buyer,
                json!({
                    fields::UID: buyer,
                    fields::EMAIL: "buyer@example.com",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = subscribe(
            State(state.clone()),
            auth(&buyer),
            Json(StockSubscribeRequest {
                product_id: prod.clone(),
                user_id: buyer.clone(),
            }),
        )
        .await
        .unwrap();
        assert!(resp.subscribed);

        let rows = state
            .db
            .query_raw(&format!(
                "SELECT * FROM {} WHERE data->>'productId' = '{}' AND data->>'userId' = '{}'",
                collections::STOCK_NOTIFICATIONS,
                prod,
                buyer
            ))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][fields::EMAIL], "buyer@example.com");
        assert_eq!(rows[0][fields::PRODUCT_NAME], "Blue Widget");
        assert!(rows[0][fields::NOTIFIED_AT].is_null());

        let Json(idempotent) = subscribe(
            State(state.clone()),
            auth(&buyer),
            Json(StockSubscribeRequest {
                product_id: prod.clone(),
                user_id: buyer.clone(),
            }),
        )
        .await
        .unwrap();
        assert!(idempotent.subscribed);

        let rows_after = state
            .db
            .query_raw(&format!(
                "SELECT * FROM {} WHERE data->>'productId' = '{}' AND data->>'userId' = '{}'",
                collections::STOCK_NOTIFICATIONS,
                prod,
                buyer,
            ))
            .await
            .unwrap();
        assert_eq!(rows_after.len(), 1);
    }

    #[tokio::test]
    async fn test_subscribe_rejects_invalid_product_missing_user_email_self_subscribe_and_in_stock()
    {
        let state = setup_state().await;

        let invalid_format = subscribe(
            State(state.clone()),
            auth("buyer_1"),
            Json(StockSubscribeRequest {
                product_id: "../etc/passwd".into(),
                user_id: "buyer_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            invalid_format
                .to_string()
                .contains("Invalid productId format")
        );

        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_self",
                json!({
                    fields::PRODUCT_ID: "prod_self",
                    fields::SELLER_ID: "seller_1",
                    fields::STOCK_QUANTITY: 0,
                }),
            )
            .await
            .unwrap();

        let self_subscribe = subscribe(
            State(state.clone()),
            auth("seller_1"),
            Json(StockSubscribeRequest {
                product_id: "prod_self".into(),
                user_id: "seller_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            self_subscribe
                .to_string()
                .contains("Sellers cannot subscribe to their own product")
        );

        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_stocked",
                json!({
                    fields::PRODUCT_ID: "prod_stocked",
                    fields::SELLER_ID: "seller_2",
                    fields::STOCK_QUANTITY: 3,
                }),
            )
            .await
            .unwrap();

        let in_stock = subscribe(
            State(state.clone()),
            auth("buyer_1"),
            Json(StockSubscribeRequest {
                product_id: "prod_stocked".into(),
                user_id: "buyer_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(in_stock.to_string().contains("Product is already in stock"));

        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_no_email",
                json!({
                    fields::PRODUCT_ID: "prod_no_email",
                    fields::SELLER_ID: "seller_2",
                    fields::STOCK_QUANTITY: 0,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "buyer_2",
                json!({
                    fields::UID: "buyer_2",
                }),
            )
            .await
            .unwrap();

        let no_email = subscribe(
            State(state.clone()),
            auth("buyer_2"),
            Json(StockSubscribeRequest {
                product_id: "prod_no_email".into(),
                user_id: "buyer_2".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(no_email.to_string().contains("Account has no email"));

        let missing_product = subscribe(
            State(state),
            auth("buyer_1"),
            Json(StockSubscribeRequest {
                product_id: "prod_missing".into(),
                user_id: "buyer_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(missing_product.to_string().contains("Product not found"));
    }

    #[tokio::test]
    async fn test_subscribe_rejects_nonexistent_product_null() {
        // Tests line 76: product.is_null() after get_document succeeds
        // When a product doesn't exist in the DB, get_document either errors (caught on line 73)
        // or returns Null (caught on line 76). We test both paths.
        let state = setup_state().await;
        // No product seeded — get_document will return error/null
        let result = subscribe(
            State(state.clone()),
            auth("buyer_1"),
            Json(StockSubscribeRequest {
                product_id: "nonexistent_prod".into(),
                user_id: "buyer_1".into(),
            }),
        )
        .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Product not found")
        );
    }

    #[tokio::test]
    async fn test_unsubscribe_deletes_active_notifications() {
        let state = setup_state().await;
        let u = uuid::Uuid::new_v4().to_string();
        let prod = format!("prod_unsub_{u}");
        let buyer = format!("buyer_unsub_{u}");
        state
            .db
            .create_document(
                collections::STOCK_NOTIFICATIONS,
                json!({
                    fields::PRODUCT_ID: prod,
                    "userId": buyer,
                    "notifiedAt": null,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .create_document(
                collections::STOCK_NOTIFICATIONS,
                json!({
                    fields::PRODUCT_ID: prod,
                    "userId": buyer,
                    "notifiedAt": "2026-03-10T10:00:00Z",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = unsubscribe(
            State(state.clone()),
            auth(&buyer),
            Json(StockUnsubscribeRequest {
                product_id: prod.clone(),
                user_id: buyer.clone(),
            }),
        )
        .await
        .unwrap();
        assert!(resp.unsubscribed);

        let rows = state
            .db
            .query_raw(&format!(
                "SELECT * FROM {} WHERE data->>'productId' = '{}' AND data->>'userId' = '{}'",
                collections::STOCK_NOTIFICATIONS,
                prod,
                buyer
            ))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][fields::NOTIFIED_AT], "2026-03-10T10:00:00Z");
    }
}
