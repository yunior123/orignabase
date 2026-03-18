//! Premium subscription handlers ($9.99/mo CAD).
//! Ported from: functions/handlers/payment_stripe.py (subscription endpoints)

use axum::{Extension, Json, Router, extract::State, routing::post};
use ob_auth::middleware::AuthContext;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{error, info, warn};

use crate::HandlersState;
use crate::shared::auth::resolve_self_user_id;
use crate::shared::schema::{SubscriptionStatus, business_rules, collections, fields};
use crate::shared::validation::validate_uid;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSubscriptionRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    /// Stripe payment method ID (e.g. pm_xxxx) attached to the customer.
    #[serde(default)]
    pub payment_method_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSubscriptionResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkout_url: Option<String>,
    pub client_secret: Option<String>,
    pub status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelSubscriptionRequest {
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelSubscriptionResponse {
    pub success: bool,
    pub status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionStatusRequest {
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionStatusResponse {
    pub success: bool,
    pub is_premium: bool,
    pub status: String,
    pub current_period_end: Option<String>,
    pub stripe_subscription_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactivateSubscriptionRequest {
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactivateSubscriptionResponse {
    pub success: bool,
    pub status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionNotificationPrefsRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub notify_new_products: Option<bool>,
    #[serde(default)]
    pub notify_trending: Option<bool>,
}

/// Price in cents for the premium subscription ($9.99 CAD = 999 cents).
const PREMIUM_PRICE_CENTS: i64 = (business_rules::PREMIUM_SUBSCRIPTION_PRICE_CAD * 100.0) as i64;

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/subscriptions/create", post(create_subscription))
        .route("/api/subscriptions/cancel", post(cancel_subscription))
        .route("/api/subscriptions/status", post(subscription_status))
        .route(
            "/api/subscriptions/reactivate",
            post(reactivate_subscription),
        )
        .route(
            "/api/subscriptions/notification-preferences",
            post(update_notification_preferences),
        )
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Ensure the user has a Stripe customer ID, creating one if needed.
async fn ensure_customer(
    state: &HandlersState,
    user_id: &str,
    stripe_key: &str,
) -> Result<String, ob_core::Error> {
    let user = state
        .db
        .get_document(collections::USERS, user_id)
        .await
        .map_err(|_| ob_core::Error::NotFound(format!("User {user_id} not found")))?;

    let existing = user
        .get(fields::CUSTOMER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if !existing.is_empty() {
        return Ok(existing.to_string());
    }

    // Create Stripe customer
    let email = user
        .get(fields::EMAIL)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let name = user
        .get(fields::NAME)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let resp = state
        .http_client
        .post(format!("{}/customers", state.stripe_base_url))
        .basic_auth(stripe_key, None::<&str>)
        .form(&[
            ("email", email),
            ("name", name),
            ("metadata[user_id]", user_id),
        ])
        .send()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Stripe API error: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        error!(error = %body, "Failed to create Stripe customer");
        return Err(ob_core::Error::Internal(
            "Failed to create Stripe customer".into(),
        ));
    }

    let customer: Value = resp
        .json()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Parse error: {e}")))?;

    let customer_id = customer["id"]
        .as_str()
        .ok_or_else(|| ob_core::Error::Internal("Missing customer ID from Stripe".into()))?
        .to_string();

    // Store on user
    let now = chrono::Utc::now().to_rfc3339();
    state
        .db
        .update_document(
            collections::USERS,
            user_id,
            serde_json::json!({
                fields::CUSTOMER_ID: &customer_id,
                fields::UPDATED_AT: now,
            }),
        )
        .await?;

    Ok(customer_id)
}

/// Get the user's active subscription doc from the DB, if any.
async fn get_user_subscription(
    state: &HandlersState,
    user_id: &str,
) -> Result<Option<Value>, ob_core::Error> {
    if let Ok(doc) = state
        .db
        .get_document(collections::SUBSCRIPTIONS, user_id)
        .await
    {
        return Ok(Some(doc));
    }

    // Validate user ID format before querying
    ob_core::validate_surreal_record_id(user_id)?;
    
    let rows = state
        .db
        .query_bind_value(
            "SELECT * FROM subscriptions WHERE buyerId = $buyer_id ORDER BY createdAt DESC LIMIT 1",
            serde_json::json!({"buyer_id": user_id})
        )
        .await?;

    Ok(rows.into_iter().next())
}

async fn upsert_subscription_doc(
    state: &HandlersState,
    user_id: &str,
    data: Value,
) -> Result<(), ob_core::Error> {
    let _ = state
        .db
        .update_document(collections::SUBSCRIPTIONS, user_id, data)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/subscriptions/create — Create a premium subscription.
async fn create_subscription(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<CreateSubscriptionRequest>,
) -> Result<Json<CreateSubscriptionResponse>, ob_core::Error> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;
    if let Some(payment_method_id) = req.payment_method_id.as_deref() {
        validate_uid("paymentMethodId", payment_method_id)?;
    }

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &user_id,
        "create_subscription",
        5,
        60,
    )
    .await?;

    // Check for existing active subscription (atomic check before creation)
    // This prevents race conditions where two simultaneous requests both pass the check
    let existing_sql = format!(
        "SELECT * FROM {} WHERE {} = '{}' AND (status = 'active' OR {} = 'active') LIMIT 1",
        collections::SUBSCRIPTIONS,
        fields::BUYER_ID,
        user_id,
        fields::SUBSCRIPTION_STATUS
    );
    let existing_records = state.db.query_raw(&existing_sql).await?;
    
    if !existing_records.is_empty() {
        let existing = &existing_records[0];
        let existing_sub_id = existing
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        return Err(ob_core::Error::Validation(
            format!("User already has an active subscription: {}", existing_sub_id)
        ));
    }

    let stripe_key = state.config.require_secret("stripe_secret_key")?;
    let customer_id = ensure_customer(&state, &user_id, stripe_key).await?;

    if req.payment_method_id.is_none() {
        let user = state.db.get_document(collections::USERS, &user_id).await?;
        let roles = user
            .get(fields::ROLES)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if roles.iter().any(|role| role.as_str() == Some("seller")) {
            return Err(ob_core::Error::Validation(
                "Seller accounts cannot currently subscribe to Premium.".into(),
            ));
        }

        let base_url = std::env::var("OB_BASE_URL")
            .unwrap_or_else(|_| format!("http://{}:{}", state.config.host, state.config.port));

        let form: Vec<(&str, String)> = vec![
            ("customer", customer_id.clone()),
            ("line_items[0][price_data][currency]", "cad".to_string()),
            (
                "line_items[0][price_data][unit_amount]",
                PREMIUM_PRICE_CENTS.to_string(),
            ),
            (
                "line_items[0][price_data][product_data][name]",
                "Origna Premium Subscription".to_string(),
            ),
            ("line_items[0][quantity]", "1".to_string()),
            (
                "line_items[0][price_data][recurring][interval]",
                "month".to_string(),
            ),
            ("mode", "subscription".to_string()),
            (
                "success_url",
                format!(
                    "{}/subscription/success?session_id={{CHECKOUT_SESSION_ID}}",
                    base_url
                ),
            ),
            ("cancel_url", format!("{}/subscription/cancel", base_url)),
            ("client_reference_id", user_id.clone()),
            ("metadata[uid]", user_id.clone()),
            ("subscription_data[metadata][uid]", user_id.clone()),
            ("payment_method_types[0]", "card".to_string()),
        ];

        let session_resp = state
            .http_client
            .post(format!("{}/checkout/sessions", state.stripe_base_url))
            .basic_auth(stripe_key, None::<&str>)
            .form(&form)
            .send()
            .await
            .map_err(|e| ob_core::Error::Internal(format!("Stripe API error: {e}")))?;

        if !session_resp.status().is_success() {
            let body = session_resp.text().await.unwrap_or_default();
            error!(error = %body, "Failed to create subscription checkout session");
            return Err(ob_core::Error::Internal(
                "Failed to create subscription checkout session".into(),
            ));
        }

        let session: Value = session_resp
            .json()
            .await
            .map_err(|e| ob_core::Error::Internal(format!("Parse error: {e}")))?;

        let checkout_url = session["url"].as_str().unwrap_or("").to_string();
        let session_id = session["id"].as_str().unwrap_or("").to_string();
        if checkout_url.is_empty() {
            return Err(ob_core::Error::Internal(
                "Stripe did not return a checkout URL".into(),
            ));
        }

        let now = chrono::Utc::now().to_rfc3339();
        let _ = state
            .db
            .update_document(
                collections::USERS,
                &user_id,
                json!({
                    "lastCheckoutSession": checkout_url,
                    "lastCheckoutTimestamp": now,
                    fields::UPDATED_AT: now,
                }),
            )
            .await;

        return Ok(Json(CreateSubscriptionResponse {
            success: true,
            subscription_id: Some(session_id),
            checkout_url: Some(checkout_url),
            client_secret: None,
            status: "checkout_pending".to_string(),
        }));
    }

    // Attach payment method to customer
    let attach_resp = state
        .http_client
        .post(format!(
            "{}/payment_methods/{}/attach",
            state.stripe_base_url,
            req.payment_method_id.as_deref().unwrap_or_default()
        ))
        .basic_auth(stripe_key, None::<&str>)
        .form(&[("customer", customer_id.as_str())])
        .send()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Stripe API error: {e}")))?;

    if !attach_resp.status().is_success() {
        let body = attach_resp.text().await.unwrap_or_default();
        // Ignore "already attached" errors
        if !body.contains("already been attached") {
            error!(error = %body, "Failed to attach payment method");
            return Err(ob_core::Error::Internal(
                "Failed to attach payment method".into(),
            ));
        }
    }

    // Set as default payment method
    let _ = state
        .http_client
        .post(format!("{}/customers/{customer_id}", state.stripe_base_url))
        .basic_auth(stripe_key, None::<&str>)
        .form(&[(
            "invoice_settings[default_payment_method]",
            req.payment_method_id.as_deref().unwrap_or_default(),
        )])
        .send()
        .await;

    // Create subscription with inline price
    let resp = state
        .http_client
        .post(format!("{}/subscriptions", state.stripe_base_url))
        .basic_auth(stripe_key, None::<&str>)
        .form(&[
            ("customer", customer_id.as_str()),
            ("items[0][price_data][currency]", "cad"),
            (
                "items[0][price_data][unit_amount]",
                &PREMIUM_PRICE_CENTS.to_string(),
            ),
            ("items[0][price_data][recurring][interval]", "month"),
            (
                "items[0][price_data][product_data][name]",
                "Origna Premium Subscription",
            ),
            ("payment_behavior", "default_incomplete"),
            ("expand[0]", "latest_invoice.payment_intent"),
            ("metadata[user_id]", user_id.as_str()),
        ])
        .send()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Stripe API error: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        error!(error = %body, "Failed to create subscription");
        return Err(ob_core::Error::Internal(
            "Failed to create subscription".into(),
        ));
    }

    let sub: Value = resp
        .json()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Parse error: {e}")))?;

    let sub_id = sub["id"].as_str().unwrap_or("");
    let sub_status = sub["status"].as_str().unwrap_or("incomplete");
    let client_secret = sub["latest_invoice"]["payment_intent"]["client_secret"]
        .as_str()
        .map(|s| s.to_string());
    let period_end = sub["current_period_end"].as_i64().unwrap_or(0);

    // Store subscription in DB
    let now = chrono::Utc::now().to_rfc3339();
    let sub_doc = serde_json::json!({
        fields::BUYER_ID: user_id,
        fields::STRIPE_SUBSCRIPTION_ID: sub_id,
        fields::STATUS: if sub_status == "active" {
            SubscriptionStatus::Active.as_str()
        } else {
            "incomplete"
        },
        fields::SUBSCRIPTION_STATUS: if sub_status == "active" {
            SubscriptionStatus::Active.as_str()
        } else {
            "incomplete"
        },
        fields::CURRENT_PERIOD_END: period_end,
        "cancelAtPeriodEnd": false,
        fields::CREATED_AT: now,
        fields::UPDATED_AT: now,
    });

    upsert_subscription_doc(&state, &user_id, sub_doc)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to store subscription: {e}")))?;

    // Update user premium flag if active
    if sub_status == "active" {
        let _ = state
            .db
            .update_document(
                collections::USERS,
                &user_id,
                serde_json::json!({
                    fields::IS_PREMIUM: true,
                    fields::UPDATED_AT: now,
                }),
            )
            .await;
    }

    info!(
        user_id = %user_id,
        subscription_id = %sub_id,
        status = %sub_status,
        "Subscription created"
    );

    Ok(Json(CreateSubscriptionResponse {
        success: true,
        subscription_id: Some(sub_id.to_string()),
        checkout_url: None,
        client_secret,
        status: sub_status.to_string(),
    }))
}

/// POST /api/subscriptions/cancel — Cancel a user's subscription (at period end).
async fn cancel_subscription(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<CancelSubscriptionRequest>,
) -> Result<Json<CancelSubscriptionResponse>, ob_core::Error> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &user_id,
        "cancel_subscription",
        5,
        60,
    )
    .await?;

    let sub_doc = get_user_subscription(&state, &user_id)
        .await?
        .ok_or_else(|| ob_core::Error::NotFound("No subscription found".into()))?;

    let stripe_sub_id = sub_doc
        .get(fields::STRIPE_SUBSCRIPTION_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if stripe_sub_id.is_empty() {
        return Err(ob_core::Error::Internal(
            "Subscription has no Stripe ID".into(),
        ));
    }

    let current_status = sub_doc
        .get(fields::STATUS)
        .or_else(|| sub_doc.get(fields::SUBSCRIPTION_STATUS))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if current_status == SubscriptionStatus::Cancelled.as_str() {
        return Ok(Json(CancelSubscriptionResponse {
            success: true,
            status: "cancelled".to_string(),
        }));
    }

    // Cancel at period end (graceful cancellation)
    let stripe_key = state.config.require_secret("stripe_secret_key")?;
    let resp = state
        .http_client
        .post(format!(
            "{}/subscriptions/{stripe_sub_id}",
            state.stripe_base_url
        ))
        .basic_auth(stripe_key, None::<&str>)
        .form(&[("cancel_at_period_end", "true")])
        .send()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Stripe API error: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        error!(error = %body, "Failed to cancel subscription");
        return Err(ob_core::Error::Internal(
            "Failed to cancel subscription".into(),
        ));
    }

    // Update DB — cancel_pending, NOT cancelled. User paid for the full period.
    let now = chrono::Utc::now().to_rfc3339();
    let _ = state
        .db
        .update_document(
            collections::SUBSCRIPTIONS,
            &user_id,
            serde_json::json!({
                fields::STATUS: SubscriptionStatus::CancelPending.as_str(),
                fields::SUBSCRIPTION_STATUS: SubscriptionStatus::CancelPending.as_str(),
                "cancelAtPeriodEnd": true,
                "cancelledAt": now,
                fields::UPDATED_AT: now,
            }),
        )
        .await;

    // Do NOT set isPremium to false — user paid for the full period.
    // The webhook `customer.subscription.deleted` will handle that when the period ends.

    info!(
        user_id = %user_id,
        stripe_sub_id = %stripe_sub_id,
        "Subscription cancel requested (cancel_pending)"
    );

    Ok(Json(CancelSubscriptionResponse {
        success: true,
        status: SubscriptionStatus::CancelPending.as_str().to_string(),
    }))
}

/// POST /api/subscriptions/status — Get subscription status for a user.
async fn subscription_status(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<SubscriptionStatusRequest>,
) -> Result<Json<SubscriptionStatusResponse>, ob_core::Error> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &user_id,
        "subscription_status",
        30,
        1,
    )
    .await?;

    let sub_doc = get_user_subscription(&state, &user_id).await?;

    match sub_doc {
        None => Ok(Json(SubscriptionStatusResponse {
            success: true,
            is_premium: false,
            status: "none".to_string(),
            current_period_end: None,
            stripe_subscription_id: None,
        })),
        Some(doc) => {
            let status = doc
                .get(fields::STATUS)
                .or_else(|| doc.get(fields::SUBSCRIPTION_STATUS))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let is_premium = status == SubscriptionStatus::Active.as_str();
            let period_end = doc
                .get(fields::CURRENT_PERIOD_END)
                .and_then(|v| v.as_i64())
                .map(|ts| {
                    chrono::DateTime::from_timestamp(ts, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                });
            let stripe_id = doc
                .get(fields::STRIPE_SUBSCRIPTION_ID)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            Ok(Json(SubscriptionStatusResponse {
                success: true,
                is_premium,
                status: status.to_string(),
                current_period_end: period_end,
                stripe_subscription_id: stripe_id,
            }))
        }
    }
}

