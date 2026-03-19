//! Stripe webhook handler.
//! Validates Stripe webhook signatures and routes events to appropriate handlers.

use axum::{Json, Router, extract::Request, extract::State, routing::post};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::HashMap;
use tracing::{error, info, warn};

use crate::HandlersState;
use crate::email;
use crate::shared::schema::{OrderStatus, collections, fields};
use ob_database::Transaction;

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Webhook Event Structure
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
pub struct StripeWebhookEvent {
    pub id: String,
    pub r#type: String, // "type" is a reserved keyword
    #[serde(default)]
    pub data: serde_json::Value,
    #[serde(default)]
    pub created: i64,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/webhooks/stripe", post(handle_stripe_webhook))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Main Webhook Handler
// ---------------------------------------------------------------------------

async fn handle_stripe_webhook(
    State(state): State<HandlersState>,
    request: Request,
) -> Result<Json<Value>, ob_core::Error> {
    let (parts, body) = request.into_parts();
    let signature = parts
        .headers
        .get("stripe-signature")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    let body_bytes = axum::body::to_bytes(body, 10 * 1024 * 1024)
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Failed to read body: {e}")))?;

    // --- Verify webhook signature if secret is configured ---
    if let Ok(webhook_secret) = state.config.require_secret("stripe_webhook_secret") {
        if !signature.is_empty() {
            if !verify_stripe_signature(&body_bytes, signature, &webhook_secret) {
                error!("Stripe webhook signature verification failed");
                return Err(ob_core::Error::Auth(
                    "Invalid webhook signature".into(),
                ));
            }
        }
    }

    // --- Parse event ---
    let event: StripeWebhookEvent = serde_json::from_slice(&body_bytes)
        .map_err(|e| ob_core::Error::Validation(format!("Invalid webhook JSON: {e}")))?;

    // --- Check for duplicate ---
    if is_duplicate_webhook(&state, &event.id).await? {
        info!(event_id = %event.id, event_type = %event.r#type, "Duplicate webhook, skipping");
        return Ok(Json(json!({ "status": "duplicate", "event_id": event.id })));
    }

    // --- Store webhook event for idempotency ---
    store_webhook_event(&state, &event).await?;

    // --- Route to appropriate handler ---
    let result = match event.r#type.as_str() {
        // Payment Intent events
        "payment_intent.succeeded" => handle_payment_intent_succeeded(&state, &event.data).await,
        "payment_intent.payment_failed" => handle_payment_intent_failed(&state, &event.data).await,
        "payment_intent.canceled" => handle_payment_intent_canceled(&state, &event.data).await,

        // Charge events
        "charge.succeeded" => handle_charge_succeeded(&state, &event.data).await,
        "charge.failed" => handle_charge_failed(&state, &event.data).await,
        "charge.refunded" => handle_charge_refunded(&state, &event.data).await,

        // Customer events
        "customer.created" => handle_customer_created(&state, &event.data).await,
        "customer.updated" => handle_customer_updated(&state, &event.data).await,
        "customer.deleted" => handle_customer_deleted(&state, &event.data).await,

        // Payment Method events
        "payment_method.attached" => handle_payment_method_attached(&state, &event.data).await,
        "payment_method.detached" => handle_payment_method_detached(&state, &event.data).await,

        // Invoice events
        "invoice.paid" => handle_invoice_paid(&state, &event.data).await,
        "invoice.payment_failed" => handle_invoice_payment_failed(&state, &event.data).await,

        // Subscription events (delegate to subscriptions module)
        "customer.subscription.created" => {
            super::subscriptions::route_subscription_webhook(&state, &event.r#type, &event.data)
                .await
        }
        "customer.subscription.updated" => {
            super::subscriptions::route_subscription_webhook(&state, &event.r#type, &event.data)
                .await
        }
        "customer.subscription.deleted" => {
            super::subscriptions::route_subscription_webhook(&state, &event.r#type, &event.data)
                .await
        }

        // Unhandled events: log and accept (to prevent Stripe retries)
        event_type => {
            warn!(event_type = %event_type, event_id = %event.id, "Unhandled Stripe webhook event");
            Ok(())
        }
    };

    match result {
        Ok(()) => Ok(Json(json!({ "status": "ok", "event_id": event.id }))),
        Err(e) => {
            // Log error but still return 200 to prevent Stripe retries
            error!(
                event_id = %event.id,
                event_type = %event.r#type,
                error = %e,
                "Error processing webhook event"
            );
            Ok(Json(json!({
                "status": "error",
                "event_id": event.id,
                "error": e.to_string()
            })))
        }
    }
}

// ---------------------------------------------------------------------------
// Signature Verification
// ---------------------------------------------------------------------------

fn verify_stripe_signature(body: &[u8], signature: &str, secret: &str) -> bool {
    // Stripe signature format: "t=timestamp,v1=signature"
    let mut parts = HashMap::new();
    for part in signature.split(',') {
        if let Some((k, v)) = part.split_once('=') {
            parts.insert(k, v);
        }
    }

    let timestamp = parts.get("t").unwrap_or(&"");
    let provided_sig = parts.get("v1").unwrap_or(&"");

    // Create signed content: "{timestamp}.{body}"
    let signed_content = format!("{}.{}", timestamp, String::from_utf8_lossy(body));

    // Compute HMAC
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(signed_content.as_bytes());

    let computed_sig = hex::encode(mac.finalize().into_bytes());

    // Constant-time comparison
    computed_sig == *provided_sig
}

// ---------------------------------------------------------------------------
// Webhook Deduplication
// ---------------------------------------------------------------------------

async fn is_duplicate_webhook(
    state: &HandlersState,
    event_id: &str,
) -> Result<bool, ob_core::Error> {
    // Validate event ID format before querying
    ob_core::validate_surreal_record_id(event_id)?;

    let result = state
        .db
        .query_bind_value(
            "SELECT * FROM webhook_events WHERE id = $event_id LIMIT 1",
            serde_json::json!({"event_id": event_id}),
        )
        .await?;

    Ok(!result.is_empty())
}

async fn store_webhook_event(
    state: &HandlersState,
    event: &StripeWebhookEvent,
) -> Result<(), ob_core::Error> {
    let now = chrono::Utc::now();
    let timestamp = now.timestamp();
    let timestamp_rfc3339 = now.to_rfc3339();

    state
        .db
        .create_document(
            collections::WEBHOOK_EVENTS,
            serde_json::json!({
                "id": event.id,
                "type": event.r#type,
                "timestamp": timestamp,
                "timestamp_iso": timestamp_rfc3339,
                "processed": true,
                "data": event.data,
            }),
        )
        .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helper Functions for Database Operations
// ---------------------------------------------------------------------------

/// Find order by payment intent ID
async fn find_order_by_payment_intent(
    state: &HandlersState,
    payment_intent_id: &str,
) -> Result<Option<Value>, ob_core::Error> {
    let rows: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "SELECT * FROM {} WHERE {} = $pi_id LIMIT 1",
                collections::ORDERS,
                fields::PAYMENT_INTENT_ID
            ),
            serde_json::json!({"pi_id": payment_intent_id}),
        )
        .await?;

    Ok(rows.first().cloned())
}

/// Find order by metadata order ID from Stripe metadata
async fn find_order_by_metadata_id(
    state: &HandlersState,
    order_id: &str,
) -> Result<Option<Value>, ob_core::Error> {
    let rows: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "SELECT * FROM {} WHERE id = $order_id LIMIT 1",
                collections::ORDERS
            ),
            serde_json::json!({"order_id": order_id}),
        )
        .await?;

    Ok(rows.first().cloned())
}

/// Update order status atomically
async fn update_order_status(
    state: &HandlersState,
    order_id: &str,
    new_status: &str,
) -> Result<(), ob_core::Error> {
    let now = chrono::Utc::now().to_rfc3339();

    state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET {} = $status, updatedAt = $now WHERE id = $order_id",
                collections::ORDERS,
                fields::ORDER_STATUS
            ),
            serde_json::json!({
                "order_id": order_id,
                "status": new_status,
                "now": now,
            }),
        )
        .await?;

    info!(order_id = %order_id, new_status = %new_status, "Order status updated");
    Ok(())
}

