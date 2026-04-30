//! Payment provider admin handlers.
//! Ported from: functions/handlers/payment_stripe.py (provider admin endpoints)

use axum::{Extension, Json, Router, extract::State, routing::post};
use ob_auth::middleware::AuthContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

use crate::HandlersState;
use crate::shared::schema::{collections, documents, fields};
use crate::shared::validation::validate_uid;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProviderRequest {
    #[serde(default)]
    pub admin_user_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProviderResponse {
    pub success: bool,
    pub providers: Vec<ProviderInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub name: String,
    pub enabled: bool,
    pub mode: String,
    pub supported_currencies: Vec<String>,
    pub webhook_configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_webhook_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProviderRequest {
    #[serde(default)]
    pub admin_user_id: String,
    #[serde(alias = "provider")]
    pub provider_name: String,
    pub enabled: Option<bool>,
    pub mode: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProviderResponse {
    pub success: bool,
    pub provider: ProviderInfo,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatusRequest {
    #[serde(default)]
    pub admin_user_id: String,
    #[serde(default = "default_provider_name")]
    pub provider_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatusResponse {
    pub success: bool,
    pub provider_name: String,
    pub api_reachable: bool,
    pub webhook_secret_configured: bool,
    pub mode: String,
}

const VALID_MODES: &[&str] = &["live", "test"];
const VALID_PROVIDERS: &[&str] = &["stripe"];

fn default_provider_name() -> String {
    "stripe".to_string()
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route(
            "/api/admin/payment-providers/get",
            post(get_payment_providers),
        )
        .route(
            "/api/admin/payment-providers/update",
            post(update_payment_provider),
        )
        .route(
            "/api/admin/payment-providers/status",
            post(get_provider_status),
        )
        // Flutter-compatible aliases (Flutter calls /api/payments/providers/*)
        .route("/api/payments/providers/list", post(get_payment_providers))
        .route(
            "/api/payments/providers/update",
            post(update_payment_provider),
        )
        .route("/api/payments/providers/status", post(get_provider_status))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Verify that the caller is an admin user.
async fn verify_admin(state: &HandlersState, admin_user_id: &str) -> Result<(), ob_core::Error> {
    let user = state
        .db
        .get_document(collections::USERS, admin_user_id)
        .await
        .map_err(|_| ob_core::Error::NotFound(format!("User {admin_user_id} not found")))?;

    let roles = user
        .get(fields::ROLES)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();

    if !roles.contains(&"admin") {
        return Err(ob_core::Error::Forbidden("Admin access required".into()));
    }

    Ok(())
}

fn validate_provider_name(name: &str) -> Result<(), ob_core::Error> {
    if !VALID_PROVIDERS.contains(&name) {
        return Err(ob_core::Error::Validation(format!(
            "Unknown provider '{}'. Valid: {}",
            name,
            VALID_PROVIDERS.join(", ")
        )));
    }
    Ok(())
}

/// Build default provider info for Stripe.
fn default_stripe_provider() -> ProviderInfo {
    ProviderInfo {
        name: "stripe".to_string(),
        enabled: true,
        mode: "test".to_string(),
        supported_currencies: vec!["cad".to_string()],
        webhook_configured: false,
        last_webhook_at: None,
    }
}

/// Load provider config from the DB config collection.
async fn load_providers(state: &HandlersState) -> Result<Vec<ProviderInfo>, ob_core::Error> {
    let result = state
        .db
        .get_document(collections::CONFIG, documents::PAYMENT_PROVIDERS)
        .await;

    match result {
        Ok(doc) => {
            let providers_val = doc
                .get("providers")
                .cloned()
                .unwrap_or(Value::Array(vec![]));
            let providers: Vec<ProviderInfo> = serde_json::from_value(providers_val)
                .unwrap_or_else(|_| vec![default_stripe_provider()]);
            Ok(providers)
        }
        Err(_) => {
            // No config doc yet, return defaults
            Ok(vec![default_stripe_provider()])
        }
    }
}

/// Save provider config to the DB.
async fn save_providers(
    state: &HandlersState,
    providers: &[ProviderInfo],
) -> Result<(), ob_core::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    let data = serde_json::json!({
        "providers": providers,
        fields::UPDATED_AT: now,
    });

    // Try update first, create if not found
    let result = state
        .db
        .update_document(
            collections::CONFIG,
            documents::PAYMENT_PROVIDERS,
            data.clone(),
        )
        .await;

    if result.is_err() {
        state
            .db
            .upsert_document(collections::CONFIG, documents::PAYMENT_PROVIDERS, data)
            .await
            .map_err(|e| ob_core::Error::Database(format!("Failed to save config: {e}")))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/admin/payment-providers/get — List all payment providers.
async fn get_payment_providers(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<GetProviderRequest>,
) -> Result<Json<GetProviderResponse>, ob_core::Error> {
    let admin_user_id = if req.admin_user_id.is_empty() {
        auth.user_id.clone()
    } else {
        req.admin_user_id
    };
    if !admin_user_id.is_empty() {
        validate_uid("adminUserId", &admin_user_id)?;
        if auth.user_id != admin_user_id {
            return Err(ob_core::Error::Forbidden("Admin identity mismatch".into()));
        }
        verify_admin(&state, &admin_user_id).await?;
    }

    let providers = load_providers(&state).await?;

    Ok(Json(GetProviderResponse {
        success: true,
        providers,
    }))
}

/// POST /api/admin/payment-providers/update — Update provider configuration.
async fn update_payment_provider(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<UpdateProviderRequest>,
) -> Result<Json<UpdateProviderResponse>, ob_core::Error> {
    let admin_user_id = if req.admin_user_id.is_empty() {
        auth.user_id.clone()
    } else {
        req.admin_user_id.clone()
    };
    validate_uid("adminUserId", &admin_user_id)?;
    if auth.user_id != admin_user_id {
        return Err(ob_core::Error::Forbidden("Admin identity mismatch".into()));
    }
    verify_admin(&state, &admin_user_id).await?;
    validate_provider_name(&req.provider_name)?;

    if let Some(ref mode) = req.mode
        && !VALID_MODES.contains(&mode.as_str())
    {
        return Err(ob_core::Error::Validation(format!(
            "Invalid mode '{}'. Valid: {}",
            mode,
            VALID_MODES.join(", ")
        )));
    }

    let mut providers = load_providers(&state).await?;

    // Find or create the provider entry
    let idx = providers.iter().position(|p| p.name == req.provider_name);
    let provider = match idx {
        Some(i) => &mut providers[i],
        None => {
            providers.push(ProviderInfo {
                name: req.provider_name.clone(),
                enabled: true,
                mode: "test".to_string(),
                supported_currencies: vec!["cad".to_string()],
                webhook_configured: false,
                last_webhook_at: None,
            });
            providers.last_mut().unwrap()
        }
    };

    if let Some(enabled) = req.enabled {
        provider.enabled = enabled;
    }
    if let Some(ref mode) = req.mode {
        provider.mode = mode.clone();
    }

    let updated_provider = provider.clone();

    save_providers(&state, &providers).await?;

    // Log admin action
    let now = chrono::Utc::now().to_rfc3339();
    let log = serde_json::json!({
        "action": "update_payment_provider",
        "adminUserId": admin_user_id,
        "providerName": req.provider_name,
        "changes": {
            "enabled": req.enabled,
            "mode": req.mode,
        },
        fields::CREATED_AT: now,
    });
    let _ = state.db.create_document(collections::ADMIN_LOGS, log).await;

    info!(
        admin = %admin_user_id,
        provider = %req.provider_name,
        "Payment provider updated"
    );

    Ok(Json(UpdateProviderResponse {
        success: true,
        provider: updated_provider,
    }))
}

/// POST /api/admin/payment-providers/status — Check live status of a provider.
async fn get_provider_status(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<ProviderStatusRequest>,
) -> Result<Json<ProviderStatusResponse>, ob_core::Error> {
    if !req.admin_user_id.is_empty() {
        validate_uid("adminUserId", &req.admin_user_id)?;
        if auth.user_id != req.admin_user_id {
            return Err(ob_core::Error::Forbidden("Admin identity mismatch".into()));
        }
        verify_admin(&state, &req.admin_user_id).await?;
    }
    validate_provider_name(&req.provider_name)?;

    // Determine current mode
    let providers = load_providers(&state).await?;
    let current_mode = providers
        .iter()
        .find(|p| p.name == req.provider_name)
        .map(|p| p.mode.clone())
        .unwrap_or_else(|| "test".to_string());

    // Check if Stripe API is reachable
    let api_reachable = match req.provider_name.as_str() {
        "stripe" => match state.config.require_secret("stripe_secret_key") {
            Ok(key) => {
                let resp = state
                    .http_client
                    .get(format!("{}/balance", state.stripe_base_url))
                    .basic_auth(key, None::<&str>)
                    .send()
                    .await;
                match resp {
                    Ok(r) => r.status().is_success(),
                    Err(e) => {
                        warn!(error = %e, "Stripe API health check failed");
                        false
                    }
                }
            }
            Err(_) => false,
        },
        _ => false,
    };

    // Check if webhook secret is configured
    let webhook_secret_configured = state.config.require_secret("stripe_webhook_secret").is_ok();

    info!(
        admin = %req.admin_user_id,
        provider = %req.provider_name,
        reachable = api_reachable,
        webhook = webhook_secret_configured,
        "Provider status checked"
    );

    Ok(Json(ProviderStatusResponse {
        success: true,
        provider_name: req.provider_name,
        api_reachable,
        webhook_secret_configured,
        mode: current_mode,
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
        HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
        }
    }

    #[test]
    fn test_get_provider_request_deser() {
        let json = r#"{"adminUserId": "admin1"}"#;
        let req: GetProviderRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.admin_user_id, "admin1");
    }

    #[test]
    fn test_provider_info_ser() {
        let p = default_stripe_provider();
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["name"], "stripe");
        assert_eq!(json["enabled"], true);
        assert_eq!(json["mode"], "test");
        assert_eq!(json["supportedCurrencies"][0], "cad");
        assert_eq!(json["webhookConfigured"], false);
        // last_webhook_at should be absent due to skip_serializing_if
        assert!(json.get("lastWebhookAt").is_none());
    }

    #[test]
    fn test_provider_info_with_last_webhook() {
        let p = ProviderInfo {
            last_webhook_at: Some("2026-03-10T00:00:00Z".to_string()),
            ..default_stripe_provider()
        };
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["lastWebhookAt"], "2026-03-10T00:00:00Z");
    }

    #[test]
    fn test_update_provider_request_deser() {
        let json = r#"{
            "adminUserId": "a1",
            "providerName": "stripe",
            "enabled": false,
            "mode": "live"
        }"#;
        let req: UpdateProviderRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.provider_name, "stripe");
        assert_eq!(req.enabled, Some(false));
        assert_eq!(req.mode.as_deref(), Some("live"));
    }

    #[test]
    fn test_update_provider_request_partial() {
        let json = r#"{"adminUserId": "a1", "providerName": "stripe"}"#;
        let req: UpdateProviderRequest = serde_json::from_str(json).unwrap();
        assert!(req.enabled.is_none());
        assert!(req.mode.is_none());
    }

    #[test]
    fn test_validate_provider_name_valid() {
        assert!(validate_provider_name("stripe").is_ok());
    }

    #[test]
    fn test_validate_provider_name_invalid() {
        assert!(validate_provider_name("paypal").is_err());
        assert!(validate_provider_name("").is_err());
    }

    #[test]
    fn test_provider_status_response_ser() {
        let resp = ProviderStatusResponse {
            success: true,
            provider_name: "stripe".to_string(),
            api_reachable: true,
            webhook_secret_configured: true,
            mode: "live".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["providerName"], "stripe");
        assert_eq!(json["apiReachable"], true);
        assert_eq!(json["webhookSecretConfigured"], true);
        assert_eq!(json["mode"], "live");
    }

    #[test]
    fn test_valid_modes() {
        assert!(VALID_MODES.contains(&"live"));
        assert!(VALID_MODES.contains(&"test"));
        assert!(!VALID_MODES.contains(&"sandbox"));
    }

    // --- Ported from Python test_handlers_payment_providers*.py ---

    #[test]
    fn test_default_stripe_provider_currency() {
        let p = default_stripe_provider();
        assert_eq!(p.supported_currencies, vec!["cad"]);
    }

    #[test]
    fn test_default_stripe_provider_mode_is_test() {
        let p = default_stripe_provider();
        assert_eq!(p.mode, "test");
    }

    #[test]
    fn test_default_stripe_provider_enabled() {
        let p = default_stripe_provider();
        assert!(p.enabled);
    }

    #[test]
    fn test_default_stripe_provider_webhook_not_configured() {
        let p = default_stripe_provider();
        assert!(!p.webhook_configured);
        assert!(p.last_webhook_at.is_none());
    }

    #[test]
    fn test_provider_info_deser_roundtrip() {
        let p = default_stripe_provider();
        let json_str = serde_json::to_string(&p).unwrap();
        let p2: ProviderInfo = serde_json::from_str(&json_str).unwrap();
        assert_eq!(p2.name, p.name);
        assert_eq!(p2.enabled, p.enabled);
        assert_eq!(p2.mode, p.mode);
        assert_eq!(p2.supported_currencies, p.supported_currencies);
        assert_eq!(p2.webhook_configured, p.webhook_configured);
    }

    #[test]
    fn test_provider_info_deser_with_webhook() {
        let json = r#"{
            "name": "stripe",
            "enabled": true,
            "mode": "live",
            "supportedCurrencies": ["cad", "usd"],
            "webhookConfigured": true,
            "lastWebhookAt": "2026-03-10T12:00:00Z"
        }"#;
        let p: ProviderInfo = serde_json::from_str(json).unwrap();
        assert!(p.webhook_configured);
        assert_eq!(p.last_webhook_at.as_deref(), Some("2026-03-10T12:00:00Z"));
        assert_eq!(p.supported_currencies.len(), 2);
    }

    #[test]
    fn test_provider_status_request_defaults() {
        let json = r#"{}"#;
        let req: ProviderStatusRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.admin_user_id, ""); // default empty
        assert_eq!(req.provider_name, "stripe"); // default_provider_name
    }

    #[test]
    fn test_provider_status_request_custom_provider() {
        let json = r#"{"adminUserId":"a1","providerName":"stripe"}"#;
        let req: ProviderStatusRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.provider_name, "stripe");
    }

    #[test]
    fn test_get_provider_request_empty_defaults() {
        let json = r#"{}"#;
        let req: GetProviderRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.admin_user_id, "");
    }

    #[test]
    fn test_validate_provider_name_case_sensitive() {
        assert!(validate_provider_name("Stripe").is_err());
        assert!(validate_provider_name("STRIPE").is_err());
        assert!(validate_provider_name("stripe").is_ok());
    }

    #[test]
    fn test_valid_providers_only_stripe() {
        assert_eq!(VALID_PROVIDERS.len(), 1);
        assert_eq!(VALID_PROVIDERS[0], "stripe");
    }

    #[test]
    fn test_update_provider_response_ser() {
        let resp = UpdateProviderResponse {
            success: true,
            provider: default_stripe_provider(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["provider"]["name"], "stripe");
    }

    #[test]
    fn test_get_provider_response_ser() {
        let resp = GetProviderResponse {
            success: true,
            providers: vec![default_stripe_provider()],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["providers"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_update_request_mode_only() {
        let json = r#"{"adminUserId":"a1","providerName":"stripe","mode":"live"}"#;
        let req: UpdateProviderRequest = serde_json::from_str(json).unwrap();
        assert!(req.enabled.is_none());
        assert_eq!(req.mode.as_deref(), Some("live"));
    }

    #[test]
    fn test_update_request_enabled_only() {
        let json = r#"{"adminUserId":"a1","providerName":"stripe","enabled":false}"#;
        let req: UpdateProviderRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.enabled, Some(false));
        assert!(req.mode.is_none());
    }

    #[tokio::test]
    async fn test_get_payment_providers_defaults_without_admin_or_config() {
        let state = setup_state().await;

        let Json(resp) = get_payment_providers(
            State(state),
            Extension(auth("test")),
            Json(GetProviderRequest {
                admin_user_id: String::new(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.providers.len(), 1);
        assert_eq!(resp.providers[0].name, "stripe");
        assert!(resp.providers[0].enabled);
    }

    #[tokio::test]
    async fn test_get_payment_providers_requires_admin_when_admin_id_present() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_1",
                json!({
                    fields::UID: "user_1",
                    fields::ROLES: ["buyer"],
                }),
            )
            .await
            .unwrap();

        let err = get_payment_providers(
            State(state),
            Extension(auth("test")),
            Json(GetProviderRequest {
                admin_user_id: "user_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Admin access required"));
    }

    #[tokio::test]
    async fn test_update_payment_provider_persists_and_logs_changes() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "admin_1",
                json!({
                    fields::UID: "admin_1",
                    fields::ROLES: ["admin"],
                }),
            )
            .await
            .unwrap();

        let Json(resp) = update_payment_provider(
            State(state.clone()),
            Extension(auth("test")),
            Json(UpdateProviderRequest {
                admin_user_id: "admin_1".into(),
                provider_name: "stripe".into(),
                enabled: Some(false),
                mode: Some("live".into()),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(!resp.provider.enabled);
        assert_eq!(resp.provider.mode, "live");

        let providers = load_providers(&state).await.unwrap();
        assert_eq!(providers.len(), 1);
        assert!(!providers[0].enabled);
        assert_eq!(providers[0].mode, "live");

        let logs = state
            .db
            .query_bind_value(
                "SELECT * FROM admin_logs WHERE action = $action",
                json!({"action": "update_payment_provider"}),
            )
            .await
            .unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0]["providerName"], "stripe");
    }

    #[tokio::test]
    async fn test_update_payment_provider_rejects_invalid_mode_and_unknown_provider() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "admin_1",
                json!({
                    fields::UID: "admin_1",
                    fields::ROLES: ["admin"],
                }),
            )
            .await
            .unwrap();

        let bad_mode = update_payment_provider(
            State(state.clone()),
            Extension(auth("test")),
            Json(UpdateProviderRequest {
                admin_user_id: "admin_1".into(),
                provider_name: "stripe".into(),
                enabled: None,
                mode: Some("sandbox".into()),
            }),
        )
        .await
        .unwrap_err();
        assert!(bad_mode.to_string().contains("Invalid mode"));

        let bad_provider = update_payment_provider(
            State(state),
            Extension(auth("test")),
            Json(UpdateProviderRequest {
                admin_user_id: "admin_1".into(),
                provider_name: "paypal".into(),
                enabled: Some(true),
                mode: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(bad_provider.to_string().contains("Unknown provider"));
    }

    #[tokio::test]
    async fn test_get_provider_status_uses_state_base_url_and_secret_presence() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/balance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"object": "balance"})))
            .mount(&mock_server)
            .await;

        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_secret_key".to_string(), "sk_test_123".to_string());
        config.secrets.values.insert(
            "stripe_webhook_secret".to_string(),
            "whsec_test_123".to_string(),
        );
        let state = HandlersState {
            config: Arc::new(config),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: mock_server.uri(),
            turnstile_secret_key: None,
        };

        let Json(resp) = get_provider_status(
            State(state),
            Extension(auth("test")),
            Json(ProviderStatusRequest {
                admin_user_id: String::new(),
                provider_name: "stripe".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(resp.api_reachable);
        assert!(resp.webhook_secret_configured);
        assert_eq!(resp.mode, "test");
    }

    #[tokio::test]
    async fn test_get_provider_status_handles_missing_secrets_and_unreachable_api() {
        let state = setup_state().await;

        let Json(resp) = get_provider_status(
            State(state),
            Extension(auth("test")),
            Json(ProviderStatusRequest {
                admin_user_id: String::new(),
                provider_name: "stripe".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(!resp.api_reachable);
        assert!(!resp.webhook_secret_configured);
        assert_eq!(resp.mode, "test");
    }

    // --- Coverage: save_providers upsert fallback (line 212) ---

    #[tokio::test]
    async fn test_save_providers_creates_config_doc_when_missing() {
        let state = setup_state().await;
        // No config doc exists yet — save_providers should create via upsert fallback
        let providers = vec![default_stripe_provider()];
        save_providers(&state, &providers).await.unwrap();

        let doc = state
            .db
            .get_document(collections::CONFIG, documents::PAYMENT_PROVIDERS)
            .await
            .unwrap();
        assert!(doc.get("providers").is_some());
    }

    // --- Coverage: new provider creation when not found in list (lines 264-272) ---

    #[tokio::test]
    async fn test_update_payment_provider_creates_new_entry_when_provider_not_in_list() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "admin_new",
                json!({ fields::UID: "admin_new", fields::ROLES: ["admin"] }),
            )
            .await
            .unwrap();

        // First create a config doc with an empty providers array
        state
            .db
            .upsert_document(
                collections::CONFIG,
                documents::PAYMENT_PROVIDERS,
                json!({ "providers": [] }),
            )
            .await
            .unwrap();

        let Json(resp) = update_payment_provider(
            State(state.clone()),
            Extension(auth("test")),
            Json(UpdateProviderRequest {
                admin_user_id: "admin_new".into(),
                provider_name: "stripe".into(),
                enabled: Some(true),
                mode: Some("live".into()),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.provider.name, "stripe");
        assert_eq!(resp.provider.mode, "live");

        let providers = load_providers(&state).await.unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "stripe");
    }

    // --- Coverage: verify_admin in get_provider_status with non-empty admin_user_id (lines 319-320) ---

    #[tokio::test]
    async fn test_get_provider_status_with_admin_user_id_verifies_admin() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "admin_status",
                json!({ fields::UID: "admin_status", fields::ROLES: ["admin"] }),
            )
            .await
            .unwrap();

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/balance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"object": "balance"})))
            .mount(&mock_server)
            .await;

        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_secret_key".to_string(), "sk_test_123".to_string());
        let state = HandlersState {
            config: Arc::new(config),
            db: state.db,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: mock_server.uri(),
            turnstile_secret_key: None,
        };

        let Json(resp) = get_provider_status(
            State(state),
            Extension(auth("test")),
            Json(ProviderStatusRequest {
                admin_user_id: "admin_status".into(),
                provider_name: "stripe".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
    }

    // --- Coverage: Stripe API health check failure (lines 344-346) ---

    #[tokio::test]
    async fn test_get_provider_status_stripe_api_unreachable() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/balance"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Server Error"))
            .mount(&mock_server)
            .await;

        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_secret_key".to_string(), "sk_test_123".to_string());
        let state = HandlersState {
            config: Arc::new(config),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: mock_server.uri(),
            turnstile_secret_key: None,
        };

        let Json(resp) = get_provider_status(
            State(state),
            Extension(auth("test")),
            Json(ProviderStatusRequest {
                admin_user_id: String::new(),
                provider_name: "stripe".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(!resp.api_reachable);
    }

    // --- Coverage: unknown provider in api_reachable match (line 352) ---

    // Note: validate_provider_name blocks unknown providers before reaching the match,
    // so this line is only reachable if VALID_PROVIDERS were to include something other than "stripe".
    // The line 352 `_ => false` is defensive code. We test that stripe is the only reachable path.

    // --- Coverage: Stripe API health check network error (lines 344-346) ---

    #[tokio::test]
    async fn test_get_provider_status_stripe_api_network_error() {
        // Point at a port that will refuse connections immediately
        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_secret_key".to_string(), "sk_test_123".to_string());
        let state = HandlersState {
            config: Arc::new(config),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "http://127.0.0.1:1".into(), // port 1 = connection refused
            turnstile_secret_key: None,
        };

        let Json(resp) = get_provider_status(
            State(state),
            Extension(auth("test")),
            Json(ProviderStatusRequest {
                admin_user_id: String::new(),
                provider_name: "stripe".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(!resp.api_reachable); // Network error → false
    }
}
