//! Order tools — list, get, return requests, checkout

use crate::McpState;
use crate::errors::{McpError, McpResult};
use crate::safeguards::{IdempotencyTracker, SpendLimit};
use ob_core::constants::{collections, fields, mcp_params as p};
use serde_json::{Value, json};

/// List orders for user
pub async fn list_orders(_state: McpState, user_id: &str, params: &Value) -> McpResult<Value> {
    let _status = params.get(p::STATUS).and_then(|v| v.as_str());
    let limit = params.get(p::LIMIT).and_then(|v| v.as_u64()).unwrap_or(20);
    let offset = params.get(p::OFFSET).and_then(|v| v.as_u64()).unwrap_or(0);

    if limit > 100 {
        return Err(McpError::ValidationError(
            "Limit must be <= 100".to_string(),
        ));
    }

    Ok(json!({
        "user_id": user_id,
        "orders": [],
        "total": 0,
        "limit": limit,
        "offset": offset
    }))
}

/// Get order details
pub async fn get_order(state: McpState, user_id: &str, params: &Value) -> McpResult<Value> {
    let order_id = params
        .get(p::ORDER_ID)
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'order_id'".to_string()))?;

    if !order_id.contains(':') {
        return Err(McpError::ValidationError(
            "Invalid order ID format".to_string(),
        ));
    }

    let record_id = order_id
        .split_once(':')
        .map(|(_, id)| id)
        .unwrap_or(order_id);

    let order = state
        .db
        .get_document(collections::ORDERS, record_id)
        .await
        .map_err(|e| McpError::Internal(format!("Failed to fetch order: {e}")))?;

    if order.is_null() {
        return Err(McpError::NotFound("Order not found".to_string()));
    }

    let order_buyer_id = order
        .get(fields::BUYER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let order_seller_id = order
        .get(fields::SELLER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if order_buyer_id != user_id && order_seller_id != user_id {
        return Err(McpError::Forbidden("Access denied".to_string()));
    }

    Ok(order)
}

/// Request a return for an order
pub async fn request_return(state: McpState, user_id: &str, params: &Value) -> McpResult<Value> {
    let order_id = params
        .get(p::ORDER_ID)
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'order_id'".to_string()))?;

    let reason = params
        .get(p::REASON)
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'reason'".to_string()))?;

    if !order_id.contains(':') {
        return Err(McpError::ValidationError(
            "Invalid order ID format".to_string(),
        ));
    }

    let record_id = order_id
        .split_once(':')
        .map(|(_, id)| id)
        .unwrap_or(order_id);

    let order = state
        .db
        .get_document(collections::ORDERS, record_id)
        .await
        .map_err(|e| McpError::Internal(format!("Failed to fetch order: {e}")))?;

    if order.is_null() {
        return Err(McpError::NotFound("Order not found".to_string()));
    }

    let order_buyer_id = order
        .get(fields::BUYER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if order_buyer_id != user_id {
        return Err(McpError::Forbidden(
            "You can only request returns for your own orders".to_string(),
        ));
    }

    let order_status = order
        .get(fields::ORDER_STATUS)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if order_status != "delivered" {
        return Err(McpError::ValidationError(
            "Returns can only be requested for delivered orders".to_string(),
        ));
    }

    const RETURN_WINDOW_DAYS: i64 = 30;
    let delivered_at = order
        .get("deliveredAt")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());

    if let Some(delivered_at) = delivered_at {
        let now = chrono::Utc::now();
        let days_since_delivery = (now.timestamp() - delivered_at.timestamp()) / 86400;
        if days_since_delivery > RETURN_WINDOW_DAYS {
            return Err(McpError::ValidationError(format!(
                "Return window expired ({} days since delivery)",
                days_since_delivery
            )));
        }
    }

    let return_doc_id = format!("return_{}", uuid::Uuid::new_v4());
    let return_data = json!({
        fields::ID: format!("return_requests:{}", return_doc_id),
        fields::ORDER_ID: order_id,
        fields::BUYER_ID: user_id,
        fields::REASON: reason,
        fields::STATUS: "pending",
        fields::CREATED_AT: chrono::Utc::now().to_rfc3339(),
    });

    state
        .db
        .create_document(collections::RETURN_REQUESTS, return_data)
        .await
        .map_err(|e| McpError::Internal(format!("Failed to create return request: {e}")))?;

    Ok(json!({
        "return_id": return_doc_id,
        "order_id": order_id,
        "buyer_id": user_id,
        "reason": reason,
        "status": "pending"
    }))
}

/// Create checkout session
pub async fn create_checkout(
    state: McpState,
    user_id: &str,
    params: &Value,
    spend_limit: Option<&SpendLimit>,
    idempotency: Option<&IdempotencyTracker>,
) -> McpResult<Value> {
    let items = params
        .get(p::ITEMS)
        .and_then(|v| v.as_array())
        .ok_or_else(|| McpError::InvalidParams("Missing 'items' array".to_string()))?;

    if items.is_empty() {
        return Err(McpError::ValidationError(
            "Items array cannot be empty".to_string(),
        ));
    }

    let _shipping_address = params
        .get(p::SHIPPING_ADDRESS)
        .ok_or_else(|| McpError::InvalidParams("Missing 'shipping_address'".to_string()))?;

    let idempotency_key = params.get(p::IDEMPOTENCY_KEY).and_then(|v| v.as_str());

    if let (Some(key), Some(tracker)) = (idempotency_key, idempotency)
        && let Some(cached) = tracker.check(key).await
    {
        return Ok(cached);
    }

    let mut total_cents: u64 = 0;
    for item in items {
        let product_id = item
            .get(p::PRODUCT_ID)
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidParams("Item missing 'product_id'".to_string()))?;

        let record_id = product_id
            .split_once(':')
            .map(|(_, id)| id)
            .unwrap_or(product_id);

        let quantity = item
            .get(p::QUANTITY)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| McpError::InvalidParams("Item missing 'quantity'".to_string()))?;

        let product_doc = state
            .db
            .get_document(collections::PRODUCTS, record_id)
            .await;

        let product_doc = match product_doc {
            Ok(doc) if !doc.is_null() => doc,
            Ok(_) | Err(ob_core::Error::NotFound(_)) => {
                return Err(McpError::NotFound(format!(
                    "Product {product_id} not found"
                )));
            }
            Err(e) => {
                return Err(McpError::Internal(format!(
                    "Failed to fetch product {product_id}: {e}"
                )));
            }
        };

        let price_cents = product_doc
            .get(fields::PRICE_CENTS)
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let lifecycle_status = product_doc
            .get(fields::LIFECYCLE_STATUS)
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if lifecycle_status != "active" {
            return Err(McpError::ValidationError(format!(
                "Product {product_id} is not available for purchase"
            )));
        }

        let stock = product_doc
            .get(fields::STOCK_QUANTITY)
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        if stock < quantity {
            return Err(McpError::ValidationError(format!(
                "Product {product_id} has insufficient stock (available: {stock})"
            )));
        }

        total_cents = total_cents.saturating_add(price_cents.saturating_mul(quantity));
    }

    if let Some(safeguards) = spend_limit {
        safeguards.check(user_id, total_cents).await?;
    }

    if let Some(safeguards) = spend_limit {
        safeguards.record(user_id.to_string(), total_cents).await;
    }

    let result = json!({
        "checkout_id": uuid::Uuid::new_v4().to_string(),
        "user_id": user_id,
        "total_cents": total_cents,
        "expires_at": chrono::Utc::now().timestamp() + 1800
    });

    if let (Some(key), Some(tracker)) = (idempotency_key, idempotency) {
        tracker.mark(key.to_string(), result.clone()).await;
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::McpState;
    use std::sync::Arc;

    async fn make_state() -> McpState {
        McpState {
            db: Arc::new(ob_database::DatabaseClient::new_mem().await),
            search: None,
            config: Arc::new(ob_core::Config::load(None).unwrap()),
            jwt_keys: Arc::new(ob_auth::JwtKeys::from_secret("test-secret")),
        }
    }

    // ── list_orders ──

    #[tokio::test]
    async fn test_list_orders_default() {
        let state = make_state().await;
        let result = list_orders(state, "users:u1", &json!({})).await.unwrap();
        assert_eq!(result["user_id"], "users:u1");
        assert_eq!(result["total"], 0);
        assert_eq!(result["limit"], 20);
        assert_eq!(result["offset"], 0);
    }

    #[tokio::test]
    async fn test_list_orders_limit_exceeds_max() {
        let state = make_state().await;
        let result = list_orders(state, "users:u1", &json!({"limit": 101})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::ValidationError(_)));
    }

    // ── get_order ──

    #[tokio::test]
    async fn test_get_order_missing_id() {
        let state = make_state().await;
        let result = get_order(state, "users:u1", &json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn test_get_order_ownership_denied() {
        let state = make_state().await;
        let order_id = format!("o_denied_{}", uuid::Uuid::new_v4());
        let buyer_id = format!("users:buyer-{}", uuid::Uuid::new_v4());
        let seller_id = format!("users:seller-{}", uuid::Uuid::new_v4());
        state
            .db
            .upsert_document(
                collections::ORDERS,
                &order_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id,
                    fields::ORDER_STATUS: "pending",
                }),
            )
            .await
            .unwrap();
        let result = get_order(
            state,
            "users:attacker",
            &json!({"order_id": format!("orders:{order_id}")}),
        )
        .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::Forbidden(_)));
    }

    #[tokio::test]
    async fn test_get_order_seller_can_access() {
        let state = make_state().await;
        let order_id = format!("o_seller_access_{}", uuid::Uuid::new_v4());
        let buyer_id = format!("users:buyer-{}", uuid::Uuid::new_v4());
        let seller_id = format!("users:seller-{}", uuid::Uuid::new_v4());
        state
            .db
            .upsert_document(
                collections::ORDERS,
                &order_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id.clone(),
                    fields::ORDER_STATUS: "shipped",
                }),
            )
            .await
            .unwrap();
        let result = get_order(
            state,
            &seller_id,
            &json!({"order_id": format!("orders:{order_id}")}),
        )
        .await;
        assert!(result.is_ok());
    }

    // ── request_return ──

    #[tokio::test]
    async fn test_request_return_missing_order_id() {
        let state = make_state().await;
        let result = request_return(state, "users:u1", &json!({"reason": "defective"})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn test_request_return_not_delivered() {
        let state = make_state().await;
        let order_id = format!("o_pending_{}", uuid::Uuid::new_v4());
        let buyer_id = "users:u1";
        state
            .db
            .upsert_document(
                collections::ORDERS,
                &order_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::ORDER_STATUS: "pending",
                }),
            )
            .await
            .unwrap();
        let result = request_return(
            state,
            buyer_id,
            &json!({"order_id": format!("orders:{order_id}"), "reason": "defective"}),
        )
        .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::ValidationError(_)));
    }

    #[tokio::test]
    async fn test_request_return_not_owner() {
        let state = make_state().await;
        let order_id = format!("o_other_{}", uuid::Uuid::new_v4());
        state
            .db
            .upsert_document(
                collections::ORDERS,
                &order_id,
                json!({
                    fields::BUYER_ID: "users:other",
                    fields::ORDER_STATUS: "delivered",
                }),
            )
            .await
            .unwrap();
        let result = request_return(
            state,
            "users:u1",
            &json!({"order_id": format!("orders:{order_id}"), "reason": "defective"}),
        )
        .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::Forbidden(_)));
    }

    // ── create_checkout ──

    #[tokio::test]
    async fn test_create_checkout_missing_items() {
        let state = make_state().await;
        let result = create_checkout(
            state,
            "users:u1",
            &json!({"shipping_address": {"line1": "123 Main"}}),
            None,
            None,
        )
        .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn test_create_checkout_empty_items() {
        let state = make_state().await;
        let result = create_checkout(
            state,
            "users:u1",
            &json!({"items": [], "shipping_address": {}}),
            None,
            None,
        )
        .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::ValidationError(_)));
    }

    #[tokio::test]
    async fn test_create_checkout_product_not_found() {
        let state = make_state().await;
        let params = json!({
            "items": [{"product_id": "products:nonexistent", "quantity": 1}],
            "shipping_address": {"line1": "123 Main St"}
        });
        let result = create_checkout(state, "users:u1", &params, None, None).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_create_checkout_uses_db_price() {
        let state = make_state().await;
        let product_id = format!("p_{}", uuid::Uuid::new_v4());
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                &product_id,
                json!({
                    fields::PRICE_CENTS: 2500u64,
                    fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 100u64,
                }),
            )
            .await
            .unwrap();
        let params = json!({
            "items": [{"product_id": format!("products:{product_id}"), "quantity": 2}],
            "shipping_address": {"line1": "123 Main St"}
        });
        let result = create_checkout(state, "users:u1", &params, None, None)
            .await
            .unwrap();
        assert_eq!(result["total_cents"], 5000u64);
    }

    #[tokio::test]
    async fn test_create_checkout_inactive_product() {
        let state = make_state().await;
        let product_id = format!("p_inactive_{}", uuid::Uuid::new_v4());
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                &product_id,
                json!({
                    fields::PRICE_CENTS: 1000u64,
                    fields::LIFECYCLE_STATUS: "draft",
                    fields::STOCK_QUANTITY: 100u64,
                }),
            )
            .await
            .unwrap();
        let params = json!({
            "items": [{"product_id": format!("products:{product_id}"), "quantity": 1}],
            "shipping_address": {"line1": "123 Main St"}
        });
        let result = create_checkout(state, "users:u1", &params, None, None).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::ValidationError(_)));
    }

    #[tokio::test]
    async fn test_create_checkout_insufficient_stock() {
        let state = make_state().await;
        let product_id = format!("p_low_stock_{}", uuid::Uuid::new_v4());
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                &product_id,
                json!({
                    fields::PRICE_CENTS: 1000u64,
                    fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 1u64,
                }),
            )
            .await
            .unwrap();
        let params = json!({
            "items": [{"product_id": format!("products:{product_id}"), "quantity": 5}],
            "shipping_address": {"line1": "123 Main St"}
        });
        let result = create_checkout(state, "users:u1", &params, None, None).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::ValidationError(_)));
    }

    #[tokio::test]
    async fn test_create_checkout_spend_limit_exceeded() {
        let state = make_state().await;
        let product_id = format!("p_spend_{}", uuid::Uuid::new_v4());
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                &product_id,
                json!({
                    fields::PRICE_CENTS: 10000u64,
                    fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 100u64,
                }),
            )
            .await
            .unwrap();
        let spend_limit = SpendLimit::new(5000, 100_000);
        let params = json!({
            "items": [{"product_id": format!("products:{product_id}"), "quantity": 1}],
            "shipping_address": {"line1": "123 Main"}
        });
        let result = create_checkout(state, "users:u1", &params, Some(&spend_limit), None).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::ValidationError(_)));
    }

    #[tokio::test]
    async fn test_create_checkout_idempotency() {
        let state = make_state().await;
        let product_id = format!("p_idem_{}", uuid::Uuid::new_v4());
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                &product_id,
                json!({
                    fields::PRICE_CENTS: 1000u64,
                    fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 100u64,
                }),
            )
            .await
            .unwrap();
        let tracker = IdempotencyTracker::new();
        let params = json!({
            "items": [{"product_id": format!("products:{product_id}"), "quantity": 1}],
            "shipping_address": {"line1": "123 Main"},
            "idempotency_key": "idem-key-1"
        });
        let r1 = create_checkout(state.clone(), "users:u1", &params, None, Some(&tracker))
            .await
            .unwrap();
        let r2 = create_checkout(state, "users:u1", &params, None, Some(&tracker))
            .await
            .unwrap();
        assert_eq!(r1["checkout_id"], r2["checkout_id"]);
    }
}