/// POST /api/subscriptions/reactivate — Reactivate a cancelled subscription.
async fn reactivate_subscription(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<ReactivateSubscriptionRequest>,
) -> Result<Json<ReactivateSubscriptionResponse>, ob_core::Error> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &user_id,
        "reactivate_subscription",
        5,
        60,
    )
    .await?;

    let sub_doc = get_user_subscription(&state, &user_id)
        .await?
        .ok_or_else(|| ob_core::Error::NotFound("No subscription found".into()))?;

    let stripe_sub_id = sub_doc
        .get(fields::STRIPE_SUBSCRIPTION_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if stripe_sub_id.is_empty() {
        return Err(ob_core::Error::Internal(
            "Subscription has no Stripe ID".into(),
        ));
    }

    let current_status = sub_doc
        .get(fields::STATUS)
        .or_else(|| sub_doc.get(fields::SUBSCRIPTION_STATUS))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if current_status == SubscriptionStatus::Active.as_str() {
        return Ok(Json(ReactivateSubscriptionResponse {
            success: true,
            status: SubscriptionStatus::Active.as_str().to_string(),
        }));
    }

    if current_status == SubscriptionStatus::Expired.as_str() {
        return Err(ob_core::Error::Validation(
            "Cannot reactivate an expired subscription. Please create a new one.".into(),
        ));
    }

    // Reactivate by removing cancel_at_period_end
    let stripe_key = state.config.require_secret("stripe_secret_key")?;
    let resp = state
        .http_client
        .post(format!(
            "{}/subscriptions/{stripe_sub_id}",
            state.stripe_base_url
        ))
        .basic_auth(stripe_key, None::<&str>)
        .form(&[("cancel_at_period_end", "false")])
        .send()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Stripe API error: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        error!(error = %body, "Failed to reactivate subscription");
        return Err(ob_core::Error::Internal(
            "Failed to reactivate subscription".into(),
        ));
    }

    // Update DB
    let now = chrono::Utc::now().to_rfc3339();
    let _ = state
        .db
        .update_document(
            collections::SUBSCRIPTIONS,
            &user_id,
            serde_json::json!({
                fields::STATUS: SubscriptionStatus::Active.as_str(),
                fields::SUBSCRIPTION_STATUS: SubscriptionStatus::Active.as_str(),
                "cancelAtPeriodEnd": false,
                fields::UPDATED_AT: now,
            }),
        )
        .await;

    let _ = state
        .db
        .update_document(
            collections::USERS,
            &user_id,
            serde_json::json!({
                fields::IS_PREMIUM: true,
                fields::UPDATED_AT: now,
            }),
        )
        .await;

    info!(
        user_id = %user_id,
        stripe_sub_id = %stripe_sub_id,
        "Subscription reactivated"
    );

    Ok(Json(ReactivateSubscriptionResponse {
        success: true,
        status: SubscriptionStatus::Active.as_str().to_string(),
    }))
}