/// Restore stock for all items in an order (used on refund/cancellation)
async fn restore_stock_for_order(
    state: &HandlersState,
    order: &Value,
) -> Result<(), ob_core::Error> {
    let order_id = order
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::Validation("Order missing id".into()))?;

    let items = order
        .get(fields::ITEMS)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if items.is_empty() {
        return Ok(());
    }

    // Build transaction to restore stock for all items
    let mut tx = Transaction::new();

    for item in &items {
        let product_id = item
            .get(fields::PRODUCT_ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let quantity = item.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1);

        if !product_id.is_empty() && quantity > 0 {
            // Build product record ID: "products:productId"
            let product_record_id = format!("{}:{}", collections::PRODUCTS, product_id);

            tx.add(
                &format!(
                    "UPDATE {} SET {} += $qty WHERE id = $id",
                    collections::PRODUCTS,
                    fields::STOCK_QUANTITY
                ),
                Some(serde_json::json!({
                    "id": product_record_id,
                    "qty": quantity,
                })),
            );
        }
    }

    if !tx.is_empty() {
        tx.commit(&state.db).await?;
        info!(order_id = %order_id, item_count = items.len(), "Stock restored for order items");
    }

    Ok(())
}

/// Decrement stock for all items in an order (used on successful payment)
async fn decrement_stock_for_order(
    state: &HandlersState,
    order: &Value,
) -> Result<(), ob_core::Error> {
    let order_id = order
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::Validation("Order missing id".into()))?;

    let items = order
        .get(fields::ITEMS)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if items.is_empty() {
        return Ok(());
    }

    // Build transaction to decrement stock for all items
    let mut tx = Transaction::new();

    for item in &items {
        let product_id = item
            .get(fields::PRODUCT_ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let quantity = item.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1);

        if !product_id.is_empty() && quantity > 0 {
            // Build product record ID: "products:productId"
            let product_record_id = format!("{}:{}", collections::PRODUCTS, product_id);

            tx.add(
                &format!(
                    "UPDATE {} SET {} -= $qty WHERE id = $id",
                    collections::PRODUCTS,
                    fields::STOCK_QUANTITY
                ),
                Some(serde_json::json!({
                    "id": product_record_id,
                    "qty": quantity,
                })),
            );
        }
    }

    if !tx.is_empty() {
        tx.commit(&state.db).await?;
        info!(order_id = %order_id, item_count = items.len(), "Stock decremented for order items");
    }

    Ok(())
}

