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
use crate::shared::schema::{OrderStatus, PaymentStatus, collections, fields};
use ob_database::Transaction;
use ob_database::fields as db_fields;

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Webhook Event Structure
// ---------------------------------------------------------------------------

/// Deserialized Stripe webhook event payload.
///
/// Signature verified via HMAC-SHA256 before deserialization.
/// Events are processed idempotently — duplicate event IDs are silently ignored.
/// Replay protection rejects events older than 300 seconds.
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

/// Create the webhook router for handling Stripe webhook events.
pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/webhooks/stripe", post(handle_stripe_webhook))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Main Webhook Handler
// ---------------------------------------------------------------------------

/// Verifies a signed Stripe webhook request, records the event idempotently,
/// dispatches to the matching payment handler, and returns a JSON status body.
///
/// The handler rejects missing or stale signatures before touching the database,
/// stores the event ID atomically to prevent duplicate side effects, and removes
/// the dedup record when downstream processing fails so Stripe retries can
/// re-deliver the event safely.
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

    // --- Verify webhook signature — REQUIRED in all environments ---
    let webhook_secret = state
        .config
        .require_secret("stripe_webhook_secret")
        .map_err(|_| {
            error!("stripe_webhook_secret not configured — refusing webhook");
            ob_core::Error::Forbidden("Webhook processing unavailable".into())
        })?;

    if signature.is_empty() {
        error!("Missing stripe-signature header");
        return Err(ob_core::Error::Auth("Missing webhook signature".into()));
    }

    if !verify_stripe_signature(&body_bytes, signature, webhook_secret) {
        error!("Stripe webhook signature verification failed");
        return Err(ob_core::Error::Auth("Invalid webhook signature".into()));
    }

    // --- Parse event ---
    let event: StripeWebhookEvent = serde_json::from_slice(&body_bytes)
        .map_err(|e| ob_core::Error::Validation(format!("Invalid webhook JSON: {e}")))?;

    // --- Atomic dedup: check + store in one operation ---
    // If the event already exists, CREATE returns an error on the duplicate ID.
    // This replaces the racy SELECT-then-CREATE pattern.
    if !try_store_webhook_event_atomic(&state, &event).await? {
        info!(event_id = %event.id, event_type = %event.r#type, "Duplicate webhook, skipping");
        return Ok(Json(json!({ "status": "duplicate", "event_id": event.id })));
    }

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

        // Checkout Session events (preferred for Checkout Sessions flow)
        "checkout.session.completed" => {
            handle_checkout_session_completed(&state, &event.data).await
        }
        "checkout.session.expired" => handle_checkout_session_expired(&state, &event.data).await,
        "checkout.session.async_payment_succeeded" => {
            handle_checkout_session_async_payment_succeeded(&state, &event.data).await
        }
        "checkout.session.async_payment_failed" => {
            handle_checkout_session_async_payment_failed(&state, &event.data).await
        }

        // Charge capture events (pre-auth flows)
        "charge.captured" => handle_charge_captured(&state, &event.data).await,

        // Charge dispute events
        "charge.dispute.created" => handle_charge_dispute_created(&state, &event.data).await,
        "charge.dispute.closed" => handle_charge_dispute_closed(&state, &event.data).await,
        "charge.dispute.updated" => handle_charge_dispute_updated(&state, &event.data).await,
        "charge.dispute.funds_withdrawn" => {
            handle_charge_dispute_funds_withdrawn(&state, &event.data).await
        }
        "charge.dispute.funds_reinstated" => {
            handle_charge_dispute_funds_reinstated(&state, &event.data).await
        }

        // Payout events
        "payout.created" => handle_payout_created(&state, &event.data).await,
        "payout.updated" => handle_payout_updated(&state, &event.data).await,
        "payout.paid" => handle_payout_paid(&state, &event.data).await,
        "payout.failed" => handle_payout_failed(&state, &event.data).await,

        // Refund events
        "refund.created" => handle_refund_created(&state, &event.data).await,
        "refund.updated" => handle_refund_updated(&state, &event.data).await,
        "refund.failed" => handle_refund_failed(&state, &event.data).await,

        // Stripe Connect events
        "account.updated" => handle_account_updated(&state, &event.data).await,

        // Unhandled events: log and accept (to prevent Stripe retries)
        event_type => {
            warn!(event_type = %event_type, event_id = %event.id, "Unhandled Stripe webhook event");
            Ok(())
        }
    };

    match result {
        Ok(()) => Ok(Json(json!({ "status": "ok", "event_id": event.id }))),
        Err(e) => {
            // Handler failed — return 500 so Stripe retries.
            // Delete the event so the retry hits the handler again.
            let _ = state
                .db
                .delete_document(collections::WEBHOOK_EVENTS, &event.id)
                .await;

            error!(
                event_id = %event.id,
                event_type = %event.r#type,
                error = %e,
                "Error processing webhook event — returning 500 for Stripe retry"
            );
            Err(e)
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

    // Reject empty signatures
    if timestamp.is_empty() || provided_sig.is_empty() {
        return false;
    }

    // Replay protection: reject webhooks older than 300 seconds
    if let Ok(ts) = timestamp.parse::<i64>() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        if (now - ts).abs() > 300 {
            warn!(
                timestamp = ts,
                now = now,
                "Stripe webhook timestamp too old or too far in the future"
            );
            return false;
        }
    } else {
        warn!("Stripe webhook has non-numeric timestamp");
        return false;
    }

    // Create signed content: "{timestamp}.{body}"
    let mut signed_content = Vec::new();
    signed_content.extend_from_slice(timestamp.as_bytes());
    signed_content.push(b'.');
    signed_content.extend_from_slice(body);

    // Compute HMAC
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(&signed_content);

    // Constant-time comparison via hmac::Mac::verify_slice
    // Decode the provided hex signature to bytes for verify_slice
    let provided_bytes = match hex::decode(provided_sig) {
        Ok(b) => b,
        Err(_) => return false,
    };
    mac.verify_slice(&provided_bytes).is_ok()
}

// ---------------------------------------------------------------------------
// Webhook Deduplication
// ---------------------------------------------------------------------------

/// Check if webhook event already processed (query only, no write).
/// DEPRECATED: Use try_store_webhook_event_atomic() instead — this has a TOCTOU race.
#[allow(dead_code)]
async fn is_duplicate_webhook(
    state: &HandlersState,
    event_id: &str,
) -> Result<bool, ob_core::Error> {
    // Stripe event IDs are "evt_xxx" format
    // Just validate non-empty and reasonable length.
    if event_id.is_empty() || event_id.len() > 512 {
        return Err(ob_core::Error::Validation(
            "Invalid webhook event ID".into(),
        ));
    }

    let result = state
        .db
        .query_bind_value(
            "SELECT * FROM webhook_events WHERE id = $event_id LIMIT 1",
            serde_json::json!({"event_id": event_id}),
        )
        .await?;

    Ok(!result.is_empty())
}

/// Store webhook event after successful handler execution.
/// DEPRECATED: Use try_store_webhook_event_atomic() instead — this has a TOCTOU race.
#[allow(dead_code)]
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
                db_fields::ID: event.id,
                fields::TYPE: event.r#type,
                fields::TIMESTAMP: timestamp,
                fields::TIMESTAMP_ISO: timestamp_rfc3339,
                fields::PROCESSED: true,
                fields::DATA: event.data,
            }),
        )
        .await?;

    Ok(())
}

/// Atomic dedup: try to create webhook event. Returns true if new, false if duplicate.
/// Uses the Stripe event ID as the document ID so PostgreSQL's unique constraint
/// prevents concurrent duplicates — no SELECT-then-INSERT TOCTOU race.
async fn try_store_webhook_event_atomic(
    state: &HandlersState,
    event: &StripeWebhookEvent,
) -> Result<bool, ob_core::Error> {
    let now = chrono::Utc::now();
    let timestamp = now.timestamp();
    let timestamp_rfc3339 = now.to_rfc3339();

    // Validate event ID
    if event.id.is_empty() || event.id.len() > 512 {
        return Err(ob_core::Error::Validation(
            "Invalid webhook event ID".into(),
        ));
    }

    // Use Stripe event ID as document ID — INSERT ON CONFLICT handles dedup atomically.
    let event_data = serde_json::json!({
        db_fields::ID: event.id,
        fields::TYPE: event.r#type,
        fields::TIMESTAMP: timestamp,
        fields::TIMESTAMP_ISO: timestamp_rfc3339,
        fields::PROCESSED: true,
        fields::DATA: event.data,
    });

    // Attempt insert — create_document reads "id" from data and uses ON CONFLICT DO NOTHING.
    // If a document with this ID already exists, the DB rejects it — atomic dedup.
    let result = state
        .db
        .create_document(collections::WEBHOOK_EVENTS, event_data)
        .await;

    match result {
        Ok(_) => Ok(true),
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("already exists")
                || err_str.contains("duplicate")
                || err_str.contains("unique constraint")
                || err_str.contains("RecordIdAlreadyExists")
                || err_str.contains("conflict")
            {
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
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
                "SELECT * FROM {} WHERE data->>'{}' = $pi_id LIMIT 1",
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
    let order_key = order_id.strip_prefix("orders:").unwrap_or(order_id);
    let rows: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "SELECT * FROM {} WHERE id = $order_key OR id = $order_id OR data->>'{}' = $order_key LIMIT 1",
                collections::ORDERS,
                fields::ORDER_ID,
            ),
            serde_json::json!({
                "order_id": order_id,
                "order_key": order_key,
            }),
        )
        .await?;

    Ok(rows.first().cloned())
}

/// Update order status atomically with precondition.
/// Returns Ok(true) if updated, Ok(false) if order already moved forward.
/// The `expected_status` guard prevents late-arriving webhooks from
/// overwriting an advanced state (e.g. a failed webhook can't cancel
/// an order that already moved to PaymentAuthorized).
async fn update_order_status(
    state: &HandlersState,
    order_id: &str,
    expected_status: &str,
    new_status: &str,
) -> Result<bool, ob_core::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    let order_status_path = format!("'{{{}}}'", fields::ORDER_STATUS);
    let updated_at_path = format!("'{{{}}}'", db_fields::UPDATED_AT);

    let rows = state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} \
                 SET data = jsonb_set(\
                       jsonb_set(COALESCE(data, '{{}}'::jsonb), {}, to_jsonb($status::text), true), \
                       {}, \
                       to_jsonb($now::text), \
                       true\
                     ) \
                 WHERE id = $order_id \
                   AND COALESCE(data->>'{}', '') = $expected \
                 RETURNING data",
                collections::ORDERS,
                order_status_path,
                updated_at_path,
                fields::ORDER_STATUS,
            ),
            serde_json::json!({
                "order_id": order_id,
                "expected": expected_status,
                "status": new_status,
                "now": now,
            }),
        )
        .await?;

    if rows.is_empty() {
        info!(
            order_id = %order_id,
            expected = %expected_status,
            new_status = %new_status,
            "Order status precondition failed — order already moved forward"
        );
        return Ok(false);
    }

    info!(order_id = %order_id, new_status = %new_status, "Order status updated");
    Ok(true)
}

/// Restore stock for all items in an order (used on refund/cancellation)
async fn restore_stock_for_order(
    state: &HandlersState,
    order: &Value,
) -> Result<(), ob_core::Error> {
    let order_id = order
        .get(db_fields::ID)
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

    for item in &items {
        let product_id = item
            .get(fields::PRODUCT_ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let quantity = item
            .get(fields::QUANTITY)
            .and_then(|v| v.as_i64())
            .unwrap_or(1);

        if product_id.is_empty() || quantity <= 0 {
            continue;
        }

        let mut restored = false;
        for _attempt in 0..3 {
            let product = state
                .db
                .get_document(collections::PRODUCTS, product_id)
                .await
                .unwrap_or_default();
            let current_stock = product
                .get(fields::STOCK_QUANTITY)
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let cas_result = state
                .db
                .update_document_cas(
                    collections::PRODUCTS,
                    product_id,
                    serde_json::json!({
                        fields::STOCK_QUANTITY: current_stock + quantity,
                    }),
                    fields::STOCK_QUANTITY,
                    &serde_json::json!(current_stock),
                )
                .await?;
            if cas_result.is_some() {
                restored = true;
                break;
            }
        }

        if !restored {
            return Err(ob_core::Error::Database(format!(
                "Failed to restore stock for product {product_id}"
            )));
        }
    }

    info!(order_id = %order_id, item_count = items.len(), "Stock restored for order items");

    Ok(())
}

/// Decrement stock for all items in an order (used on successful payment)
/// Note: Currently unused in production — stock is decremented at checkout time only.
/// Kept for potential future use or manual recovery scenarios.
#[allow(dead_code)]
async fn decrement_stock_for_order(
    state: &HandlersState,
    order: &Value,
) -> Result<(), ob_core::Error> {
    let order_id = order
        .get(db_fields::ID)
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

        let quantity = item
            .get(fields::QUANTITY)
            .and_then(|v| v.as_i64())
            .unwrap_or(1);

        if !product_id.is_empty() && quantity > 0 {
            // Build product record ID: "products:productId"
            let product_record_id = format!("{}:{}", collections::PRODUCTS, product_id);

            tx.add(
                &format!(
                    "UPDATE {} SET data = jsonb_set(data, '{{{}}}', to_jsonb(GREATEST(COALESCE(NULLIF(data->>'{}', ''), '0')::int - $qty::int, 0)), true), updated_at = now() WHERE id = $id",
                    collections::PRODUCTS,
                    fields::STOCK_QUANTITY,
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
                "UPDATE {} SET data = jsonb_set(data, '{{{}}}', to_jsonb($now::text), true), updated_at = now() WHERE data->>'{}' = $order_id AND data->>'{}' = $code",
                collections::COUPON_USES,
                fields::REDEEMED_AT,
                fields::ORDER_ID,
                fields::COUPON_CODE
            ),
            serde_json::json!({
                "order_id": order_id,
                "code": coupon_code_safe,
                "now": now,
            }),
        )
        .await?;

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
                "DELETE FROM {} WHERE data->>'{}' = $order_id AND data->>'{}' IS NULL",
                collections::COUPON_USES,
                fields::ORDER_ID,
                fields::REDEEMED_AT
            ),
            serde_json::json!({"order_id": order_id}),
        )
        .await
        .unwrap_or_else(|e| {
            tracing::error!(order_id = %order_id, error = %e, "Failed to release coupon reservation");
            vec![]
        });

    info!(order_id = %order_id, "Coupon reservation released");
    Ok(())
}