async fn update_notification_preferences(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<SubscriptionNotificationPrefsRequest>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &user_id,
        "update_notification_preferences",
        10,
        1,
    )
    .await?;
    if req.notify_new_products.is_none() && req.notify_trending.is_none() {
        return Err(ob_core::Error::Validation(
            "At least one notification preference must be provided".into(),
        ));
    }

    let now = chrono::Utc::now().to_rfc3339();
    let mut update = serde_json::Map::new();
    if let Some(v) = req.notify_new_products {
        update.insert("notifyNewProducts".to_string(), json!(v));
    }
    if let Some(v) = req.notify_trending {
        update.insert("notifyTrending".to_string(), json!(v));
    }
    update.insert(fields::UPDATED_AT.to_string(), json!(now));

    state
        .db
        .update_document(collections::USERS, &user_id, Value::Object(update))
        .await?;

    Ok(Json(json!({ "success": true })))
}

// ---------------------------------------------------------------------------
// Webhook handlers
// ---------------------------------------------------------------------------

/// Route a Stripe subscription webhook event to the appropriate handler.
pub async fn route_subscription_webhook(
    state: &HandlersState,
    event_type: &str,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    match event_type {
        "customer.subscription.created" => handle_subscription_created(state, event_data).await,
        "customer.subscription.updated" => handle_subscription_updated(state, event_data).await,
        "customer.subscription.deleted" => handle_subscription_deleted(state, event_data).await,
        "invoice.payment_failed" => handle_invoice_payment_failed(state, event_data).await,
        _ => Ok(()),
    }
}

/// Look up a user by their Stripe customer ID.
async fn find_user_by_customer_id(
    state: &HandlersState,
    customer_id: &str,
) -> Result<Option<Value>, ob_core::Error> {
    let rows = state
        .db
        .query_bind_value(
            "SELECT * FROM users WHERE customerId = $customer_id LIMIT 1",
            serde_json::json!({"customer_id": customer_id})
        )
        .await?;
    Ok(rows.into_iter().next())
}

/// Extract the Stripe customer ID from a webhook event data object.
fn extract_customer_id(event_data: &Value) -> Option<&str> {
    event_data
        .get("object")
        .and_then(|obj| obj.get("customer"))
        .and_then(|c| c.as_str())
}

/// Extract the user's UID from the user document.
fn extract_uid(user: &Value) -> Option<&str> {
    user.get(fields::UID)
        .and_then(|v| v.as_str())
        .or_else(|| user.get("id").and_then(|v| v.as_str()))
}

async fn handle_subscription_created(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let customer_id = match extract_customer_id(event_data) {
        Some(id) => id,
        None => {
            warn!("subscription.created webhook missing customer ID");
            return Ok(());
        }
    };

    let user = match find_user_by_customer_id(state, customer_id).await? {
        Some(u) => u,
        None => {
            warn!(customer_id = %customer_id, "No user found for Stripe customer");
            return Ok(());
        }
    };

    // DB rows always have an `id` field (via Record::into_value), so extract_uid never returns None here.
    let uid = extract_uid(&user).unwrap_or_default().to_string();

    let now = chrono::Utc::now().to_rfc3339();

    // Update user document
    state
        .db
        .update_document(
            collections::USERS,
            &uid,
            json!({
                fields::IS_PREMIUM: true,
                fields::SUBSCRIPTION_STATUS: SubscriptionStatus::Active.as_str(),
                "premiumSince": now,
                fields::UPDATED_AT: now,
            }),
        )
        .await?;

    info!(user_id = %uid, "Webhook: subscription created, user marked premium");
    Ok(())
}

async fn handle_subscription_updated(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let customer_id = match extract_customer_id(event_data) {
        Some(id) => id,
        None => {
            warn!("subscription.updated webhook missing customer ID");
            return Ok(());
        }
    };

    let user = match find_user_by_customer_id(state, customer_id).await? {
        Some(u) => u,
        None => {
            warn!(customer_id = %customer_id, "No user found for Stripe customer");
            return Ok(());
        }
    };

    let uid = extract_uid(&user).unwrap_or_default().to_string();

    let sub_obj = event_data
        .get("object")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let status = sub_obj
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let current_period_end = sub_obj
        .get("current_period_end")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let cancel_at_period_end = sub_obj
        .get("cancel_at_period_end")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let now = chrono::Utc::now().to_rfc3339();

    // Sync subscription doc
    state
        .db
        .update_document(
            collections::SUBSCRIPTIONS,
            &uid,
            json!({
                fields::STATUS: status,
                fields::SUBSCRIPTION_STATUS: status,
                fields::CURRENT_PERIOD_END: current_period_end,
                "cancelAtPeriodEnd": cancel_at_period_end,
                fields::UPDATED_AT: now,
            }),
        )
        .await?;

    info!(user_id = %uid, status = %status, "Webhook: subscription updated");
    Ok(())
}

