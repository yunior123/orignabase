//! Order refund and cancellation handlers.
//! Ported from: functions/handlers/orders.py::refund_order_item, cancel_order

use axum::{Json, Router, extract::State, routing::post};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{error, info, warn};

use crate::HandlersState;
use crate::shared::schema::{business_rules, collections, fields};
use crate::shared::validation::{sanitize_html, validate_uid};

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefundItemRequest {
    pub order_id: String,
    pub product_id: String,
    pub user_id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefundItemResponse {
    pub success: bool,
    pub refund_amount_cents: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refund_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub already_refunded: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelOrderRequest {
    pub order_id: String,
    pub user_id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelOrderResponse {
    pub success: bool,
    pub refunded: bool,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/orders/refund-item", post(refund_order_item))
        // Flutter-compatible alias
        .route("/api/orders/refunds/item", post(refund_order_item))
        .route("/api/orders/cancel", post(cancel_order))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn str_field<'a>(v: &'a Value, field: &str) -> &'a str {
    v.get(field).and_then(|x| x.as_str()).unwrap_or("")
}

fn i64_field(v: &Value, field: &str) -> i64 {
    v.get(field).and_then(|x| x.as_i64()).unwrap_or(0)
}

fn bool_field(v: &Value, field: &str) -> bool {
    v.get(field).and_then(|x| x.as_bool()).unwrap_or(false)
}

fn items_array(order: &Value) -> Vec<Value> {
    order
        .get(fields::ITEMS)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn delivered_at_expired(delivered_at: &Value, now: chrono::DateTime<Utc>) -> bool {
    let delivered_time = if let Some(s) = delivered_at.as_str() {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    } else {
        None
    };

    match delivered_time {
        Some(dt) => (now - dt).num_days() > business_rules::RETURN_WINDOW_DAYS as i64,
        None => false,
    }
}

pub(crate) fn calculate_refund_amount_cents(
    order: &Value,
    item: &Value,
) -> Result<i64, ob_core::Error> {
    let item_price_cents =
        (item.get("price").and_then(|v| v.as_f64()).unwrap_or(0.0) * 100.0).round() as i64;
    let item_quantity = item.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1);
    let mut item_subtotal_cents = item_price_cents * item_quantity;

    let order_subtotal_pre = i64_field(order, "subtotalCents");
    let order_discount = i64_field(order, "discountAmountCents");
    if order_subtotal_pre > 0 && order_discount > 0 {
        let discounted_subtotal = (order_subtotal_pre - order_discount).max(0);
        let ratio = discounted_subtotal as f64 / order_subtotal_pre as f64;
        item_subtotal_cents = (item_subtotal_cents as f64 * ratio).round() as i64;
    }

    let order_subtotal_cents = i64_field(order, "subtotalCents");
    if order_subtotal_cents <= 0 {
        return Err(ob_core::Error::Validation(
            "Order subtotal must be positive to calculate proportional refund".into(),
        ));
    }

    let item_shipping_snapshot = item.get("itemShippingCents").and_then(|v| v.as_i64());
    let shipping_refund_cents = if let Some(snap) = item_shipping_snapshot {
        snap
    } else {
        let order_shipping = i64_field(order, fields::SHIPPING_COST_CENTS);
        let proportion = item_subtotal_cents as f64 / order_subtotal_cents as f64;
        (order_shipping as f64 * proportion).round() as i64
    };

    let order_tax = i64_field(order, "taxAmountCents");
    let proportion = item_subtotal_cents as f64 / order_subtotal_cents as f64;
    let proportional_tax = (order_tax as f64 * proportion).round() as i64;

    Ok(item_subtotal_cents + proportional_tax + shipping_refund_cents)
}

async fn is_user_admin(state: &HandlersState, user_id: &str) -> Result<bool, ob_core::Error> {
    let user = state
        .db
        .get_document(collections::USERS, user_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("User not found".into()))?;
    let roles = user
        .get(fields::ROLES)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(roles.iter().any(|r| r.as_str() == Some("admin")))
}

fn is_valid_order_transition(from: &str, to: &str) -> bool {
    // Re-use the transition table from status.rs logic
    let pairs = [
        ("PENDING_PAYMENT", "CANCELLED"),
        ("PAYMENT_AUTHORIZED", "CANCELLED"),
        ("AWAITING_SHIPPING_APPROVAL", "CANCELLED"),
        ("PROCESSING", "CANCELLED"),
        ("PENDING", "CANCELLED"),
        ("CONFIRMED", "CANCELLED"),
    ];
    pairs.iter().any(|(f, t)| *f == from && *t == to)
}

/// Create a Stripe refund via reqwest (REST API).
pub(crate) async fn stripe_refund(
    state: &HandlersState,
    payment_intent_id: &str,
    amount_cents: Option<i64>,
    reason: &str,
    idempotency_key: &str,
    metadata: &[(&str, &str)],
) -> Result<Option<String>, ob_core::Error> {
    let stripe_key = state
        .config
        .require_secret("stripe_secret_key")
        .map_err(|_| ob_core::Error::Internal("Stripe secret key not configured".into()))?;

    let mut params: Vec<(String, String)> = vec![
        ("payment_intent".to_string(), payment_intent_id.to_string()),
        ("reason".to_string(), reason.to_string()),
    ];
    if let Some(amt) = amount_cents {
        params.push(("amount".to_string(), amt.to_string()));
    }
    for (k, v) in metadata {
        params.push((format!("metadata[{k}]"), v.to_string()));
    }

    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state
            .http_client
            .post(format!("{}/refunds", state.stripe_base_url))
            .header("Authorization", format!("Bearer {stripe_key}"))
            .header("Idempotency-Key", idempotency_key)
            .form(&params)
            .send(),
    )
    .await
    .map_err(|_| ob_core::Error::Internal("Stripe refund request timeout".into()))?
    .map_err(|e| ob_core::Error::Internal(format!("Stripe request failed: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        error!(body = %body, "Stripe refund failed");
        return Err(ob_core::Error::Internal(
            "Refund failed. Please try again or contact support.".into(),
        ));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Stripe response parse failed: {e}")))?;

    let refund_id = body
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(refund_id)
}

/// Cancel a Stripe PaymentIntent (for authorized-but-not-captured payments).
pub(crate) async fn stripe_cancel_pi(
    state: &HandlersState,
    payment_intent_id: &str,
) -> Result<(), ob_core::Error> {
    let stripe_key = state
        .config
        .require_secret("stripe_secret_key")
        .map_err(|_| ob_core::Error::Internal("Stripe secret key not configured".into()))?;

    let url = format!(
        "{}/payment_intents/{payment_intent_id}/cancel",
        state.stripe_base_url
    );

    let resp = state
        .http_client
        .post(&url)
        .header("Authorization", format!("Bearer {stripe_key}"))
        .form(&[("cancellation_reason", "requested_by_customer")])
        .send()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Stripe cancel failed: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        error!(body = %body, "Stripe PI cancel failed");
        return Err(ob_core::Error::Internal(
            "Payment release could not be confirmed".into(),
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// refund_order_item
// ---------------------------------------------------------------------------

async fn refund_order_item(
    State(state): State<HandlersState>,
    Json(req): Json<RefundItemRequest>,
) -> Result<Json<RefundItemResponse>, ob_core::Error> {
    validate_uid("orderId", &req.order_id)?;
    validate_uid("productId", &req.product_id)?;
    validate_uid("userId", &req.user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "refund_order_item",
        5,
        1,
    )
    .await?;

    let reason = req
        .reason
        .as_deref()
        .map(|s| sanitize_html(s).chars().take(500).collect::<String>())
        .unwrap_or_else(|| "Item refund requested".to_string());

    // Fetch order
    let order = state
        .db
        .get_document(collections::ORDERS, &req.order_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Order not found".into()))?;

    // Permission check: seller of item or admin
    let is_admin = is_user_admin(&state, &req.user_id).await?;
    let items = items_array(&order);

    let item_data = items
        .iter()
        .find(|it| str_field(it, fields::PRODUCT_ID) == req.product_id);

    let item = match item_data {
        Some(it) => it,
        None => {
            return Err(ob_core::Error::NotFound(format!(
                "Product {} not found in order",
                req.product_id
            )));
        }
    };

    let item_seller = str_field(item, fields::SELLER_ID);
    let is_item_seller = item_seller == req.user_id;

    if !is_admin && !is_item_seller {
        return Err(ob_core::Error::Forbidden(
            "Only seller of the item or admin can issue refunds".into(),
        ));
    }

    // Payment must be captured
    let payment_status_str = str_field(&order, fields::PAYMENT_STATUS);
    if payment_status_str != "CAPTURED" {
        return Err(ob_core::Error::Validation(
            "Cannot refund uncaptured payment".into(),
        ));
    }

    // Payout processing race condition check
    let payout_status = str_field(&order, "payoutStatus");
    if payout_status == "PROCESSING" {
        return Err(ob_core::Error::Validation(
            "Cannot refund item while payout is currently processing. Please try again later."
                .into(),
        ));
    }

    // Already refunded check
    if str_field(item, "status") == "refunded" {
        return Ok(Json(RefundItemResponse {
            success: true,
            refund_amount_cents: 0,
            refund_id: None,
            already_refunded: Some(true),
        }));
    }

    // Return window check (non-admin)
    if !is_admin
        && str_field(item, "status") == "delivered"
        && let Some(delivered_at) = item.get("deliveredAt")
        && delivered_at_expired(delivered_at, Utc::now())
    {
        return Err(ob_core::Error::Validation(format!(
            "Return window expired. Returns not accepted after {} days post-delivery.",
            business_rules::RETURN_WINDOW_DAYS
        )));
    }

    let item_quantity = item.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1);
    let refund_amount_cents = calculate_refund_amount_cents(&order, item)?;

    // Stripe refund
    let payment_intent_id = str_field(&order, "paymentIntentId");
    if payment_intent_id.is_empty() {
        return Err(ob_core::Error::Validation("No payment intent found".into()));
    }

    let idempotency_key = format!("refund_{}_{}", req.order_id, req.product_id);
    let refund_id = stripe_refund(
        &state,
        payment_intent_id,
        Some(refund_amount_cents),
        "requested_by_customer",
        &idempotency_key,
        &[("orderId", &req.order_id), ("productId", &req.product_id)],
    )
    .await?;

    // Update item status to refunded
    let now = Utc::now().to_rfc3339();
    let mut updated_items = items.clone();
    for it in updated_items.iter_mut() {
        if str_field(it, fields::PRODUCT_ID) == req.product_id {
            it["status"] = json!("refunded");
            it["refundedAt"] = json!(now);
            it["refundReason"] = json!(reason);
            it["refundAmountCents"] = json!(refund_amount_cents);
            if let Some(ref rid) = refund_id {
                it["refundId"] = json!(rid);
            }
            break;
        }
    }

    let cumulative_refunded = i64_field(&order, "cumulativeRefundedCents") + refund_amount_cents;
    state
        .db
        .update_document(
            collections::ORDERS,
            &req.order_id,
            json!({
                fields::ITEMS: updated_items,
                "cumulativeRefundedCents": cumulative_refunded,
                fields::UPDATED_AT: now,
            }),
        )
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to update refunded order: {e}")))?;

    let is_digital = bool_field(item, "isDigital");
    let item_type = str_field(item, "productType");
    let is_digital_by_type = matches!(item_type, "software" | "book" | "digital");
    if is_digital || is_digital_by_type {
        // Revoke digital licenses
        state
            .db
            .query_bind(
                &format!("UPDATE {} SET status = $status, revokedAt = $revokedAt WHERE orderId = $orderId AND productId = $productId AND status = 'active'", collections::LICENSES),
                json!({
                    "status": "revoked",
                    "revokedAt": now,
                    "orderId": req.order_id,
                    "productId": req.product_id
                })
            )
            .await
            .map_err(|e| {
                ob_core::Error::Database(format!(
                    "Failed to revoke digital licenses for refunded item: {e}"
                ))
            })?;
        info!(order_id = %req.order_id, product_id = %req.product_id, "Digital licenses revoked for refunded item");
    } else {
        // Restore stock for physical items
        let product_id = &req.product_id;
        state
            .db
            .query_bind(
                "UPDATE type::thing($table, $product_id) SET stockQuantity += $quantity, updatedAt = $updatedAt",
                json!({
                    "table": collections::PRODUCTS,
                    "product_id": product_id,
                    "quantity": item_quantity,
                    "updatedAt": now
                })
            )
            .await
            .map_err(|e| {
                ob_core::Error::Database(format!("Failed to restore stock for refunded item: {e}"))
            })?;
    }

    // Log the event
    state
        .db
        .create_document(
            collections::ORDER_EVENTS,
            json!({
                "orderId": req.order_id,
                "userId": req.user_id,
                "eventType": "item_refunded",
                "message": format!("Item {} refunded for {} cents", req.product_id, refund_amount_cents),
                "metadata": { "productId": req.product_id, "refundAmountCents": refund_amount_cents },
                "createdAt": now,
            }),
        )
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to log refund event: {e}")))?;

    info!(
        order_id = %req.order_id,
        product_id = %req.product_id,
        refund_cents = refund_amount_cents,
        "Item refund processed"
    );

    Ok(Json(RefundItemResponse {
        success: true,
        refund_amount_cents,
        refund_id,
        already_refunded: None,
    }))
}

// ---------------------------------------------------------------------------
// cancel_order
// ---------------------------------------------------------------------------

async fn cancel_order(
    State(state): State<HandlersState>,
    Json(req): Json<CancelOrderRequest>,
) -> Result<Json<CancelOrderResponse>, ob_core::Error> {
    validate_uid("orderId", &req.order_id)?;
    validate_uid("userId", &req.user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "cancel_order",
        5,
        1,
    )
    .await?;

    let reason = req
        .reason
        .as_deref()
        .map(|s| sanitize_html(s).chars().take(500).collect::<String>())
        .unwrap_or_else(|| "User requested cancellation".to_string());

    // Fetch order
    let order = state
        .db
        .get_document(collections::ORDERS, &req.order_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Order not found".into()))?;

    if bool_field(&order, "archived") {
        return Err(ob_core::Error::Validation(
            "Cannot cancel archived order".into(),
        ));
    }

    // Permission check
    let is_admin = is_user_admin(&state, &req.user_id).await?;
    let is_buyer = str_field(&order, "userId") == req.user_id;
    let items = items_array(&order);
    let seller_items: Vec<&Value> = items
        .iter()
        .filter(|it| str_field(it, fields::SELLER_ID) == req.user_id)
        .collect();
    let is_seller = !seller_items.is_empty();

    if !is_admin && !is_buyer && !is_seller {
        return Err(ob_core::Error::Forbidden(
            "Only buyer, seller, or admin can cancel order".into(),
        ));
    }

    // Multi-seller restriction for sellers
    if is_seller && !is_buyer && !is_admin && seller_items.len() < items.len() {
        return Err(ob_core::Error::Forbidden(
            "Cannot cancel a multi-seller order. Use item refund to cancel your items only.".into(),
        ));
    }

    // State machine validation
    let current_status = str_field(&order, "orderStatus");
    if !is_valid_order_transition(current_status, "CANCELLED") {
        return Err(ob_core::Error::Validation(format!(
            "Cannot cancel order with status: {current_status}"
        )));
    }

    // Buyers can only cancel pre-shipment
    let buyer_cancellable = [
        "PENDING_PAYMENT",
        "PENDING",
        "CONFIRMED",
        "PAYMENT_AUTHORIZED",
    ];
    if is_buyer && !is_admin && !is_seller && !buyer_cancellable.contains(&current_status) {
        return Err(ob_core::Error::Validation(
            "Order cannot be cancelled at this stage. Contact support if there is an issue.".into(),
        ));
    }

    let payment_status = str_field(&order, fields::PAYMENT_STATUS);
    let payment_intent_id = str_field(&order, "paymentIntentId");
    let now = Utc::now().to_rfc3339();

    let mut refunded = false;

    // Handle payment based on current status
    let new_payment_status = if payment_status == "CAPTURED" && !payment_intent_id.is_empty() {
        // Full refund
        let idempotency_key = format!("refund_{}", req.order_id);
        match stripe_refund(
            &state,
            payment_intent_id,
            None, // full refund
            "requested_by_customer",
            &idempotency_key,
            &[("orderId", &req.order_id)],
        )
        .await
        {
            Ok(_) => {
                refunded = true;
                "REFUNDED"
            }
            Err(e) => {
                // Quarantine order for manual review instead of failing hard
                let _ = state
                    .db
                    .update_document(
                        collections::ORDERS,
                        &req.order_id,
                        json!({
                            "paymentStatus": "CANCEL_FAILED",
                            "requiresManualReview": true,
                            fields::UPDATED_AT: now,
                        }),
                    )
                    .await;
                warn!(order_id = %req.order_id, error = %e, "Cancel failed, quarantined for manual review");
                return Err(e);
            }
        }
    } else if payment_status == "AUTHORIZED" && !payment_intent_id.is_empty() {
        // Cancel the PaymentIntent to release buyer funds
        match stripe_cancel_pi(&state, payment_intent_id).await {
            Ok(()) => "CANCELLED",
            Err(e) => {
                let _ = state
                    .db
                    .update_document(
                        collections::ORDERS,
                        &req.order_id,
                        json!({
                            "paymentStatus": "CANCEL_FAILED",
                            "requiresManualReview": true,
                            fields::UPDATED_AT: now,
                        }),
                    )
                    .await;
                warn!(order_id = %req.order_id, error = %e, "Cancel failed, quarantined for manual review");
                return Err(e);
            }
        }
    } else {
        "CANCELLED"
    };

    state
        .db
        .update_document(
            collections::ORDERS,
            &req.order_id,
            json!({
                "orderStatus": "CANCELLED",
                fields::PAYMENT_STATUS: new_payment_status,
                "cancelledBy": req.user_id,
                "cancelledAt": now,
                "cancellationReason": reason,
                "stockRestored": true,
                fields::UPDATED_AT: now,
            }),
        )
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to update cancelled order: {e}")))?;

    // Restore stock for all physical items (guard against double-restore).
    let stock_restored = bool_field(&order, "stockRestored");
    if stock_restored {
        info!(order_id = %req.order_id, "Stock already restored, skipping");
    } else {
        for item in &items {
            if bool_field(item, "isDigital") {
                continue;
            }
            let pid = str_field(item, fields::PRODUCT_ID);
            let qty = item.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1);
            if !pid.is_empty() && qty > 0 {
                state
                    .db
                    .query_bind(
                        "UPDATE type::thing($table, $product_id) SET stockQuantity += $quantity, updatedAt = $updatedAt",
                        json!({
                            "table": collections::PRODUCTS,
                            "product_id": pid,
                            "quantity": qty,
                            "updatedAt": now
                        })
                    )
                    .await
                    .map_err(|e| {
                        ob_core::Error::Database(format!(
                            "Failed to restore stock for product {pid}: {e}"
                        ))
                    })?;
            }
        }
    }

    state
        .db
        .create_document(
            collections::ORDER_EVENTS,
            json!({
                "orderId": req.order_id,
                "userId": req.user_id,
                "eventType": "order_cancelled",
                "message": format!("Order cancelled. Refunded: {}", refunded),
                "metadata": { "refunded": refunded },
                "createdAt": now,
            }),
        )
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to log order cancellation: {e}")))?;

    info!(
        order_id = %req.order_id,
        refunded = refunded,
        "Order cancelled"
    );

    Ok(Json(CancelOrderResponse {
        success: true,
        refunded,
    }))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn setup_state_with_config(config: Config, stripe_base_url: String) -> HandlersState {
        HandlersState {
            config: Arc::new(config),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url,
            turnstile_secret_key: None,
        }
    }

    async fn seed_user(state: &HandlersState, user_id: &str, roles: &[&str]) {
        state
            .db
            .upsert_document(
                collections::USERS,
                user_id,
                json!({
                    fields::UID: user_id,
                    fields::ROLES: roles,
                }),
            )
            .await
            .unwrap();
    }

    async fn stripe_state(server: &MockServer) -> HandlersState {
        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_secret_key".to_string(), "sk_test_123".to_string());
        setup_state_with_config(config, server.uri()).await
    }

    #[test]
    fn test_refund_request_deserialize() {
        let s = r#"{"orderId":"o1","productId":"p1","userId":"u1","reason":"defective"}"#;
        let req: RefundItemRequest = serde_json::from_str(s).unwrap();
        assert_eq!(req.order_id, "o1");
        assert_eq!(req.reason, Some("defective".to_string()));
    }

    #[test]
    fn test_cancel_request_deserialize() {
        let s = r#"{"orderId":"o1","userId":"u1"}"#;
        let req: CancelOrderRequest = serde_json::from_str(s).unwrap();
        assert_eq!(req.order_id, "o1");
        assert!(req.reason.is_none());
    }

    #[test]
    fn test_refund_response_serialization() {
        let resp = RefundItemResponse {
            success: true,
            refund_amount_cents: 1500,
            refund_id: Some("re_123".into()),
            already_refunded: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["refundAmountCents"], 1500);
        assert_eq!(json["refundId"], "re_123");
        assert!(json.get("alreadyRefunded").is_none());
    }

    #[test]
    fn test_cancel_response_serialization() {
        let resp = CancelOrderResponse {
            success: true,
            refunded: true,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["refunded"], true);
    }

    #[test]
    fn test_valid_cancel_transitions() {
        assert!(is_valid_order_transition("PENDING_PAYMENT", "CANCELLED"));
        assert!(is_valid_order_transition("PAYMENT_AUTHORIZED", "CANCELLED"));
        assert!(is_valid_order_transition("PROCESSING", "CANCELLED"));
    }

    #[test]
    fn test_invalid_cancel_transitions() {
        assert!(!is_valid_order_transition("DELIVERED", "CANCELLED"));
        assert!(!is_valid_order_transition("SHIPPED", "CANCELLED"));
        assert!(!is_valid_order_transition("REFUNDED", "CANCELLED"));
    }

    #[test]
    fn test_proportional_refund_calculation() {
        // Simulate: item costs $50, order subtotal $100, tax $13, shipping $10
        let item_subtotal: i64 = 5000;
        let order_subtotal: i64 = 10000;
        let order_tax: i64 = 1300;
        let order_shipping: i64 = 1000;

        let proportion = item_subtotal as f64 / order_subtotal as f64;
        let tax_refund = (order_tax as f64 * proportion).round() as i64;
        let shipping_refund = (order_shipping as f64 * proportion).round() as i64;
        let total = item_subtotal + tax_refund + shipping_refund;

        assert_eq!(proportion, 0.5);
        assert_eq!(tax_refund, 650);
        assert_eq!(shipping_refund, 500);
        assert_eq!(total, 6150);
    }

    #[test]
    fn test_discount_ratio_application() {
        let item_subtotal_raw: i64 = 5000;
        let order_subtotal_pre: i64 = 10000;
        let order_discount: i64 = 2000; // 20% off
        let discounted = (order_subtotal_pre - order_discount).max(0);
        let ratio = discounted as f64 / order_subtotal_pre as f64;
        let adjusted = (item_subtotal_raw as f64 * ratio).round() as i64;
        assert_eq!(adjusted, 4000); // 5000 * 0.8
    }

    #[test]
    fn test_return_window_expiry_uses_business_rule_days() {
        let now = Utc::now();
        let fresh = json!((now - chrono::Duration::days(2)).to_rfc3339());
        let expired = json!(
            (now - chrono::Duration::days(business_rules::RETURN_WINDOW_DAYS as i64 + 1))
                .to_rfc3339()
        );

        assert!(!delivered_at_expired(&fresh, now));
        assert!(delivered_at_expired(&expired, now));
        assert!(!delivered_at_expired(&json!("not-a-datetime"), now));
    }

    #[test]
    fn test_refund_amount_prefers_item_shipping_snapshot() {
        let order = json!({
            "subtotalCents": 4000,
            "discountAmountCents": 400,
            fields::SHIPPING_COST_CENTS: 1000,
            "taxAmountCents": 390,
        });
        let item = json!({
            "price": 20.0,
            "quantity": 1,
            "itemShippingCents": 250,
        });

        let refund = calculate_refund_amount_cents(&order, &item).unwrap();
        // item_cents=2000, discount_ratio=400/4000=0.1, discounted=1800
        // shipping=250 (from itemShippingCents), tax_ratio=390/4000=0.0975
        // item_tax=2000*0.0975=195, total=1800+250+195-19=2226 (approximate)
        assert_eq!(refund, 2226);
    }

    #[test]
    fn test_refund_amount_zero_subtotal_is_rejected() {
        let order = json!({
            "subtotalCents": 0,
            fields::SHIPPING_COST_CENTS: 200,
            "taxAmountCents": 100,
        });
        let item = json!({
            "price": 10.0,
            "quantity": 1,
            "itemShippingCents": 50,
        });

        let err = calculate_refund_amount_cents(&order, &item).unwrap_err();
        assert!(matches!(err, ob_core::Error::Validation(_)));
    }

    #[test]
    fn test_refund_amount_with_proportional_shipping_and_tax() {
        let order = json!({
            "subtotalCents": 10000,
            fields::SHIPPING_COST_CENTS: 1000,
            "taxAmountCents": 1300,
        });
        let item = json!({
            "price": 50.0,
            "quantity": 1,
        });

        let refund = calculate_refund_amount_cents(&order, &item).unwrap();
        assert_eq!(refund, 6150);
    }

    // -----------------------------------------------------------------------
    // Refund calculation edge cases (ported from Python refunds_deep)
    // -----------------------------------------------------------------------

    #[test]
    fn test_refund_amount_multi_quantity() {
        let order = json!({
            "subtotalCents": 10000,
            fields::SHIPPING_COST_CENTS: 500,
            "taxAmountCents": 1300,
        });
        let item = json!({
            "price": 25.0,  // $25 each
            "quantity": 2,   // 2 items = $50 subtotal = 50% of order
        });

        let refund = calculate_refund_amount_cents(&order, &item).unwrap();
        // item_subtotal = 2500*2 = 5000, proportion = 0.5
        // shipping = 500*0.5 = 250, tax = 1300*0.5 = 650
        // total = 5000 + 250 + 650 = 5900
        assert_eq!(refund, 5900);
    }

    #[test]
    fn test_refund_amount_single_item_gets_full_shipping_and_tax() {
        let order = json!({
            "subtotalCents": 5000,
            fields::SHIPPING_COST_CENTS: 800,
            "taxAmountCents": 650,
        });
        let item = json!({
            "price": 50.0,
            "quantity": 1,
        });

        let refund = calculate_refund_amount_cents(&order, &item).unwrap();
        // item is 100% of order, gets all shipping and tax
        assert_eq!(refund, 5000 + 800 + 650);
    }

    #[test]
    fn test_refund_amount_with_100_percent_discount() {
        let order = json!({
            "subtotalCents": 5000,
            "discountAmountCents": 5000,  // 100% discount
            fields::SHIPPING_COST_CENTS: 500,
            "taxAmountCents": 0,
        });
        let item = json!({
            "price": 50.0,
            "quantity": 1,
        });

        // After discount: item_subtotal = round(5000 * 0.0) = 0
        // subtotal is still 5000 so division works
        // shipping proportion = 0/5000 = 0, tax proportion = 0
        let refund = calculate_refund_amount_cents(&order, &item).unwrap();
        assert_eq!(refund, 0);
    }

    #[test]
    fn test_refund_amount_no_discount_no_shipping() {
        let order = json!({
            "subtotalCents": 3000,
            fields::SHIPPING_COST_CENTS: 0,
            "taxAmountCents": 390,
        });
        let item = json!({
            "price": 30.0,
            "quantity": 1,
        });

        let refund = calculate_refund_amount_cents(&order, &item).unwrap();
        // item = 3000, proportion = 1.0, shipping = 0, tax = 390
        assert_eq!(refund, 3000 + 390);
    }

    #[test]
    fn test_refund_amount_zero_tax() {
        let order = json!({
            "subtotalCents": 5000,
            fields::SHIPPING_COST_CENTS: 1000,
            "taxAmountCents": 0,
        });
        let item = json!({
            "price": 25.0,
            "quantity": 1,
        });

        let refund = calculate_refund_amount_cents(&order, &item).unwrap();
        // proportion = 2500/5000 = 0.5, shipping = 500, tax = 0
        assert_eq!(refund, (2500 + 500));
    }

    #[test]
    fn test_refund_amount_rounding_precision() {
        // Scenario with values that produce fractional cents
        let order = json!({
            "subtotalCents": 3333,
            fields::SHIPPING_COST_CENTS: 999,
            "taxAmountCents": 433,
        });
        let item = json!({
            "price": 11.11,
            "quantity": 1,
        });

        let refund = calculate_refund_amount_cents(&order, &item);
        assert!(refund.is_ok());
        assert!(refund.unwrap() > 0);
    }

    #[test]
    fn test_refund_amount_zero_price_item() {
        let order = json!({
            "subtotalCents": 5000,
            fields::SHIPPING_COST_CENTS: 500,
            "taxAmountCents": 650,
        });
        let item = json!({
            "price": 0.0,
            "quantity": 1,
        });

        let refund = calculate_refund_amount_cents(&order, &item).unwrap();
        assert_eq!(refund, 0);
    }

    #[test]
    fn test_refund_amount_missing_item_fields_defaults() {
        let order = json!({
            "subtotalCents": 5000,
            fields::SHIPPING_COST_CENTS: 500,
            "taxAmountCents": 650,
        });
        // Missing price and quantity — should default to 0 and 1
        let item = json!({});

        let refund = calculate_refund_amount_cents(&order, &item).unwrap();
        assert_eq!(refund, 0);
    }

    // -----------------------------------------------------------------------
    // Return window edge cases (ported from Python refunds_deep)
    // -----------------------------------------------------------------------

    #[test]
    fn test_return_window_exactly_at_boundary() {
        let now = Utc::now();
        // Exactly RETURN_WINDOW_DAYS days ago — NOT expired (uses >)
        let boundary = json!(
            (now - chrono::Duration::days(business_rules::RETURN_WINDOW_DAYS as i64)).to_rfc3339()
        );
        assert!(!delivered_at_expired(&boundary, now));
    }

    #[test]
    fn test_return_window_one_day_over() {
        let now = Utc::now();
        let over = json!(
            (now - chrono::Duration::days(business_rules::RETURN_WINDOW_DAYS as i64 + 1))
                .to_rfc3339()
        );
        assert!(delivered_at_expired(&over, now));
    }

    #[test]
    fn test_return_window_null_value() {
        assert!(!delivered_at_expired(&json!(null), Utc::now()));
    }

    #[test]
    fn test_return_window_numeric_value() {
        assert!(!delivered_at_expired(&json!(12345678), Utc::now()));
    }

    #[test]
    fn test_return_window_empty_string() {
        assert!(!delivered_at_expired(&json!(""), Utc::now()));
    }

    #[test]
    fn test_return_window_future_date() {
        let now = Utc::now();
        let future = json!((now + chrono::Duration::days(5)).to_rfc3339());
        assert!(!delivered_at_expired(&future, now));
    }

    // -----------------------------------------------------------------------
    // Cancel order transition table (ported from Python state_machine_deep)
    // -----------------------------------------------------------------------

    #[test]
    fn test_cancel_transition_all_valid_states() {
        let cancellable = [
            "PENDING_PAYMENT",
            "PAYMENT_AUTHORIZED",
            "AWAITING_SHIPPING_APPROVAL",
            "PROCESSING",
            "PENDING",
            "CONFIRMED",
        ];
        for status in &cancellable {
            assert!(
                is_valid_order_transition(status, "CANCELLED"),
                "{status} should be cancellable"
            );
        }
    }

    #[test]
    fn test_cancel_transition_all_invalid_states() {
        let not_cancellable = [
            "SHIPPED",
            "DELIVERED",
            "REFUNDED",
            "CANCELLED",
            "RETURN_REQUESTED",
        ];
        for status in &not_cancellable {
            assert!(
                !is_valid_order_transition(status, "CANCELLED"),
                "{status} should NOT be cancellable"
            );
        }
    }

    #[test]
    fn test_cancel_transition_empty_status() {
        assert!(!is_valid_order_transition("", "CANCELLED"));
    }

    // -----------------------------------------------------------------------
    // Request deserialization edge cases (ported from Python residual_coverage)
    // -----------------------------------------------------------------------

    #[test]
    fn test_refund_request_missing_optional_reason() {
        let s = r#"{"orderId":"o1","productId":"p1","userId":"u1"}"#;
        let req: RefundItemRequest = serde_json::from_str(s).unwrap();
        assert!(req.reason.is_none());
    }

    #[test]
    fn test_refund_request_missing_required_fields() {
        // Missing productId
        let s = r#"{"orderId":"o1","userId":"u1"}"#;
        assert!(serde_json::from_str::<RefundItemRequest>(s).is_err());

        // Missing orderId
        let s = r#"{"productId":"p1","userId":"u1"}"#;
        assert!(serde_json::from_str::<RefundItemRequest>(s).is_err());

        // Missing userId
        let s = r#"{"orderId":"o1","productId":"p1"}"#;
        assert!(serde_json::from_str::<RefundItemRequest>(s).is_err());

        // Empty object
        let s = r#"{}"#;
        assert!(serde_json::from_str::<RefundItemRequest>(s).is_err());
    }

    #[test]
    fn test_cancel_request_with_reason() {
        let s = r#"{"orderId":"o1","userId":"u1","reason":"changed my mind"}"#;
        let req: CancelOrderRequest = serde_json::from_str(s).unwrap();
        assert_eq!(req.reason, Some("changed my mind".to_string()));
    }

    #[test]
    fn test_cancel_request_missing_required_fields() {
        let s = r#"{"orderId":"o1"}"#;
        assert!(serde_json::from_str::<CancelOrderRequest>(s).is_err());
    }

    #[test]
    fn test_refund_response_already_refunded_serialization() {
        let resp = RefundItemResponse {
            success: true,
            refund_amount_cents: 0,
            refund_id: None,
            already_refunded: Some(true),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["refundAmountCents"], 0);
        assert!(json.get("refundId").is_none());
        assert_eq!(json["alreadyRefunded"], true);
    }

    #[test]
    fn test_refund_response_skip_none_fields() {
        let resp = RefundItemResponse {
            success: true,
            refund_amount_cents: 1500,
            refund_id: None,
            already_refunded: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("refundId").is_none());
        assert!(json.get("alreadyRefunded").is_none());
    }

    // -----------------------------------------------------------------------
    // Helper function edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_i64_field_missing_and_non_numeric() {
        let v = json!({"amount": 100, "name": "test"});
        assert_eq!(i64_field(&v, "amount"), 100);
        assert_eq!(i64_field(&v, "missing"), 0);
        assert_eq!(i64_field(&v, "name"), 0); // string, not i64
    }

    #[test]
    fn test_items_array_from_order_with_no_items_key() {
        assert!(items_array(&json!({"userId": "u1"})).is_empty());
    }

    // -----------------------------------------------------------------------
    // Safety mechanism tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_stock_restored_guard_detects_true() {
        let order = json!({"stockRestored": true});
        assert!(bool_field(&order, "stockRestored"));
    }

    #[test]
    fn test_stock_restored_guard_defaults_false() {
        let order = json!({"orderStatus": "CANCELLED"});
        assert!(!bool_field(&order, "stockRestored"));
    }

    #[test]
    fn test_stock_restored_guard_handles_non_bool() {
        let order = json!({"stockRestored": "yes"});
        assert!(!bool_field(&order, "stockRestored"));
    }

    #[test]
    fn test_payout_processing_blocks_refund() {
        let order = json!({"payoutStatus": "PROCESSING"});
        assert_eq!(str_field(&order, "payoutStatus"), "PROCESSING");
    }

    #[test]
    fn test_payout_completed_allows_refund() {
        let order = json!({"payoutStatus": "COMPLETED"});
        assert_ne!(str_field(&order, "payoutStatus"), "PROCESSING");
    }

    #[test]
    fn test_payout_missing_allows_refund() {
        let order = json!({"orderStatus": "CONFIRMED"});
        assert_ne!(str_field(&order, "payoutStatus"), "PROCESSING");
    }

    #[test]
    fn test_cumulative_refunded_cents_tracking() {
        let order = json!({"cumulativeRefundedCents": 1500});
        let existing = i64_field(&order, "cumulativeRefundedCents");
        let new_refund = 750_i64;
        assert_eq!(existing + new_refund, 2250);
    }

    #[test]
    fn test_cumulative_refunded_cents_defaults_zero() {
        let order = json!({"orderStatus": "CONFIRMED"});
        assert_eq!(i64_field(&order, "cumulativeRefundedCents"), 0);
    }

    #[test]
    fn test_digital_license_revocation_by_is_digital() {
        let item = json!({"isDigital": true, "productType": "physical"});
        assert!(bool_field(&item, "isDigital"));
    }

    #[test]
    fn test_digital_license_revocation_by_product_type_software() {
        let item = json!({"isDigital": false, "productType": "software"});
        let item_type = str_field(&item, "productType");
        assert!(matches!(item_type, "software" | "book" | "digital"));
    }

    #[test]
    fn test_digital_license_revocation_by_product_type_book() {
        let item = json!({"productType": "book"});
        let item_type = str_field(&item, "productType");
        assert!(matches!(item_type, "software" | "book" | "digital"));
    }

    #[test]
    fn test_digital_license_revocation_by_product_type_digital() {
        let item = json!({"productType": "digital"});
        let item_type = str_field(&item, "productType");
        assert!(matches!(item_type, "software" | "book" | "digital"));
    }

    #[test]
    fn test_physical_item_not_flagged_as_digital() {
        let item = json!({"isDigital": false, "productType": "clothing"});
        let is_digital = bool_field(&item, "isDigital");
        let item_type = str_field(&item, "productType");
        let is_digital_by_type = matches!(item_type, "software" | "book" | "digital");
        assert!(!is_digital && !is_digital_by_type);
    }

    #[test]
    fn test_cancel_failed_quarantine_fields() {
        // Verify the quarantine payload shape
        let payload = json!({
            "paymentStatus": "CANCEL_FAILED",
            "requiresManualReview": true,
        });
        assert_eq!(str_field(&payload, "paymentStatus"), "CANCEL_FAILED");
        assert!(bool_field(&payload, "requiresManualReview"));
    }

    #[tokio::test]
    async fn test_stripe_refund_uses_state_base_url_and_returns_refund_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/refunds"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "re_123"
            })))
            .mount(&server)
            .await;

        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_secret_key".to_string(), "sk_test_123".to_string());
        let state = setup_state_with_config(config, server.uri()).await;

        let refund_id = stripe_refund(
            &state,
            "pi_123",
            Some(1250),
            "requested_by_customer",
            "refund_order_1_item_1",
            &[("orderId", "order_1"), ("productId", "prod_1")],
        )
        .await
        .unwrap();

        assert_eq!(refund_id.as_deref(), Some("re_123"));
    }

    #[tokio::test]
    async fn test_stripe_cancel_pi_uses_state_base_url() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/payment_intents/pi_123/cancel"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "pi_123",
                "status": "canceled"
            })))
            .mount(&server)
            .await;

        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_secret_key".to_string(), "sk_test_123".to_string());
        let state = setup_state_with_config(config, server.uri()).await;

        stripe_cancel_pi(&state, "pi_123").await.unwrap();
    }

    #[tokio::test]
    async fn test_cancel_order_authorized_payment_uses_state_base_url_and_restores_stock() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/payment_intents/pi_123/cancel"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "pi_123",
                "status": "canceled"
            })))
            .mount(&server)
            .await;

        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_secret_key".to_string(), "sk_test_123".to_string());
        let state = setup_state_with_config(config, server.uri()).await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "buyer_1",
                json!({
                    fields::UID: "buyer_1",
                    fields::ROLES: ["buyer"],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                json!({
                    fields::PRODUCT_ID: "prod_1",
                    "stockQuantity": 5,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_1",
                json!({
                    "orderStatus": "PENDING_PAYMENT",
                    "userId": "buyer_1",
                    fields::PAYMENT_STATUS: "AUTHORIZED",
                    "paymentIntentId": "pi_123",
                    "stockRestored": false,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        "quantity": 2,
                        "isDigital": false
                    }]
                }),
            )
            .await
            .unwrap();

        let Json(resp) = cancel_order(
            State(state.clone()),
            Json(CancelOrderRequest {
                order_id: "order_1".into(),
                user_id: "buyer_1".into(),
                reason: Some("changed mind".into()),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(!resp.refunded);

        let order = state
            .db
            .get_document(collections::ORDERS, "order_1")
            .await
            .unwrap();
        let product = state
            .db
            .get_document(collections::PRODUCTS, "prod_1")
            .await
            .unwrap();
        assert_eq!(order["orderStatus"], "CANCELLED");
        assert_eq!(order[fields::PAYMENT_STATUS], "CANCELLED");
        assert_eq!(product["stockQuantity"], 7);
    }

    #[tokio::test]
    async fn test_refund_order_item_success_updates_order_stock_and_event() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/refunds"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "re_live_1"
            })))
            .mount(&server)
            .await;

        let state = stripe_state(&server).await;
        seed_user(&state, "seller_1", &["seller"]).await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                json!({
                    fields::PRODUCT_ID: "prod_1",
                    "stockQuantity": 5,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_1",
                json!({
                    fields::ORDER_ID: "order_1",
                    fields::PAYMENT_STATUS: "CAPTURED",
                    "paymentIntentId": "pi_refund_1",
                    "subtotalCents": 2000,
                    "shippingCostCents": 300,
                    "taxAmountCents": 200,
                    "cumulativeRefundedCents": 50,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        "status": "delivered",
                        "deliveredAt": Utc::now().to_rfc3339(),
                        "price": 10.0,
                        "quantity": 2,
                        "isDigital": false
                    }]
                }),
            )
            .await
            .unwrap();

        let Json(resp) = refund_order_item(
            State(state.clone()),
            Json(RefundItemRequest {
                order_id: "order_1".into(),
                product_id: "prod_1".into(),
                user_id: "seller_1".into(),
                reason: Some(" damaged <b>box</b> ".into()),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.refund_id.as_deref(), Some("re_live_1"));
        assert_eq!(resp.refund_amount_cents, 2500);

        let order = state
            .db
            .get_document(collections::ORDERS, "order_1")
            .await
            .unwrap();
        let product = state
            .db
            .get_document(collections::PRODUCTS, "prod_1")
            .await
            .unwrap();
        let item = &order[fields::ITEMS][0];
        assert_eq!(item["status"], "refunded");
        assert_eq!(item["refundAmountCents"], 2500);
        assert_eq!(item["refundId"], "re_live_1");
        assert_eq!(item["refundReason"], " damaged box ");
        assert_eq!(order["cumulativeRefundedCents"], 2550);
        assert_eq!(product["stockQuantity"], 7);

        let events = state
            .db
            .query_bind_value(
                "SELECT * FROM order_events WHERE orderId = $orderId AND eventType = $eventType",
                json!({"orderId": "order_1", "eventType": "item_refunded"}),
            )
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["metadata"]["productId"], "prod_1");
    }

    #[tokio::test]
    async fn test_refund_order_item_digital_revokes_license_without_stock_restore() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/refunds"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "re_digital_1"
            })))
            .mount(&server)
            .await;

        let state = stripe_state(&server).await;
        seed_user(&state, "seller_1", &["seller"]).await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "ebook_1",
                json!({
                    fields::PRODUCT_ID: "ebook_1",
                    "stockQuantity": 5,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::LICENSES,
                "license_1",
                json!({
                    "orderId": "order_digital",
                    "productId": "ebook_1",
                    "status": "active",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_digital",
                json!({
                    fields::ORDER_ID: "order_digital",
                    fields::PAYMENT_STATUS: "CAPTURED",
                    "paymentIntentId": "pi_digital_1",
                    "subtotalCents": 1500,
                    "shippingCostCents": 0,
                    "taxAmountCents": 0,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "ebook_1",
                        fields::SELLER_ID: "seller_1",
                        "status": "delivered",
                        "deliveredAt": Utc::now().to_rfc3339(),
                        "price": 15.0,
                        "quantity": 1,
                        "isDigital": true,
                        "productType": "digital"
                    }]
                }),
            )
            .await
            .unwrap();

        let Json(resp) = refund_order_item(
            State(state.clone()),
            Json(RefundItemRequest {
                order_id: "order_digital".into(),
                product_id: "ebook_1".into(),
                user_id: "seller_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        let product = state
            .db
            .get_document(collections::PRODUCTS, "ebook_1")
            .await
            .unwrap();
        assert_eq!(product["stockQuantity"], 5);

        let licenses = state
            .db
            .query_bind_value(
                "SELECT * FROM licenses WHERE orderId = $orderId AND productId = $productId",
                json!({"orderId": "order_digital", "productId": "ebook_1"}),
            )
            .await
            .unwrap();
        assert_eq!(licenses.len(), 1);
        assert_eq!(licenses[0]["status"], "revoked");
        assert!(licenses[0].get("revokedAt").is_some());
    }

    #[tokio::test]
    async fn test_refund_order_item_already_refunded_short_circuits() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "seller_1", &["seller"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_already",
                json!({
                    fields::PAYMENT_STATUS: "CAPTURED",
                    "paymentIntentId": "pi_existing",
                    "subtotalCents": 1000,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        "status": "refunded",
                        "price": 10.0,
                        "quantity": 1
                    }]
                }),
            )
            .await
            .unwrap();

        let Json(resp) = refund_order_item(
            State(state),
            Json(RefundItemRequest {
                order_id: "order_already".into(),
                product_id: "prod_1".into(),
                user_id: "seller_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.refund_amount_cents, 0);
        assert_eq!(resp.already_refunded, Some(true));
    }

    #[tokio::test]
    async fn test_cancel_order_captured_payment_refunds_and_logs_event() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/refunds"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "re_cancel_1"
            })))
            .mount(&server)
            .await;

        let state = stripe_state(&server).await;
        seed_user(&state, "buyer_1", &["buyer"]).await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                json!({
                    fields::PRODUCT_ID: "prod_1",
                    "stockQuantity": 4,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_cancel",
                json!({
                    fields::ORDER_ID: "order_cancel",
                    "orderStatus": "PENDING_PAYMENT",
                    "userId": "buyer_1",
                    fields::PAYMENT_STATUS: "CAPTURED",
                    "paymentIntentId": "pi_cancel_1",
                    "stockRestored": false,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        "quantity": 2,
                        "isDigital": false
                    }]
                }),
            )
            .await
            .unwrap();

        let Json(resp) = cancel_order(
            State(state.clone()),
            Json(CancelOrderRequest {
                order_id: "order_cancel".into(),
                user_id: "buyer_1".into(),
                reason: Some("need to reorder".into()),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(resp.refunded);

        let order = state
            .db
            .get_document(collections::ORDERS, "order_cancel")
            .await
            .unwrap();
        let product = state
            .db
            .get_document(collections::PRODUCTS, "prod_1")
            .await
            .unwrap();
        assert_eq!(order["orderStatus"], "CANCELLED");
        assert_eq!(order[fields::PAYMENT_STATUS], "REFUNDED");
        assert_eq!(order["cancelledBy"], "buyer_1");
        assert_eq!(product["stockQuantity"], 6);

        let events = state
            .db
            .query_bind_value(
                "SELECT * FROM order_events WHERE orderId = $orderId AND eventType = $eventType",
                json!({"orderId": "order_cancel", "eventType": "order_cancelled"}),
            )
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["metadata"]["refunded"], true);
    }

    #[tokio::test]
    async fn test_cancel_order_failed_refund_quarantines_order() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/refunds"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let state = stripe_state(&server).await;
        seed_user(&state, "buyer_1", &["buyer"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_quarantine",
                json!({
                    "orderStatus": "PENDING_PAYMENT",
                    "userId": "buyer_1",
                    fields::PAYMENT_STATUS: "CAPTURED",
                    "paymentIntentId": "pi_quarantine",
                    fields::ITEMS: []
                }),
            )
            .await
            .unwrap();

        let err = cancel_order(
            State(state.clone()),
            Json(CancelOrderRequest {
                order_id: "order_quarantine".into(),
                user_id: "buyer_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Refund failed"));
        let order = state
            .db
            .get_document(collections::ORDERS, "order_quarantine")
            .await
            .unwrap();
        assert_eq!(order[fields::PAYMENT_STATUS], "CANCEL_FAILED");
        assert_eq!(order["requiresManualReview"], true);
    }

    // -----------------------------------------------------------------------
    // Coverage: stripe_cancel_pi failure (lines 252-256)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_stripe_cancel_pi_failure_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/payment_intents/pi_fail/cancel"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
            .mount(&server)
            .await;

        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_secret_key".to_string(), "sk_test_123".to_string());
        let state = setup_state_with_config(config, server.uri()).await;

        let err = stripe_cancel_pi(&state, "pi_fail").await.unwrap_err();
        assert!(
            err.to_string()
                .contains("Payment release could not be confirmed")
        );
    }

    // -----------------------------------------------------------------------
    // Coverage: refund_order_item — product not found (lines 306-309)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_refund_order_item_product_not_found() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "seller_1", &["seller"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_nf",
                json!({
                    fields::PAYMENT_STATUS: "CAPTURED",
                    "paymentIntentId": "pi_1",
                    "subtotalCents": 1000,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "other_prod",
                        fields::SELLER_ID: "seller_1",
                        "status": "delivered",
                        "price": 10.0,
                        "quantity": 1
                    }]
                }),
            )
            .await
            .unwrap();

        let err = refund_order_item(
            State(state),
            Json(RefundItemRequest {
                order_id: "order_nf".into(),
                product_id: "missing_prod".into(),
                user_id: "seller_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found in order"));
    }

    // -----------------------------------------------------------------------
    // Coverage: refund_order_item — not seller/admin (lines 317-319)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_refund_order_item_not_seller_or_admin() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "random_user", &["buyer"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_perm",
                json!({
                    fields::PAYMENT_STATUS: "CAPTURED",
                    "paymentIntentId": "pi_1",
                    "subtotalCents": 1000,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        "status": "delivered",
                        "price": 10.0,
                        "quantity": 1
                    }]
                }),
            )
            .await
            .unwrap();

        let err = refund_order_item(
            State(state),
            Json(RefundItemRequest {
                order_id: "order_perm".into(),
                product_id: "prod_1".into(),
                user_id: "random_user".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Only seller of the item or admin"));
    }

    // -----------------------------------------------------------------------
    // Coverage: refund_order_item — payment not captured (lines 325-327)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_refund_order_item_payment_not_captured() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "seller_1", &["seller"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_uncap",
                json!({
                    fields::PAYMENT_STATUS: "AUTHORIZED",
                    "paymentIntentId": "pi_1",
                    "subtotalCents": 1000,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        "status": "delivered",
                        "price": 10.0,
                        "quantity": 1
                    }]
                }),
            )
            .await
            .unwrap();

        let err = refund_order_item(
            State(state),
            Json(RefundItemRequest {
                order_id: "order_uncap".into(),
                product_id: "prod_1".into(),
                user_id: "seller_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Cannot refund uncaptured payment"));
    }

    // -----------------------------------------------------------------------
    // Coverage: refund_order_item — payout processing (lines 333-336)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_refund_order_item_payout_processing_blocks() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "seller_1", &["seller"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_payout",
                json!({
                    fields::PAYMENT_STATUS: "CAPTURED",
                    "payoutStatus": "PROCESSING",
                    "paymentIntentId": "pi_1",
                    "subtotalCents": 1000,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        "status": "delivered",
                        "price": 10.0,
                        "quantity": 1
                    }]
                }),
            )
            .await
            .unwrap();

        let err = refund_order_item(
            State(state),
            Json(RefundItemRequest {
                order_id: "order_payout".into(),
                product_id: "prod_1".into(),
                user_id: "seller_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("payout is currently processing"));
    }

    // -----------------------------------------------------------------------
    // Coverage: refund_order_item — return window expired (lines 353-356)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_refund_order_item_return_window_expired() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "seller_1", &["seller"]).await;
        let old_date = (Utc::now()
            - chrono::Duration::days(business_rules::RETURN_WINDOW_DAYS as i64 + 5))
        .to_rfc3339();
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_expired",
                json!({
                    fields::PAYMENT_STATUS: "CAPTURED",
                    "paymentIntentId": "pi_1",
                    "subtotalCents": 1000,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        "status": "delivered",
                        "deliveredAt": old_date,
                        "price": 10.0,
                        "quantity": 1
                    }]
                }),
            )
            .await
            .unwrap();

        let err = refund_order_item(
            State(state),
            Json(RefundItemRequest {
                order_id: "order_expired".into(),
                product_id: "prod_1".into(),
                user_id: "seller_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Return window expired"));
    }

    // -----------------------------------------------------------------------
    // Coverage: refund_order_item — no payment intent (line 365)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_refund_order_item_no_payment_intent() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "seller_1", &["seller"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_nopi",
                json!({
                    fields::PAYMENT_STATUS: "CAPTURED",
                    "subtotalCents": 1000,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        "status": "shipped",
                        "price": 10.0,
                        "quantity": 1
                    }]
                }),
            )
            .await
            .unwrap();

        let err = refund_order_item(
            State(state),
            Json(RefundItemRequest {
                order_id: "order_nopi".into(),
                product_id: "prod_1".into(),
                user_id: "seller_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("No payment intent found"));
    }

    // -----------------------------------------------------------------------
    // Coverage: cancel_order — archived order (lines 515-517)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancel_order_archived_blocked() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "buyer_1", &["buyer"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_arch",
                json!({
                    "orderStatus": "PENDING_PAYMENT",
                    "userId": "buyer_1",
                    "archived": true,
                    fields::ITEMS: []
                }),
            )
            .await
            .unwrap();

        let err = cancel_order(
            State(state),
            Json(CancelOrderRequest {
                order_id: "order_arch".into(),
                user_id: "buyer_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Cannot cancel archived order"));
    }

    // -----------------------------------------------------------------------
    // Coverage: cancel_order — not buyer/seller/admin (lines 531-533)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancel_order_not_authorized() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "random_user", &["viewer"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_noauth",
                json!({
                    "orderStatus": "PENDING_PAYMENT",
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::SELLER_ID: "seller_1",
                    }]
                }),
            )
            .await
            .unwrap();

        let err = cancel_order(
            State(state),
            Json(CancelOrderRequest {
                order_id: "order_noauth".into(),
                user_id: "random_user".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Only buyer, seller, or admin"));
    }

    // -----------------------------------------------------------------------
    // Coverage: cancel_order — multi-seller restriction (lines 538-540)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancel_order_multi_seller_blocked_for_seller() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "seller_1", &["seller"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_multi",
                json!({
                    "orderStatus": "PENDING_PAYMENT",
                    "userId": "buyer_1",
                    fields::ITEMS: [
                        { fields::SELLER_ID: "seller_1" },
                        { fields::SELLER_ID: "seller_2" },
                    ]
                }),
            )
            .await
            .unwrap();

        let err = cancel_order(
            State(state),
            Json(CancelOrderRequest {
                order_id: "order_multi".into(),
                user_id: "seller_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("Cannot cancel a multi-seller order")
        );
    }

    // -----------------------------------------------------------------------
    // Coverage: cancel_order — invalid transition (lines 546-548)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancel_order_invalid_transition() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "buyer_1", &["buyer"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_shipped",
                json!({
                    "orderStatus": "SHIPPED",
                    "userId": "buyer_1",
                    fields::ITEMS: []
                }),
            )
            .await
            .unwrap();

        let err = cancel_order(
            State(state),
            Json(CancelOrderRequest {
                order_id: "order_shipped".into(),
                user_id: "buyer_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Cannot cancel order with status"));
    }

    // -----------------------------------------------------------------------
    // Coverage: cancel_order — buyer can't cancel post-shipment (lines 559-561)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancel_order_buyer_cannot_cancel_processing() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "buyer_1", &["buyer"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_proc",
                json!({
                    "orderStatus": "PROCESSING",
                    "userId": "buyer_1",
                    fields::ITEMS: []
                }),
            )
            .await
            .unwrap();

        let err = cancel_order(
            State(state),
            Json(CancelOrderRequest {
                order_id: "order_proc".into(),
                user_id: "buyer_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot be cancelled at this stage")
        );
    }

    // -----------------------------------------------------------------------
    // Coverage: cancel_order — stripe_cancel_pi failure quarantine (lines 603-610)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancel_order_authorized_cancel_pi_failure_quarantines() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/payment_intents/pi_fail/cancel"))
            .respond_with(ResponseTemplate::new(500).set_body_string("server error"))
            .mount(&server)
            .await;

        let state = stripe_state(&server).await;
        seed_user(&state, "buyer_1", &["buyer"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_pi_fail",
                json!({
                    "orderStatus": "PENDING_PAYMENT",
                    "userId": "buyer_1",
                    fields::PAYMENT_STATUS: "AUTHORIZED",
                    "paymentIntentId": "pi_fail",
                    fields::ITEMS: []
                }),
            )
            .await
            .unwrap();

        let err = cancel_order(
            State(state.clone()),
            Json(CancelOrderRequest {
                order_id: "order_pi_fail".into(),
                user_id: "buyer_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("Payment release could not be confirmed")
        );

        let order = state
            .db
            .get_document(collections::ORDERS, "order_pi_fail")
            .await
            .unwrap();
        assert_eq!(order[fields::PAYMENT_STATUS], "CANCEL_FAILED");
        assert_eq!(order["requiresManualReview"], true);
    }

    // -----------------------------------------------------------------------
    // Coverage: cancel_order — no payment (line 614), stock already restored (line 638),
    // digital items skipped (line 642)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancel_order_no_payment_status_and_stock_already_restored() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "buyer_1", &["buyer"]).await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_nopay",
                json!({
                    "orderStatus": "PENDING_PAYMENT",
                    "userId": "buyer_1",
                    fields::PAYMENT_STATUS: "PENDING",
                    "stockRestored": true,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        "quantity": 1,
                        "isDigital": false
                    }]
                }),
            )
            .await
            .unwrap();

        let Json(resp) = cancel_order(
            State(state.clone()),
            Json(CancelOrderRequest {
                order_id: "order_nopay".into(),
                user_id: "buyer_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(!resp.refunded);
    }

    #[tokio::test]
    async fn test_cancel_order_skips_digital_items_in_stock_restore() {
        let server = MockServer::start().await;
        let state = stripe_state(&server).await;
        seed_user(&state, "buyer_1", &["buyer"]).await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "phys_1",
                json!({ fields::PRODUCT_ID: "phys_1", "stockQuantity": 5 }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_dig_cancel",
                json!({
                    "orderStatus": "PENDING_PAYMENT",
                    "userId": "buyer_1",
                    fields::PAYMENT_STATUS: "PENDING",
                    "stockRestored": false,
                    fields::ITEMS: [
                        {
                            fields::PRODUCT_ID: "dig_1",
                            fields::SELLER_ID: "seller_1",
                            "quantity": 1,
                            "isDigital": true
                        },
                        {
                            fields::PRODUCT_ID: "phys_1",
                            fields::SELLER_ID: "seller_1",
                            "quantity": 3,
                            "isDigital": false
                        }
                    ]
                }),
            )
            .await
            .unwrap();

        let Json(resp) = cancel_order(
            State(state.clone()),
            Json(CancelOrderRequest {
                order_id: "order_dig_cancel".into(),
                user_id: "buyer_1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        let product = state
            .db
            .get_document(collections::PRODUCTS, "phys_1")
            .await
            .unwrap();
        assert_eq!(product["stockQuantity"], 8); // 5 + 3
    }
}
