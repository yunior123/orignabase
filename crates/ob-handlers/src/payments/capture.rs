//! Payment capture handler.
//! Ported from: functions/handlers/payment_stripe.py::capture_payment

use axum::{Extension, Json, Router, extract::State, routing::post};
use ob_auth::middleware::AuthContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{error, info, warn};

use crate::HandlersState;
use crate::shared::auth::resolve_self_user_id;
use crate::shared::schema::{OrderStatus, PaymentStatus, collections, fields};
use crate::shared::validation::validate_uid;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturePaymentRequest {
    pub order_id: String,
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturePaymentResponse {
    pub success: bool,
    pub order_id: String,
    pub payment_status: String,
    pub order_status: String,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/payments/capture", post(capture_payment))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

async fn capture_payment(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<CapturePaymentRequest>,
) -> Result<Json<CapturePaymentResponse>, ob_core::Error> {
    // --- Input validation ---
    validate_uid("orderId", &req.order_id)?;
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &user_id,
        "capture_payment",
        5,
        1,
    )
    .await?;

    // --- Fetch the order ---
    let order = state
        .db
        .get_document(collections::ORDERS, &req.order_id)
        .await
        .map_err(|_| ob_core::Error::NotFound(format!("Order {} not found", req.order_id)))?;

    // --- Verify the caller is the seller for this order ---
    // Determine seller from items[0].sellerId (multi-seller orders route to first item seller)
    let seller_id = order
        .get(fields::ITEMS)
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|item| item.get(fields::SELLER_ID))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if seller_id.is_empty() || seller_id != user_id {
        warn!(
            order_id = %req.order_id,
            caller = %user_id,
            seller = %seller_id,
            "Capture denied: caller is not the order seller"
        );
        return Err(ob_core::Error::Forbidden(
            "Only the seller can capture payment for this order".into(),
        ));
    }

    // --- Verify order status allows capture ---
    let current_status = order
        .get(fields::STATUS)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if current_status != OrderStatus::PaymentAuthorized.as_str()
        && current_status != OrderStatus::AwaitingShippingApproval.as_str()
    {
        return Err(ob_core::Error::Validation(format!(
            "Cannot capture payment for order in status '{current_status}'. Expected PAYMENT_AUTHORIZED or AWAITING_SHIPPING_APPROVAL"
        )));
    }

    let current_payment = order
        .get(fields::PAYMENT_STATUS)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if current_payment != "AUTHORIZED" && current_payment != "PENDING" {
        return Err(ob_core::Error::Validation(format!(
            "Cannot capture payment with payment status '{current_payment}'"
        )));
    }

    // --- Retrieve PaymentIntent ID ---
    let payment_intent_id = order
        .get(fields::PAYMENT_INTENT_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // If no payment_intent_id, try fetching via checkout session
    let pi_id = if payment_intent_id.is_empty() {
        let session_id = order
            .get(fields::CHECKOUT_SESSION_ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if session_id.is_empty() {
            return Err(ob_core::Error::Internal(
                "Order has no payment intent or checkout session".into(),
            ));
        }
        // Retrieve the session from Stripe to get the PI
        let stripe_key = state.config.require_secret("stripe_secret_key")?;
        let url = format!("{}/checkout/sessions/{session_id}", state.stripe_base_url);
        let resp = state
            .http_client
            .get(&url)
            .basic_auth(stripe_key, None::<&str>)
            .send()
            .await
            .map_err(|e| ob_core::Error::Internal(format!("Stripe API error: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            error!(error = %body, "Failed to fetch checkout session from Stripe");
            return Err(ob_core::Error::Internal(
                "Failed to fetch checkout session".into(),
            ));
        }

        let session: Value = resp
            .json()
            .await
            .map_err(|e| ob_core::Error::Internal(format!("Parse error: {e}")))?;
        let pi = session["payment_intent"].as_str().unwrap_or("").to_string();
        if pi.is_empty() {
            return Err(ob_core::Error::Internal(
                "Checkout session has no payment intent".into(),
            ));
        }
        pi
    } else {
        payment_intent_id.to_string()
    };

    // --- Capture the PaymentIntent via Stripe ---
    let stripe_key = state.config.require_secret("stripe_secret_key")?;
    let capture_url = format!("{}/payment_intents/{pi_id}/capture", state.stripe_base_url);
    // Generate idempotency key for this capture
    let idempotency_key = format!("{}_capture", req.order_id);
    let capture_resp = state
        .http_client
        .post(&capture_url)
        .basic_auth(stripe_key, None::<&str>)
        .header("Idempotency-Key", &idempotency_key)
        .send()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Stripe capture error: {e}")))?;

    if !capture_resp.status().is_success() {
        let body = capture_resp.text().await.unwrap_or_default();
        error!(
            order_id = %req.order_id,
            pi_id = %pi_id,
            error = %body,
            "Stripe capture failed"
        );
        return Err(ob_core::Error::Internal(
            "Failed to capture payment with Stripe".into(),
        ));
    }

    let capture_result: Value = capture_resp
        .json()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Parse error: {e}")))?;

    let stripe_status = capture_result["status"].as_str().unwrap_or("unknown");

    if stripe_status != "succeeded" {
        error!(
            order_id = %req.order_id,
            stripe_status = %stripe_status,
            "Capture did not succeed"
        );
        return Err(ob_core::Error::Internal(format!(
            "Payment capture returned status: {stripe_status}"
        )));
    }

    // --- Update order in DB ---
    let now = chrono::Utc::now().to_rfc3339();
    let update_data = serde_json::json!({
        fields::STATUS: OrderStatus::Processing.as_str(),
        fields::PAYMENT_STATUS: PaymentStatus::Captured.as_str(),
        fields::PAYMENT_INTENT_ID: pi_id,
        fields::UPDATED_AT: now,
    });

    state
        .db
        .update_document(collections::ORDERS, &req.order_id, update_data)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to update order: {e}")))?;

    info!(
        order_id = %req.order_id,
        seller_id = %user_id,
        pi_id = %pi_id,
        "Payment captured successfully"
    );

    Ok(Json(CapturePaymentResponse {
        success: true,
        order_id: req.order_id,
        payment_status: PaymentStatus::Captured.as_str().to_string(),
        order_status: OrderStatus::Processing.as_str().to_string(),
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    fn auth(uid: &str) -> AuthContext {
        AuthContext {
            user_id: uid.to_string(),
            roles: vec![],
            authenticated: true,
            email_verified: true,
            custom_claims: serde_json::Value::Null,
        }
    }

    use super::*;
    use axum::extract::State;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use serde_json::json;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn setup_state() -> HandlersState {
        let db = DatabaseClient::new_mem().await;
        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_secret_key".to_string(), "sk_test_123".to_string());

        HandlersState {
            config: Arc::new(config),
            db,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
        }
    }

    #[test]
    fn test_capture_request_deser() {
        let json = r#"{"orderId": "order-abc-123", "userId": "seller-xyz"}"#;
        let req: CapturePaymentRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.order_id, "order-abc-123");
        assert_eq!(req.user_id, "seller-xyz");
    }

    #[test]
    fn test_capture_response_ser() {
        let resp = CapturePaymentResponse {
            success: true,
            order_id: "order-1".to_string(),
            payment_status: "CAPTURED".to_string(),
            order_status: "PROCESSING".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["orderId"], "order-1");
        assert_eq!(json["paymentStatus"], "CAPTURED");
        assert_eq!(json["orderStatus"], "PROCESSING");
    }

    #[test]
    fn test_capture_request_missing_fields() {
        let json = r#"{"orderId": "order-1"}"#;
        let result: Result<CapturePaymentRequest, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "Missing userId should fail deserialization"
        );
    }

    #[test]
    fn test_payment_status_captured_str() {
        assert_eq!(PaymentStatus::Captured.as_str(), "CAPTURED");
    }

    #[test]
    fn test_order_status_processing_str() {
        assert_eq!(OrderStatus::Processing.as_str(), "PROCESSING");
    }

    // --- Ported from Python test_handlers_payment_stripe.py capture scenarios ---

    #[test]
    fn test_capture_request_empty_order_id() {
        let json = r#"{"orderId": "", "userId": "seller-1"}"#;
        let req: CapturePaymentRequest = serde_json::from_str(json).unwrap();
        assert!(req.order_id.is_empty());
    }

    #[test]
    fn test_capture_request_missing_order_id_fails() {
        let json = r#"{"userId": "seller-1"}"#;
        let result: std::result::Result<CapturePaymentRequest, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_capture_request_missing_user_id_fails() {
        let json = r#"{"orderId": "order-1"}"#;
        let result: std::result::Result<CapturePaymentRequest, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_capture_response_failure_serialization() {
        let resp = CapturePaymentResponse {
            success: false,
            order_id: "order-fail".to_string(),
            payment_status: "FAILED".to_string(),
            order_status: "FAILED".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["success"], false);
        assert_eq!(json["paymentStatus"], "FAILED");
    }

    #[test]
    fn test_order_status_payment_authorized_str() {
        assert_eq!(
            OrderStatus::PaymentAuthorized.as_str(),
            "PAYMENT_AUTHORIZED"
        );
    }

    #[test]
    fn test_order_status_awaiting_shipping_approval_str() {
        assert_eq!(
            OrderStatus::AwaitingShippingApproval.as_str(),
            "AWAITING_SHIPPING_APPROVAL"
        );
    }

    #[test]
    fn test_payment_status_authorized_str() {
        assert_eq!(PaymentStatus::Authorized.as_str(), "AUTHORIZED");
    }

    #[test]
    fn test_payment_status_failed_str() {
        assert_eq!(PaymentStatus::Failed.as_str(), "FAILED");
    }

    #[tokio::test]
    async fn test_capture_payment_rejects_non_seller_and_invalid_status() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_1",
                json!({
                    fields::ORDER_ID: "order_1",
                    fields::STATUS: OrderStatus::PendingPayment.as_str(),
                    fields::PAYMENT_STATUS: PaymentStatus::Authorized.as_str(),
                    fields::ITEMS: [{
                        fields::SELLER_ID: "seller_1"
                    }],
                    fields::PAYMENT_INTENT_ID: "pi_1",
                }),
            )
            .await
            .unwrap();

        let forbidden = capture_payment(
            State(state.clone()),
            Extension(auth("test")),
            Json(CapturePaymentRequest {
                order_id: "order_1".into(),
                user_id: Some("seller_2".to_string()),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            forbidden
                .to_string()
                .contains("Only the seller can capture payment")
        );

        let invalid_status = capture_payment(
            State(state),
            Extension(auth("test")),
            Json(CapturePaymentRequest {
                order_id: "order_1".into(),
                user_id: Some("seller_1".to_string()),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            invalid_status
                .to_string()
                .contains("Cannot capture payment for order in status")
        );
    }

    #[tokio::test]
    async fn test_capture_payment_rejects_invalid_payment_state_and_missing_payment_refs() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_1",
                json!({
                    fields::ORDER_ID: "order_1",
                    fields::STATUS: OrderStatus::PaymentAuthorized.as_str(),
                    fields::PAYMENT_STATUS: PaymentStatus::Captured.as_str(),
                    fields::ITEMS: [{
                        fields::SELLER_ID: "seller_1"
                    }],
                }),
            )
            .await
            .unwrap();

        let invalid_payment = capture_payment(
            State(state.clone()),
            Extension(auth("test")),
            Json(CapturePaymentRequest {
                order_id: "order_1".into(),
                user_id: Some("seller_1".to_string()),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            invalid_payment
                .to_string()
                .contains("Cannot capture payment with payment status")
        );

        state
            .db
            .update_document(
                collections::ORDERS,
                "order_1",
                json!({
                    fields::PAYMENT_STATUS: PaymentStatus::Authorized.as_str(),
                    fields::PAYMENT_INTENT_ID: serde_json::Value::Null,
                    fields::CHECKOUT_SESSION_ID: serde_json::Value::Null,
                }),
            )
            .await
            .unwrap();

        let missing_refs = capture_payment(
            State(state),
            Extension(auth("test")),
            Json(CapturePaymentRequest {
                order_id: "order_1".into(),
                user_id: Some("seller_1".to_string()),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            missing_refs
                .to_string()
                .contains("Order has no payment intent or checkout session")
        );
    }

    #[tokio::test]
    async fn test_capture_payment_success_uses_existing_payment_intent() {
        let state = setup_state().await;
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/payment_intents/pi_capture/capture"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "pi_capture",
                "status": "succeeded"
            })))
            .mount(&mock_server)
            .await;

        let state = HandlersState {
            stripe_base_url: mock_server.uri(),
            turnstile_secret_key: None,
            ..state
        };

        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_1",
                json!({
                    fields::ORDER_ID: "order_1",
                    fields::STATUS: OrderStatus::PaymentAuthorized.as_str(),
                    fields::PAYMENT_STATUS: PaymentStatus::Authorized.as_str(),
                    fields::PAYMENT_INTENT_ID: "pi_capture",
                    fields::ITEMS: [{
                        fields::SELLER_ID: "seller_1"
                    }],
                }),
            )
            .await
            .unwrap();

        let Json(resp) = capture_payment(
            State(state.clone()),
            Extension(auth("test")),
            Json(CapturePaymentRequest {
                order_id: "order_1".into(),
                user_id: Some("seller_1".to_string()),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.payment_status, PaymentStatus::Captured.as_str());
        assert_eq!(resp.order_status, OrderStatus::Processing.as_str());

        let order = state
            .db
            .get_document(collections::ORDERS, "order_1")
            .await
            .unwrap();
        assert_eq!(
            order[fields::PAYMENT_STATUS],
            PaymentStatus::Captured.as_str()
        );
        assert_eq!(order[fields::STATUS], OrderStatus::Processing.as_str());
    }

    #[tokio::test]
    async fn test_capture_payment_fetches_checkout_session_when_pi_missing() {
        let state = setup_state().await;
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/checkout/sessions/cs_123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cs_123",
                "payment_intent": "pi_from_session"
            })))
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/payment_intents/pi_from_session/capture"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "pi_from_session",
                "status": "succeeded"
            })))
            .mount(&mock_server)
            .await;

        let state = HandlersState {
            stripe_base_url: mock_server.uri(),
            turnstile_secret_key: None,
            ..state
        };

        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_1",
                json!({
                    fields::ORDER_ID: "order_1",
                    fields::STATUS: OrderStatus::AwaitingShippingApproval.as_str(),
                    fields::PAYMENT_STATUS: PaymentStatus::Pending.as_str(),
                    fields::CHECKOUT_SESSION_ID: "cs_123",
                    fields::ITEMS: [{
                        fields::SELLER_ID: "seller_1"
                    }],
                }),
            )
            .await
            .unwrap();

        let Json(resp) = capture_payment(
            State(state.clone()),
            Extension(auth("test")),
            Json(CapturePaymentRequest {
                order_id: "order_1".into(),
                user_id: Some("seller_1".to_string()),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.payment_status, PaymentStatus::Captured.as_str());

        let order = state
            .db
            .get_document(collections::ORDERS, "order_1")
            .await
            .unwrap();
        assert_eq!(order[fields::PAYMENT_INTENT_ID], "pi_from_session");
        assert_eq!(
            order[fields::PAYMENT_STATUS],
            PaymentStatus::Captured.as_str()
        );
    }

    #[tokio::test]
    async fn test_capture_payment_handles_checkout_session_and_capture_failures() {
        let state = setup_state().await;
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/checkout/sessions/cs_missing_pi"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cs_missing_pi"
            })))
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/payment_intents/pi_bad/capture"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "pi_bad",
                "status": "requires_capture"
            })))
            .mount(&mock_server)
            .await;

        let state = HandlersState {
            stripe_base_url: mock_server.uri(),
            turnstile_secret_key: None,
            ..state
        };

        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_missing_pi",
                json!({
                    fields::ORDER_ID: "order_missing_pi",
                    fields::STATUS: OrderStatus::PaymentAuthorized.as_str(),
                    fields::PAYMENT_STATUS: PaymentStatus::Authorized.as_str(),
                    fields::CHECKOUT_SESSION_ID: "cs_missing_pi",
                    fields::ITEMS: [{
                        fields::SELLER_ID: "seller_1"
                    }],
                }),
            )
            .await
            .unwrap();

        let missing_pi = capture_payment(
            State(state.clone()),
            Extension(auth("test")),
            Json(CapturePaymentRequest {
                order_id: "order_missing_pi".into(),
                user_id: Some("seller_1".to_string()),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            missing_pi
                .to_string()
                .contains("Checkout session has no payment intent")
        );

        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_bad_capture",
                json!({
                    fields::ORDER_ID: "order_bad_capture",
                    fields::STATUS: OrderStatus::PaymentAuthorized.as_str(),
                    fields::PAYMENT_STATUS: PaymentStatus::Authorized.as_str(),
                    fields::PAYMENT_INTENT_ID: "pi_bad",
                    fields::ITEMS: [{
                        fields::SELLER_ID: "seller_1"
                    }],
                }),
            )
            .await
            .unwrap();

        let bad_capture = capture_payment(
            State(state),
            Extension(auth("test")),
            Json(CapturePaymentRequest {
                order_id: "order_bad_capture".into(),
                user_id: Some("seller_1".to_string()),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            bad_capture
                .to_string()
                .contains("Payment capture returned status")
        );
    }

    // --- Coverage: Stripe checkout session fetch returns HTTP error (lines 146-150) ---

    #[tokio::test]
    async fn test_capture_payment_checkout_session_fetch_fails() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/checkout/sessions/cs_fail"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Server Error"))
            .mount(&mock_server)
            .await;

        let state = setup_state().await;
        let state = HandlersState {
            stripe_base_url: mock_server.uri(),
            turnstile_secret_key: None,
            ..state
        };

        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_cs_fail",
                json!({
                    fields::ORDER_ID: "order_cs_fail",
                    fields::STATUS: OrderStatus::PaymentAuthorized.as_str(),
                    fields::PAYMENT_STATUS: PaymentStatus::Authorized.as_str(),
                    fields::CHECKOUT_SESSION_ID: "cs_fail",
                    fields::ITEMS: [{ fields::SELLER_ID: "seller_1" }],
                }),
            )
            .await
            .unwrap();

        let err = capture_payment(
            State(state),
            Extension(auth("test")),
            Json(CapturePaymentRequest {
                order_id: "order_cs_fail".into(),
                user_id: Some("seller_1".to_string()),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Failed to fetch checkout session"));
    }

    // --- Coverage: Stripe capture HTTP error (lines 180-181, 187-189) ---

    #[tokio::test]
    async fn test_capture_payment_stripe_capture_http_error() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/payment_intents/pi_http_err/capture"))
            .respond_with(ResponseTemplate::new(402).set_body_string("Payment Required"))
            .mount(&mock_server)
            .await;

        let state = setup_state().await;
        let state = HandlersState {
            stripe_base_url: mock_server.uri(),
            turnstile_secret_key: None,
            ..state
        };

        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_http_err",
                json!({
                    fields::ORDER_ID: "order_http_err",
                    fields::STATUS: OrderStatus::PaymentAuthorized.as_str(),
                    fields::PAYMENT_STATUS: PaymentStatus::Authorized.as_str(),
                    fields::PAYMENT_INTENT_ID: "pi_http_err",
                    fields::ITEMS: [{ fields::SELLER_ID: "seller_1" }],
                }),
            )
            .await
            .unwrap();

        let err = capture_payment(
            State(state),
            Extension(auth("test")),
            Json(CapturePaymentRequest {
                order_id: "order_http_err".into(),
                user_id: Some("seller_1".to_string()),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("Failed to capture payment with Stripe")
        );
    }
}