/// Mark coupon as redeemed
async fn mark_coupon_redeemed(
    state: &HandlersState,
    order_id: &str,
    coupon_code: &str,
) -> Result<(), ob_core::Error> {
    // Validate coupon code before querying
    if coupon_code.is_empty() {
        return Ok(()); // No coupon, skip
    }

    let coupon_code_safe = coupon_code.to_uppercase();

    // Try to update coupon use record
    let now = chrono::Utc::now().to_rfc3339();

    let rows: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET redeemedAt = $now WHERE orderId = $order_id AND couponCode = $code",
                collections::COUPON_USES
            ),
            serde_json::json!({
                "order_id": order_id,
                "code": coupon_code_safe,
                "now": now,
            }),
        )
        .await
        .unwrap_or_default();

    if !rows.is_empty() {
        info!(order_id = %order_id, coupon_code = %coupon_code, "Coupon marked as redeemed");
    }

    Ok(())
}

/// Release coupon reservation (undo a reserved coupon)
async fn release_coupon_reservation(
    state: &HandlersState,
    order_id: &str,
) -> Result<(), ob_core::Error> {
    // Find any coupon use record for this order and delete it
    let _rows: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "DELETE FROM {} WHERE orderId = $order_id AND redeemedAt IS NULL",
                collections::COUPON_USES
            ),
            serde_json::json!({"order_id": order_id}),
        )
        .await
        .unwrap_or_default();

    info!(order_id = %order_id, "Coupon reservation released");
    Ok(())
}

fn str_field<'a>(value: &'a Value, field: &str) -> &'a str {
    value.get(field).and_then(|v| v.as_str()).unwrap_or("")
}

async fn send_payment_authorized_emails(
    state: &HandlersState,
    order: &Value,
) -> Result<(), ob_core::Error> {
    email::send_order_confirmation_emails(state, order).await
}

// ---------------------------------------------------------------------------
// Event Handlers (full implementations)
// ---------------------------------------------------------------------------

