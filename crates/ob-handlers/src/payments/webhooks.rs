//! Stripe webhook handler.
//! Validates Stripe webhook signatures and routes events to appropriate handlers.

use axum::{Json, Router, extract::State, extract::Request, routing::post};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::HashMap;
use tracing::{error, info, warn};

use crate::HandlersState;
use crate::shared::schema::collections;

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
                return Err(ob_core::Error::Unauthorized(
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

async fn is_duplicate_webhook(state: &HandlersState, event_id: &str) -> Result<bool, ob_core::Error> {
    // Validate event ID format before querying
    ob_core::validate_surreal_record_id(event_id)?;
    
    let result = state
        .db
        .query_bind_value(
            "SELECT * FROM webhook_events WHERE id = $event_id LIMIT 1",
            serde_json::json!({"event_id": event_id})
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
            &event.id,
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
// Event Handlers (minimal logging implementations)
// ---------------------------------------------------------------------------

async fn handle_payment_intent_succeeded(
    _state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    if let Some(pi_id) = event_data
        .get("object")
        .and_then(|o| o.get("id"))
        .and_then(|i| i.as_str())
    {
        info!(payment_intent_id = %pi_id, "Payment intent succeeded");
    }
    Ok(())
}

async fn handle_payment_intent_failed(
    _state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    if let Some(pi_id) = event_data
        .get("object")
        .and_then(|o| o.get("id"))
        .and_then(|i| i.as_str())
    {
        warn!(payment_intent_id = %pi_id, "Payment intent failed");
    }
    Ok(())
}

async fn handle_payment_intent_canceled(
    _state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    if let Some(pi_id) = event_data
        .get("object")
        .and_then(|o| o.get("id"))
        .and_then(|i| i.as_str())
    {
        info!(payment_intent_id = %pi_id, "Payment intent canceled");
    }
    Ok(())
}

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

async fn handle_charge_refunded(
    _state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    if let Some(charge_id) = event_data
        .get("object")
        .and_then(|o| o.get("id"))
        .and_then(|i| i.as_str())
    {
        info!(charge_id = %charge_id, "Charge refunded");
    }
    Ok(())
}

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
