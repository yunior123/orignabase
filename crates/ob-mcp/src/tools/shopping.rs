//! Shopping tools — cart management

use crate::errors::{McpError, McpResult};
use crate::McpState;
use serde_json::{json, Value};

/// Get user's cart
pub async fn get_cart(_state: McpState, user_id: &str, _params: &Value) -> McpResult<Value> {
    // Verify user ID format
    if !user_id.contains(':') {
        return Err(McpError::ValidationError("Invalid user ID format".to_string()));
    }

    // Fetch user document and extract cart field
    // NOTE: state.db.get_document("users", user_id) -> extract cart array
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
) -> McpResult<Value> {
    let product_id = params
        .get("product_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'product_id'".to_string()))?;

    let quantity = params
        .get("quantity")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| McpError::InvalidParams("Missing 'quantity'".to_string()))?;

    if quantity == 0 {
        return Err(McpError::ValidationError("Quantity must be > 0".to_string()));
    }

    // Check idempotency key if provided
    let _idempotency_key = params.get("idempotency_key").and_then(|v| v.as_str());
    // NOTE: Check idempotency tracker for duplicate add_to_cart calls

    // Validate product exists and get price
    // NOTE: state.db.get_document("products", product_id)
    // Check stock, add to user's cart array, update subtotal/tax

    Ok(json!({
        "user_id": user_id,
        "product_id": product_id,
        "quantity": quantity,
        "added": true
    }))
}

/// Remove item from cart
pub async fn remove_from_cart(
    _state: McpState,
    user_id: &str,
    params: &Value,
) -> McpResult<Value> {
    let product_id = params
        .get("product_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'product_id'".to_string()))?;

    // NOTE: state.db.update_document("users", user_id, { cart: remove product_id })

    Ok(json!({
        "user_id": user_id,
        "product_id": product_id,
        "removed": true
    }))
}

/// Apply coupon to cart
pub async fn apply_coupon(
    _state: McpState,
    user_id: &str,
    params: &Value,
) -> McpResult<Value> {
    let code = params
        .get("code")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'code'".to_string()))?;

    // Fetch coupon from database
    // NOTE: state.db.query("SELECT * FROM coupons WHERE code = $code AND active = true")
    // Validate coupon is active, not expired, meets requirements
    // Add to user's cart.coupons array

    Ok(json!({
        "user_id": user_id,
        "coupon_code": code,
        "applied": true,
        "discount_cents": 0
    }))
}