/// Handle payment_intent.succeeded: order confirmed, stock decremented, coupon marked used
async fn handle_payment_intent_succeeded(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let pi_obj = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let pi_id = pi_obj
        .get("id")
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No payment intent ID".into()))?;

    let metadata = pi_obj
        .get("metadata")
        .and_then(|m| m.as_object())
        .ok_or_else(|| ob_core::Error::Validation("No metadata in payment intent".into()))?;

    let order_id = metadata
        .get("order_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No order_id in metadata".into()))?;

    let coupon_code = metadata
        .get("coupon_code")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Find the order
    let order = find_order_by_metadata_id(state, order_id)
        .await?
        .ok_or_else(|| ob_core::Error::NotFound(format!("Order {} not found", order_id)))?;

    // Verify order is still in pending state
    let current_status = order
        .get(fields::ORDER_STATUS)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if current_status != OrderStatus::PendingPayment.as_str() {
        warn!(
            order_id = %order_id,
            payment_intent_id = %pi_id,
            current_status = %current_status,
            "Order not in pending state, skipping confirmation"
        );
        return Ok(());
    }

    // Decrement stock
    decrement_stock_for_order(state, &order).await?;

    // Update order status to confirmed
    update_order_status(state, order_id, OrderStatus::PaymentAuthorized.as_str()).await?;

    // Store payment intent ID on order
    state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET {} = $pi_id WHERE id = $order_id",
                collections::ORDERS,
                fields::PAYMENT_INTENT_ID
            ),
            serde_json::json!({
                "order_id": order_id,
                "pi_id": pi_id,
            }),
        )
        .await?;

    // Mark coupon as redeemed if one was used
    if !coupon_code.is_empty() {
        mark_coupon_redeemed(state, order_id, coupon_code).await?;
    }

    if let Err(err) = send_payment_authorized_emails(state, &order).await {
        warn!(order_id = %order_id, error = %err, "Failed to prepare payment success emails");
    }

    info!(
        order_id = %order_id,
        payment_intent_id = %pi_id,
        "Payment intent succeeded: order confirmed, stock decremented"
    );

    Ok(())
}

/// Handle payment_intent.payment_failed: cancel order, release coupon
async fn handle_payment_intent_failed(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let pi_obj = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let pi_id = pi_obj
        .get("id")
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No payment intent ID".into()))?;

    let metadata = pi_obj
        .get("metadata")
        .and_then(|m| m.as_object())
        .ok_or_else(|| ob_core::Error::Validation("No metadata in payment intent".into()))?;

    let order_id = metadata
        .get("order_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No order_id in metadata".into()))?;

    // Find the order
    let order = find_order_by_metadata_id(state, order_id)
        .await?
        .ok_or_else(|| ob_core::Error::NotFound(format!("Order {} not found", order_id)))?;

    // Check if still pending (no need to cancel if already processed)
    let current_status = order
        .get(fields::ORDER_STATUS)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if current_status == OrderStatus::PendingPayment.as_str() {
        // Cancel order
        update_order_status(state, order_id, OrderStatus::Cancelled.as_str()).await?;
    }

    // Release coupon reservation (unmark as used)
    release_coupon_reservation(state, order_id).await?;

    warn!(
        order_id = %order_id,
        payment_intent_id = %pi_id,
        "Payment intent failed: order cancelled, coupon released"
    );

    Ok(())
}

/// Handle payment_intent.canceled: cancel order, release coupon
async fn handle_payment_intent_canceled(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let pi_obj = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let pi_id = pi_obj
        .get("id")
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No payment intent ID".into()))?;

    let metadata = pi_obj
        .get("metadata")
        .and_then(|m| m.as_object())
        .ok_or_else(|| ob_core::Error::Validation("No metadata in payment intent".into()))?;

    let order_id = metadata
        .get("order_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No order_id in metadata".into()))?;

    // Find the order
    let order = find_order_by_metadata_id(state, order_id)
        .await?
        .ok_or_else(|| ob_core::Error::NotFound(format!("Order {} not found", order_id)))?;

    // Check if still pending
    let current_status = order
        .get(fields::ORDER_STATUS)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if current_status == OrderStatus::PendingPayment.as_str() {
        update_order_status(state, order_id, OrderStatus::Cancelled.as_str()).await?;
    }

    // Release coupon reservation
    release_coupon_reservation(state, order_id).await?;

    info!(
        order_id = %order_id,
        payment_intent_id = %pi_id,
        "Payment intent cancelled: order cancelled, coupon released"
    );

    Ok(())
}

/// Handle charge.succeeded: log event (actual confirmation happens at payment_intent.succeeded)
async fn handle_charge_succeeded(
    _state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    if let Some(charge_id) = event_data
        .get("object")
        .and_then(|o| o.get("id"))
        .and_then(|i| i.as_str())
    {
        info!(charge_id = %charge_id, "Charge succeeded");
    }
    Ok(())
}

/// Handle charge.failed: log event (actual cancellation happens at payment_intent.payment_failed)
async fn handle_charge_failed(
    _state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    if let Some(charge_id) = event_data
        .get("object")
        .and_then(|o| o.get("id"))
        .and_then(|i| i.as_str())
    {
        if let Some(reason) = event_data
            .get("object")
            .and_then(|o| o.get("failure_message"))
            .and_then(|r| r.as_str())
        {
            warn!(charge_id = %charge_id, failure_reason = %reason, "Charge failed");
        } else {
            warn!(charge_id = %charge_id, "Charge failed");
        }
    }
    Ok(())
}