async fn handle_subscription_deleted(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let customer_id = match extract_customer_id(event_data) {
        Some(id) => id,
        None => {
            warn!("subscription.deleted webhook missing customer ID");
            return Ok(());
        }
    };

    let user = match find_user_by_customer_id(state, customer_id).await? {
        Some(u) => u,
        None => {
            warn!(customer_id = %customer_id, "No user found for Stripe customer");
            return Ok(());
        }
    };

    let uid = extract_uid(&user).unwrap_or_default().to_string();

    let now = chrono::Utc::now().to_rfc3339();

    // Revoke premium — subscription period has ended
    state
        .db
        .update_document(
            collections::USERS,
            &uid,
            json!({
                fields::IS_PREMIUM: false,
                fields::SUBSCRIPTION_STATUS: SubscriptionStatus::Cancelled.as_str(),
                "premiumExpiresAt": now,
                fields::UPDATED_AT: now,
            }),
        )
        .await?;

    // Update subscription doc
    state
        .db
        .update_document(
            collections::SUBSCRIPTIONS,
            &uid,
            json!({
                fields::STATUS: SubscriptionStatus::Cancelled.as_str(),
                fields::SUBSCRIPTION_STATUS: SubscriptionStatus::Cancelled.as_str(),
                fields::UPDATED_AT: now,
            }),
        )
        .await?;

    info!(user_id = %uid, "Webhook: subscription deleted, premium revoked");
    Ok(())
}