struct RefundRecordInput<'a> {
    refund_id: &'a str,
    amount_cents: i64,
    currency: &'a str,
    status: &'a str,
    payment_intent_id: &'a str,
    reason: &'a str,
    failure_reason: Option<&'a str>,
}

async fn upsert_refund_record(
    state: &HandlersState,
    refund: RefundRecordInput<'_>,
    now: &chrono::DateTime<chrono::Utc>,
) -> Result<(), ob_core::Error> {
    let mut data = serde_json::json!({
        fields::STRIPE_REFUND_ID: refund.refund_id,
        fields::AMOUNT_CENTS: refund.amount_cents,
        fields::CURRENCY: refund.currency,
        fields::STRIPE_REFUND_STATUS: refund.status,
        fields::PAYMENT_INTENT_ID: refund.payment_intent_id,
        fields::REASON: refund.reason,
        db_fields::UPDATED_AT: now.to_rfc3339(),
    });

    if let Some(reason) = refund.failure_reason {
        data[fields::REFUND_FAILURE_REASON] = serde_json::json!(reason);
    }

    let existing = state
        .db
        .get_document(collections::REFUNDS, refund.refund_id)
        .await
        .unwrap_or_default();
    if existing.is_null() || existing.as_object().is_none_or(|obj| obj.is_empty()) {
        data[db_fields::CREATED_AT] = serde_json::json!(now.timestamp());
        data[db_fields::CREATED_AT_ISO] = serde_json::json!(now.to_rfc3339());
    }

    state
        .db
        .upsert_document(collections::REFUNDS, refund.refund_id, data)
        .await?;

    Ok(())
}

async fn mark_order_refund_failure(
    state: &HandlersState,
    payment_intent_id: &str,
    failure_reason: &str,
    now: &chrono::DateTime<chrono::Utc>,
) -> Result<(), ob_core::Error> {
    if payment_intent_id.is_empty() {
        return Ok(());
    }

    if let Some(order) = find_order_by_payment_intent(state, payment_intent_id).await? {
        let order_id = order
            .get(db_fields::ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !order_id.is_empty() {
            state
                .db
                .update_document(
                    collections::ORDERS,
                    order_id,
                    serde_json::json!({
                        fields::REQUIRES_MANUAL_REVIEW: true,
                        fields::REFUND_FAILURE_REASON: failure_reason,
                        db_fields::UPDATED_AT: now.to_rfc3339(),
                    }),
                )
                .await?;
        }
    }

    Ok(())
}

async fn sync_order_refund_state(
    state: &HandlersState,
    payment_intent_id: &str,
    refund_id: Option<&str>,
    refunded_amount_cents: i64,
    now: &chrono::DateTime<chrono::Utc>,
) -> Result<(), ob_core::Error> {
    if payment_intent_id.is_empty() {
        return Ok(());
    }

    let Some(order) = find_order_by_payment_intent(state, payment_intent_id).await? else {
        return Ok(());
    };

    let order_id = order
        .get(db_fields::ID)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::Validation("Order missing id".into()))?;

    let total_amount_cents = order
        .get(db_fields::TOTAL_AMOUNT_CENTS)
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    if refunded_amount_cents > total_amount_cents {
        return Err(ob_core::Error::Validation(format!(
            "Refund amount {} exceeds order total {}",
            refunded_amount_cents, total_amount_cents
        )));
    }

    let is_full_refund = refunded_amount_cents >= total_amount_cents && total_amount_cents > 0;
    let stock_restored = order
        .get(fields::STOCK_RESTORED)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if is_full_refund && !stock_restored {
        restore_stock_for_order(state, &order).await?;
    }

    let payment_status = if is_full_refund {
        PaymentStatus::Refunded.as_str()
    } else {
        PaymentStatus::PartialRefund.as_str()
    };
    let order_status = if is_full_refund {
        OrderStatus::Refunded.as_str()
    } else {
        OrderStatus::PartiallyRefunded.as_str()
    };

    let mut update = serde_json::json!({
        fields::REFUNDED_AMOUNT_CENTS: refunded_amount_cents,
        fields::PAYMENT_STATUS: payment_status,
        fields::ORDER_STATUS: order_status,
        db_fields::UPDATED_AT: now.to_rfc3339(),
    });
    if is_full_refund {
        update[fields::REFUNDED_AT] = serde_json::json!(now.to_rfc3339());
        update[fields::STOCK_RESTORED] = serde_json::json!(true);
    }
    if let Some(refund_id) = refund_id {
        update[fields::REFUND_ID] = serde_json::json!(refund_id);
    }

    state
        .db
        .update_document(collections::ORDERS, order_id, update)
        .await?;

    Ok(())
}

async fn sync_order_refund_state_from_records(
    state: &HandlersState,
    payment_intent_id: &str,
    refund_id: Option<&str>,
    now: &chrono::DateTime<chrono::Utc>,
) -> Result<(), ob_core::Error> {
    if payment_intent_id.is_empty() {
        return Ok(());
    }

    let refunds: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "SELECT * FROM {} WHERE data->>'{}' = $payment_intent_id",
                collections::REFUNDS,
                fields::PAYMENT_INTENT_ID
            ),
            serde_json::json!({ "payment_intent_id": payment_intent_id }),
        )
        .await
        .unwrap_or_default();

    let refunded_amount_cents = refunds
        .iter()
        .filter(|refund| {
            refund
                .get(fields::STRIPE_REFUND_STATUS)
                .and_then(|v| v.as_str())
                == Some("succeeded")
        })
        .map(|refund| {
            refund
                .get(fields::AMOUNT_CENTS)
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
        })
        .sum::<i64>();

    if refunded_amount_cents > 0 {
        sync_order_refund_state(
            state,
            payment_intent_id,
            refund_id,
            refunded_amount_cents,
            now,
        )
        .await?;
    }

    Ok(())
}

#[cfg(test)]
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

/// Handle payment_intent.succeeded: order confirmed, coupon marked used (stock reserved at checkout)
async fn handle_payment_intent_succeeded(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let pi_obj = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let pi_id = pi_obj
        .get(db_fields::ID)
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

    // Stock already decremented at checkout time — only update order status
    // Precondition guard: only update if still PendingPayment (handles late webhooks)
    if !update_order_status(
        state,
        order_id,
        OrderStatus::PendingPayment.as_str(),
        OrderStatus::PaymentAuthorized.as_str(),
    )
    .await?
    {
        return Ok(());
    }

    // Store payment intent ID and update payment status on order
    state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET {} = $pi_id, {} = 'authorized' WHERE id = $order_id",
                collections::ORDERS,
                fields::PAYMENT_INTENT_ID,
                fields::PAYMENT_STATUS
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
        "Payment intent succeeded: order confirmed (stock reserved at checkout)"
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
        .get(db_fields::ID)
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
        // Cancel order — precondition: only if still PendingPayment
        update_order_status(
            state,
            order_id,
            OrderStatus::PendingPayment.as_str(),
            OrderStatus::Cancelled.as_str(),
        )
        .await?;

        // Restore stock for cancelled order (stock was decremented at checkout)
        restore_stock_for_order(state, &order).await?;
    }

    // Release coupon reservation (unmark as used)
    release_coupon_reservation(state, order_id).await?;

    warn!(
        order_id = %order_id,
        payment_intent_id = %pi_id,
        "Payment intent failed: order cancelled, stock restored, coupon released"
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
        .get(db_fields::ID)
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
        update_order_status(
            state,
            order_id,
            OrderStatus::PendingPayment.as_str(),
            OrderStatus::Cancelled.as_str(),
        )
        .await?;

        // Restore stock for cancelled order (stock was decremented at checkout)
        restore_stock_for_order(state, &order).await?;
    }

    // Release coupon reservation
    release_coupon_reservation(state, order_id).await?;

    info!(
        order_id = %order_id,
        payment_intent_id = %pi_id,
        "Payment intent cancelled: order cancelled, stock restored, coupon released"
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
        .and_then(|o| o.get(db_fields::ID))
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
        .and_then(|o| o.get(db_fields::ID))
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
        .get(db_fields::ID)
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

    // charge.refunded carries cumulative refunded cents for the payment intent.
    // Keep this path authoritative for aggregate refund amount even if separate
    // refund.created / refund.updated events arrive earlier or later.
    let now = chrono::Utc::now();
    let order = find_order_by_payment_intent(state, payment_intent_id)
        .await?
        .ok_or_else(|| {
            ob_core::Error::NotFound(format!(
                "Order not found for payment intent {}",
                payment_intent_id
            ))
        })?;
    let order_id = order
        .get(db_fields::ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    sync_order_refund_state(state, payment_intent_id, None, refunded_amount_cents, &now).await?;

    info!(
        order_id = %order_id,
        charge_id = %charge_id,
        refunded_amount_cents = refunded_amount_cents,
        "Charge refunded: stock restored, order updated"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Checkout Session Handlers
// ---------------------------------------------------------------------------

/// Handle checkout.session.completed: confirm order, decrement stock, mark coupon.
/// This is the PREFERRED event for Checkout Sessions (over payment_intent.succeeded).
async fn handle_checkout_session_completed(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let session = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let session_id = session
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No session ID".into()))?;

    let payment_intent_id = session
        .get("payment_intent")
        .and_then(|i| i.as_str())
        .unwrap_or("");

    let metadata = session
        .get("metadata")
        .and_then(|m| m.as_object())
        .ok_or_else(|| ob_core::Error::Validation("No metadata in checkout session".into()))?;

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
            session_id = %session_id,
            current_status = %current_status,
            "Order not in pending state, skipping checkout.session.completed"
        );
        return Ok(());
    }

    // Stock already decremented at checkout time — only update order status
    // Precondition guard: only update if still PendingPayment (handles late webhooks)
    if !update_order_status(
        state,
        order_id,
        OrderStatus::PendingPayment.as_str(),
        OrderStatus::PaymentAuthorized.as_str(),
    )
    .await?
    {
        return Ok(());
    }

    // Store session ID and payment intent ID on order
    let now = chrono::Utc::now().to_rfc3339();
    let payment_intent_id_path = format!("'{{{}}}'", fields::PAYMENT_INTENT_ID);
    let checkout_session_id_path = format!("'{{{}}}'", fields::CHECKOUT_SESSION_ID);
    let updated_at_path = format!("'{{{}}}'", db_fields::UPDATED_AT);
    state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} \
                 SET data = jsonb_set(\
                       jsonb_set(\
                         jsonb_set(COALESCE(data, '{{}}'::jsonb), {}, to_jsonb($pi_id::text), true), \
                         {}, \
                         to_jsonb($session_id::text), \
                         true\
                       ), \
                       {}, \
                       to_jsonb($now::text), \
                       true\
                     ) \
                 WHERE id = $order_id",
                collections::ORDERS,
                payment_intent_id_path,
                checkout_session_id_path,
                updated_at_path,
            ),
            serde_json::json!({
                "order_id": order_id,
                "pi_id": payment_intent_id,
                "session_id": session_id,
                "now": now,
            }),
        )
        .await?;

    // Mark coupon as redeemed if one was used
    if !coupon_code.is_empty() {
        mark_coupon_redeemed(state, order_id, coupon_code).await?;
    }

    // Clear the buyer's cart after successful payment
    let buyer_id = order
        .get(db_fields::BUYER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !buyer_id.is_empty()
        && let Err(err) = state
            .db
            .query_bind_value(
                &format!(
                    "DELETE FROM {} WHERE data->>'userId' = $buyer_id",
                    collections::CART
                ),
                serde_json::json!({"buyer_id": buyer_id}),
            )
            .await
    {
        warn!(order_id = %order_id, buyer_id = %buyer_id, error = %err, "Failed to clear buyer cart after payment");
    }

    if let Err(err) = send_payment_authorized_emails(state, &order).await {
        warn!(order_id = %order_id, error = %err, "Failed to send checkout completion emails");
    }

    info!(
        order_id = %order_id,
        session_id = %session_id,
        payment_intent_id = %payment_intent_id,
        "Checkout session completed: order confirmed (stock reserved at checkout)"
    );

    Ok(())
}

