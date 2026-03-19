//! Stripe Connect handlers for seller onboarding.
//! Ported from: functions/handlers/payment_stripe.py (connect endpoints)

use axum::{Extension, Json, Router, extract::State, routing::post};
use ob_auth::middleware::AuthContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{error, info};

use crate::HandlersState;
use crate::shared::auth::resolve_self_user_id;
use crate::shared::schema::{app_config, collections, fields};
use crate::shared::validation::{validate_email, validate_uid};

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAccountRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    pub country: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAccountResponse {
    pub success: bool,
    pub account_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLinkRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLinkResponse {
    pub success: bool,
    pub url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatusRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatusResponse {
    pub success: bool,
    pub account_id: String,
    pub charges_enabled: bool,
    pub payouts_enabled: bool,
    pub onboarding_completed: bool,
    pub details_submitted: bool,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/connect/create-account", post(create_account))
        .route("/api/connect/account-link", post(create_account_link))
        .route("/api/connect/status", post(get_account_status))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/connect/create-account — Create a Stripe Express connected account.
async fn create_account(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<CreateAccountRequest>,
) -> Result<Json<CreateAccountResponse>, ob_core::Error> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;

    // Check if user already has a Stripe account
    let user = state
        .db
        .get_document(collections::USERS, &user_id)
        .await
        .map_err(|_| ob_core::Error::NotFound(format!("User {} not found", user_id)))?;

    let existing_account = user
        .get(fields::STRIPE_ACCOUNT_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if !existing_account.is_empty() {
        return Ok(Json(CreateAccountResponse {
            success: true,
            account_id: existing_account.to_string(),
        }));
    }

    let email = req
        .email
        .clone()
        .or_else(|| {
            user.get(fields::EMAIL)
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
        })
        .ok_or_else(|| ob_core::Error::Validation("User email is required".into()))?;
    validate_email(&email)?;

    // Create Stripe Express account
    let stripe_key = state.config.require_secret("stripe_secret_key")?;
    let country = req.country.as_deref().unwrap_or("CA");

    let resp = state
        .http_client
        .post(format!("{}/accounts", state.stripe_base_url))
        .basic_auth(stripe_key, None::<&str>)
        .form(&[
            ("type", "express"),
            ("country", country),
            ("email", email.as_str()),
            ("capabilities[card_payments][requested]", "true"),
            ("capabilities[transfers][requested]", "true"),
            ("business_type", "individual"),
            ("metadata[user_id]", user_id.as_str()),
            ("metadata[platform]", app_config::PLATFORM_NAME),
        ])
        .send()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Stripe API error: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        error!(error = %body, "Failed to create Stripe Connect account");
        return Err(ob_core::Error::Internal(
            "Failed to create Stripe Connect account".into(),
        ));
    }

    let account: Value = resp
        .json()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Parse error: {e}")))?;

    let account_id = account["id"]
        .as_str()
        .ok_or_else(|| ob_core::Error::Internal("Missing account ID from Stripe".into()))?;

    // Store account ID on user document
    let now = chrono::Utc::now().to_rfc3339();
    let update = serde_json::json!({
        fields::STRIPE_ACCOUNT_ID: account_id,
        fields::PAYOUTS_ENABLED: false,
        fields::CHARGES_ENABLED: false,
        fields::ONBOARDING_COMPLETED: false,
        fields::UPDATED_AT: now,
    });

    state
        .db
        .update_document(collections::USERS, &user_id, update)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to update user: {e}")))?;

    info!(
        user_id = %user_id,
        account_id = %account_id,
        "Stripe Connect account created"
    );

    Ok(Json(CreateAccountResponse {
        success: true,
        account_id: account_id.to_string(),
    }))
}

/// POST /api/connect/account-link — Create an onboarding link for a connected account.
async fn create_account_link(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<AccountLinkRequest>,
) -> Result<Json<AccountLinkResponse>, ob_core::Error> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;

    // Verify user owns this account
    let user = state
        .db
        .get_document(collections::USERS, &user_id)
        .await
        .map_err(|_| ob_core::Error::NotFound(format!("User {} not found", user_id)))?;

    let stored_account = user
        .get(fields::STRIPE_ACCOUNT_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let account_id = req.account_id.as_deref().unwrap_or(stored_account);
    if account_id.is_empty() {
        return Err(ob_core::Error::Validation(
            "No Stripe account found for this user".into(),
        ));
    }
    validate_uid("accountId", account_id)?;

    if stored_account != account_id {
        return Err(ob_core::Error::Forbidden(
            "Account ID does not match user's Stripe account".into(),
        ));
    }

    let stripe_key = state.config.require_secret("stripe_secret_key")?;

    let refresh_url = format!(
        "{}{}",
        app_config::SITE_URL,
        app_config::SELLER_REFRESH_PATH
    );
    let return_url = format!("{}{}", app_config::SITE_URL, app_config::SELLER_RETURN_PATH);

    let resp = state
        .http_client
        .post(format!("{}/account_links", state.stripe_base_url))
        .basic_auth(stripe_key, None::<&str>)
        .form(&[
            ("account", account_id),
            ("refresh_url", refresh_url.as_str()),
            ("return_url", return_url.as_str()),
            ("type", "account_onboarding"),
        ])
        .send()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Stripe API error: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        error!(error = %body, "Failed to create account link");
        return Err(ob_core::Error::Internal(
            "Failed to create onboarding link".into(),
        ));
    }

    let link: Value = resp
        .json()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Parse error: {e}")))?;

    let url = link["url"]
        .as_str()
        .ok_or_else(|| ob_core::Error::Internal("Missing URL from Stripe account link".into()))?;

    info!(
        user_id = %user_id,
        account_id = %account_id,
        "Account onboarding link created"
    );

    Ok(Json(AccountLinkResponse {
        success: true,
        url: url.to_string(),
    }))
}

/// POST /api/connect/status — Get the status of a connected account.
async fn get_account_status(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<AccountStatusRequest>,
) -> Result<Json<AccountStatusResponse>, ob_core::Error> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;

    // Verify user owns this account
    let user = state
        .db
        .get_document(collections::USERS, &user_id)
        .await
        .map_err(|_| ob_core::Error::NotFound(format!("User {} not found", user_id)))?;

    let stored_account = user
        .get(fields::STRIPE_ACCOUNT_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let account_id = req.account_id.as_deref().unwrap_or(stored_account);
    if account_id.is_empty() {
        return Err(ob_core::Error::Validation(
            "No Stripe account found for this user".into(),
        ));
    }
    validate_uid("accountId", account_id)?;

    if stored_account != account_id {
        return Err(ob_core::Error::Forbidden(
            "Account ID does not match user's Stripe account".into(),
        ));
    }

    // Fetch account from Stripe
    let stripe_key = state.config.require_secret("stripe_secret_key")?;
    let url = format!("{}/accounts/{account_id}", state.stripe_base_url);

    let resp = state
        .http_client
        .get(&url)
        .basic_auth(stripe_key, None::<&str>)
        .send()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Stripe API error: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        error!(error = %body, "Failed to fetch Stripe account");
        return Err(ob_core::Error::Internal(
            "Failed to fetch account status".into(),
        ));
    }

    let account: Value = resp
        .json()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Parse error: {e}")))?;

    let charges_enabled = account["charges_enabled"].as_bool().unwrap_or(false);
    let payouts_enabled = account["payouts_enabled"].as_bool().unwrap_or(false);
    let details_submitted = account["details_submitted"].as_bool().unwrap_or(false);
    let onboarding_completed = charges_enabled && payouts_enabled && details_submitted;

    // Sync status back to our DB
    let now = chrono::Utc::now().to_rfc3339();
    let update = serde_json::json!({
        fields::CHARGES_ENABLED: charges_enabled,
        fields::PAYOUTS_ENABLED: payouts_enabled,
        fields::ONBOARDING_COMPLETED: onboarding_completed,
        fields::UPDATED_AT: now,
    });

    state
        .db
        .update_document(collections::USERS, &user_id, update)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to sync account status: {e}")))?;

    info!(
        user_id = %user_id,
        account_id = %account_id,
        charges = charges_enabled,
        payouts = payouts_enabled,
        onboarded = onboarding_completed,
        "Account status fetched"
    );

    Ok(Json(AccountStatusResponse {
        success: true,
        account_id: account_id.to_string(),
        charges_enabled,
        payouts_enabled,
        onboarding_completed,
        details_submitted,
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
    fn test_create_account_request_deser() {
        let json = r#"{"userId": "u1", "email": "test@example.com"}"#;
        let req: CreateAccountRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.user_id, Some("u1".to_string()));
        assert_eq!(req.email.as_deref(), Some("test@example.com"));
        assert!(req.country.is_none());
    }

    #[test]
    fn test_create_account_request_with_country() {
        let json = r#"{"userId": "u1", "email": "a@b.com", "country": "US"}"#;
        let req: CreateAccountRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.country.as_deref(), Some("US"));
    }

    #[test]
    fn test_create_account_response_ser() {
        let resp = CreateAccountResponse {
            success: true,
            account_id: "acct_123".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["accountId"], "acct_123");
    }

    #[test]
    fn test_account_link_request_deser() {
        let json = r#"{"userId": "u1", "accountId": "acct_xyz"}"#;
        let req: AccountLinkRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.account_id.as_deref(), Some("acct_xyz"));
    }

    #[test]
    fn test_account_link_response_ser() {
        let resp = AccountLinkResponse {
            success: true,
            url: "https://connect.stripe.com/setup/e/xxx".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["success"], true);
        assert!(json["url"].as_str().unwrap().starts_with("https://"));
    }

    #[test]
    fn test_account_status_response_ser() {
        let resp = AccountStatusResponse {
            success: true,
            account_id: "acct_1".to_string(),
            charges_enabled: true,
            payouts_enabled: true,
            onboarding_completed: true,
            details_submitted: true,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["chargesEnabled"], true);
        assert_eq!(json["payoutsEnabled"], true);
        assert_eq!(json["onboardingCompleted"], true);
        assert_eq!(json["detailsSubmitted"], true);
    }

    #[test]
    fn test_account_status_request_missing_field() {
        let json = r#"{"userId": "u1"}"#;
        let result: AccountStatusRequest = serde_json::from_str(json).unwrap();
        assert_eq!(result.user_id, Some("u1".to_string()));
        assert!(result.account_id.is_none());
    }

    #[tokio::test]
    async fn test_create_account_returns_existing_account_without_stripe_call() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({
                    fields::STRIPE_ACCOUNT_ID: "acct_existing",
                    fields::EMAIL: "seller@example.com",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = create_account(
            State(state),
            Extension(auth("test")),
            Json(CreateAccountRequest {
                user_id: Some("seller_1".to_string()),
                email: None,
                country: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.account_id, "acct_existing");
    }

    #[tokio::test]
    async fn test_create_account_rejects_missing_and_invalid_email() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(collections::USERS, "seller_1", json!({}))
            .await
            .unwrap();

        let missing_err = create_account(
            State(state.clone()),
            Extension(auth("test")),
            Json(CreateAccountRequest {
                user_id: Some("seller_1".to_string()),
                email: None,
                country: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(missing_err.to_string().contains("User email is required"));

        let invalid_err = create_account(
            State(state),
            Extension(auth("test")),
            Json(CreateAccountRequest {
                user_id: Some("seller_1".to_string()),
                email: Some("not-an-email".into()),
                country: Some("CA".into()),
            }),
        )
        .await
        .unwrap_err();
        assert!(invalid_err.to_string().contains("Invalid email address"));
    }

    #[tokio::test]
    async fn test_create_account_success_updates_user_and_uses_mock_server() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/accounts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "acct_new_1" })))
            .mount(&server)
            .await;

        let mut state = setup_state().await;
        state.stripe_base_url = server.uri();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_2",
                json!({
                    fields::EMAIL: "seller2@example.com",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = create_account(
            State(state.clone()),
            Extension(auth("test")),
            Json(CreateAccountRequest {
                user_id: Some("seller_2".to_string()),
                email: None,
                country: Some("US".into()),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.account_id, "acct_new_1");

        let user = state
            .db
            .get_document(collections::USERS, "seller_2")
            .await
            .unwrap();
        assert_eq!(
            user.get(fields::STRIPE_ACCOUNT_ID).and_then(|v| v.as_str()),
            Some("acct_new_1")
        );
        assert_eq!(
            user.get(fields::CHARGES_ENABLED).and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            user.get(fields::PAYOUTS_ENABLED).and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[tokio::test]
    async fn test_create_account_link_rejects_missing_or_mismatched_account() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(collections::USERS, "seller_3", json!({}))
            .await
            .unwrap();

        let missing_err = create_account_link(
            State(state.clone()),
            Extension(auth("test")),
            Json(AccountLinkRequest {
                user_id: Some("seller_3".to_string()),
                account_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(missing_err.to_string().contains("No Stripe account found"));

        state
            .db
            .update_document(
                collections::USERS,
                "seller_3",
                json!({ fields::STRIPE_ACCOUNT_ID: "acct_stored" }),
            )
            .await
            .unwrap();

        let mismatch_err = create_account_link(
            State(state),
            Extension(auth("test")),
            Json(AccountLinkRequest {
                user_id: Some("seller_3".to_string()),
                account_id: Some("acct_other".into()),
            }),
        )
        .await
        .unwrap_err();
        assert!(mismatch_err.to_string().contains("does not match"));
    }

    #[tokio::test]
    async fn test_create_account_link_success_uses_mock_server() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/account_links"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "url": "https://connect.stripe.test/setup/abc" })),
            )
            .mount(&server)
            .await;

        let mut state = setup_state().await;
        state.stripe_base_url = server.uri();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_4",
                json!({ fields::STRIPE_ACCOUNT_ID: "acct_link_1" }),
            )
            .await
            .unwrap();

        let Json(resp) = create_account_link(
            State(state),
            Extension(auth("test")),
            Json(AccountLinkRequest {
                user_id: Some("seller_4".to_string()),
                account_id: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.url, "https://connect.stripe.test/setup/abc");
    }

    #[tokio::test]
    async fn test_get_account_status_rejects_missing_or_mismatched_account() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(collections::USERS, "seller_5", json!({}))
            .await
            .unwrap();

        let missing_err = get_account_status(
            State(state.clone()),
            Extension(auth("test")),
            Json(AccountStatusRequest {
                user_id: Some("seller_5".to_string()),
                account_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(missing_err.to_string().contains("No Stripe account found"));

        state
            .db
            .update_document(
                collections::USERS,
                "seller_5",
                json!({ fields::STRIPE_ACCOUNT_ID: "acct_ok" }),
            )
            .await
            .unwrap();

        let mismatch_err = get_account_status(
            State(state),
            Extension(auth("test")),
            Json(AccountStatusRequest {
                user_id: Some("seller_5".to_string()),
                account_id: Some("acct_other".into()),
            }),
        )
        .await
        .unwrap_err();
        assert!(mismatch_err.to_string().contains("does not match"));
    }

    #[tokio::test]
    async fn test_get_account_status_success_updates_user_flags() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/accounts/acct_status_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "charges_enabled": true,
                "payouts_enabled": true,
                "details_submitted": true
            })))
            .mount(&server)
            .await;

        let mut state = setup_state().await;
        state.stripe_base_url = server.uri();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_6",
                json!({ fields::STRIPE_ACCOUNT_ID: "acct_status_1" }),
            )
            .await
            .unwrap();

        let Json(resp) = get_account_status(
            State(state.clone()),
            Extension(auth("test")),
            Json(AccountStatusRequest {
                user_id: Some("seller_6".to_string()),
                account_id: None,
            }),
        )
        .await
        .unwrap();

        assert!(resp.charges_enabled);
        assert!(resp.payouts_enabled);
        assert!(resp.details_submitted);
        assert!(resp.onboarding_completed);

        let user = state
            .db
            .get_document(collections::USERS, "seller_6")
            .await
            .unwrap();
        assert_eq!(
            user.get(fields::CHARGES_ENABLED).and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            user.get(fields::PAYOUTS_ENABLED).and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            user.get(fields::ONBOARDING_COMPLETED)
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    // --- Coverage: Stripe API error paths ---

    #[tokio::test]
    async fn test_create_account_stripe_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/accounts"))
            .respond_with(ResponseTemplate::new(400).set_body_string("Bad Request"))
            .mount(&server)
            .await;

        let mut state = setup_state().await;
        state.stripe_base_url = server.uri();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_err",
                json!({ fields::EMAIL: "seller@example.com" }),
            )
            .await
            .unwrap();

        let err = create_account(
            State(state),
            Extension(auth("test")),
            Json(CreateAccountRequest {
                user_id: Some("seller_err".to_string()),
                email: Some("seller@example.com".into()),
                country: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("Failed to create Stripe Connect account")
        );
    }

    #[tokio::test]
    async fn test_create_account_link_stripe_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/account_links"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&server)
            .await;

        let mut state = setup_state().await;
        state.stripe_base_url = server.uri();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_link_err",
                json!({ fields::STRIPE_ACCOUNT_ID: "acct_link_err" }),
            )
            .await
            .unwrap();

        let err = create_account_link(
            State(state),
            Extension(auth("test")),
            Json(AccountLinkRequest {
                user_id: Some("seller_link_err".to_string()),
                account_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Failed to create onboarding link"));
    }

    #[tokio::test]
    async fn test_get_account_status_stripe_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/accounts/acct_status_err"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            .mount(&server)
            .await;

        let mut state = setup_state().await;
        state.stripe_base_url = server.uri();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_status_err",
                json!({ fields::STRIPE_ACCOUNT_ID: "acct_status_err" }),
            )
            .await
            .unwrap();

        let err = get_account_status(
            State(state),
            Extension(auth("test")),
            Json(AccountStatusRequest {
                user_id: Some("seller_status_err".to_string()),
                account_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Failed to fetch account status"));
    }
}