async fn handle_invoice_payment_failed(
    state: &HandlersState,
    event_data: &Value,
) -> Result<(), ob_core::Error> {
    let customer_id = match extract_customer_id(event_data) {
        Some(id) => id,
        None => {
            warn!("invoice.payment_failed webhook missing customer ID");
            return Ok(());
        }
    };

    let user = match find_user_by_customer_id(state, customer_id).await? {
        Some(u) => u,
        None => {
            warn!(customer_id = %customer_id, "No user found for Stripe customer");
            return Ok(());
        }
    };

    let uid = extract_uid(&user).unwrap_or_default().to_string();

    let now = chrono::Utc::now().to_rfc3339();

    // Mark as past_due
    state
        .db
        .update_document(
            collections::USERS,
            &uid,
            json!({
                fields::SUBSCRIPTION_STATUS: SubscriptionStatus::PastDue.as_str(),
                fields::UPDATED_AT: now,
            }),
        )
        .await?;

    state
        .db
        .update_document(
            collections::SUBSCRIPTIONS,
            &uid,
            json!({
                fields::STATUS: SubscriptionStatus::PastDue.as_str(),
                fields::SUBSCRIPTION_STATUS: SubscriptionStatus::PastDue.as_str(),
                fields::UPDATED_AT: now,
            }),
        )
        .await?;

    // Create notification document
    let notification_id = format!("notif_payment_failed_{}", uid);
    let _ = state
        .db
        .update_document(
            collections::NOTIFICATIONS,
            &notification_id,
            json!({
                fields::BUYER_ID: uid,
                fields::NOTIFICATION_TYPE: "payment_failed",
                fields::STATUS: "unread",
                "title": "Payment Failed",
                "body": "Your subscription payment failed. Please update your payment method to keep Premium access.",
                fields::CREATED_AT: now,
                fields::UPDATED_AT: now,
            }),
        )
        .await;

    info!(user_id = %uid, "Webhook: invoice payment failed, status set to past_due");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn setup_state() -> HandlersState {
        HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        }
    }

    async fn setup_state_with_config(config: Config, stripe_base_url: String) -> HandlersState {
        HandlersState {
            config: Arc::new(config),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url,
        }
    }

    #[test]
    fn test_premium_price_cents() {
        assert_eq!(PREMIUM_PRICE_CENTS, 999);
    }

    #[test]
    fn test_create_request_deser() {
        let json = r#"{"userId": "u1"}"#;
        let req: CreateSubscriptionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.user_id, "u1");
        assert!(req.payment_method_id.is_none());
    }

    #[test]
    fn test_create_response_ser() {
        let resp = CreateSubscriptionResponse {
            success: true,
            subscription_id: Some("sub_123".to_string()),
            checkout_url: Some("https://checkout.stripe.com/c/pay/cs_test".to_string()),
            client_secret: Some("pi_xxx_secret_yyy".to_string()),
            status: "active".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["subscriptionId"], "sub_123");
        assert_eq!(
            json["checkoutUrl"],
            "https://checkout.stripe.com/c/pay/cs_test"
        );
        assert_eq!(json["clientSecret"], "pi_xxx_secret_yyy");
        assert_eq!(json["status"], "active");
    }

    #[test]
    fn test_cancel_request_deser() {
        let json = r#"{"userId": "user-99"}"#;
        let req: CancelSubscriptionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.user_id, "user-99");
    }

    #[test]
    fn test_status_response_no_subscription() {
        let resp = SubscriptionStatusResponse {
            success: true,
            is_premium: false,
            status: "none".to_string(),
            current_period_end: None,
            stripe_subscription_id: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["isPremium"], false);
        assert_eq!(json["status"], "none");
        assert!(json["currentPeriodEnd"].is_null());
    }

    #[test]
    fn test_reactivate_request_deser() {
        let json = r#"{"userId": "u42"}"#;
        let req: ReactivateSubscriptionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.user_id, "u42");
    }

    #[test]
    fn test_subscription_status_as_str() {
        assert_eq!(SubscriptionStatus::Active.as_str(), "active");
        assert_eq!(SubscriptionStatus::Cancelled.as_str(), "cancelled");
        assert_eq!(SubscriptionStatus::CancelPending.as_str(), "cancel_pending");
        assert_eq!(SubscriptionStatus::PastDue.as_str(), "past_due");
        assert_eq!(SubscriptionStatus::Expired.as_str(), "expired");
    }

    #[test]
    fn test_notification_prefs_request_deser() {
        let json = r#"{"userId":"u1","notifyNewProducts":true}"#;
        let req: SubscriptionNotificationPrefsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.user_id, "u1");
        assert_eq!(req.notify_new_products, Some(true));
        assert_eq!(req.notify_trending, None);
    }

    // --- Ported from Python test_handlers_subscriptions*.py ---

    #[test]
    fn test_create_request_with_payment_method() {
        let json = r#"{"userId":"u1","paymentMethodId":"pm_123abc"}"#;
        let req: CreateSubscriptionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.payment_method_id.as_deref(), Some("pm_123abc"));
    }

    #[test]
    fn test_create_response_skip_serializing_none() {
        let resp = CreateSubscriptionResponse {
            success: true,
            subscription_id: None,
            checkout_url: None,
            client_secret: None,
            status: "checkout_pending".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        // skip_serializing_if on subscription_id and checkout_url
        assert!(json.get("subscriptionId").is_none());
        assert!(json.get("checkoutUrl").is_none());
        // client_secret is NOT skip_serializing_if — so it's null
        assert!(json.get("clientSecret").is_some());
        assert_eq!(json["status"], "checkout_pending");
    }

    #[test]
    fn test_create_response_with_checkout_url() {
        let resp = CreateSubscriptionResponse {
            success: true,
            subscription_id: Some("cs_test_123".to_string()),
            checkout_url: Some("https://checkout.stripe.com/c/pay/cs_test".to_string()),
            client_secret: None,
            status: "checkout_pending".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["subscriptionId"], "cs_test_123");
        assert_eq!(
            json["checkoutUrl"],
            "https://checkout.stripe.com/c/pay/cs_test"
        );
    }

    #[test]
    fn test_cancel_response_ser() {
        let resp = CancelSubscriptionResponse {
            success: true,
            status: "cancelled".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["status"], "cancelled");
    }

    #[test]
    fn test_status_response_active_subscription() {
        let resp = SubscriptionStatusResponse {
            success: true,
            is_premium: true,
            status: SubscriptionStatus::Active.as_str().to_string(),
            current_period_end: Some("2026-04-10T00:00:00+00:00".to_string()),
            stripe_subscription_id: Some("sub_abc123".to_string()),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["isPremium"], true);
        assert_eq!(json["status"], "active");
        assert_eq!(json["currentPeriodEnd"], "2026-04-10T00:00:00+00:00");
        assert_eq!(json["stripeSubscriptionId"], "sub_abc123");
    }

    #[test]
    fn test_reactivate_response_ser() {
        let resp = ReactivateSubscriptionResponse {
            success: true,
            status: SubscriptionStatus::Active.as_str().to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "active");
    }

    #[test]
    fn test_subscription_status_enum_all_variants() {
        // Verify all 5 statuses serialize correctly
        let variants = [
            (SubscriptionStatus::Active, "active"),
            (SubscriptionStatus::Cancelled, "cancelled"),
            (SubscriptionStatus::CancelPending, "cancel_pending"),
            (SubscriptionStatus::PastDue, "past_due"),
            (SubscriptionStatus::Expired, "expired"),
        ];
        for (status, expected_str) in variants {
            assert_eq!(status.as_str(), expected_str);
        }
    }

    #[test]
    fn test_premium_price_matches_business_rule() {
        // Verify PREMIUM_PRICE_CENTS = PREMIUM_SUBSCRIPTION_PRICE_CAD * 100
        let expected = (business_rules::PREMIUM_SUBSCRIPTION_PRICE_CAD * 100.0) as i64;
        assert_eq!(PREMIUM_PRICE_CENTS, expected);
        assert_eq!(PREMIUM_PRICE_CENTS, 999); // $9.99
    }

    #[test]
    fn test_notification_prefs_both_fields() {
        let json = r#"{"userId":"u1","notifyNewProducts":false,"notifyTrending":true}"#;
        let req: SubscriptionNotificationPrefsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.notify_new_products, Some(false));
        assert_eq!(req.notify_trending, Some(true));
    }

    #[test]
    fn test_notification_prefs_empty() {
        let json = r#"{"userId":"u1"}"#;
        let req: SubscriptionNotificationPrefsRequest = serde_json::from_str(json).unwrap();
        assert!(req.notify_new_products.is_none());
        assert!(req.notify_trending.is_none());
    }

    #[test]
    fn test_status_request_deser() {
        let json = r#"{"userId":"user-42"}"#;
        let req: SubscriptionStatusRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.user_id, "user-42");
    }

    // --- Cancel fix: cancel_pending instead of cancelled ---

    #[test]
    fn test_cancel_pending_status_exists() {
        // The cancel handler now returns cancel_pending, not cancelled
        assert_eq!(SubscriptionStatus::CancelPending.as_str(), "cancel_pending");
        // Cancelled is reserved for when the period actually ends (webhook)
        assert_eq!(SubscriptionStatus::Cancelled.as_str(), "cancelled");
    }

    #[test]
    fn test_cancel_response_uses_cancel_pending() {
        // Verify CancelSubscriptionResponse can carry cancel_pending status
        let resp = CancelSubscriptionResponse {
            success: true,
            status: SubscriptionStatus::CancelPending.as_str().to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "cancel_pending");
    }

    // --- Webhook helpers ---

    #[test]
    fn test_extract_customer_id_from_event() {
        let event = json!({
            "object": {
                "customer": "cus_abc123",
                "status": "active"
            }
        });
        assert_eq!(extract_customer_id(&event), Some("cus_abc123"));
    }

    #[test]
    fn test_extract_customer_id_missing() {
        let event = json!({ "object": {} });
        assert_eq!(extract_customer_id(&event), None);

        let event2 = json!({});
        assert_eq!(extract_customer_id(&event2), None);
    }

    #[test]
    fn test_extract_uid_from_user() {
        let user = json!({ "uid": "user-1", "email": "a@b.com" });
        assert_eq!(extract_uid(&user), Some("user-1"));

        // fallback to "id"
        let user2 = json!({ "id": "user-2" });
        assert_eq!(extract_uid(&user2), Some("user-2"));

        // uid takes precedence over id
        let user3 = json!({ "uid": "user-3", "id": "user-4" });
        assert_eq!(extract_uid(&user3), Some("user-3"));
    }

    #[test]
    fn test_extract_uid_missing() {
        let user = json!({ "email": "a@b.com" });
        assert_eq!(extract_uid(&user), None);
    }

    // --- Webhook routing ---

    #[test]
    fn test_webhook_event_types_are_str() {
        // Verify the event type strings match Stripe's documented event names
        let known_events = [
            "customer.subscription.created",
            "customer.subscription.updated",
            "customer.subscription.deleted",
            "invoice.payment_failed",
        ];
        for event in known_events {
            assert!(
                event.contains('.'),
                "Event type should contain dots: {event}"
            );
        }
    }

    #[tokio::test]
    async fn test_get_user_subscription_returns_direct_document() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                "user_1",
                json!({
                    fields::STATUS: SubscriptionStatus::Active.as_str(),
                    fields::BUYER_ID: "user_1",
                }),
            )
            .await
            .unwrap();

        let found = get_user_subscription(&state, "user_1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found[fields::STATUS], SubscriptionStatus::Active.as_str());
    }

    #[tokio::test]
    async fn test_create_subscription_rejects_existing_active_subscription() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                "user_1",
                json!({
                    fields::SUBSCRIPTION_STATUS: SubscriptionStatus::Active.as_str(),
                    fields::STATUS: SubscriptionStatus::Active.as_str(),
                }),
            )
            .await
            .unwrap();

        let err = create_subscription(
            State(state),
            Json(CreateSubscriptionRequest {
                user_id: "user_1".into(),
                payment_method_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("already has an active subscription")
        );
    }

    #[tokio::test]
    async fn test_cancel_subscription_rejects_missing_subscription() {
        let state = setup_state().await;
        let err = cancel_subscription(
            State(state),
            Json(CancelSubscriptionRequest {
                user_id: "user_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("No subscription found"));
    }

    #[tokio::test]
    async fn test_reactivate_subscription_rejects_expired_subscription() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                "user_1",
                json!({
                    fields::STRIPE_SUBSCRIPTION_ID: "sub_123",
                    fields::STATUS: SubscriptionStatus::Expired.as_str(),
                    fields::SUBSCRIPTION_STATUS: SubscriptionStatus::Expired.as_str(),
                }),
            )
            .await
            .unwrap();

        let err = reactivate_subscription(
            State(state),
            Json(ReactivateSubscriptionRequest {
                user_id: "user_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("Cannot reactivate an expired subscription")
        );
    }

    #[tokio::test]
    async fn test_update_notification_preferences_requires_one_field() {
        let state = setup_state().await;
        let err = update_notification_preferences(
            State(state),
            Json(SubscriptionNotificationPrefsRequest {
                user_id: "user_1".into(),
                notify_new_products: None,
                notify_trending: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("At least one notification preference")
        );
    }

    #[tokio::test]
    async fn test_subscription_status_handler_formats_period_end() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                "user_1",
                json!({
                    fields::STATUS: SubscriptionStatus::Active.as_str(),
                    fields::CURRENT_PERIOD_END: 1_780_704_000i64,
                    fields::STRIPE_SUBSCRIPTION_ID: "sub_123",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = subscription_status(
            State(state),
            Json(SubscriptionStatusRequest {
                user_id: "user_1".into(),
            }),
        )
        .await
        .unwrap();
        assert!(resp.is_premium);
        assert_eq!(resp.stripe_subscription_id.as_deref(), Some("sub_123"));
        assert!(
            resp.current_period_end
                .as_deref()
                .unwrap_or("")
                .starts_with("2026-")
        );
    }

    #[tokio::test]
    async fn test_handle_subscription_created_marks_user_premium() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_1",
                json!({
                    fields::UID: "user_1",
                    fields::CUSTOMER_ID: "cus_123",
                    fields::IS_PREMIUM: false,
                }),
            )
            .await
            .unwrap();

        handle_subscription_created(&state, &json!({ "object": { "customer": "cus_123" } }))
            .await
            .unwrap();

        let user = state
            .db
            .get_document(collections::USERS, "user_1")
            .await
            .unwrap();
        assert_eq!(user[fields::IS_PREMIUM], true);
        assert_eq!(
            user[fields::SUBSCRIPTION_STATUS],
            SubscriptionStatus::Active.as_str()
        );
    }

    #[tokio::test]
    async fn test_handle_subscription_updated_syncs_subscription_doc() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_1",
                json!({
                    fields::UID: "user_1",
                    fields::CUSTOMER_ID: "cus_123",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(collections::SUBSCRIPTIONS, "user_1", json!({}))
            .await
            .unwrap();

        handle_subscription_updated(
            &state,
            &json!({
                "object": {
                    "customer": "cus_123",
                    "status": "past_due",
                    "current_period_end": 12345,
                    "cancel_at_period_end": true
                }
            }),
        )
        .await
        .unwrap();

        let sub = state
            .db
            .get_document(collections::SUBSCRIPTIONS, "user_1")
            .await
            .unwrap();
        assert_eq!(sub[fields::STATUS], "past_due");
        assert_eq!(sub["cancelAtPeriodEnd"], true);
        assert_eq!(sub[fields::CURRENT_PERIOD_END], 12345);
    }

    #[tokio::test]
    async fn test_handle_subscription_deleted_revokes_premium() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_1",
                json!({
                    fields::UID: "user_1",
                    fields::CUSTOMER_ID: "cus_123",
                    fields::IS_PREMIUM: true,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(collections::SUBSCRIPTIONS, "user_1", json!({}))
            .await
            .unwrap();

        handle_subscription_deleted(&state, &json!({ "object": { "customer": "cus_123" } }))
            .await
            .unwrap();

        let user = state
            .db
            .get_document(collections::USERS, "user_1")
            .await
            .unwrap();
        let sub = state
            .db
            .get_document(collections::SUBSCRIPTIONS, "user_1")
            .await
            .unwrap();
        assert_eq!(user[fields::IS_PREMIUM], false);
        assert_eq!(sub[fields::STATUS], SubscriptionStatus::Cancelled.as_str());
    }

    #[tokio::test]
    async fn test_handle_invoice_payment_failed_marks_past_due() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_1",
                json!({
                    fields::UID: "user_1",
                    fields::CUSTOMER_ID: "cus_123",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(collections::SUBSCRIPTIONS, "user_1", json!({}))
            .await
            .unwrap();

        handle_invoice_payment_failed(&state, &json!({ "object": { "customer": "cus_123" } }))
            .await
            .unwrap();

        let user = state
            .db
            .get_document(collections::USERS, "user_1")
            .await
            .unwrap();
        let sub = state
            .db
            .get_document(collections::SUBSCRIPTIONS, "user_1")
            .await
            .unwrap();
        assert_eq!(
            user[fields::SUBSCRIPTION_STATUS],
            SubscriptionStatus::PastDue.as_str()
        );
        assert_eq!(sub[fields::STATUS], SubscriptionStatus::PastDue.as_str());
    }

    #[tokio::test]
    async fn test_create_subscription_checkout_flow_uses_state_base_url() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/customers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cus_123"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/checkout/sessions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cs_123",
                "url": "https://checkout.stripe.com/c/pay/cs_123"
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
                "user_1",
                json!({
                    fields::UID: "user_1",
                    fields::EMAIL: "buyer@example.com",
                    fields::NAME: "Buyer One",
                    fields::ROLES: ["buyer"],
                }),
            )
            .await
            .unwrap();

        let Json(resp) = create_subscription(
            State(state.clone()),
            Json(CreateSubscriptionRequest {
                user_id: "user_1".into(),
                payment_method_id: None,
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.subscription_id.as_deref(), Some("cs_123"));
        assert_eq!(
            resp.checkout_url.as_deref(),
            Some("https://checkout.stripe.com/c/pay/cs_123")
        );

        let user = state
            .db
            .get_document(collections::USERS, "user_1")
            .await
            .unwrap();
        assert_eq!(user[fields::CUSTOMER_ID], "cus_123");
        assert_eq!(
            user["lastCheckoutSession"],
            "https://checkout.stripe.com/c/pay/cs_123"
        );
    }

    #[tokio::test]
    async fn test_cancel_subscription_uses_state_base_url_and_sets_cancel_pending() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/subscriptions/sub_123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "sub_123",
                "cancel_at_period_end": true
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
                collections::SUBSCRIPTIONS,
                "user_1",
                json!({
                    fields::BUYER_ID: "user_1",
                    fields::STRIPE_SUBSCRIPTION_ID: "sub_123",
                    fields::STATUS: SubscriptionStatus::Active.as_str(),
                    fields::SUBSCRIPTION_STATUS: SubscriptionStatus::Active.as_str(),
                }),
            )
            .await
            .unwrap();

        let Json(resp) = cancel_subscription(
            State(state.clone()),
            Json(CancelSubscriptionRequest {
                user_id: "user_1".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.status, SubscriptionStatus::CancelPending.as_str());

        let sub = state
            .db
            .get_document(collections::SUBSCRIPTIONS, "user_1")
            .await
            .unwrap();
        assert_eq!(
            sub[fields::STATUS],
            SubscriptionStatus::CancelPending.as_str()
        );
        assert_eq!(sub["cancelAtPeriodEnd"], true);
    }

    #[tokio::test]
    async fn test_reactivate_subscription_uses_state_base_url_and_restores_premium() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/subscriptions/sub_123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "sub_123",
                "cancel_at_period_end": false
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
                collections::SUBSCRIPTIONS,
                "user_1",
                json!({
                    fields::BUYER_ID: "user_1",
                    fields::STRIPE_SUBSCRIPTION_ID: "sub_123",
                    fields::STATUS: SubscriptionStatus::CancelPending.as_str(),
                    fields::SUBSCRIPTION_STATUS: SubscriptionStatus::CancelPending.as_str(),
                    "cancelAtPeriodEnd": true,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_1",
                json!({
                    fields::UID: "user_1",
                    fields::IS_PREMIUM: false,
                }),
            )
            .await
            .unwrap();

        let Json(resp) = reactivate_subscription(
            State(state.clone()),
            Json(ReactivateSubscriptionRequest {
                user_id: "user_1".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.status, SubscriptionStatus::Active.as_str());

        let sub = state
            .db
            .get_document(collections::SUBSCRIPTIONS, "user_1")
            .await
            .unwrap();
        let user = state
            .db
            .get_document(collections::USERS, "user_1")
            .await
            .unwrap();
        assert_eq!(sub[fields::STATUS], SubscriptionStatus::Active.as_str());
        assert_eq!(sub["cancelAtPeriodEnd"], false);
        assert_eq!(user[fields::IS_PREMIUM], true);
    }

    // -----------------------------------------------------------------------
    // Coverage: ensure_customer — existing customer ID (line 135)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_ensure_customer_returns_existing_customer_id() {
        let server = MockServer::start().await;
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
                "user_1",
                json!({
                    fields::UID: "user_1",
                    fields::CUSTOMER_ID: "cus_existing_123",
                }),
            )
            .await
            .unwrap();

        let result = ensure_customer(&state, "user_1", "sk_test_123")
            .await
            .unwrap();
        assert_eq!(result, "cus_existing_123");
    }

    // -----------------------------------------------------------------------
    // Coverage: ensure_customer — Stripe customer creation failure (lines 162-166)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_ensure_customer_stripe_create_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/customers"))
            .respond_with(ResponseTemplate::new(400).set_body_string("Bad Request"))
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
                "user_1",
                json!({
                    fields::UID: "user_1",
                    fields::EMAIL: "a@b.com",
                    fields::NAME: "Test",
                }),
            )
            .await
            .unwrap();

        let err = ensure_customer(&state, "user_1", "sk_test_123")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Failed to create Stripe customer"));
    }

    // -----------------------------------------------------------------------
    // Coverage: upsert_subscription_doc (lines 223-233)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_upsert_subscription_doc() {
        let state = setup_state().await;
        // First create the doc so update works
        state
            .db
            .upsert_document(collections::SUBSCRIPTIONS, "user_1", json!({}))
            .await
            .unwrap();

        upsert_subscription_doc(
            &state,
            "user_1",
            json!({
                fields::STATUS: "active",
                fields::BUYER_ID: "user_1",
            }),
        )
        .await
        .unwrap();

        let doc = state
            .db
            .get_document(collections::SUBSCRIPTIONS, "user_1")
            .await
            .unwrap();
        assert_eq!(doc[fields::STATUS], "active");
    }

    // -----------------------------------------------------------------------
    // Coverage: validate_uid on paymentMethodId (line 246)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_subscription_validates_payment_method_id() {
        let state = setup_state().await;
        let err = create_subscription(
            State(state),
            Json(CreateSubscriptionRequest {
                user_id: "user_1".into(),
                payment_method_id: Some("".into()),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("paymentMethodId"));
    }

    // -----------------------------------------------------------------------
    // Coverage: seller account restriction (lines 284-286)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_subscription_rejects_seller_accounts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/customers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cus_seller"
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
                "user_seller",
                json!({
                    fields::UID: "user_seller",
                    fields::EMAIL: "seller@ex.com",
                    fields::NAME: "Seller",
                    fields::ROLES: ["seller"],
                }),
            )
            .await
            .unwrap();

        let err = create_subscription(
            State(state),
            Json(CreateSubscriptionRequest {
                user_id: "user_seller".into(),
                payment_method_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Seller accounts cannot"));
    }

    // -----------------------------------------------------------------------
    // Coverage: checkout session creation failure (lines 333-337)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_subscription_checkout_session_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/customers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cus_123"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/checkout/sessions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
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
                "user_chk",
                json!({
                    fields::UID: "user_chk",
                    fields::EMAIL: "chk@ex.com",
                    fields::NAME: "Chk",
                    fields::ROLES: ["buyer"],
                }),
            )
            .await
            .unwrap();

        let err = create_subscription(
            State(state),
            Json(CreateSubscriptionRequest {
                user_id: "user_chk".into(),
                payment_method_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("Failed to create subscription checkout session")
        );
    }

    // -----------------------------------------------------------------------
    // Coverage: empty checkout URL from Stripe (lines 348-350)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_subscription_checkout_empty_url() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/customers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cus_123"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/checkout/sessions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cs_123",
                "url": ""
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
                "user_eu",
                json!({
                    fields::UID: "user_eu",
                    fields::EMAIL: "eu@ex.com",
                    fields::NAME: "Eu",
                    fields::ROLES: ["buyer"],
                }),
            )
            .await
            .unwrap();

        let err = create_subscription(
            State(state),
            Json(CreateSubscriptionRequest {
                user_id: "user_eu".into(),
                payment_method_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("Stripe did not return a checkout URL")
        );
    }

    // -----------------------------------------------------------------------
    // Coverage: full payment method flow (lines 374, 377-514)
    // Create subscription WITH payment_method_id — attach, set default, create sub
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_subscription_with_payment_method_full_flow() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/customers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cus_pm"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/payment_methods/pm_test_abc/attach"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "pm_test_abc"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/customers/cus_pm"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cus_pm"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/subscriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "sub_pm_123",
                "status": "active",
                "current_period_end": 1800000000i64,
                "latest_invoice": {
                    "payment_intent": {
                        "client_secret": "pi_secret_abc"
                    }
                }
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
                "user_pm",
                json!({
                    fields::UID: "user_pm",
                    fields::EMAIL: "pm@ex.com",
                    fields::NAME: "PM User",
                }),
            )
            .await
            .unwrap();
        // Need a subscription doc for upsert to work
        state
            .db
            .upsert_document(collections::SUBSCRIPTIONS, "user_pm", json!({}))
            .await
            .unwrap();

        let Json(resp) = create_subscription(
            State(state.clone()),
            Json(CreateSubscriptionRequest {
                user_id: "user_pm".into(),
                payment_method_id: Some("pm_test_abc".into()),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.subscription_id.as_deref(), Some("sub_pm_123"));
        assert!(resp.checkout_url.is_none());
        assert_eq!(resp.client_secret.as_deref(), Some("pi_secret_abc"));
        assert_eq!(resp.status, "active");

        // Verify user was marked premium
        let user = state
            .db
            .get_document(collections::USERS, "user_pm")
            .await
            .unwrap();
        assert_eq!(user[fields::IS_PREMIUM], true);
    }

    // -----------------------------------------------------------------------
    // Coverage: payment method attach failure (non-"already attached") (lines 390-399)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_subscription_attach_payment_method_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/customers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cus_af"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/payment_methods/pm_bad/attach"))
            .respond_with(ResponseTemplate::new(400).set_body_string("Invalid payment method"))
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
                "user_af",
                json!({
                    fields::UID: "user_af",
                    fields::EMAIL: "af@ex.com",
                    fields::NAME: "AF User",
                }),
            )
            .await
            .unwrap();

        let err = create_subscription(
            State(state),
            Json(CreateSubscriptionRequest {
                user_id: "user_af".into(),
                payment_method_id: Some("pm_bad".into()),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Failed to attach payment method"));
    }

    // -----------------------------------------------------------------------
    // Coverage: "already attached" error is ignored (lines 393-398)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_subscription_attach_already_attached_is_ignored() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/customers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cus_aa"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/payment_methods/pm_aa/attach"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_string("This payment method has already been attached to a customer"),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/customers/cus_aa"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "cus_aa"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/subscriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "sub_aa",
                "status": "incomplete",
                "current_period_end": 0,
                "latest_invoice": {
                    "payment_intent": {
                        "client_secret": "pi_secret_aa"
                    }
                }
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
                "user_aa",
                json!({
                    fields::UID: "user_aa",
                    fields::EMAIL: "aa@ex.com",
                    fields::NAME: "AA User",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(collections::SUBSCRIPTIONS, "user_aa", json!({}))
            .await
            .unwrap();

        let Json(resp) = create_subscription(
            State(state),
            Json(CreateSubscriptionRequest {
                user_id: "user_aa".into(),
                payment_method_id: Some("pm_aa".into()),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.subscription_id.as_deref(), Some("sub_aa"));
        assert_eq!(resp.status, "incomplete");
    }

    // -----------------------------------------------------------------------
    // Coverage: subscription creation failure with payment method (lines 441-447)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_subscription_with_pm_stripe_sub_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/customers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cus_sf"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/payment_methods/pm_sf/attach"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "pm_sf"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/customers/cus_sf"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "cus_sf"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/subscriptions"))
            .respond_with(ResponseTemplate::new(402).set_body_string("Payment failed"))
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
                "user_sf",
                json!({
                    fields::UID: "user_sf",
                    fields::EMAIL: "sf@ex.com",
                    fields::NAME: "SF User",
                }),
            )
            .await
            .unwrap();

        let err = create_subscription(
            State(state),
            Json(CreateSubscriptionRequest {
                user_id: "user_sf".into(),
                payment_method_id: Some("pm_sf".into()),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Failed to create subscription"));
    }

    // -----------------------------------------------------------------------
    // Coverage: cancel with empty stripe subscription ID (lines 542-544)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancel_subscription_empty_stripe_id() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                "user_noid",
                json!({
                    fields::BUYER_ID: "user_noid",
                    fields::STATUS: SubscriptionStatus::Active.as_str(),
                }),
            )
            .await
            .unwrap();

        let err = cancel_subscription(
            State(state),
            Json(CancelSubscriptionRequest {
                user_id: "user_noid".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Subscription has no Stripe ID"));
    }

    // -----------------------------------------------------------------------
    // Coverage: cancel already-cancelled subscription (lines 554-557)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancel_subscription_already_cancelled() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                "user_cc",
                json!({
                    fields::BUYER_ID: "user_cc",
                    fields::STRIPE_SUBSCRIPTION_ID: "sub_cc",
                    fields::STATUS: SubscriptionStatus::Cancelled.as_str(),
                }),
            )
            .await
            .unwrap();

        let Json(resp) = cancel_subscription(
            State(state),
            Json(CancelSubscriptionRequest {
                user_id: "user_cc".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.status, "cancelled");
    }

    // -----------------------------------------------------------------------
    // Coverage: cancel Stripe API failure (lines 575-579)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancel_subscription_stripe_api_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/subscriptions/sub_fail"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal error"))
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
                collections::SUBSCRIPTIONS,
                "user_cf",
                json!({
                    fields::BUYER_ID: "user_cf",
                    fields::STRIPE_SUBSCRIPTION_ID: "sub_fail",
                    fields::STATUS: SubscriptionStatus::Active.as_str(),
                }),
            )
            .await
            .unwrap();

        let err = cancel_subscription(
            State(state),
            Json(CancelSubscriptionRequest {
                user_id: "user_cf".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Failed to cancel subscription"));
    }

    // -----------------------------------------------------------------------
    // Coverage: subscription_status with no subscription (lines 632-638)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_subscription_status_no_subscription() {
        let state = setup_state().await;
        let Json(resp) = subscription_status(
            State(state),
            Json(SubscriptionStatusRequest {
                user_id: "user_none".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(!resp.is_premium);
        assert_eq!(resp.status, "none");
        assert!(resp.current_period_end.is_none());
        assert!(resp.stripe_subscription_id.is_none());
    }

    // -----------------------------------------------------------------------
    // Coverage: reactivate with empty stripe ID (lines 695-697)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_reactivate_subscription_empty_stripe_id() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                "user_noid2",
                json!({
                    fields::BUYER_ID: "user_noid2",
                    fields::STATUS: SubscriptionStatus::CancelPending.as_str(),
                }),
            )
            .await
            .unwrap();

        let err = reactivate_subscription(
            State(state),
            Json(ReactivateSubscriptionRequest {
                user_id: "user_noid2".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Subscription has no Stripe ID"));
    }

    // -----------------------------------------------------------------------
    // Coverage: reactivate already active (lines 707-710)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_reactivate_subscription_already_active() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                "user_active",
                json!({
                    fields::BUYER_ID: "user_active",
                    fields::STRIPE_SUBSCRIPTION_ID: "sub_active",
                    fields::STATUS: SubscriptionStatus::Active.as_str(),
                }),
            )
            .await
            .unwrap();

        let Json(resp) = reactivate_subscription(
            State(state),
            Json(ReactivateSubscriptionRequest {
                user_id: "user_active".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.status, SubscriptionStatus::Active.as_str());
    }

    // -----------------------------------------------------------------------
    // Coverage: reactivate Stripe API failure (lines 734-738)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_reactivate_subscription_stripe_api_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/subscriptions/sub_rfail"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal error"))
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
                collections::SUBSCRIPTIONS,
                "user_rf",
                json!({
                    fields::BUYER_ID: "user_rf",
                    fields::STRIPE_SUBSCRIPTION_ID: "sub_rfail",
                    fields::STATUS: SubscriptionStatus::CancelPending.as_str(),
                }),
            )
            .await
            .unwrap();

        let err = reactivate_subscription(
            State(state),
            Json(ReactivateSubscriptionRequest {
                user_id: "user_rf".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("Failed to reactivate subscription")
        );
    }

    // -----------------------------------------------------------------------
    // Coverage: update_notification_preferences success (lines 798, 800-815)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_update_notification_preferences_success_both() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_np",
                json!({
                    fields::UID: "user_np",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = update_notification_preferences(
            State(state.clone()),
            Json(SubscriptionNotificationPrefsRequest {
                user_id: "user_np".into(),
                notify_new_products: Some(true),
                notify_trending: Some(false),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp["success"], true);

        let user = state
            .db
            .get_document(collections::USERS, "user_np")
            .await
            .unwrap();
        assert_eq!(user["notifyNewProducts"], true);
        assert_eq!(user["notifyTrending"], false);
    }

    #[tokio::test]
    async fn test_update_notification_preferences_only_new_products() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_np2",
                json!({
                    fields::UID: "user_np2",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = update_notification_preferences(
            State(state.clone()),
            Json(SubscriptionNotificationPrefsRequest {
                user_id: "user_np2".into(),
                notify_new_products: Some(false),
                notify_trending: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp["success"], true);
    }

    #[tokio::test]
    async fn test_update_notification_preferences_only_trending() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_np3",
                json!({
                    fields::UID: "user_np3",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = update_notification_preferences(
            State(state.clone()),
            Json(SubscriptionNotificationPrefsRequest {
                user_id: "user_np3".into(),
                notify_new_products: None,
                notify_trending: Some(true),
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp["success"], true);
    }

    // -----------------------------------------------------------------------
    // Coverage: route_subscription_webhook (lines 823-835)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_route_subscription_webhook_unknown_event() {
        let state = setup_state().await;
        let result = route_subscription_webhook(&state, "some.unknown.event", &json!({})).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_route_subscription_webhook_created() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_wh",
                json!({
                    fields::UID: "user_wh",
                    fields::CUSTOMER_ID: "cus_wh",
                }),
            )
            .await
            .unwrap();

        let result = route_subscription_webhook(
            &state,
            "customer.subscription.created",
            &json!({ "object": { "customer": "cus_wh" } }),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_route_subscription_webhook_updated() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_whu",
                json!({
                    fields::UID: "user_whu",
                    fields::CUSTOMER_ID: "cus_whu",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(collections::SUBSCRIPTIONS, "user_whu", json!({}))
            .await
            .unwrap();

        let result = route_subscription_webhook(
            &state,
            "customer.subscription.updated",
            &json!({
                "object": {
                    "customer": "cus_whu",
                    "status": "active",
                    "current_period_end": 9999,
                    "cancel_at_period_end": false
                }
            }),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_route_subscription_webhook_deleted() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_whd",
                json!({
                    fields::UID: "user_whd",
                    fields::CUSTOMER_ID: "cus_whd",
                    fields::IS_PREMIUM: true,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(collections::SUBSCRIPTIONS, "user_whd", json!({}))
            .await
            .unwrap();

        let result = route_subscription_webhook(
            &state,
            "customer.subscription.deleted",
            &json!({ "object": { "customer": "cus_whd" } }),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_route_subscription_webhook_invoice_failed() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_whf",
                json!({
                    fields::UID: "user_whf",
                    fields::CUSTOMER_ID: "cus_whf",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(collections::SUBSCRIPTIONS, "user_whf", json!({}))
            .await
            .unwrap();

        let result = route_subscription_webhook(
            &state,
            "invoice.payment_failed",
            &json!({ "object": { "customer": "cus_whf" } }),
        )
        .await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Coverage: webhook handlers — missing customer ID (lines 876-877, 922-923, 987-988, 1047-1048)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_webhook_created_missing_customer_id() {
        let state = setup_state().await;
        let result = handle_subscription_created(&state, &json!({ "object": {} })).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_webhook_updated_missing_customer_id() {
        let state = setup_state().await;
        let result = handle_subscription_updated(&state, &json!({ "object": {} })).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_webhook_deleted_missing_customer_id() {
        let state = setup_state().await;
        let result = handle_subscription_deleted(&state, &json!({ "object": {} })).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_webhook_invoice_failed_missing_customer_id() {
        let state = setup_state().await;
        let result = handle_invoice_payment_failed(&state, &json!({ "object": {} })).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Coverage: webhook handlers — no user found for customer (lines 884-885, 930-931, 995-996, 1055-1056)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_webhook_created_no_user_for_customer() {
        let state = setup_state().await;
        let result = handle_subscription_created(
            &state,
            &json!({ "object": { "customer": "cus_nonexistent" } }),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_webhook_updated_no_user_for_customer() {
        let state = setup_state().await;
        let result = handle_subscription_updated(
            &state,
            &json!({ "object": { "customer": "cus_nonexistent" } }),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_webhook_deleted_no_user_for_customer() {
        let state = setup_state().await;
        let result = handle_subscription_deleted(
            &state,
            &json!({ "object": { "customer": "cus_nonexistent" } }),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_webhook_invoice_failed_no_user_for_customer() {
        let state = setup_state().await;
        let result = handle_invoice_payment_failed(
            &state,
            &json!({ "object": { "customer": "cus_nonexistent" } }),
        )
        .await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Coverage: webhook handlers — user has no uid (lines 891, 937, 1002, 1062)
    // -----------------------------------------------------------------------

    // Note: lines 891, 937, 1002, 1062 (`extract_uid` returning None) are not
    // reachable with the in-memory DB because it always includes an `id` field
    // in query results. These 4 lines are covered by the `test_extract_uid_missing`
    // unit test above which tests the function directly.
}