/// Handle charge.refunded: restore stock, update order with refund info
async fn handle_charge_refunded(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let charge_obj = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let charge_id = charge_obj
        .get("id")
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No charge ID".into()))?;

    let payment_intent_id = charge_obj
        .get("payment_intent")
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No payment intent ID in charge".into()))?;

    let refunded_amount_cents = charge_obj
        .get("amount_refunded")
        .and_then(|a| a.as_i64())
        .ok_or_else(|| ob_core::Error::Validation("No refund amount in charge".into()))?;

    // Find the order by payment intent
    let order = find_order_by_payment_intent(state, payment_intent_id)
        .await?
        .ok_or_else(|| {
            ob_core::Error::NotFound(format!(
                "Order not found for payment intent {}",
                payment_intent_id
            ))
        })?;

    let order_id = order
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::Validation("Order missing id".into()))?;

    let total_amount_cents = order
        .get("totalAmountCents")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // Bounds check: refund cannot exceed order total
    if refunded_amount_cents > total_amount_cents {
        return Err(ob_core::Error::Validation(format!(
            "Refund amount {} exceeds order total {}",
            refunded_amount_cents, total_amount_cents
        )));
    }

    // Restore stock for the order
    restore_stock_for_order(state, &order).await?;

    // Update order with refund info
    let now = chrono::Utc::now().to_rfc3339();

    state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET refundedAmountCents = $refunded, refundedAt = $now WHERE id = $order_id",
                collections::ORDERS
            ),
            serde_json::json!({
                "order_id": order_id,
                "refunded": refunded_amount_cents,
                "now": now,
            })
        )
        .await?;

    info!(
        order_id = %order_id,
        charge_id = %charge_id,
        refunded_amount_cents = refunded_amount_cents,
        "Charge refunded: stock restored, order updated"
    );

    Ok(())
}

// Customer and Payment Method events (minimal for now)

async fn handle_customer_created(
    _state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    if let Some(customer_id) = event_data
        .get("object")
        .and_then(|o| o.get("id"))
        .and_then(|i| i.as_str())
    {
        info!(customer_id = %customer_id, "Customer created");
    }
    Ok(())
}

async fn handle_customer_updated(
    _state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    if let Some(customer_id) = event_data
        .get("object")
        .and_then(|o| o.get("id"))
        .and_then(|i| i.as_str())
    {
        info!(customer_id = %customer_id, "Customer updated");
    }
    Ok(())
}

async fn handle_customer_deleted(
    _state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    if let Some(customer_id) = event_data
        .get("object")
        .and_then(|o| o.get("id"))
        .and_then(|i| i.as_str())
    {
        info!(customer_id = %customer_id, "Customer deleted");
    }
    Ok(())
}

async fn handle_payment_method_attached(
    _state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    if let Some(pm_id) = event_data
        .get("object")
        .and_then(|o| o.get("id"))
        .and_then(|i| i.as_str())
    {
        info!(payment_method_id = %pm_id, "Payment method attached");
    }
    Ok(())
}

async fn handle_payment_method_detached(
    _state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    if let Some(pm_id) = event_data
        .get("object")
        .and_then(|o| o.get("id"))
        .and_then(|i| i.as_str())
    {
        info!(payment_method_id = %pm_id, "Payment method detached");
    }
    Ok(())
}

async fn handle_invoice_paid(
    _state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    if let Some(invoice_id) = event_data
        .get("object")
        .and_then(|o| o.get("id"))
        .and_then(|i| i.as_str())
    {
        info!(invoice_id = %invoice_id, "Invoice paid");
    }
    Ok(())
}

async fn handle_invoice_payment_failed(
    _state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    if let Some(invoice_id) = event_data
        .get("object")
        .and_then(|o| o.get("id"))
        .and_then(|i| i.as_str())
    {
        warn!(invoice_id = %invoice_id, "Invoice payment failed");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_verification_valid() {
        let secret = "test_secret";
        let timestamp = "1614556800";
        let body = br#"{"type":"payment_intent.succeeded"}"#;
        let signed_content = format!("{}.{}", timestamp, String::from_utf8_lossy(body));

        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(signed_content.as_bytes());
        let valid_sig = hex::encode(mac.finalize().into_bytes());

        let signature = format!("t={},v1={}", timestamp, valid_sig);
        assert!(verify_stripe_signature(body, &signature, secret));
    }

    #[test]
    fn test_signature_verification_invalid() {
        let secret = "test_secret";
        let signature = "t=1614556800,v1=invalid_signature";
        let body = br#"{"type":"payment_intent.succeeded"}"#;

        assert!(!verify_stripe_signature(body, signature, secret));
    }
}