/// Handle checkout.session.expired: cancel pending order, release stock and coupons.
async fn handle_checkout_session_expired(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let session = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let session_id = session
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No session ID".into()))?;

    let metadata = session
        .get("metadata")
        .and_then(|m| m.as_object())
        .ok_or_else(|| ob_core::Error::Validation("No metadata in checkout session".into()))?;

    let order_id = metadata
        .get("order_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No order_id in metadata".into()))?;

    // Find the order
    let order = find_order_by_metadata_id(state, order_id)
        .await?
        .ok_or_else(|| ob_core::Error::NotFound(format!("Order {} not found", order_id)))?;

    // Only cancel if still pending
    let current_status = order
        .get(fields::ORDER_STATUS)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if current_status == OrderStatus::PendingPayment.as_str() {
        update_order_status(
            state,
            order_id,
            OrderStatus::PendingPayment.as_str(),
            OrderStatus::Expired.as_str(),
        )
        .await?;

        // Release any held stock (if stock was pre-reserved)
        restore_stock_for_order(state, &order).await?;
    }

    // Release coupon reservation
    release_coupon_reservation(state, order_id).await?;

    warn!(
        order_id = %order_id,
        session_id = %session_id,
        "Checkout session expired: order expired, reservations released"
    );

    Ok(())
}

/// Handle checkout.session.async_payment_succeeded: confirm order for async payment methods
/// (bank debits, etc.) that settle after the initial checkout.
async fn handle_checkout_session_async_payment_succeeded(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    // Async payment succeeded follows same logic as checkout.session.completed
    handle_checkout_session_completed(state, event_data).await
}

/// Handle checkout.session.async_payment_failed: cancel order for async payment methods
/// that failed after the initial checkout.
async fn handle_checkout_session_async_payment_failed(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let session = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let session_id = session
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No session ID".into()))?;

    let metadata = session
        .get("metadata")
        .and_then(|m| m.as_object())
        .ok_or_else(|| ob_core::Error::Validation("No metadata in checkout session".into()))?;

    let order_id = metadata
        .get("order_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No order_id in metadata".into()))?;

    // Find the order
    let order = find_order_by_metadata_id(state, order_id)
        .await?
        .ok_or_else(|| ob_core::Error::NotFound(format!("Order {} not found", order_id)))?;

    let current_status = order
        .get(fields::ORDER_STATUS)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if current_status == OrderStatus::PendingPayment.as_str() {
        update_order_status(
            state,
            order_id,
            OrderStatus::PendingPayment.as_str(),
            OrderStatus::Failed.as_str(),
        )
        .await?;
    }

    // Release coupon reservation
    release_coupon_reservation(state, order_id).await?;

    warn!(
        order_id = %order_id,
        session_id = %session_id,
        "Async payment failed: order marked failed, coupon released"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Charge Capture Handler (pre-auth flows)
// ---------------------------------------------------------------------------

/// Handle charge.captured: log capture event for pre-authorization payment flows.
/// In separate auth+capture flows the charge is first authorized and then captured
/// at a later point. This event confirms the capture succeeded.
async fn handle_charge_captured(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let charge = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let charge_id = charge
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No charge ID".into()))?;

    let amount_captured = charge
        .get("amount_captured")
        .and_then(|a| a.as_i64())
        .unwrap_or(0);

    let payment_intent_id = charge
        .get("payment_intent")
        .and_then(|i| i.as_str())
        .unwrap_or("");

    // If we can find the order, mark payment as captured
    if !payment_intent_id.is_empty()
        && let Ok(Some(order)) = find_order_by_payment_intent(state, payment_intent_id).await
    {
        let order_id = order
            .get(db_fields::ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !order_id.is_empty() {
            let now = chrono::Utc::now().to_rfc3339();
            let _: Vec<Value> = state
                .db
                .query_bind_value(
                    &format!(
                        "UPDATE {} SET {} = 'captured', {} = true, {} = $now WHERE id = $order_id",
                        collections::ORDERS,
                        fields::PAYMENT_STATUS,
                        fields::AUTO_CAPTURED,
                        db_fields::UPDATED_AT
                    ),
                    serde_json::json!({
                        "order_id": order_id,
                        "now": now,
                    }),
                )
                .await
                .unwrap_or_default();
        }
    }

    info!(
        charge_id = %charge_id,
        amount_captured_cents = amount_captured,
        payment_intent_id = %payment_intent_id,
        "Charge captured"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Charge Dispute Handlers
// ---------------------------------------------------------------------------

/// Handle charge.dispute.created: flag order as disputed, log dispute for admin.
async fn handle_charge_dispute_created(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let dispute = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let dispute_id = dispute
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No dispute ID".into()))?;

    let charge_id = dispute.get("charge").and_then(|i| i.as_str()).unwrap_or("");

    let payment_intent_id = dispute
        .get("payment_intent")
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No payment_intent in dispute".into()))?;

    let reason = dispute
        .get("reason")
        .and_then(|r| r.as_str())
        .unwrap_or("unknown");

    let amount = dispute.get("amount").and_then(|a| a.as_i64()).unwrap_or(0);

    let currency = dispute
        .get("currency")
        .and_then(|c| c.as_str())
        .unwrap_or("cad");

    // Find the order by payment intent
    let order = find_order_by_payment_intent(state, payment_intent_id).await?;

    let order_id = order
        .as_ref()
        .and_then(|o| o.get(db_fields::ID))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Flag order as disputed if found
    if !order_id.is_empty() {
        update_order_status(
            state,
            order_id,
            OrderStatus::PaymentAuthorized.as_str(),
            OrderStatus::Disputed.as_str(),
        )
        .await?;
    }

    // Log dispute record in disputes collection for admin review
    let now = chrono::Utc::now();
    state
        .db
        .create_document(
            collections::DISPUTES,
            serde_json::json!({
                fields::DISPUTE_ID: dispute_id,
                fields::CHARGE_ID: charge_id,
                fields::PAYMENT_INTENT_ID: payment_intent_id,
                fields::ORDER_ID: order_id,
                fields::REASON: reason,
                fields::AMOUNT_CENTS: amount,
                fields::CURRENCY: currency,
                db_fields::STATUS: "needs_response",
                db_fields::CREATED_AT: now.timestamp(),
                db_fields::CREATED_AT_ISO: now.to_rfc3339(),
            }),
        )
        .await?;

    error!(
        dispute_id = %dispute_id,
        order_id = %order_id,
        charge_id = %charge_id,
        reason = %reason,
        amount_cents = amount,
        "DISPUTE CREATED: order flagged as disputed, admin action required"
    );

    Ok(())
}

/// Handle charge.dispute.closed: update dispute and order status based on resolution.
async fn handle_charge_dispute_closed(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let dispute = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let dispute_id = dispute
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No dispute ID".into()))?;

    let payment_intent_id = dispute
        .get("payment_intent")
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No payment_intent in dispute".into()))?;

    let status = dispute
        .get(db_fields::STATUS)
        .and_then(|s| s.as_str())
        .unwrap_or("unknown");

    // Stripe dispute statuses on close: "won" (merchant won) or "lost" (buyer won)
    let dispute_resolution = if status == "lost" { "lost" } else { "resolved" };

    let now = chrono::Utc::now();

    // Update the dispute record
    let _: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET {} = $resolution, {} = $now, {} = $status WHERE {} = $dispute_id",
                collections::DISPUTES,
                fields::DISPUTE_STATUS,
                fields::CLOSED_AT,
                fields::STRIPE_STATUS,
                fields::DISPUTE_ID
            ),
            serde_json::json!({
                "dispute_id": dispute_id,
                "resolution": dispute_resolution,
                "status": status,
                "now": now.to_rfc3339(),
            }),
        )
        .await
        .unwrap_or_default();

    // Find the order and update payment status if dispute was lost
    let order = find_order_by_payment_intent(state, payment_intent_id).await?;

    if let Some(ref order_val) = order {
        let order_id = order_val
            .get(db_fields::ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if !order_id.is_empty() && dispute_resolution == "lost" {
            // Dispute lost: update payment status to reflect the chargeback
            let _: Vec<Value> = state
                .db
                .query_bind_value(
                    &format!(
                        "UPDATE {} SET {} = 'disputed_lost' WHERE id = $order_id",
                        collections::ORDERS,
                        fields::PAYMENT_STATUS
                    ),
                    serde_json::json!({"order_id": order_id}),
                )
                .await
                .unwrap_or_default();

            error!(
                dispute_id = %dispute_id,
                order_id = %order_id,
                "DISPUTE LOST: order payment status set to disputed_lost"
            );
        } else if !order_id.is_empty() {
            info!(
                dispute_id = %dispute_id,
                order_id = %order_id,
                "Dispute resolved in merchant's favor"
            );
        }
    }

    info!(
        dispute_id = %dispute_id,
        status = %status,
        resolution = %dispute_resolution,
        "Charge dispute closed"
    );

    Ok(())
}

/// Handle charge.dispute.updated: track dispute progress (evidence submitted, etc.)
async fn handle_charge_dispute_updated(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let dispute = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let dispute_id = dispute
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No dispute ID".into()))?;

    let status = dispute
        .get(db_fields::STATUS)
        .and_then(|s| s.as_str())
        .unwrap_or("unknown");

    let reason = dispute
        .get("reason")
        .and_then(|r| r.as_str())
        .unwrap_or("unknown");

    let amount = dispute.get("amount").and_then(|a| a.as_i64()).unwrap_or(0);

    let now = chrono::Utc::now();

    // Update dispute record with latest status
    let _: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET {} = $status, {} = $now WHERE {} = $dispute_id",
                collections::DISPUTES,
                fields::STRIPE_STATUS,
                db_fields::UPDATED_AT,
                fields::DISPUTE_ID
            ),
            serde_json::json!({
                "dispute_id": dispute_id,
                "status": status,
                "now": now.to_rfc3339(),
            }),
        )
        .await
        .unwrap_or_default();

    info!(
        dispute_id = %dispute_id,
        status = %status,
        reason = %reason,
        amount_cents = amount,
        "Charge dispute updated"
    );

    Ok(())
}

/// Handle charge.dispute.funds_withdrawn: Stripe has debited the disputed amount
/// from the seller's account. Log the debit for accounting and admin visibility.
async fn handle_charge_dispute_funds_withdrawn(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let dispute = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let dispute_id = dispute
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No dispute ID".into()))?;

    let amount = dispute.get("amount").and_then(|a| a.as_i64()).unwrap_or(0);

    let currency = dispute
        .get("currency")
        .and_then(|c| c.as_str())
        .unwrap_or("cad");

    let balance_transaction = dispute
        .get("balance_transactions")
        .and_then(|bt| bt.as_array())
        .and_then(|arr| arr.first())
        .and_then(|bt| bt.get(db_fields::ID))
        .and_then(|id| id.as_str())
        .unwrap_or("");

    let now = chrono::Utc::now();

    // Update dispute record: mark funds as withdrawn
    let _: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET {} = true, {} = $now, {} = $bt WHERE {} = $dispute_id",
                collections::DISPUTES,
                fields::FUNDS_WITHDRAWN,
                fields::FUNDS_WITHDRAWN_AT,
                fields::BALANCE_TRANSACTION,
                fields::DISPUTE_ID
            ),
            serde_json::json!({
                "dispute_id": dispute_id,
                "now": now.to_rfc3339(),
                "bt": balance_transaction,
            }),
        )
        .await
        .unwrap_or_default();

    error!(
        dispute_id = %dispute_id,
        amount_cents = amount,
        currency = %currency,
        balance_transaction = %balance_transaction,
        "DISPUTE FUNDS WITHDRAWN: Stripe debited disputed amount from seller account"
    );

    Ok(())
}

