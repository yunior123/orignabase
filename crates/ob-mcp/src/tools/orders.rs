//! Order tools — list, get, return requests, checkout

use crate::McpState;
use crate::errors::{McpError, McpResult};
use serde_json::{Value, json};

/// List orders for user
pub async fn list_orders(_state: McpState, user_id: &str, params: &Value) -> McpResult<Value> {
    let _status = params.get("status").and_then(|v| v.as_str());
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(20);
    let offset = params.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);

    if limit > 100 {
        return Err(McpError::ValidationError(
            "Limit must be <= 100".to_string(),
        ));
    }

    // Query orders where buyerId = user_id
    // Filter by status if provided
    // Sort by createdAt DESC (newest first)
    // NOTE: state.db.query("SELECT * FROM orders WHERE buyerId = $userId AND status = $status ORDER BY createdAt DESC LIMIT $limit OFFSET $offset")

    Ok(json!({
        "user_id": user_id,
        "orders": [],
        "total": 0,
        "limit": limit,
        "offset": offset
    }))
}

/// Get order details
pub async fn get_order(_state: McpState, user_id: &str, params: &Value) -> McpResult<Value> {
    let order_id = params
        .get("order_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'order_id'".to_string()))?;

    if !order_id.contains(':') {
        return Err(McpError::ValidationError(
            "Invalid order ID format".to_string(),
        ));
    }

    // Fetch order
    // NOTE: state.db.get_document("orders", order_id)
    // Verify buyerId matches user_id (ownership check)

    Ok(json!({
        "id": order_id,
        "buyer_id": user_id,
        "status": "pending",
        "items": [],
        "total_cents": 0,
        "created_at": 0
    }))
}

/// Request a return for an order
pub async fn request_return(_state: McpState, user_id: &str, params: &Value) -> McpResult<Value> {
    let order_id = params
        .get("order_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'order_id'".to_string()))?;

    let reason = params
        .get("reason")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'reason'".to_string()))?;

    if !order_id.contains(':') {
        return Err(McpError::ValidationError(
            "Invalid order ID format".to_string(),
        ));
    }

    // Fetch order and verify ownership
    // Check if order is in 'delivered' status
    // Check if within 30-day return window
    // Create return request with status 'pending'
    // NOTE: state.db.create_document("return_requests", { orderId, buyerId, reason, status: "pending" })

    Ok(json!({
        "return_id": format!("return_{}", uuid::Uuid::new_v4()),
        "order_id": order_id,
        "buyer_id": user_id,
        "reason": reason,
        "status": "pending"
    }))
}

/// Create checkout session
pub async fn create_checkout(_state: McpState, user_id: &str, params: &Value) -> McpResult<Value> {
    let items = params
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or_else(|| McpError::InvalidParams("Missing 'items' array".to_string()))?;

    if items.is_empty() {
        return Err(McpError::ValidationError(
            "Items array cannot be empty".to_string(),
        ));
    }

    let _shipping_address = params
        .get("shipping_address")
        .ok_or_else(|| McpError::InvalidParams("Missing 'shipping_address'".to_string()))?;

    let _idempotency_key = params.get("idempotency_key").and_then(|v| v.as_str());

    // Validate each item: { product_id, quantity }
    for item in items {
        let _product_id = item
            .get("product_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidParams("Item missing 'product_id'".to_string()))?;

        let _quantity = item
            .get("quantity")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| McpError::InvalidParams("Item missing 'quantity'".to_string()))?;
    }

    // Check idempotency if key provided
    // NOTE: idempotency_tracker.check(idempotency_key) for duplicate checkout

    // Calculate totals (subtotal, tax, shipping, platform fee)
    // Create order documents (one per seller)
    // Call Stripe to create checkout session
    // NOTE: ob-handlers::payments::create_checkout_session()
    // Return session_url

    Ok(json!({
        "checkout_id": uuid::Uuid::new_v4().to_string(),
        "user_id": user_id,
        "session_url": "https://checkout.stripe.com/...",
        "expires_at": chrono::Utc::now().timestamp() + 1800
    }))
}
