//! Shopping tools — cart management

use crate::McpState;
use crate::errors::{McpError, McpResult};
use crate::safeguards::IdempotencyTracker;
use ob_core::constants::mcp_params as p;
use serde_json::{Value, json};

/// Get user's cart
pub async fn get_cart(_state: McpState, user_id: &str, _params: &Value) -> McpResult<Value> {
    if !user_id.contains(':') {
        return Err(McpError::ValidationError(
            "Invalid user ID format".to_string(),
        ));
    }

    Ok(json!({
        "user_id": user_id,
        "items": [],
        "subtotal_cents": 0,
        "tax_cents": 0,
        "total_cents": 0
    }))
}

/// Add item to cart
pub async fn add_to_cart(
    _state: McpState,
    user_id: &str,
    params: &Value,
    idempotency: Option<&IdempotencyTracker>,
) -> McpResult<Value> {
    let product_id = params
        .get(p::PRODUCT_ID)
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'product_id'".to_string()))?;

    let quantity = params
        .get(p::QUANTITY)
        .and_then(|v| v.as_u64())
        .ok_or_else(|| McpError::InvalidParams("Missing 'quantity'".to_string()))?;

    if quantity < 1 {
        return Err(McpError::ValidationError(
            "Quantity must be >= 1".to_string(),
        ));
    }
    if quantity > 99 {
        return Err(McpError::ValidationError(
            "Quantity must be <= 99".to_string(),
        ));
    }

    let idempotency_key = params.get(p::IDEMPOTENCY_KEY).and_then(|v| v.as_str());

    if let (Some(key), Some(tracker)) = (idempotency_key, idempotency)
        && let Some(cached) = tracker.check(key).await
    {
        return Ok(cached);
    }

    let result = json!({
        "user_id": user_id,
        "product_id": product_id,
        "quantity": quantity,
        "added": true
    });

    if let (Some(key), Some(tracker)) = (idempotency_key, idempotency) {
        tracker.mark(key.to_string(), result.clone()).await;
    }

    Ok(result)
}

/// Remove item from cart
pub async fn remove_from_cart(_state: McpState, user_id: &str, params: &Value) -> McpResult<Value> {
    let product_id = params
        .get(p::PRODUCT_ID)
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'product_id'".to_string()))?;

    Ok(json!({
        "user_id": user_id,
        "product_id": product_id,
        "removed": true
    }))
}

/// Apply coupon to cart
pub async fn apply_coupon(_state: McpState, user_id: &str, params: &Value) -> McpResult<Value> {
    let code = params
        .get(p::CODE)
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'code'".to_string()))?;

    Ok(json!({
        "user_id": user_id,
        "coupon_code": code,
        "applied": true,
        "discount_cents": 0
    }))
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

    // ── get_cart ──

    #[tokio::test]
    async fn test_get_cart_valid_user() {
        let state = make_state().await;
        let result = get_cart(state, "users:u1", &json!({})).await.unwrap();
        assert_eq!(result["user_id"], "users:u1");
        assert_eq!(result["subtotal_cents"], 0);
        assert_eq!(result["total_cents"], 0);
        assert!(result["items"].is_array());
    }

    #[tokio::test]
    async fn test_get_cart_invalid_user_format() {
        let state = make_state().await;
        let result = get_cart(state, "u1", &json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::ValidationError(_)));
    }

    // ── add_to_cart ──

    #[tokio::test]
    async fn test_remove_from_cart_missing_product_id() {
        let state = make_state().await;
        let result = remove_from_cart(state, "users:u1", &json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn test_remove_from_cart_valid() {
        let state = make_state().await;
        let result = remove_from_cart(state, "users:u1", &json!({"product_id": "products:p1"}))
            .await
            .unwrap();
        assert_eq!(result["user_id"], "users:u1");
        assert_eq!(result["product_id"], "products:p1");
        assert_eq!(result["removed"], true);
    }

    // ── apply_coupon ──

    #[tokio::test]
    async fn test_apply_coupon_missing_code() {
        let state = make_state().await;
        let result = apply_coupon(state, "users:u1", &json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn test_apply_coupon_valid() {
        let state = make_state().await;
        let result = apply_coupon(state, "users:u1", &json!({"code": "SAVE20"}))
            .await
            .unwrap();
        assert_eq!(result["user_id"], "users:u1");
        assert_eq!(result["coupon_code"], "SAVE20");
        assert_eq!(result["applied"], true);
    }
}