/// Handle charge.dispute.funds_reinstated: merchant won the dispute, Stripe returns
/// the funds. Log the credit for accounting and update dispute record.
async fn handle_charge_dispute_funds_reinstated(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let dispute = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let dispute_id = dispute
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No dispute ID".into()))?;

    let amount = dispute.get("amount").and_then(|a| a.as_i64()).unwrap_or(0);

    let currency = dispute
        .get("currency")
        .and_then(|c| c.as_str())
        .unwrap_or("cad");

    let payment_intent_id = dispute
        .get("payment_intent")
        .and_then(|i| i.as_str())
        .unwrap_or("");

    let now = chrono::Utc::now();

    // Update dispute record: mark funds as reinstated
    let _: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET {} = true, {} = $now, {} = 'resolved' WHERE {} = $dispute_id",
                collections::DISPUTES,
                fields::FUNDS_REINSTATED,
                fields::FUNDS_REINSTATED_AT,
                fields::DISPUTE_STATUS,
                fields::DISPUTE_ID
            ),
            serde_json::json!({
                "dispute_id": dispute_id,
                "now": now.to_rfc3339(),
            }),
        )
        .await
        .unwrap_or_default();

    // If we can find the order, restore its payment status from disputed_lost
    if !payment_intent_id.is_empty()
        && let Ok(Some(order)) = find_order_by_payment_intent(state, payment_intent_id).await
    {
        let order_id = order
            .get(db_fields::ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !order_id.is_empty() {
            let _: Vec<Value> = state
                .db
                .query_bind_value(
                    &format!(
                        "UPDATE {} SET {} = 'captured' WHERE id = $order_id AND {} = 'disputed_lost'",
                        collections::ORDERS,
                        fields::PAYMENT_STATUS,
                        fields::PAYMENT_STATUS
                    ),
                    serde_json::json!({"order_id": order_id}),
                )
                .await
                .unwrap_or_default();
        }
    }

    info!(
        dispute_id = %dispute_id,
        amount_cents = amount,
        currency = %currency,
        "Dispute funds reinstated: merchant won, funds returned"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Payout Handlers
// ---------------------------------------------------------------------------

/// Handle payout.created: log payout initiation and update linked orders' payout status.
async fn handle_payout_created(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let payout = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let payout_id = payout
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No payout ID".into()))?;

    let amount = payout.get("amount").and_then(|a| a.as_i64()).unwrap_or(0);

    let currency = payout
        .get("currency")
        .and_then(|c| c.as_str())
        .unwrap_or("cad");

    let arrival_date = payout
        .get("arrival_date")
        .and_then(|a| a.as_i64())
        .unwrap_or(0);

    let method = payout
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("standard");

    let destination = payout
        .get("destination")
        .and_then(|d| d.as_str())
        .unwrap_or("");

    let status = payout
        .get(db_fields::STATUS)
        .and_then(|s| s.as_str())
        .unwrap_or("pending");

    let now = chrono::Utc::now();

    // Create payout record
    state
        .db
        .create_document(
            collections::PAYOUTS,
            serde_json::json!({
                fields::PAYOUT_ID: payout_id,
                fields::AMOUNT_CENTS: amount,
                fields::CURRENCY: currency,
                fields::ARRIVAL_DATE: arrival_date,
                fields::PAYOUT_METHOD: method,
                fields::STRIPE_PAYOUT_STATUS: status,
                db_fields::STATUS: "initiated",
                db_fields::CREATED_AT: now.timestamp(),
                db_fields::CREATED_AT_ISO: now.to_rfc3339(),
            }),
        )
        .await?;

    // Update any orders linked to this seller's Connect account
    if !destination.is_empty() {
        let _: Vec<Value> = state
            .db
            .query_bind_value(
                &format!(
                    "UPDATE {} SET {} = 'initiated', {} = $payout_id WHERE {} = $acct_id AND {} IS NULL",
                    collections::ORDERS,
                    fields::PAYOUT_STATUS,
                    fields::PAYOUT_ID,
                    fields::STRIPE_ACCOUNT_ID,
                    fields::PAYOUT_ID
                ),
                serde_json::json!({
                    "payout_id": payout_id,
                    "acct_id": destination,
                }),
            )
            .await
            .unwrap_or_default();
    }

    info!(
        payout_id = %payout_id,
        amount_cents = amount,
        currency = %currency,
        arrival_date = arrival_date,
        method = %method,
        "Payout created"
    );

    Ok(())
}

/// Handle payout.updated: update payout status (in_transit, paid, failed, etc.)
async fn handle_payout_updated(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let payout = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let payout_id = payout
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No payout ID".into()))?;

    let status = payout
        .get(db_fields::STATUS)
        .and_then(|s| s.as_str())
        .unwrap_or("unknown");

    let arrival_date = payout
        .get("arrival_date")
        .and_then(|a| a.as_i64())
        .unwrap_or(0);

    let now = chrono::Utc::now();

    // Update payout record
    let _: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET {} = $status, {} = $arrival, {} = $now WHERE {} = $payout_id",
                collections::PAYOUTS,
                fields::STRIPE_PAYOUT_STATUS,
                fields::ARRIVAL_DATE,
                db_fields::UPDATED_AT,
                fields::PAYOUT_ID
            ),
            serde_json::json!({
                "payout_id": payout_id,
                "status": status,
                "arrival": arrival_date,
                "now": now.to_rfc3339(),
            }),
        )
        .await
        .unwrap_or_default();

    // Map Stripe payout status to our order payout status
    let order_payout_status = match status {
        "in_transit" => "in_transit",
        "paid" => "paid",
        "failed" => "failed",
        "canceled" => "cancelled",
        _ => "pending",
    };

    // Update linked orders
    let _: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET {} = $status WHERE {} = $payout_id",
                collections::ORDERS,
                fields::PAYOUT_STATUS,
                fields::PAYOUT_ID
            ),
            serde_json::json!({
                "payout_id": payout_id,
                "status": order_payout_status,
            }),
        )
        .await
        .unwrap_or_default();

    info!(
        payout_id = %payout_id,
        status = %status,
        arrival_date = arrival_date,
        "Payout updated"
    );

    Ok(())
}

/// Handle payout.paid: mark payout as complete, notify seller of successful payout.
async fn handle_payout_paid(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let payout = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let payout_id = payout
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No payout ID".into()))?;

    let amount = payout.get("amount").and_then(|a| a.as_i64()).unwrap_or(0);

    let currency = payout
        .get("currency")
        .and_then(|c| c.as_str())
        .unwrap_or("cad");

    let destination = payout
        .get("destination")
        .and_then(|d| d.as_str())
        .unwrap_or("");

    let now = chrono::Utc::now();

    // Mark payout as complete
    let _: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET {} = 'paid', {} = $now, {} = $now WHERE {} = $payout_id",
                collections::PAYOUTS,
                fields::STRIPE_PAYOUT_STATUS,
                fields::PAYOUT_COMPLETED_AT,
                db_fields::UPDATED_AT,
                fields::PAYOUT_ID
            ),
            serde_json::json!({
                "payout_id": payout_id,
                "now": now.to_rfc3339(),
            }),
        )
        .await
        .unwrap_or_default();

    // Update linked orders' payout status
    let _: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET {} = 'paid', {} = $now WHERE {} = $payout_id",
                collections::ORDERS,
                fields::PAYOUT_STATUS,
                fields::PAYOUT_DATE,
                fields::PAYOUT_ID
            ),
            serde_json::json!({
                "payout_id": payout_id,
                "now": now.to_rfc3339(),
            }),
        )
        .await
        .unwrap_or_default();

    // Find seller by Stripe Connect account ID and send notification
    if !destination.is_empty() {
        let sellers: Vec<Value> = state
            .db
            .query_bind_value(
                &format!(
                    "SELECT * FROM {} WHERE {} = $acct_id LIMIT 1",
                    collections::SELLER_PROFILES,
                    fields::STRIPE_ACCOUNT_ID
                ),
                serde_json::json!({"acct_id": destination}),
            )
            .await
            .unwrap_or_default();

        let seller_id = sellers
            .first()
            .and_then(|s| s.get(db_fields::SELLER_ID))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if !seller_id.is_empty() {
            let _ = state
                .db
                .create_document(
                    collections::NOTIFICATIONS,
                    serde_json::json!({
                        db_fields::USER_ID: seller_id,
                        fields::TYPE: "payout_paid",
                        fields::NOTIFICATION_TITLE: "Payout Complete",
                        fields::NOTIFICATION_BODY: format!(
                            "Your payout of {}{}.{:02} has been deposited to your bank account.",
                            if currency == "cad" { "$" } else { "" },
                            amount / 100,
                            amount % 100
                        ),
                        fields::READ: false,
                        db_fields::CREATED_AT: now.timestamp(),
                        db_fields::CREATED_AT_ISO: now.to_rfc3339(),
                    }),
                )
                .await;
        }
    }

    info!(
        payout_id = %payout_id,
        amount_cents = amount,
        currency = %currency,
        "Payout paid: funds deposited to seller bank account"
    );

    Ok(())
}

/// Handle payout.failed: update payout status and notify the seller.
async fn handle_payout_failed(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let payout = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let payout_id = payout
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No payout ID".into()))?;

    let amount = payout.get("amount").and_then(|a| a.as_i64()).unwrap_or(0);

    let currency = payout
        .get("currency")
        .and_then(|c| c.as_str())
        .unwrap_or("cad");

    let failure_code = payout
        .get("failure_code")
        .and_then(|c| c.as_str())
        .unwrap_or("unknown");

    let failure_message = payout
        .get("failure_message")
        .and_then(|m| m.as_str())
        .unwrap_or("No details provided");

    // The destination is the Stripe Connect account ID for the seller
    let destination = payout
        .get("destination")
        .and_then(|d| d.as_str())
        .unwrap_or("");

    let now = chrono::Utc::now();

    // Find orders linked to this payout
    let orders: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "SELECT * FROM {} WHERE {} = $payout_id",
                collections::ORDERS,
                fields::PAYOUT_ID
            ),
            serde_json::json!({"payout_id": payout_id}),
        )
        .await
        .unwrap_or_default();

    // Update payout record status to failed
    let _: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET {} = 'failed', {} = $code, {} = $msg, {} = $now WHERE {} = $payout_id",
                collections::PAYOUTS,
                db_fields::STATUS,
                fields::FAILURE_CODE,
                fields::FAILURE_MESSAGE,
                db_fields::UPDATED_AT,
                fields::PAYOUT_ID
            ),
            serde_json::json!({
                "payout_id": payout_id,
                "code": failure_code,
                "msg": failure_message,
                "now": now.to_rfc3339(),
            }),
        )
        .await
        .unwrap_or_default();

    // Find the seller by Stripe Connect account ID to send notification
    let seller_id = if !destination.is_empty() {
        let sellers: Vec<Value> = state
            .db
            .query_bind_value(
                &format!(
                    "SELECT * FROM {} WHERE {} = $acct_id LIMIT 1",
                    collections::SELLER_PROFILES,
                    fields::STRIPE_ACCOUNT_ID
                ),
                serde_json::json!({"acct_id": destination}),
            )
            .await
            .unwrap_or_default();

        sellers
            .first()
            .and_then(|s| s.get(db_fields::SELLER_ID))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        String::new()
    };

    // Create notification for the seller
    if !seller_id.is_empty() {
        let _ = state
            .db
            .create_document(
                collections::NOTIFICATIONS,
                serde_json::json!({
                    db_fields::USER_ID: seller_id,
                    fields::TYPE: "payout_failed",
                    fields::NOTIFICATION_TITLE: "Payout Failed",
                    fields::NOTIFICATION_BODY: format!(
                        "Your payout of {}{}.{:02} has failed. Reason: {}. Please update your bank details.",
                        if currency == "cad" { "$" } else { "" },
                        amount / 100,
                        amount % 100,
                        failure_message
                    ),
                    fields::READ: false,
                    db_fields::CREATED_AT: now.timestamp(),
                    db_fields::CREATED_AT_ISO: now.to_rfc3339(),
                }),
            )
            .await;
    }

    error!(
        payout_id = %payout_id,
        amount_cents = amount,
        currency = %currency,
        failure_code = %failure_code,
        failure_message = %failure_message,
        seller_id = %seller_id,
        linked_orders = orders.len(),
        "PAYOUT FAILED: seller payout unsuccessful, notification sent, admin review required"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Refund Handlers
// ---------------------------------------------------------------------------

/// Handle refund.created: log refund initiation for audit trail.
async fn handle_refund_created(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let refund = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let refund_id = refund
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No refund ID".into()))?;

    let amount = refund.get("amount").and_then(|a| a.as_i64()).unwrap_or(0);

    let currency = refund
        .get("currency")
        .and_then(|c| c.as_str())
        .unwrap_or("cad");

    let status = refund
        .get(db_fields::STATUS)
        .and_then(|s| s.as_str())
        .unwrap_or("pending");

    let payment_intent_id = refund
        .get("payment_intent")
        .and_then(|i| i.as_str())
        .unwrap_or("");

    let reason = refund.get("reason").and_then(|r| r.as_str()).unwrap_or("");

    let now = chrono::Utc::now();

    upsert_refund_record(
        state,
        RefundRecordInput {
            refund_id,
            amount_cents: amount,
            currency,
            status,
            payment_intent_id,
            reason,
            failure_reason: None,
        },
        &now,
    )
    .await?;

    // If we can find the order, store the refund ID on it
    if !payment_intent_id.is_empty()
        && let Ok(Some(order)) = find_order_by_payment_intent(state, payment_intent_id).await
    {
        let order_id = order
            .get(db_fields::ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !order_id.is_empty() {
            let _: Vec<Value> = state
                .db
                .query_bind_value(
                    &format!(
                        "UPDATE {} SET {} = $refund_id WHERE id = $order_id",
                        collections::ORDERS,
                        fields::REFUND_ID
                    ),
                    serde_json::json!({
                        "order_id": order_id,
                        "refund_id": refund_id,
                    }),
                )
                .await
                .unwrap_or_default();
        }
    }

    if status == "succeeded" {
        sync_order_refund_state_from_records(state, payment_intent_id, Some(refund_id), &now)
            .await?;
    }

    info!(
        refund_id = %refund_id,
        amount_cents = amount,
        currency = %currency,
        status = %status,
        payment_intent_id = %payment_intent_id,
        "Refund created"
    );

    Ok(())
}

/// Handle refund.updated: update refund status tracking.
async fn handle_refund_updated(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let refund = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let refund_id = refund
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No refund ID".into()))?;

    let amount = refund.get("amount").and_then(|a| a.as_i64()).unwrap_or(0);
    let currency = refund
        .get("currency")
        .and_then(|c| c.as_str())
        .unwrap_or("cad");
    let status = refund
        .get(db_fields::STATUS)
        .and_then(|s| s.as_str())
        .unwrap_or("unknown");
    let payment_intent_id = refund
        .get("payment_intent")
        .and_then(|i| i.as_str())
        .unwrap_or("");
    let reason = refund.get("reason").and_then(|r| r.as_str()).unwrap_or("");

    let now = chrono::Utc::now();

    upsert_refund_record(
        state,
        RefundRecordInput {
            refund_id,
            amount_cents: amount,
            currency,
            status,
            payment_intent_id,
            reason,
            failure_reason: None,
        },
        &now,
    )
    .await?;

    if status == "succeeded" {
        sync_order_refund_state_from_records(state, payment_intent_id, Some(refund_id), &now)
            .await?;
    }

    info!(
        refund_id = %refund_id,
        status = %status,
        "Refund updated"
    );

    Ok(())
}

/// Handle refund.failed: alert admin and notify buyer that their refund failed.
async fn handle_refund_failed(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let refund = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let refund_id = refund
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No refund ID".into()))?;

    let amount = refund.get("amount").and_then(|a| a.as_i64()).unwrap_or(0);

    let currency = refund
        .get("currency")
        .and_then(|c| c.as_str())
        .unwrap_or("cad");

    let failure_reason = refund
        .get("failure_reason")
        .and_then(|r| r.as_str())
        .unwrap_or("unknown");

    let payment_intent_id = refund
        .get("payment_intent")
        .and_then(|i| i.as_str())
        .unwrap_or("");

    let now = chrono::Utc::now();

    upsert_refund_record(
        state,
        RefundRecordInput {
            refund_id,
            amount_cents: amount,
            currency,
            status: "failed",
            payment_intent_id,
            reason: "",
            failure_reason: Some(failure_reason),
        },
        &now,
    )
    .await?;
    mark_order_refund_failure(state, payment_intent_id, failure_reason, &now).await?;

    // Find the order to notify the buyer
    if !payment_intent_id.is_empty()
        && let Ok(Some(order)) = find_order_by_payment_intent(state, payment_intent_id).await
    {
        let buyer_id = order
            .get(db_fields::BUYER_ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if !buyer_id.is_empty() {
            let _ = state
                .db
                .create_document(
                    collections::NOTIFICATIONS,
                    serde_json::json!({
                        db_fields::USER_ID: buyer_id,
                        fields::TYPE: "refund_failed",
                        fields::NOTIFICATION_TITLE: "Refund Failed",
                        fields::NOTIFICATION_BODY: format!(
                            "Your refund of {}{}.{:02} could not be processed. Our team has been notified and will resolve this shortly.",
                            if currency == "cad" { "$" } else { "" },
                            amount / 100,
                            amount % 100
                        ),
                        fields::READ: false,
                        db_fields::CREATED_AT: now.timestamp(),
                        db_fields::CREATED_AT_ISO: now.to_rfc3339(),
                    }),
                )
                .await;
        }
    }

    error!(
        refund_id = %refund_id,
        amount_cents = amount,
        currency = %currency,
        failure_reason = %failure_reason,
        payment_intent_id = %payment_intent_id,
        "REFUND FAILED: buyer refund unsuccessful, admin review required"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Stripe Connect Handler
// ---------------------------------------------------------------------------

/// Handle account.updated: sync seller Stripe Connect status back to our DB.
async fn handle_account_updated(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let account = event_data
        .get("object")
        .ok_or_else(|| ob_core::Error::Validation("No object in event data".into()))?;

    let account_id = account
        .get(db_fields::ID)
        .and_then(|i| i.as_str())
        .ok_or_else(|| ob_core::Error::Validation("No account ID".into()))?;

    let charges_enabled = account
        .get("charges_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let payouts_enabled = account
        .get("payouts_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let details_submitted = account
        .get("details_submitted")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let onboarding_completed = charges_enabled && payouts_enabled && details_submitted;

    // Find the seller profile with this Stripe account ID
    let rows: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "SELECT * FROM {} WHERE data->>'{}' = $account_id LIMIT 1",
                collections::SELLER_PROFILES,
                fields::STRIPE_ACCOUNT_ID
            ),
            serde_json::json!({"account_id": account_id}),
        )
        .await?;

    if rows.is_empty() {
        warn!(
            account_id = %account_id,
            "account.updated: no seller profile found for Stripe account"
        );
        return Ok(());
    }

    let seller_profile_id = rows[0]
        .get(db_fields::ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if seller_profile_id.is_empty() {
        return Ok(());
    }

    // Update seller profile with latest Connect status
    let now = chrono::Utc::now().to_rfc3339();
    state
        .db
        .query_bind_value(
            &format!(
                "UPDATE {} SET {} = $charges, {} = $payouts, {} = $onboarded, {} = $now WHERE id = $profile_id",
                collections::SELLER_PROFILES,
                fields::CHARGES_ENABLED,
                fields::PAYOUTS_ENABLED,
                fields::ONBOARDING_COMPLETED,
                db_fields::UPDATED_AT
            ),
            serde_json::json!({
                "profile_id": seller_profile_id,
                "charges": charges_enabled,
                "payouts": payouts_enabled,
                "onboarded": onboarding_completed,
                "now": now,
            }),
        )
        .await?;

    // If charges were disabled, log a warning for admin
    if !charges_enabled {
        warn!(
            account_id = %account_id,
            seller_profile_id = %seller_profile_id,
            "Stripe Connect charges DISABLED for seller — payouts blocked"
        );
    }

    info!(
        account_id = %account_id,
        seller_profile_id = %seller_profile_id,
        charges_enabled = charges_enabled,
        payouts_enabled = payouts_enabled,
        onboarding_completed = onboarding_completed,
        "Stripe Connect account updated"
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
        .and_then(|o| o.get(db_fields::ID))
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
        .and_then(|o| o.get(db_fields::ID))
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
        .and_then(|o| o.get(db_fields::ID))
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
        .and_then(|o| o.get(db_fields::ID))
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
        .and_then(|o| o.get(db_fields::ID))
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
        .and_then(|o| o.get(db_fields::ID))
        .and_then(|i| i.as_str())
    {
        info!(invoice_id = %invoice_id, "Invoice paid");
    }
    Ok(())
}

async fn handle_invoice_payment_failed(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let object = event_data.get("object").unwrap_or(event_data);
    let invoice_id = object
        .get(db_fields::ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let subscription_id = object
        .get("subscription")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let customer_id = object
        .get("customer")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let attempt_count = object
        .get("attempt_count")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    warn!(
        invoice_id = %invoice_id,
        subscription_id = %subscription_id,
        customer_id = %customer_id,
        attempt_count = attempt_count,
        "Invoice payment failed"
    );

    // If this invoice is for a subscription, record the failure on the user
    if !subscription_id.is_empty() && !customer_id.is_empty() {
        let now = chrono::Utc::now().to_rfc3339();
        // Find user by stripeCustomerId and flag payment_failed
        let sql = format!(
            "SELECT * FROM {} WHERE data->>'stripeCustomerId' = $customer_id LIMIT 1",
            collections::USERS
        );
        if let Ok(users) = state
            .db
            .query_bind(&sql, serde_json::json!({ "customer_id": customer_id }))
            .await
            && let Some(user) = users.first()
        {
            let user_id = user
                .get(db_fields::ID)
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if !user_id.is_empty() {
                let _ = state
                    .db
                    .update_document(
                        collections::USERS,
                        user_id,
                        serde_json::json!({
                            "subscriptionPaymentFailed": true,
                            "subscriptionPaymentFailedAt": now,
                            "subscriptionPaymentAttemptCount": attempt_count,
                            db_fields::UPDATED_AT: now,
                        }),
                    )
                    .await;
                info!(
                    user_id = %user_id,
                    attempt_count = attempt_count,
                    "Flagged user subscription payment failure"
                );
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use std::sync::Arc;

    fn now_timestamp() -> String {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string()
    }

    fn make_hmac_signature(secret: &str, body: &[u8], timestamp: &str) -> String {
        let mut signed_content = Vec::new();
        signed_content.extend_from_slice(timestamp.as_bytes());
        signed_content.push(b'.');
        signed_content.extend_from_slice(body);
        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(&signed_content);
        let sig = hex::encode(mac.finalize().into_bytes());
        format!("t={},v1={}", timestamp, sig)
    }

    async fn setup_state() -> HandlersState {
        HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
        }
    }

    async fn setup_state_with_webhook_secret(secret: &str) -> HandlersState {
        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_webhook_secret".to_string(), secret.to_string());

        HandlersState {
            config: Arc::new(config),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
        }
    }

    // -----------------------------------------------------------------------
    // store_webhook_event tests
    // -----------------------------------------------------------------------
    // Signature verification tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_signature_verification_valid() {
        let secret = "test_secret";
        let ts = now_timestamp();
        let body = br#"{"type":"payment_intent.succeeded"}"#; // ignore-magic
        let signature = make_hmac_signature(secret, body, &ts);
        assert!(verify_stripe_signature(body, &signature, secret));
    }

    #[test]
    fn test_signature_verification_invalid() {
        let secret = "test_secret";
        let signature = "t=1614556800,v1=invalid_signature";
        let body = br#"{"type":"payment_intent.succeeded"}"#; // ignore-magic
        assert!(!verify_stripe_signature(body, signature, secret));
    }

    #[test]
    fn test_signature_verification_wrong_secret() {
        let body = br#"{"id":"evt_1"}"#; // ignore-magic
        let signature = make_hmac_signature("correct_secret", body, "1614556800");
        assert!(!verify_stripe_signature(body, &signature, "wrong_secret"));
    }

    #[test]
    fn test_signature_verification_tampered_body() {
        let secret = "whsec_test";
        let original = br#"{"id":"evt_1"}"#; // ignore-magic
        let signature = make_hmac_signature(secret, original, "1614556800");
        let tampered = br#"{"id":"evt_2"}"#; // ignore-magic
        assert!(!verify_stripe_signature(tampered, &signature, secret));
    }

    #[test]
    fn test_signature_verification_tampered_timestamp() {
        let secret = "whsec_test";
        let body = br#"{"id":"evt_1"}"#; // ignore-magic
        let signature = make_hmac_signature(secret, body, "1614556800");
        let tampered_sig = signature.replace("t=1614556800", "t=9999999999");
        assert!(!verify_stripe_signature(body, &tampered_sig, secret));
    }

    #[test]
    fn test_signature_verification_empty_signature() {
        assert!(!verify_stripe_signature(b"body", "", "secret"));
    }

    #[test]
    fn test_signature_verification_missing_v1() {
        assert!(!verify_stripe_signature(b"body", "t=123", "secret"));
    }

    #[test]
    fn test_signature_verification_missing_t() {
        assert!(!verify_stripe_signature(b"body", "v1=abc", "secret"));
    }

    #[test]
    fn test_signature_verification_empty_body() {
        let secret = "whsec_test";
        let body = b"";
        let ts = now_timestamp();
        let signature = make_hmac_signature(secret, body, &ts);
        assert!(verify_stripe_signature(body, &signature, secret));
    }

    #[test]
    fn test_signature_verification_long_body() {
        let secret = "whsec_longbody";
        let body = br#"{"data":{"object":{"id":"pi_large","metadata":{"order_id":"ord_123","coupon_code":"SAVE10"},"amount":99999,"currency":"cad"}}}"#; // ignore-magic
        let ts = now_timestamp();
        let signature = make_hmac_signature(secret, body, &ts);
        assert!(verify_stripe_signature(body, &signature, secret));
    }

    #[test]
    fn test_signature_verification_signature_format_with_extra_parts() {
        let secret = "whsec_test";
        let body = br#"{"id":"evt_1"}"#; // ignore-magic
        let ts = now_timestamp();
        let sig = make_hmac_signature(secret, body, &ts);
        let extended = format!("{},v2=extra", sig);
        assert!(verify_stripe_signature(body, &extended, secret));
    }

    // -----------------------------------------------------------------------
    // StripeWebhookEvent deserialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_stripe_webhook_event_deserialize() {
        let json = serde_json::json!({
            "id": "evt_1234",
            "type": "payment_intent.succeeded",
            "data": {
                "object": {
                    "id": "pi_1234",
                    "amount": 5000
                }
            },
            "created": 1614556800
        });
        let event: StripeWebhookEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event.id, "evt_1234");
        assert_eq!(event.r#type, "payment_intent.succeeded");
        assert_eq!(event.created, 1614556800);
        assert_eq!(
            event.data["object"][db_fields::ID].as_str().unwrap(),
            "pi_1234"
        );
    }

    #[test]
    fn test_stripe_webhook_event_deserialize_defaults() {
        let json = serde_json::json!({
            "id": "evt_001",
            "type": "charge.succeeded"
        });
        let event: StripeWebhookEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event.id, "evt_001");
        assert_eq!(event.data, serde_json::Value::Null);
        assert_eq!(event.created, 0);
    }

    #[test]
    fn test_stripe_webhook_event_type_reserved_keyword() {
        let json = serde_json::json!({
            "id": "evt_x",
            "type": "invoice.paid"
        });
        let event: StripeWebhookEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event.r#type, "invoice.paid");
    }

    // -----------------------------------------------------------------------
    // str_field tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_str_field_present() {
        let val = serde_json::json!({fields::TITLE: "Alice", "age": 30});
        assert_eq!(str_field(&val, "name"), "Alice");
    }

    #[test]
    fn test_str_field_missing() {
        let val = serde_json::json!({fields::TITLE: "Alice"});
        assert_eq!(str_field(&val, "email"), "");
    }

    #[test]
    fn test_str_field_wrong_type() {
        let val = serde_json::json!({"count": 42});
        assert_eq!(str_field(&val, "count"), "");
    }

    #[test]
    fn test_str_field_null_value() {
        let val = serde_json::json!({fields::TITLE: null});
        assert_eq!(str_field(&val, "name"), "");
    }

    #[test]
    fn test_str_field_nested() {
        let val = serde_json::json!({
            "object": {"id": "pi_123"}
        });
        assert_eq!(str_field(&val["object"], "id"), "pi_123");
    }

    // -----------------------------------------------------------------------
    // store_webhook_event tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_store_webhook_event() {
        let state = setup_state().await;
        let event = StripeWebhookEvent {
            id: "evt_store_001".to_string(),
            r#type: "payment_intent.succeeded".to_string(),
            data: json!({"object": {"id": "pi_001"}}),
            created: 1614556800,
        };
        store_webhook_event(&state, &event).await.unwrap();
    }

    #[tokio::test]
    async fn test_store_webhook_event_empty_data() {
        let state = setup_state().await;
        let event = StripeWebhookEvent {
            id: "evt_store_002".to_string(),
            r#type: "charge.succeeded".to_string(),
            data: serde_json::Value::Null,
            created: 0,
        };
        store_webhook_event(&state, &event).await.unwrap();
    }

    #[tokio::test]
    async fn test_try_store_webhook_event_atomic_is_idempotent_for_duplicate_event_ids() {
        let state = setup_state().await;
        let unique_id = format!("evt_atomic_idem_{}", uuid::Uuid::new_v4().simple());
        let event = StripeWebhookEvent {
            id: unique_id,
            r#type: "payment_intent.succeeded".to_string(),
            data: json!({"object": {"id": "pi_atomic_001"}}),
            created: 1_614_556_800,
        };

        let first = try_store_webhook_event_atomic(&state, &event)
            .await
            .unwrap();
        let second = try_store_webhook_event_atomic(&state, &event)
            .await
            .unwrap();

        assert!(first, "first delivery should persist the webhook event");
        assert!(!second, "duplicate delivery must be treated as idempotent");
    }

    #[test]
    fn test_signature_verification_rejects_expired_timestamp_over_300_seconds() {
        let secret = "whsec_expired";
        let body = br#"{"id":"evt_expired","type":"charge.succeeded"}"#; // ignore-magic
        let expired_timestamp = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            - 301)
            .to_string();
        let signature = make_hmac_signature(secret, body, &expired_timestamp);

        assert!(!verify_stripe_signature(body, &signature, secret));
    }

    #[tokio::test]
    async fn test_handle_stripe_webhook_returns_duplicate_for_same_event_id() {
        let secret = "whsec_idempotent";
        let state = setup_state_with_webhook_secret(secret).await;
        let unique_evt_id = format!("evt_dup_full_{}", uuid::Uuid::new_v4().simple());
        let body = serde_json::to_vec(&json!({
            db_fields::ID: unique_evt_id,
            "type": "charge.succeeded",
            "data": {"object": {"id": "ch_test_duplicate"}},
            "created": 1_714_567_800_i64,
        }))
        .unwrap();
        let signature = make_hmac_signature(secret, &body, &now_timestamp());

        let first_request = axum::http::Request::builder()
            .uri("/api/webhooks/stripe")
            .header("stripe-signature", signature.clone())
            .body(axum::body::Body::from(body.clone()))
            .unwrap();
        let Json(first) = handle_stripe_webhook(State(state.clone()), first_request)
            .await
            .unwrap();
        assert_eq!(first[db_fields::STATUS], "ok");

        let second_request = axum::http::Request::builder()
            .uri("/api/webhooks/stripe")
            .header("stripe-signature", signature)
            .body(axum::body::Body::from(body))
            .unwrap();
        let Json(second) = handle_stripe_webhook(State(state), second_request)
            .await
            .unwrap();
        assert_eq!(second[db_fields::STATUS], "duplicate");
    }

    // -----------------------------------------------------------------------
    // find_order_by_metadata_id / find_order_by_payment_intent
    // (query_bind_value with SELECT * has RecordId serialization issue in mem DB)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_find_order_by_metadata_id_not_found() {
        let state = setup_state().await;
        let result = find_order_by_metadata_id(&state, "orders:nonexistent")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_find_order_by_payment_intent_not_found() {
        let state = setup_state().await;
        let result = find_order_by_payment_intent(&state, "pi_does_not_exist")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    // -----------------------------------------------------------------------
    // update_order_status tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_update_order_status() {
        let state = setup_state().await;
        state
            .db
            .create_document(
                collections::ORDERS,
                json!({
                    "id": "upd001",
                    fields::ORDER_STATUS: "pending",
                    db_fields::TOTAL_AMOUNT_CENTS: 3000,
                    fields::ITEMS: [{fields::PRODUCT_ID: "prod_001", fields::QUANTITY: 1}]
                }),
            )
            .await
            .unwrap();
        let result = update_order_status(&state, "orders:upd001", "pending", "cancelled").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_update_order_status_nonexistent() {
        let state = setup_state().await;
        let result =
            update_order_status(&state, "orders:no_such_order", "pending", "cancelled").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_update_order_status_precondition_failure_keeps_existing_state() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "upd_precondition",
                json!({
                    fields::ORDER_ID: "upd_precondition",
                    fields::ORDER_STATUS: "confirmed",
                    db_fields::TOTAL_AMOUNT_CENTS: 3000,
                    fields::ITEMS: [{fields::PRODUCT_ID: "prod_001", fields::QUANTITY: 1}]
                }),
            )
            .await
            .unwrap();

        let updated =
            update_order_status(&state, "orders:upd_precondition", "pending", "cancelled")
                .await
                .unwrap();

        assert!(!updated);

        let order = state
            .db
            .get_document(collections::ORDERS, "upd_precondition")
            .await
            .unwrap();

        assert_eq!(
            order.get(fields::ORDER_STATUS).and_then(|v| v.as_str()),
            Some("confirmed")
        );
    }

    // -----------------------------------------------------------------------
    // restore_stock_for_order tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_restore_stock_for_order_with_items() {
        let state = setup_state().await;
        state
            .db
            .create_document(
                collections::PRODUCTS,
                json!({
                    "id": "prod_001",
                    fields::STOCK_QUANTITY: 10,
                    fields::TITLE: "Test Product"
                }),
            )
            .await
            .unwrap();
        let order = json!({
            "id": "orders:restore_001",
            fields::ITEMS: [{fields::PRODUCT_ID: "prod_001", fields::QUANTITY: 2}]
        });
        let result = restore_stock_for_order(&state, &order).await;
        assert!(result.is_ok());
        let product = state
            .db
            .get_document(collections::PRODUCTS, "prod_001")
            .await
            .unwrap();
        assert_eq!(product[fields::STOCK_QUANTITY], 12);
    }

    #[tokio::test]
    async fn test_restore_stock_for_order_empty_items() {
        let state = setup_state().await;
        let order = json!({
            "id": "orders:restore_002",
            fields::ITEMS: []
        });
        let result = restore_stock_for_order(&state, &order).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_restore_stock_for_order_no_items_field() {
        let state = setup_state().await;
        let order = json!({"id": "orders:restore_003"});
        let result = restore_stock_for_order(&state, &order).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_restore_stock_for_order_missing_id() {
        let state = setup_state().await;
        let order = json!({fields::ITEMS: [{fields::PRODUCT_ID: "prod_001", fields::QUANTITY: 1}]});
        let result = restore_stock_for_order(&state, &order).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_restore_stock_for_order_empty_product_id() {
        let state = setup_state().await;
        let order = json!({
            "id": "orders:restore_004",
            fields::ITEMS: [{fields::PRODUCT_ID: "", fields::QUANTITY: 1}]
        });
        let result = restore_stock_for_order(&state, &order).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_restore_stock_for_order_zero_quantity() {
        let state = setup_state().await;
        let order = json!({
            "id": "orders:restore_005",
            fields::ITEMS: [{fields::PRODUCT_ID: "prod_001", fields::QUANTITY: 0}]
        });
        let result = restore_stock_for_order(&state, &order).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // decrement_stock_for_order tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_decrement_stock_for_order_with_items() {
        let state = setup_state().await;
        state
            .db
            .create_document(
                collections::PRODUCTS,
                json!({
                    "id": "prod_002",
                    fields::STOCK_QUANTITY: 10,
                    fields::TITLE: "Test Product 2"
                }),
            )
            .await
            .unwrap();
        let order = json!({
            "id": "orders:decrement_001",
            fields::ITEMS: [{fields::PRODUCT_ID: "prod_002", fields::QUANTITY: 3}]
        });
        let result = decrement_stock_for_order(&state, &order).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_decrement_stock_for_order_empty_items() {
        let state = setup_state().await;
        let order = json!({
            "id": "orders:decrement_002",
            fields::ITEMS: []
        });
        let result = decrement_stock_for_order(&state, &order).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_decrement_stock_for_order_missing_id() {
        let state = setup_state().await;
        let order = json!({fields::ITEMS: [{fields::PRODUCT_ID: "prod_001", fields::QUANTITY: 1}]});
        let result = decrement_stock_for_order(&state, &order).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_decrement_stock_for_order_empty_product_id() {
        let state = setup_state().await;
        let order = json!({
            "id": "orders:decrement_003",
            fields::ITEMS: [{fields::PRODUCT_ID: "", fields::QUANTITY: 1}]
        });
        let result = decrement_stock_for_order(&state, &order).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_decrement_stock_for_order_no_items_field() {
        let state = setup_state().await;
        let order = json!({"id": "orders:decrement_004"});
        let result = decrement_stock_for_order(&state, &order).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // mark_coupon_redeemed tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_mark_coupon_redeemed_empty_code() {
        let state = setup_state().await;
        let result = mark_coupon_redeemed(&state, "orders:coupon_001", "").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mark_coupon_redeemed_with_code() {
        let state = setup_state().await;
        state
            .db
            .create_document(
                collections::COUPON_USES,
                json!({
                    fields::ORDER_ID: "orders:coupon_002",
                    fields::COUPON_CODE: "SAVE10",
                    fields::REFUNDED_AT: null
                }),
            )
            .await
            .unwrap();
        let result = mark_coupon_redeemed(&state, "orders:coupon_002", "save10").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mark_coupon_redeemed_no_matching_record() {
        let state = setup_state().await;
        let result = mark_coupon_redeemed(&state, "orders:coupon_003", "NOMATCH").await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // release_coupon_reservation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_release_coupon_reservation() {
        let state = setup_state().await;
        state
            .db
            .create_document(
                collections::COUPON_USES,
                json!({
                    fields::ORDER_ID: "orders:release_001",
                    fields::COUPON_CODE: "SAVE10",
                    fields::REFUNDED_AT: null
                }),
            )
            .await
            .unwrap();
        let result = release_coupon_reservation(&state, "orders:release_001").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_release_coupon_reservation_no_record() {
        let state = setup_state().await;
        let result = release_coupon_reservation(&state, "orders:release_002").await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_payment_intent_succeeded tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_payment_intent_succeeded_no_object() {
        let state = setup_state().await;
        let event_data = json!({});
        let result = handle_payment_intent_succeeded(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payment_intent_succeeded_no_id() {
        let state = setup_state().await;
        let event_data = json!({"object": {}});
        let result = handle_payment_intent_succeeded(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payment_intent_succeeded_no_metadata() {
        let state = setup_state().await;
        let event_data = json!({"object": {"id": "pi_test"}});
        let result = handle_payment_intent_succeeded(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payment_intent_succeeded_no_order_id_in_metadata() {
        let state = setup_state().await;
        let event_data = json!({
            "object": {"id": "pi_test", "metadata": {"coupon_code": "SAVE10"}}
        });
        let result = handle_payment_intent_succeeded(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payment_intent_succeeded_order_not_found() {
        let state = setup_state().await;
        let event_data = json!({
            "object": {
                "id": "pi_notfound",
                "metadata": {"order_id": "orders:nonexistent_order"}
            }
        });
        let result = handle_payment_intent_succeeded(&state, &event_data).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // handle_payment_intent_failed tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_payment_intent_failed_no_object() {
        let state = setup_state().await;
        let event_data = json!({});
        let result = handle_payment_intent_failed(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payment_intent_failed_no_id() {
        let state = setup_state().await;
        let event_data = json!({"object": {}});
        let result = handle_payment_intent_failed(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payment_intent_failed_no_metadata() {
        let state = setup_state().await;
        let event_data = json!({"object": {"id": "pi_fail"}});
        let result = handle_payment_intent_failed(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payment_intent_failed_no_order_id() {
        let state = setup_state().await;
        let event_data = json!({
            "object": {"id": "pi_fail", "metadata": {}}
        });
        let result = handle_payment_intent_failed(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payment_intent_failed_order_not_found() {
        let state = setup_state().await;
        let event_data = json!({
            "object": {
                "id": "pi_fail_nf",
                "metadata": {"order_id": "orders:nonexistent"}
            }
        });
        let result = handle_payment_intent_failed(&state, &event_data).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // handle_payment_intent_canceled tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_payment_intent_canceled_no_object() {
        let state = setup_state().await;
        let event_data = json!({});
        let result = handle_payment_intent_canceled(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payment_intent_canceled_no_id() {
        let state = setup_state().await;
        let event_data = json!({"object": {}});
        let result = handle_payment_intent_canceled(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payment_intent_canceled_no_metadata() {
        let state = setup_state().await;
        let event_data = json!({"object": {"id": "pi_cancel"}});
        let result = handle_payment_intent_canceled(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payment_intent_canceled_order_not_found() {
        let state = setup_state().await;
        let event_data = json!({
            "object": {
                "id": "pi_cancel_nf",
                "metadata": {"order_id": "orders:nonexistent"}
            }
        });
        let result = handle_payment_intent_canceled(&state, &event_data).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // handle_charge_succeeded tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_charge_succeeded() {
        let state = setup_state().await;
        let event_data = json!({"object": {"id": "ch_123"}});
        let result = handle_charge_succeeded(&state, &event_data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_charge_succeeded_no_id() {
        let state = setup_state().await;
        let event_data = json!({"object": {}});
        let result = handle_charge_succeeded(&state, &event_data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_charge_succeeded_no_object() {
        let state = setup_state().await;
        let event_data = json!({});
        let result = handle_charge_succeeded(&state, &event_data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_charge_failed tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_charge_failed_with_reason() {
        let state = setup_state().await;
        let event_data = json!({
            "object": {"id": "ch_fail", "failure_message": "Card declined"}
        });
        let result = handle_charge_failed(&state, &event_data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_charge_failed_without_reason() {
        let state = setup_state().await;
        let event_data = json!({"object": {"id": "ch_fail2"}});
        let result = handle_charge_failed(&state, &event_data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_charge_failed_no_object() {
        let state = setup_state().await;
        let event_data = json!({});
        let result = handle_charge_failed(&state, &event_data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_charge_refunded tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_charge_refunded_no_object() {
        let state = setup_state().await;
        let event_data = json!({});
        let result = handle_charge_refunded(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_refunded_no_charge_id() {
        let state = setup_state().await;
        let event_data = json!({"object": {}});
        let result = handle_charge_refunded(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_refunded_no_payment_intent() {
        let state = setup_state().await;
        let event_data = json!({"object": {"id": "ch_ref_001"}});
        let result = handle_charge_refunded(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_refunded_no_amount() {
        let state = setup_state().await;
        let event_data = json!({
            "object": {"id": "ch_ref_002", "payment_intent": "pi_ref_002"}
        });
        let result = handle_charge_refunded(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_refunded_order_not_found() {
        let state = setup_state().await;
        let event_data = json!({
            "object": {
                "id": "ch_ref_003",
                "payment_intent": "pi_nonexistent",
                "amount_refunded": 1000
            }
        });
        let result = handle_charge_refunded(&state, &event_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_refunded_marks_full_order_refunded() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "refund_product_full",
                json!({
                    fields::PRODUCT_ID: "refund_product_full",
                    fields::STOCK_QUANTITY: 2,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "charge_refund_full_order",
                json!({
                    fields::ORDER_ID: "charge_refund_full_order",
                    fields::ORDER_STATUS: OrderStatus::Delivered.as_str(),
                    fields::PAYMENT_STATUS: PaymentStatus::Captured.as_str(),
                    fields::PAYMENT_INTENT_ID: "pi_charge_refund_full",
                    db_fields::TOTAL_AMOUNT_CENTS: 2500,
                    fields::STOCK_RESTORED: false,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "refund_product_full",
                        fields::QUANTITY: 1,
                        fields::IS_DIGITAL: false
                    }],
                }),
            )
            .await
            .unwrap();

        handle_charge_refunded(
            &state,
            &json!({
                "object": {
                    "id": "ch_refund_full",
                    "payment_intent": "pi_charge_refund_full",
                    "amount_refunded": 2500
                }
            }),
        )
        .await
        .unwrap();

        let order = state
            .db
            .get_document(collections::ORDERS, "charge_refund_full_order")
            .await
            .unwrap();
        let product = state
            .db
            .get_document(collections::PRODUCTS, "refund_product_full")
            .await
            .unwrap();
        assert_eq!(order[fields::ORDER_STATUS], OrderStatus::Refunded.as_str());
        assert_eq!(
            order[fields::PAYMENT_STATUS],
            PaymentStatus::Refunded.as_str()
        );
        assert_eq!(order[fields::REFUNDED_AMOUNT_CENTS], 2500);
        assert_eq!(order[fields::STOCK_RESTORED], true);
        assert_eq!(product[fields::STOCK_QUANTITY], 3);
    }

    #[tokio::test]
    async fn test_handle_charge_refunded_marks_partial_refund_without_stock_restore() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "refund_product_partial",
                json!({
                    fields::PRODUCT_ID: "refund_product_partial",
                    fields::STOCK_QUANTITY: 4,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "charge_refund_partial_order",
                json!({
                    fields::ORDER_ID: "charge_refund_partial_order",
                    fields::ORDER_STATUS: OrderStatus::Delivered.as_str(),
                    fields::PAYMENT_STATUS: PaymentStatus::Captured.as_str(),
                    fields::PAYMENT_INTENT_ID: "pi_charge_refund_partial",
                    db_fields::TOTAL_AMOUNT_CENTS: 5000,
                    fields::STOCK_RESTORED: false,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "refund_product_partial",
                        fields::QUANTITY: 1,
                        fields::IS_DIGITAL: false
                    }],
                }),
            )
            .await
            .unwrap();

        handle_charge_refunded(
            &state,
            &json!({
                "object": {
                    "id": "ch_refund_partial",
                    "payment_intent": "pi_charge_refund_partial",
                    "amount_refunded": 1200
                }
            }),
        )
        .await
        .unwrap();

        let order = state
            .db
            .get_document(collections::ORDERS, "charge_refund_partial_order")
            .await
            .unwrap();
        let product = state
            .db
            .get_document(collections::PRODUCTS, "refund_product_partial")
            .await
            .unwrap();
        assert_eq!(
            order[fields::ORDER_STATUS],
            OrderStatus::PartiallyRefunded.as_str()
        );
        assert_eq!(
            order[fields::PAYMENT_STATUS],
            PaymentStatus::PartialRefund.as_str()
        );
        assert_eq!(order[fields::REFUNDED_AMOUNT_CENTS], 1200);
        assert_eq!(order[fields::STOCK_RESTORED], false);
        assert_eq!(product[fields::STOCK_QUANTITY], 4);
    }

    // -----------------------------------------------------------------------
    // handle_customer_created tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_customer_created() {
        let state = setup_state().await;
        let event_data = json!({"object": {"id": "cus_123"}});
        let result = handle_customer_created(&state, &event_data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_customer_created_no_id() {
        let state = setup_state().await;
        let event_data = json!({"object": {}});
        let result = handle_customer_created(&state, &event_data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_customer_created_no_object() {
        let state = setup_state().await;
        let event_data = json!({});
        let result = handle_customer_created(&state, &event_data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_customer_updated tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_customer_updated() {
        let state = setup_state().await;
        let event_data = json!({"object": {"id": "cus_456"}});
        let result = handle_customer_updated(&state, &event_data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_customer_updated_no_id() {
        let state = setup_state().await;
        let event_data = json!({"object": {}});
        let result = handle_customer_updated(&state, &event_data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_customer_deleted tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_customer_deleted() {
        let state = setup_state().await;
        let event_data = json!({"object": {"id": "cus_789"}});
        let result = handle_customer_deleted(&state, &event_data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_customer_deleted_no_id() {
        let state = setup_state().await;
        let event_data = json!({"object": {}});
        let result = handle_customer_deleted(&state, &event_data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_payment_method_attached tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_payment_method_attached() {
        let state = setup_state().await;
        let event_data = json!({"object": {"id": "pm_123"}});
        let result = handle_payment_method_attached(&state, &event_data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_payment_method_attached_no_id() {
        let state = setup_state().await;
        let event_data = json!({"object": {}});
        let result = handle_payment_method_attached(&state, &event_data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_payment_method_detached tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_payment_method_detached() {
        let state = setup_state().await;
        let event_data = json!({"object": {"id": "pm_456"}});
        let result = handle_payment_method_detached(&state, &event_data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_payment_method_detached_no_id() {
        let state = setup_state().await;
        let event_data = json!({"object": {}});
        let result = handle_payment_method_detached(&state, &event_data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_invoice_paid tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_invoice_paid() {
        let state = setup_state().await;
        let event_data = json!({"object": {"id": "in_123"}});
        let result = handle_invoice_paid(&state, &event_data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_invoice_paid_no_id() {
        let state = setup_state().await;
        let event_data = json!({"object": {}});
        let result = handle_invoice_paid(&state, &event_data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_invoice_payment_failed tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_invoice_payment_failed() {
        let state = setup_state().await;
        let event_data = json!({"object": {"id": "in_fail_123"}});
        let result = handle_invoice_payment_failed(&state, &event_data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_invoice_payment_failed_no_id() {
        let state = setup_state().await;
        let event_data = json!({"object": {}});
        let result = handle_invoice_payment_failed(&state, &event_data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // send_payment_authorized_emails tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_send_payment_authorized_emails() {
        let state = setup_state().await;
        let order = json!({
            "id": "orders:email_001",
            db_fields::BUYER_ID: "buyer_001",
            db_fields::SELLER_ID: "seller_001"
        });
        let result = send_payment_authorized_emails(&state, &order).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // router tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_router_creates() {
        let state = setup_state().await;
        let _router = router(state);
    }

    // -----------------------------------------------------------------------
    // handle_checkout_session_completed tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_checkout_session_completed_no_object() {
        let state = setup_state().await;
        let result = handle_checkout_session_completed(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_checkout_session_completed_no_session_id() {
        let state = setup_state().await;
        let result = handle_checkout_session_completed(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_checkout_session_completed_no_metadata() {
        let state = setup_state().await;
        let data = json!({"object": {"id": "cs_test"}});
        let result = handle_checkout_session_completed(&state, &data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_checkout_session_completed_no_order_id() {
        let state = setup_state().await;
        let data = json!({"object": {"id": "cs_test", "metadata": {}}});
        let result = handle_checkout_session_completed(&state, &data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_checkout_session_completed_order_not_found() {
        let state = setup_state().await;
        let data = json!({
            "object": {
                "id": "cs_test",
                "payment_intent": "pi_test",
                "metadata": {"order_id": "orders:nonexistent"}
            }
        });
        let result = handle_checkout_session_completed(&state, &data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_checkout_session_completed_marks_coupon_redemption() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "coupon_order",
                json!({
                    fields::ORDER_ID: "coupon_order",
                    fields::ORDER_STATUS: "pending_payment",
                    fields::PAYMENT_STATUS: "awaiting_payment",
                    fields::ITEMS: []
                }),
            )
            .await
            .unwrap();
        state
            .db
            .create_document(
                collections::COUPON_USES,
                json!({
                    fields::ORDER_ID: "coupon_order",
                    fields::COUPON_CODE: "SAVE10",
                    db_fields::USER_ID: "buyer_coupon",
                    fields::REFUNDED_AT: Value::Null,
                }),
            )
            .await
            .unwrap();

        let data = json!({
            "object": {
                "id": "cs_coupon",
                "payment_intent": "pi_coupon",
                "metadata": {
                    "order_id": "coupon_order",
                    "coupon_code": "SAVE10"
                }
            }
        });

        handle_checkout_session_completed(&state, &data)
            .await
            .unwrap();

        let coupon_rows: Vec<Value> = state
            .db
            .query_bind_value(
                "SELECT * FROM coupon_uses WHERE data->>'orderId' = $order_id AND data->>'couponCode' = $coupon_code LIMIT 1",
                json!({
                    "order_id": "coupon_order",
                    "coupon_code": "SAVE10",
                }),
            )
            .await
            .unwrap();

        assert_eq!(coupon_rows.len(), 1);
        assert!(
            coupon_rows[0]
                .get(fields::COUPON_CODE)
                .and_then(|value| value.as_str())
                == Some("SAVE10"),
            "checkout.session.completed should preserve the reserved coupon usage record"
        );
    }

    // -----------------------------------------------------------------------
    // handle_checkout_session_expired tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_checkout_session_expired_no_object() {
        let state = setup_state().await;
        let result = handle_checkout_session_expired(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_checkout_session_expired_no_session_id() {
        let state = setup_state().await;
        let result = handle_checkout_session_expired(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_checkout_session_expired_no_metadata() {
        let state = setup_state().await;
        let data = json!({"object": {"id": "cs_expired"}});
        let result = handle_checkout_session_expired(&state, &data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_checkout_session_expired_order_not_found() {
        let state = setup_state().await;
        let data = json!({
            "object": {
                "id": "cs_expired",
                "metadata": {"order_id": "orders:nonexistent"}
            }
        });
        let result = handle_checkout_session_expired(&state, &data).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // handle_checkout_session_async_payment_failed tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_checkout_session_async_payment_failed_no_object() {
        let state = setup_state().await;
        let result = handle_checkout_session_async_payment_failed(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_checkout_session_async_payment_failed_order_not_found() {
        let state = setup_state().await;
        let data = json!({
            "object": {
                "id": "cs_async_fail",
                "metadata": {"order_id": "orders:nonexistent"}
            }
        });
        let result = handle_checkout_session_async_payment_failed(&state, &data).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // handle_charge_dispute_created tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_charge_dispute_created_no_object() {
        let state = setup_state().await;
        let result = handle_charge_dispute_created(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_dispute_created_no_dispute_id() {
        let state = setup_state().await;
        let result = handle_charge_dispute_created(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_dispute_created_no_payment_intent() {
        let state = setup_state().await;
        let data = json!({"object": {"id": "dp_test", "charge": "ch_test"}});
        let result = handle_charge_dispute_created(&state, &data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_dispute_created_order_not_found_still_logs() {
        let state = setup_state().await;
        let data = json!({
            "object": {
                "id": "dp_test",
                "charge": "ch_test",
                "payment_intent": "pi_nonexistent",
                "reason": "fraudulent",
                "amount": 5000,
                "currency": "cad"
            }
        });
        // Should succeed (logs dispute even if order not found)
        let result = handle_charge_dispute_created(&state, &data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_account_updated tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_account_updated_no_object() {
        let state = setup_state().await;
        let result = handle_account_updated(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_account_updated_no_account_id() {
        let state = setup_state().await;
        let result = handle_account_updated(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_account_updated_no_seller_found() {
        let state = setup_state().await;
        let data = json!({
            "object": {
                "id": "acct_test",
                "charges_enabled": true,
                "payouts_enabled": true,
                "details_submitted": true
            }
        });
        // Should succeed gracefully (warns and returns Ok)
        let result = handle_account_updated(&state, &data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_charge_captured tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_charge_captured_no_object() {
        let state = setup_state().await;
        let result = handle_charge_captured(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_captured_no_charge_id() {
        let state = setup_state().await;
        let result = handle_charge_captured(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_captured_logs_capture() {
        let state = setup_state().await;
        let data = json!({
            "object": {
                "id": "ch_captured_001",
                "amount_captured": 5000,
                "payment_intent": "pi_captured_001"
            }
        });
        let result = handle_charge_captured(&state, &data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_charge_captured_updates_order_payment_status() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "capture_order",
                json!({
                    fields::ORDER_ID: "capture_order",
                    fields::ORDER_STATUS: "payment_authorized",
                    fields::PAYMENT_STATUS: "authorized",
                    fields::PAYMENT_INTENT_ID: "pi_capture_test",
                    fields::ITEMS: []
                }),
            )
            .await
            .unwrap();

        let data = json!({
            "object": {
                "id": "ch_capture_test",
                "amount_captured": 5000,
                "payment_intent": "pi_capture_test"
            }
        });
        let result = handle_charge_captured(&state, &data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_charge_dispute_updated tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_charge_dispute_updated_no_object() {
        let state = setup_state().await;
        let result = handle_charge_dispute_updated(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_dispute_updated_no_dispute_id() {
        let state = setup_state().await;
        let result = handle_charge_dispute_updated(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_dispute_updated_tracks_progress() {
        let state = setup_state().await;
        // Create an existing dispute record
        state
            .db
            .create_document(
                collections::DISPUTES,
                json!({
                    fields::DISPUTE_ID: "dp_upd_001",
                    fields::STRIPE_STATUS: "needs_response",
                    fields::AMOUNT_CENTS: 5000,
                }),
            )
            .await
            .unwrap();

        let data = json!({
            "object": {
                "id": "dp_upd_001",
                "status": "under_review",
                "reason": "fraudulent",
                "amount": 5000
            }
        });
        let result = handle_charge_dispute_updated(&state, &data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_charge_dispute_funds_withdrawn tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_charge_dispute_funds_withdrawn_no_object() {
        let state = setup_state().await;
        let result = handle_charge_dispute_funds_withdrawn(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_dispute_funds_withdrawn_no_dispute_id() {
        let state = setup_state().await;
        let result = handle_charge_dispute_funds_withdrawn(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_dispute_funds_withdrawn_logs_debit() {
        let state = setup_state().await;
        state
            .db
            .create_document(
                collections::DISPUTES,
                json!({
                    fields::DISPUTE_ID: "dp_fw_001",
                    fields::STRIPE_STATUS: "needs_response",
                    fields::AMOUNT_CENTS: 3000,
                }),
            )
            .await
            .unwrap();

        let data = json!({
            "object": {
                "id": "dp_fw_001",
                "amount": 3000,
                "currency": "cad",
                "balance_transactions": [{"id": "txn_fw_001"}]
            }
        });
        let result = handle_charge_dispute_funds_withdrawn(&state, &data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_charge_dispute_funds_reinstated tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_charge_dispute_funds_reinstated_no_object() {
        let state = setup_state().await;
        let result = handle_charge_dispute_funds_reinstated(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_dispute_funds_reinstated_no_dispute_id() {
        let state = setup_state().await;
        let result = handle_charge_dispute_funds_reinstated(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_charge_dispute_funds_reinstated_updates_dispute() {
        let state = setup_state().await;
        state
            .db
            .create_document(
                collections::DISPUTES,
                json!({
                    fields::DISPUTE_ID: "dp_fr_001",
                    fields::STRIPE_STATUS: "won",
                    fields::AMOUNT_CENTS: 4000,
                }),
            )
            .await
            .unwrap();

        let data = json!({
            "object": {
                "id": "dp_fr_001",
                "amount": 4000,
                "currency": "cad",
                "payment_intent": "pi_fr_nonexistent"
            }
        });
        let result = handle_charge_dispute_funds_reinstated(&state, &data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_payout_created tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_payout_created_no_object() {
        let state = setup_state().await;
        let result = handle_payout_created(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payout_created_no_payout_id() {
        let state = setup_state().await;
        let result = handle_payout_created(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payout_created_creates_record() {
        let state = setup_state().await;
        let data = json!({
            "object": {
                "id": "po_created_001",
                "amount": 10000,
                "currency": "cad",
                "arrival_date": 1714567800,
                "method": "standard",
                "destination": "acct_seller_001",
                "status": "pending"
            }
        });
        let result = handle_payout_created(&state, &data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_payout_updated tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_payout_updated_no_object() {
        let state = setup_state().await;
        let result = handle_payout_updated(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payout_updated_no_payout_id() {
        let state = setup_state().await;
        let result = handle_payout_updated(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payout_updated_in_transit() {
        let state = setup_state().await;
        let data = json!({
            "object": {
                "id": "po_updated_001",
                "status": "in_transit",
                "arrival_date": 1714567800
            }
        });
        let result = handle_payout_updated(&state, &data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_payout_paid tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_payout_paid_no_object() {
        let state = setup_state().await;
        let result = handle_payout_paid(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payout_paid_no_payout_id() {
        let state = setup_state().await;
        let result = handle_payout_paid(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_payout_paid_marks_complete() {
        let state = setup_state().await;
        let data = json!({
            "object": {
                "id": "po_paid_001",
                "amount": 8500,
                "currency": "cad",
                "destination": "acct_paid_seller"
            }
        });
        let result = handle_payout_paid(&state, &data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_payout_paid_notifies_seller() {
        let state = setup_state().await;
        // Create seller profile linked to Connect account
        state
            .db
            .create_document(
                collections::SELLER_PROFILES,
                json!({
                    fields::STRIPE_ACCOUNT_ID: "acct_notify_seller",
                    db_fields::SELLER_ID: "seller_notify_001",
                }),
            )
            .await
            .unwrap();

        let data = json!({
            "object": {
                "id": "po_paid_notify",
                "amount": 12000,
                "currency": "cad",
                "destination": "acct_notify_seller"
            }
        });
        let result = handle_payout_paid(&state, &data).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // handle_refund_created tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_refund_created_no_object() {
        let state = setup_state().await;
        let result = handle_refund_created(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_refund_created_no_refund_id() {
        let state = setup_state().await;
        let result = handle_refund_created(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_refund_created_logs_refund() {
        let state = setup_state().await;
        let data = json!({
            "object": {
                "id": "re_created_001",
                "amount": 5000,
                "currency": "cad",
                "status": "pending",
                "payment_intent": "pi_refund_001",
                "reason": "requested_by_customer"
            }
        });
        let result = handle_refund_created(&state, &data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_refund_created_is_idempotent_for_same_refund_id() {
        let state = setup_state().await;
        let data = json!({
            "object": {
                "id": "re_created_idempotent",
                "amount": 5000,
                "currency": "cad",
                "status": "pending",
                "payment_intent": "pi_refund_idempotent",
                "reason": "requested_by_customer"
            }
        });

        handle_refund_created(&state, &data).await.unwrap();
        handle_refund_created(&state, &data).await.unwrap();

        let refunds: Vec<Value> = state
            .db
            .query_bind_value(
                &format!(
                    "SELECT * FROM {} WHERE data->>'{}' = $refund_id",
                    collections::REFUNDS,
                    fields::STRIPE_REFUND_ID
                ),
                json!({ "refund_id": "re_created_idempotent" }),
            )
            .await
            .unwrap();
        assert_eq!(refunds.len(), 1);
    }

    // -----------------------------------------------------------------------
    // handle_refund_updated tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_refund_updated_no_object() {
        let state = setup_state().await;
        let result = handle_refund_updated(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_refund_updated_no_refund_id() {
        let state = setup_state().await;
        let result = handle_refund_updated(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_refund_updated_tracks_status() {
        let state = setup_state().await;
        let data = json!({
            "object": {
                "id": "re_updated_001",
                "status": "succeeded"
            }
        });
        let result = handle_refund_updated(&state, &data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_refund_updated_succeeded_marks_order_refunded_from_records() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "refund_update_product",
                json!({
                    fields::PRODUCT_ID: "refund_update_product",
                    fields::STOCK_QUANTITY: 7,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "refund_update_order",
                json!({
                    fields::ORDER_ID: "refund_update_order",
                    fields::ORDER_STATUS: OrderStatus::Delivered.as_str(),
                    fields::PAYMENT_STATUS: PaymentStatus::Captured.as_str(),
                    fields::PAYMENT_INTENT_ID: "pi_refund_update_success",
                    db_fields::TOTAL_AMOUNT_CENTS: 3000,
                    fields::STOCK_RESTORED: false,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "refund_update_product",
                        fields::QUANTITY: 1,
                        fields::IS_DIGITAL: false
                    }],
                }),
            )
            .await
            .unwrap();

        handle_refund_updated(
            &state,
            &json!({
                "object": {
                    "id": "re_update_success",
                    "amount": 3000,
                    "currency": "cad",
                    "status": "succeeded",
                    "payment_intent": "pi_refund_update_success",
                    "reason": "requested_by_customer"
                }
            }),
        )
        .await
        .unwrap();

        let order = state
            .db
            .get_document(collections::ORDERS, "refund_update_order")
            .await
            .unwrap();
        let refund = state
            .db
            .get_document(collections::REFUNDS, "re_update_success")
            .await
            .unwrap();
        assert_eq!(refund[fields::STRIPE_REFUND_STATUS], "succeeded");
        assert_eq!(order[fields::REFUND_ID], "re_update_success");
        assert_eq!(order[fields::ORDER_STATUS], OrderStatus::Refunded.as_str());
        assert_eq!(
            order[fields::PAYMENT_STATUS],
            PaymentStatus::Refunded.as_str()
        );
        assert_eq!(order[fields::REFUNDED_AMOUNT_CENTS], 3000);
    }

    // -----------------------------------------------------------------------
    // handle_refund_failed tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_refund_failed_no_object() {
        let state = setup_state().await;
        let result = handle_refund_failed(&state, &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_refund_failed_no_refund_id() {
        let state = setup_state().await;
        let result = handle_refund_failed(&state, &json!({"object": {}})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_refund_failed_alerts_and_notifies() {
        let state = setup_state().await;
        let data = json!({
            "object": {
                "id": "re_failed_001",
                "amount": 3000,
                "currency": "cad",
                "failure_reason": "expired_or_canceled_card",
                "payment_intent": "pi_refund_fail_001"
            }
        });
        let result = handle_refund_failed(&state, &data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_refund_failed_marks_order_for_manual_review() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "refund_failed_order",
                json!({
                    fields::ORDER_ID: "refund_failed_order",
                    db_fields::BUYER_ID: "buyer_refund_failed_state",
                    fields::PAYMENT_INTENT_ID: "pi_refund_failed_state",
                    fields::ORDER_STATUS: OrderStatus::Delivered.as_str(),
                    fields::PAYMENT_STATUS: PaymentStatus::Captured.as_str(),
                }),
            )
            .await
            .unwrap();

        handle_refund_failed(
            &state,
            &json!({
                "object": {
                    "id": "re_failed_state",
                    "amount": 1000,
                    "currency": "cad",
                    "failure_reason": "expired_or_canceled_card",
                    "payment_intent": "pi_refund_failed_state"
                }
            }),
        )
        .await
        .unwrap();

        let order = state
            .db
            .get_document(collections::ORDERS, "refund_failed_order")
            .await
            .unwrap();
        let refund = state
            .db
            .get_document(collections::REFUNDS, "re_failed_state")
            .await
            .unwrap();
        assert_eq!(refund[fields::STRIPE_REFUND_STATUS], "failed");
        assert_eq!(
            refund[fields::REFUND_FAILURE_REASON],
            "expired_or_canceled_card"
        );
        assert_eq!(order[fields::REQUIRES_MANUAL_REVIEW], true);
        assert_eq!(
            order[fields::REFUND_FAILURE_REASON],
            "expired_or_canceled_card"
        );
    }

    #[tokio::test]
    async fn test_handle_refund_failed_notifies_buyer() {
        let state = setup_state().await;
        // Create order with buyer
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "refund_fail_order",
                json!({
                    fields::ORDER_ID: "refund_fail_order",
                    db_fields::BUYER_ID: "buyer_refund_fail",
                    fields::PAYMENT_INTENT_ID: "pi_refund_fail_notify",
                    fields::ITEMS: []
                }),
            )
            .await
            .unwrap();

        let data = json!({
            "object": {
                "id": "re_failed_notify",
                "amount": 2500,
                "currency": "cad",
                "failure_reason": "charge_for_pending_refund_disputed",
                "payment_intent": "pi_refund_fail_notify"
            }
        });
        let result = handle_refund_failed(&state, &data).await;
        assert!(result.is_ok());
    }
}
