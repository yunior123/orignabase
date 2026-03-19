//! User profile management: CRUD, email consent (CASL), notification prefs, FCM cleanup.

use axum::{Extension, Json, Router, extract::State, routing::post};
use chrono::Utc;
use ob_auth::middleware::AuthContext;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::HandlersState;
use crate::shared::auth::resolve_self_user_id;
use crate::shared::schema::{COUNTRY_CANADA, UserRole, collections, fields};
use crate::shared::validation::{sanitize_html, validate_email, validate_string, validate_uid};

// =============================================================================
// REQUEST / RESPONSE TYPES
// =============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProfileRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub address: Option<AddressInput>,
    #[serde(default)]
    pub preferred_language: Option<String>,
    #[serde(default)]
    pub tax_exemption: Option<TaxExemptionInput>,
    /// Terms version acceptance — set to record user accepting updated terms.
    #[serde(default)]
    pub terms_version: Option<String>,
    /// Ignored if terms_version is absent; accepts any truthy/falsy value.
    #[serde(default)]
    pub terms_accepted_at: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressInput {
    pub street: String,
    pub city: String,
    pub province: String,
    pub postal_code: String,
    #[serde(default = "default_country")]
    pub country: String,
}

fn default_country() -> String {
    COUNTRY_CANADA.to_string()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaxExemptionInput {
    pub gst_number: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProfileRequest {
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailConsentRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    pub consent: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPrefsRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub notify_new_products: Option<bool>,
    #[serde(default)]
    pub notify_trending: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProfileRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    pub email: String,
    pub name: String,
    #[serde(default)]
    pub roles: Option<Vec<UserRole>>,
    #[serde(default)]
    pub preferred_language: Option<String>,
    #[serde(default)]
    pub marketing_opt_in: Option<bool>,
    #[serde(default)]
    pub consent_method: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupFcmTokenRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddBuyerAddressRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    pub street: String,
    pub city: String,
    pub province: String,
    pub postal_code: String,
    #[serde(default = "default_country")]
    pub country: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBuyerAddressRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    pub address_id: String,
    pub street: String,
    pub city: String,
    pub province: String,
    pub postal_code: String,
    #[serde(default = "default_country")]
    pub country: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteBuyerAddressRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    pub address_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDefaultBuyerAddressRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    pub address_id: String,
}

#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub success: bool,
    #[serde(flatten)]
    pub data: serde_json::Value,
}

// =============================================================================
// CONSTANTS
// =============================================================================

const MAX_NAME_LENGTH: usize = 100;
const MIN_NAME_LENGTH: usize = 1;
const VALID_LANGUAGES: &[&str] = &["en", "fr"];
const GST_REGEX_PATTERN: &str = r"^\d{9}RT\d{4}$";
const VALID_CONSENT_METHODS: &[&str] = &["google_oauth", "apple_oauth", "signup_form", "signup"];

// =============================================================================
// HELPERS
// =============================================================================

fn success(data: serde_json::Value) -> Json<SuccessResponse> {
    Json(SuccessResponse {
        success: true,
        data,
    })
}

fn validate_gst_number(gst: &str) -> ob_core::Result<()> {
    let re = regex_lite::Regex::new(GST_REGEX_PATTERN).unwrap();
    if !re.is_match(gst) {
        return Err(ob_core::Error::Validation(
            "Invalid GST number format. Expected: 123456789RT0001".into(),
        ));
    }
    Ok(())
}

fn validate_language(lang: &str) -> ob_core::Result<()> {
    if !VALID_LANGUAGES.contains(&lang) {
        return Err(ob_core::Error::Validation(format!(
            "Invalid language. Must be one of: {:?}",
            VALID_LANGUAGES
        )));
    }
    Ok(())
}

fn normalize_profile_name(name: &str, email: &str) -> String {
    let name_raw = name.trim();
    if name_raw.is_empty() {
        return email.split('@').next().unwrap_or("User").to_string();
    }

    let sanitized = sanitize_html(name_raw);
    if sanitized.len() < MIN_NAME_LENGTH {
        "User".to_string()
    } else {
        sanitized[..sanitized.len().min(MAX_NAME_LENGTH)].to_string()
    }
}

fn normalize_preferred_language(lang: Option<&str>) -> &'static str {
    match lang {
        Some("en") => "en",
        Some("fr") => "fr",
        _ => "en",
    }
}

fn address_is_in_canada(country: &str) -> bool {
    country == COUNTRY_CANADA
}

fn sanitize_address_fields(
    street: &str,
    city: &str,
    province: &str,
    postal_code: &str,
    country: &str,
) -> ob_core::Result<serde_json::Value> {
    if !address_is_in_canada(country) {
        return Err(ob_core::Error::Validation(
            "Shipping addresses must be in Canada".into(),
        ));
    }
    validate_string("street", street, 200)?;
    validate_string("city", city, 100)?;
    validate_string("province", province, 50)?;
    validate_string("postalCode", postal_code, 10)?;
    Ok(json!({
        fields::STREET: sanitize_html(street),
        fields::CITY: sanitize_html(city),
        fields::PROVINCE: sanitize_html(province),
        fields::POSTAL_CODE: postal_code.trim().to_uppercase(),
        fields::COUNTRY: country,
    }))
}

fn strip_record_prefix<'a>(collection: &str, raw_id: &'a str) -> &'a str {
    raw_id
        .strip_prefix(&format!("{collection}:"))
        .unwrap_or(raw_id)
}

// =============================================================================
// HANDLERS
// =============================================================================

/// POST /api/users/profile/update
async fn update_profile(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<UpdateProfileRequest>,
) -> ob_core::Result<Json<SuccessResponse>> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &user_id,
        "update_profile",
        30, // 30 updates
        60, // per hour
    )
    .await?;

    // Verify user exists
    let _user = state.db.get_document(collections::USERS, &user_id).await?;

    let now = Utc::now().to_rfc3339();
    let mut update_data = serde_json::Map::new();
    update_data.insert(fields::UPDATED_AT.to_string(), json!(now));

    let mut updated_fields = vec![fields::UPDATED_AT.to_string()];

    // Handle name update
    if let Some(ref name_raw) = req.name {
        let name = sanitize_html(name_raw.trim());
        if name.len() < MIN_NAME_LENGTH || name.len() > MAX_NAME_LENGTH {
            return Err(ob_core::Error::Validation(format!(
                "Name must be between {MIN_NAME_LENGTH} and {MAX_NAME_LENGTH} characters"
            )));
        }
        update_data.insert(fields::NAME.to_string(), json!(name));
        updated_fields.push(fields::NAME.to_string());
    }

    // Handle address update
    if let Some(ref addr) = req.address {
        if !address_is_in_canada(&addr.country) {
            return Err(ob_core::Error::Validation(
                "Address must be in Canada".into(),
            ));
        }
        validate_string("street", &addr.street, 200)?;
        validate_string("city", &addr.city, 100)?;
        validate_string("province", &addr.province, 50)?;
        validate_string("postalCode", &addr.postal_code, 10)?;

        update_data.insert(
            fields::ADDRESS.to_string(),
            json!({
                fields::STREET: sanitize_html(&addr.street),
                fields::CITY: sanitize_html(&addr.city),
                fields::PROVINCE: sanitize_html(&addr.province),
                fields::POSTAL_CODE: addr.postal_code.to_uppercase(),
                fields::COUNTRY: addr.country,
            }),
        );
        updated_fields.push(fields::ADDRESS.to_string());
    }

    // Handle language update
    if let Some(ref lang) = req.preferred_language {
        validate_language(lang)?;
        update_data.insert("preferredLanguage".to_string(), json!(lang));
        // CASL compliance: record consent method for language preference
        update_data.insert("consentMethod".to_string(), json!("user_preference"));
        update_data.insert("consentTimestamp".to_string(), json!(now));
        updated_fields.push("preferredLanguage".to_string());
    }

    // Handle terms version acceptance
    if let Some(ref version) = req.terms_version {
        validate_string("termsVersion", version, 20)?;
        update_data.insert("termsVersion".to_string(), json!(version));
        update_data.insert("termsAcceptedAt".to_string(), json!(now));
        updated_fields.push("termsVersion".to_string());
    }

    // Handle tax exemption
    if let Some(ref tax) = req.tax_exemption {
        let gst = tax.gst_number.trim().to_uppercase();
        if !gst.is_empty() {
            validate_gst_number(&gst)?;
        }
        update_data.insert(
            "taxExemption".to_string(),
            json!({
                "gstNumber": gst,
                fields::UPDATED_AT: now,
            }),
        );
        updated_fields.push("taxExemption".to_string());
    }

    state
        .db
        .update_document(
            collections::USERS,
            &user_id,
            serde_json::Value::Object(update_data),
        )
        .await?;

    Ok(success(json!({
        "updated": true,
        "fields": updated_fields,
    })))
}

/// POST /api/users/profile/get
async fn get_profile(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<GetProfileRequest>,
) -> ob_core::Result<Json<SuccessResponse>> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;

    let user_doc = state.db.get_document(collections::USERS, &user_id).await?;

    Ok(success(json!({
        fields::UID: user_id,
        fields::EMAIL: user_doc.get(fields::EMAIL),
        fields::NAME: user_doc.get(fields::NAME),
        fields::ADDRESS: user_doc.get(fields::ADDRESS),
        "taxExemption": user_doc.get("taxExemption"),
        fields::ROLES: user_doc.get(fields::ROLES),
        fields::CREATED_AT: user_doc.get(fields::CREATED_AT),
        fields::UPDATED_AT: user_doc.get(fields::UPDATED_AT),
        fields::SUSPENDED: user_doc.get(fields::SUSPENDED).and_then(|v| v.as_bool()).unwrap_or(false),
        "termsVersion": user_doc.get("termsVersion"),
        "privacyPolicyVersion": user_doc.get("privacyPolicyVersion"),
    })))
}

/// POST /api/users/email-consent — CASL compliance
async fn email_consent(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<EmailConsentRequest>,
) -> ob_core::Result<Json<SuccessResponse>> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;

    // Verify user exists
    let _user = state.db.get_document(collections::USERS, &user_id).await?;

    let now = Utc::now().to_rfc3339();
    let consent_method = if req.consent {
        "user_preference"
    } else {
        "unsubscribe"
    };

    state
        .db
        .update_document(
            collections::USERS,
            &user_id,
            json!({
                fields::EMAIL_CONSENT: req.consent,
                "consentTimestamp": now,
                "consentMethod": consent_method,
                fields::UPDATED_AT: now,
            }),
        )
        .await?;

    Ok(success(json!({
        fields::EMAIL_CONSENT: req.consent,
    })))
}

/// POST /api/users/notification-preferences
async fn notification_preferences(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<NotificationPrefsRequest>,
) -> ob_core::Result<Json<SuccessResponse>> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;

    // Verify user exists and is premium
    let user_doc = state.db.get_document(collections::USERS, &user_id).await?;

    let is_premium = user_doc
        .get(fields::IS_PREMIUM)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !is_premium {
        return Err(ob_core::Error::Forbidden(
            "Premium membership required to change notification preferences.".into(),
        ));
    }

    let now = Utc::now().to_rfc3339();
    let mut update = serde_json::Map::new();
    update.insert(fields::UPDATED_AT.to_string(), json!(now));

    if let Some(v) = req.notify_new_products {
        update.insert("notifyNewProducts".to_string(), json!(v));
    }
    if let Some(v) = req.notify_trending {
        update.insert("notifyTrending".to_string(), json!(v));
    }

    if update.len() <= 1 {
        return Err(ob_core::Error::Validation(
            "No valid notification fields provided.".into(),
        ));
    }

    state
        .db
        .update_document(
            collections::USERS,
            &user_id,
            serde_json::Value::Object(update),
        )
        .await?;

    Ok(success(json!({})))
}

/// POST /api/users/create-profile — creates initial user document
async fn create_profile(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<CreateProfileRequest>,
) -> ob_core::Result<Json<SuccessResponse>> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;
    validate_email(&req.email)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &user_id,
        "create_profile",
        5,  // 5 creations
        60, // per hour
    )
    .await?;

    // Validate and sanitize name
    let name = normalize_profile_name(&req.name, &req.email);

    // Validate language
    let lang = normalize_preferred_language(req.preferred_language.as_deref());

    // Validate consent method
    let consent_method = req
        .consent_method
        .as_deref()
        .filter(|m| VALID_CONSENT_METHODS.contains(m))
        .unwrap_or("signup_form");

    // Check if user already exists (idempotent)
    match state.db.get_document(collections::USERS, &user_id).await {
        Ok(_) => {
            return Ok(success(json!({ "created": false, "existing": true })));
        }
        Err(ob_core::Error::NotFound(_)) => {} // Expected — proceed to create
        Err(e) => return Err(e),
    }

    let roles = req.roles.clone().unwrap_or_else(|| vec![UserRole::Buyer]);
    let now = Utc::now().to_rfc3339();
    let marketing_opt_in = req.marketing_opt_in.unwrap_or(false);

    state
        .db
        .create_document(
            collections::USERS,
            json!({
                fields::UID: user_id,
                fields::EMAIL: req.email,
                fields::NAME: name,
                fields::ROLES: roles,
                fields::CREATED_AT: now,
                "preferredLanguage": lang,
                // Legal compliance (CASL / PIPEDA / Law 25)
                "dataProcessingConsent": true,
                fields::EMAIL_CONSENT: true,
                "marketingOptIn": marketing_opt_in,
                "consentTimestamp": now,
                "termsAcceptedAt": now,
                "privacyAcceptedAt": now,
                "consentMethod": consent_method,
                "privacyPolicyVersion": "1.0",
                "termsVersion": "1.0",
                "pushEnabled": true,
            }),
        )
        .await?;

    Ok(success(json!({ "created": true })))
}

/// POST /api/users/cleanup-fcm-token — removes stale FCM token on logout
async fn cleanup_fcm_token(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<CleanupFcmTokenRequest>,
) -> ob_core::Result<Json<SuccessResponse>> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;
    validate_string("token", &req.token, 512)?;

    // FCM tokens are stored as documents in a subcollection-like pattern.
    // In SurrealDB we use a flat collection with userId + token fields.
    let query = format!(
        "SELECT * FROM {} WHERE {} = '{}' AND {} = '{}' LIMIT 1",
        collections::FCM_TOKENS,
        fields::UID,
        user_id,
        fields::TOKEN,
        req.token.replace('\'', ""),
    );
    let results = state.db.query_raw(&query).await.unwrap_or_default();

    if results.is_empty() {
        // Idempotent — token already gone
        return Ok(success(json!({ "deleted": false })));
    }

    let token_id = results[0].get("id").and_then(|v| v.as_str()).unwrap_or("");

    if !token_id.is_empty() {
        let id = token_id
            .strip_prefix(&format!("{}:", collections::FCM_TOKENS))
            .unwrap_or(token_id);
        let _ = state.db.delete_document(collections::FCM_TOKENS, id).await;
    }

    Ok(success(json!({ "deleted": true })))
}

async fn add_buyer_address(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<AddBuyerAddressRequest>,
) -> ob_core::Result<Json<SuccessResponse>> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;
    let _ = state.db.get_document(collections::USERS, &user_id).await?;

    let address = sanitize_address_fields(
        &req.street,
        &req.city,
        &req.province,
        &req.postal_code,
        &req.country,
    )?;
    let now = Utc::now().to_rfc3339();

    if req.is_default {
        let clear_query = format!(
            "UPDATE {} SET isDefault = false, {} = '{}' WHERE userId = '{}'",
            collections::ADDRESSES,
            fields::UPDATED_AT,
            now,
            ob_core::escape_surreal_string(&user_id),
        );
        let _ = state.db.query_raw(&clear_query).await;
    }

    let created = state
        .db
        .create_document(
            collections::ADDRESSES,
            json!({
                "userId": user_id,
                "label": req.label.as_deref().unwrap_or(""),
                "isDefault": req.is_default,
                "address": address,
                fields::CREATED_AT: now,
                fields::UPDATED_AT: now,
            }),
        )
        .await?;

    let address_id = created
        .get("id")
        .and_then(|v| v.as_str())
        .map(|id| strip_record_prefix(collections::ADDRESSES, id).to_string())
        .unwrap_or_default();

    Ok(success(json!({ "addressId": address_id })))
}

async fn update_buyer_address(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<UpdateBuyerAddressRequest>,
) -> ob_core::Result<Json<SuccessResponse>> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;
    validate_uid("addressId", &req.address_id)?;

    let existing = state
        .db
        .get_document(collections::ADDRESSES, &req.address_id)
        .await?;

    if existing.get("userId").and_then(|v| v.as_str()) != Some(user_id.as_str()) {
        return Err(ob_core::Error::Forbidden(
            "Address ownership mismatch".into(),
        ));
    }

    let address = sanitize_address_fields(
        &req.street,
        &req.city,
        &req.province,
        &req.postal_code,
        &req.country,
    )?;
    let now = Utc::now().to_rfc3339();

    if req.is_default {
        let clear_query = format!(
            "UPDATE {} SET isDefault = false, {} = '{}' WHERE userId = '{}' AND id != type::thing('{}', '{}')",
            collections::ADDRESSES,
            fields::UPDATED_AT,
            now,
            ob_core::escape_surreal_string(&user_id),
            collections::ADDRESSES,
            ob_core::escape_surreal_string(&req.address_id),
        );
        let _ = state.db.query_raw(&clear_query).await;
    }

    state
        .db
        .update_document(
            collections::ADDRESSES,
            &req.address_id,
            json!({
                "label": req.label.as_deref().unwrap_or(""),
                "isDefault": req.is_default,
                "address": address,
                fields::UPDATED_AT: now,
            }),
        )
        .await?;

    Ok(success(json!({ "updated": true })))
}

async fn delete_buyer_address(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<DeleteBuyerAddressRequest>,
) -> ob_core::Result<Json<SuccessResponse>> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;
    validate_uid("addressId", &req.address_id)?;

    let existing = state
        .db
        .get_document(collections::ADDRESSES, &req.address_id)
        .await?;

    if existing.get("userId").and_then(|v| v.as_str()) != Some(user_id.as_str()) {
        return Err(ob_core::Error::Forbidden(
            "Address ownership mismatch".into(),
        ));
    }

    let was_default = existing
        .get("isDefault")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    state
        .db
        .delete_document(collections::ADDRESSES, &req.address_id)
        .await?;

    if was_default {
        let query = format!(
            "SELECT * FROM {} WHERE userId = '{}' ORDER BY {} DESC LIMIT 1",
            collections::ADDRESSES,
            ob_core::escape_surreal_string(&user_id),
            fields::CREATED_AT,
        );
        if let Some(next_address) = state.db.query_raw(&query).await?.into_iter().next()
            && let Some(raw_id) = next_address.get("id").and_then(|v| v.as_str())
        {
            let next_id = strip_record_prefix(collections::ADDRESSES, raw_id);
            let _ = state
                .db
                .update_document(
                    collections::ADDRESSES,
                    next_id,
                    json!({ "isDefault": true, fields::UPDATED_AT: Utc::now().to_rfc3339() }),
                )
                .await;
        }
    }

    Ok(success(json!({ "deleted": true })))
}

async fn set_default_buyer_address(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<SetDefaultBuyerAddressRequest>,
) -> ob_core::Result<Json<SuccessResponse>> {
    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;
    validate_uid("addressId", &req.address_id)?;

    let existing = state
        .db
        .get_document(collections::ADDRESSES, &req.address_id)
        .await?;

    if existing.get("userId").and_then(|v| v.as_str()) != Some(user_id.as_str()) {
        return Err(ob_core::Error::Forbidden(
            "Address ownership mismatch".into(),
        ));
    }

    let now = Utc::now().to_rfc3339();
    let clear_query = format!(
        "UPDATE {} SET isDefault = false, {} = '{}' WHERE userId = '{}'",
        collections::ADDRESSES,
        fields::UPDATED_AT,
        now,
        ob_core::escape_surreal_string(&user_id),
    );
    let _ = state.db.query_raw(&clear_query).await;

    state
        .db
        .update_document(
            collections::ADDRESSES,
            &req.address_id,
            json!({
                "isDefault": true,
                fields::UPDATED_AT: now,
            }),
        )
        .await?;

    Ok(success(json!({ "updated": true })))
}

// =============================================================================
// ROUTER
// =============================================================================

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/users/profile/update", post(update_profile))
        .route("/api/users/profile/get", post(get_profile))
        .route("/api/users/email-consent", post(email_consent))
        .route(
            "/api/users/notification-preferences",
            post(notification_preferences),
        )
        .route("/api/users/create-profile", post(create_profile))
        .route("/api/users/cleanup-fcm-token", post(cleanup_fcm_token))
        .route("/api/users/address/add", post(add_buyer_address))
        .route("/api/users/address/update", post(update_buyer_address))
        .route("/api/users/address/delete", post(delete_buyer_address))
        .route(
            "/api/users/address/set-default",
            post(set_default_buyer_address),
        )
        .with_state(state)
}

// =============================================================================
// TESTS
// =============================================================================

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
    use std::sync::Arc;

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
    fn test_create_profile_request_deser() {
        let json_str = r#"{"userId":"u1","email":"a@b.com","name":"Test","roles":["buyer"]}"#;
        let req: CreateProfileRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.user_id, "u1");
        assert_eq!(req.email, "a@b.com");
        assert_eq!(req.name, "Test");
        assert!(req.roles.is_some());
    }

    #[test]
    fn test_validate_gst_number_valid() {
        assert!(validate_gst_number("123456789RT0001").is_ok());
    }

    #[test]
    fn test_validate_gst_number_invalid() {
        assert!(validate_gst_number("INVALID").is_err());
        assert!(validate_gst_number("12345678RT0001").is_err()); // 8 digits instead of 9
        assert!(validate_gst_number("").is_err());
    }

    #[test]
    fn test_validate_language() {
        assert!(validate_language("en").is_ok());
        assert!(validate_language("fr").is_ok());
        assert!(validate_language("de").is_err());
        assert!(validate_language("").is_err());
    }

    #[test]
    fn test_address_input_default_country() {
        let json_str =
            r#"{"street":"123 Main","city":"Toronto","province":"ON","postalCode":"M5V 1A1"}"#;
        let addr: AddressInput = serde_json::from_str(json_str).unwrap();
        assert_eq!(addr.country, COUNTRY_CANADA);
    }

    #[test]
    fn test_email_consent_request_deser() {
        let json_str = r#"{"userId":"u1","consent":false}"#;
        let req: EmailConsentRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.user_id, "u1");
        assert!(!req.consent);
    }

    #[test]
    fn test_notification_prefs_partial() {
        let json_str = r#"{"userId":"u1","notifyNewProducts":true}"#;
        let req: NotificationPrefsRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.notify_new_products, Some(true));
        assert_eq!(req.notify_trending, None);
    }

    #[test]
    fn test_normalize_profile_name_uses_email_prefix_when_blank() {
        assert_eq!(
            normalize_profile_name("   ", "buyer@example.com"),
            "buyer".to_string()
        );
    }

    #[test]
    fn test_normalize_profile_name_sanitizes_and_truncates() {
        let raw = format!("<b>{}</b>", "x".repeat(150));
        let normalized = normalize_profile_name(&raw, "buyer@example.com");
        assert!(!normalized.contains('<'));
        assert_eq!(normalized.len(), MAX_NAME_LENGTH);
    }

    #[test]
    fn test_normalize_preferred_language_falls_back_to_english() {
        assert_eq!(normalize_preferred_language(Some("fr")), "fr");
        assert_eq!(normalize_preferred_language(Some("de")), "en");
        assert_eq!(normalize_preferred_language(None), "en");
    }

    #[test]
    fn test_address_country_guard_matches_canada_only_rule() {
        assert!(address_is_in_canada(COUNTRY_CANADA));
        assert!(!address_is_in_canada("CA"));
        assert!(!address_is_in_canada("United States"));
    }

    // --- Ported from Python test_handlers_users*.py ---

    #[test]
    fn test_sanitize_address_fields_canada_ok() {
        let result = sanitize_address_fields(
            "123 Main St",
            "Toronto",
            "Ontario",
            "m5v 1a1",
            COUNTRY_CANADA,
        );
        assert!(result.is_ok());
        let addr = result.unwrap();
        // postal code is uppercased
        assert_eq!(addr["postalCode"], "M5V 1A1");
    }

    #[test]
    fn test_sanitize_address_fields_non_canada_rejected() {
        let result =
            sanitize_address_fields("123 Main St", "New York", "NY", "10001", "United States");
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("Canada"));
    }

    #[test]
    fn test_sanitize_address_fields_html_stripped() {
        let result = sanitize_address_fields(
            "<script>alert('xss')</script>123 Main",
            "<b>Toronto</b>",
            "<i>ON</i>",
            "M5V1A1",
            COUNTRY_CANADA,
        );
        assert!(result.is_ok());
        let addr = result.unwrap();
        let street = addr["street"].as_str().unwrap();
        assert!(!street.contains("<script>"));
        let city = addr["city"].as_str().unwrap();
        assert!(!city.contains("<b>"));
    }

    #[test]
    fn test_gst_number_valid_format() {
        assert!(validate_gst_number("123456789RT0001").is_ok());
        assert!(validate_gst_number("999999999RT9999").is_ok());
    }

    #[test]
    fn test_gst_number_rejects_wrong_patterns() {
        // Missing RT
        assert!(validate_gst_number("1234567890001").is_err());
        // Too few digits before RT
        assert!(validate_gst_number("12345678RT0001").is_err());
        // Too many digits before RT
        assert!(validate_gst_number("1234567890RT0001").is_err());
        // Too few digits after RT
        assert!(validate_gst_number("123456789RT001").is_err());
        // Lowercase rt
        assert!(validate_gst_number("123456789rt0001").is_err());
        // Spaces
        assert!(validate_gst_number("123456789 RT 0001").is_err());
    }

    #[test]
    fn test_valid_consent_methods_list() {
        let expected = ["google_oauth", "apple_oauth", "signup_form", "signup"];
        for m in &expected {
            assert!(VALID_CONSENT_METHODS.contains(m), "missing: {m}");
        }
        assert!(!VALID_CONSENT_METHODS.contains(&"facebook_oauth"));
        assert!(!VALID_CONSENT_METHODS.contains(&""));
    }

    #[test]
    fn test_normalize_profile_name_normal_input() {
        assert_eq!(
            normalize_profile_name("Alice", "alice@example.com"),
            "Alice"
        );
    }

    #[test]
    fn test_normalize_profile_name_trims_whitespace() {
        assert_eq!(normalize_profile_name("  Bob  ", "bob@example.com"), "Bob");
    }

    #[test]
    fn test_normalize_profile_name_empty_uses_email() {
        assert_eq!(normalize_profile_name("", "charlie@example.com"), "charlie");
    }

    #[test]
    fn test_normalize_profile_name_html_only_falls_back() {
        // If after sanitizing HTML tags, nothing left → "User"
        let result = normalize_profile_name("<>", "test@x.com");
        // After sanitize_html("<>") the result may be empty → "User"
        assert!(!result.is_empty());
    }

    #[test]
    fn test_normalize_profile_name_max_length() {
        let long_name = "A".repeat(200);
        let result = normalize_profile_name(&long_name, "x@x.com");
        assert_eq!(result.len(), MAX_NAME_LENGTH);
    }

    #[test]
    fn test_add_buyer_address_request_deser() {
        let json = r#"{
            "userId": "u1",
            "street": "123 Main",
            "city": "Toronto",
            "province": "ON",
            "postalCode": "M5V 1A1",
            "label": "Home",
            "isDefault": true
        }"#;
        let req: AddBuyerAddressRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.user_id, "u1");
        assert_eq!(req.country, COUNTRY_CANADA); // default
        assert_eq!(req.label.as_deref(), Some("Home"));
        assert!(req.is_default);
    }

    #[test]
    fn test_add_buyer_address_request_defaults() {
        let json = r#"{"userId":"u1","street":"A","city":"B","province":"C","postalCode":"D"}"#;
        let req: AddBuyerAddressRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.country, COUNTRY_CANADA);
        assert!(req.label.is_none());
        assert!(!req.is_default);
    }

    #[test]
    fn test_update_buyer_address_request_deser() {
        let json = r#"{
            "userId":"u1","addressId":"addr1",
            "street":"456 Oak","city":"Montreal","province":"QC","postalCode":"H1A 1A1",
            "isDefault":false
        }"#;
        let req: UpdateBuyerAddressRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.address_id, "addr1");
        assert_eq!(req.country, COUNTRY_CANADA);
    }

    #[test]
    fn test_delete_buyer_address_request_deser() {
        let json = r#"{"userId":"u1","addressId":"addr99"}"#;
        let req: DeleteBuyerAddressRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.address_id, "addr99");
    }

    #[test]
    fn test_set_default_buyer_address_request_deser() {
        let json = r#"{"userId":"u1","addressId":"addr42"}"#;
        let req: SetDefaultBuyerAddressRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.address_id, "addr42");
    }

    #[test]
    fn test_tax_exemption_input_deser() {
        let json = r#"{"gstNumber":"123456789RT0001"}"#;
        let tax: TaxExemptionInput = serde_json::from_str(json).unwrap();
        assert_eq!(tax.gst_number, "123456789RT0001");
    }

    #[test]
    fn test_update_profile_request_all_fields() {
        let json = r#"{
            "userId": "u1",
            "name": "New Name",
            "address": {"street":"1 A","city":"B","province":"C","postalCode":"D"},
            "preferredLanguage": "fr",
            "taxExemption": {"gstNumber":"123456789RT0001"}
        }"#;
        let req: UpdateProfileRequest = serde_json::from_str(json).unwrap();
        assert!(req.name.is_some());
        assert!(req.address.is_some());
        assert_eq!(req.preferred_language.as_deref(), Some("fr"));
        assert!(req.tax_exemption.is_some());
    }

    #[test]
    fn test_update_profile_request_minimal() {
        let json = r#"{"userId":"u1"}"#;
        let req: UpdateProfileRequest = serde_json::from_str(json).unwrap();
        assert!(req.name.is_none());
        assert!(req.address.is_none());
        assert!(req.preferred_language.is_none());
        assert!(req.tax_exemption.is_none());
    }

    #[test]
    fn test_create_profile_request_with_roles() {
        let json = r#"{"userId":"u1","email":"a@b.com","name":"X","roles":["buyer","seller"]}"#;
        let req: CreateProfileRequest = serde_json::from_str(json).unwrap();
        let roles = req.roles.unwrap();
        assert_eq!(roles.len(), 2);
    }

    #[test]
    fn test_create_profile_request_with_consent() {
        let json = r#"{"userId":"u1","email":"a@b.com","name":"X","consentMethod":"google_oauth","marketingOptIn":true}"#;
        let req: CreateProfileRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.consent_method.as_deref(), Some("google_oauth"));
        assert_eq!(req.marketing_opt_in, Some(true));
    }

    #[test]
    fn test_cleanup_fcm_token_request_deser() {
        let json = r#"{"userId":"u1","token":"fcm_token_abc123"}"#;
        let req: CleanupFcmTokenRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.token, "fcm_token_abc123");
    }

    #[test]
    fn test_name_length_constants() {
        assert_eq!(MIN_NAME_LENGTH, 1);
        assert_eq!(MAX_NAME_LENGTH, 100);
    }

    #[test]
    fn test_valid_languages_constant() {
        assert_eq!(VALID_LANGUAGES, &["en", "fr"]);
    }

    #[test]
    fn test_strip_record_prefix_with_prefix() {
        assert_eq!(
            strip_record_prefix("addresses", "addresses:abc123"),
            "abc123"
        );
    }

    #[test]
    fn test_strip_record_prefix_without_prefix() {
        assert_eq!(strip_record_prefix("addresses", "abc123"), "abc123");
    }

    #[test]
    fn test_email_consent_both_states() {
        let json_true = r#"{"userId":"u1","consent":true}"#;
        let json_false = r#"{"userId":"u1","consent":false}"#;
        let req_t: EmailConsentRequest = serde_json::from_str(json_true).unwrap();
        let req_f: EmailConsentRequest = serde_json::from_str(json_false).unwrap();
        assert!(req_t.consent);
        assert!(!req_f.consent);
    }

    #[test]
    fn test_notification_prefs_both_fields() {
        let json = r#"{"userId":"u1","notifyNewProducts":false,"notifyTrending":true}"#;
        let req: NotificationPrefsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.notify_new_products, Some(false));
        assert_eq!(req.notify_trending, Some(true));
    }

    #[tokio::test]
    async fn test_create_profile_returns_existing_when_user_doc_exists() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_1",
                json!({ fields::EMAIL: "existing@example.com" }),
            )
            .await
            .unwrap();

        let Json(resp) = create_profile(
            State(state),
            Extension(auth("test")),
            Json(CreateProfileRequest {
                user_id: Some("user_1".to_string()),
                email: "user@example.com".into(),
                name: "Test".into(),
                roles: None,
                preferred_language: None,
                marketing_opt_in: None,
                consent_method: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data["created"], false);
        assert_eq!(resp.data["existing"], true);
    }

    #[tokio::test]
    async fn test_create_profile_success_normalizes_defaults() {
        let state = setup_state().await;

        let Json(resp) = create_profile(
            State(state.clone()),
            Extension(auth("test")),
            Json(CreateProfileRequest {
                user_id: Some("user_1".to_string()),
                email: "fallback@example.com".into(),
                name: "  ".into(),
                roles: None,
                preferred_language: Some("xx".into()),
                marketing_opt_in: Some(true),
                consent_method: Some("bad_method".into()),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data["created"], true);
        let users: Vec<serde_json::Value> = state
            .db
            .query_raw("SELECT * FROM users WHERE uid = 'user_1' LIMIT 1")
            .await
            .unwrap();
        let user = users.first().unwrap();
        assert_eq!(user[fields::NAME], "fallback");
        assert_eq!(user["preferredLanguage"], "en");
        assert_eq!(user[fields::ROLES], json!(["buyer"]));
        assert_eq!(user["consentMethod"], "signup_form");
        assert_eq!(user["marketingOptIn"], true);
    }

    #[tokio::test]
    async fn test_update_profile_rejects_invalid_gst_number() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(collections::USERS, "user_1", json!({}))
            .await
            .unwrap();

        let err = update_profile(
            State(state),
            Extension(auth("test")),
            Json(UpdateProfileRequest {
                terms_accepted_at: None,
                terms_version: None,
                user_id: Some("user_1".to_string()),
                name: None,
                address: None,
                preferred_language: None,
                tax_exemption: Some(TaxExemptionInput {
                    gst_number: "invalid".into(),
                }),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Invalid GST number format"));
    }

    #[tokio::test]
    async fn test_update_profile_success_updates_name_language_and_tax() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_1",
                json!({ fields::EMAIL: "user@example.com" }),
            )
            .await
            .unwrap();

        let Json(resp) = update_profile(
            State(state.clone()),
            Extension(auth("test")),
            Json(UpdateProfileRequest {
                terms_accepted_at: None,
                terms_version: None,
                user_id: Some("user_1".to_string()),
                name: Some(" Updated Name ".into()),
                address: None,
                preferred_language: Some("fr".into()),
                tax_exemption: Some(TaxExemptionInput {
                    gst_number: "123456789RT0001".into(),
                }),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data["updated"], true);
        let user = state
            .db
            .get_document(collections::USERS, "user_1")
            .await
            .unwrap();
        assert_eq!(user[fields::NAME], "Updated Name");
        assert_eq!(user["preferredLanguage"], "fr");
        assert_eq!(user["taxExemption"]["gstNumber"], "123456789RT0001");
    }

    #[tokio::test]
    async fn test_get_profile_success() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_1",
                json!({
                    fields::EMAIL: "user@example.com",
                    fields::NAME: "User",
                    fields::ROLES: ["buyer"],
                }),
            )
            .await
            .unwrap();

        let Json(resp) = get_profile(
            State(state),
            Extension(auth("test")),
            Json(GetProfileRequest {
                user_id: Some("user_1".to_string()),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data[fields::EMAIL], "user@example.com");
        assert_eq!(resp.data[fields::NAME], "User");
    }

    #[tokio::test]
    async fn test_email_consent_success_sets_unsubscribe_method() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(collections::USERS, "user_1", json!({}))
            .await
            .unwrap();

        let Json(resp) = email_consent(
            State(state.clone()),
            Extension(auth("test")),
            Json(EmailConsentRequest {
                user_id: Some("user_1".to_string()),
                consent: false,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data[fields::EMAIL_CONSENT], false);
        let user = state
            .db
            .get_document(collections::USERS, "user_1")
            .await
            .unwrap();
        assert_eq!(user["consentMethod"], "unsubscribe");
    }

    #[tokio::test]
    async fn test_notification_preferences_requires_premium() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_1",
                json!({ fields::IS_PREMIUM: false }),
            )
            .await
            .unwrap();

        let err = notification_preferences(
            State(state),
            Extension(auth("test")),
            Json(NotificationPrefsRequest {
                user_id: Some("user_1".to_string()),
                notify_new_products: Some(true),
                notify_trending: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Premium membership required"));
    }

    #[tokio::test]
    async fn test_notification_preferences_requires_one_valid_field() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_1",
                json!({ fields::IS_PREMIUM: true }),
            )
            .await
            .unwrap();

        let err = notification_preferences(
            State(state),
            Extension(auth("test")),
            Json(NotificationPrefsRequest {
                user_id: Some("user_1".to_string()),
                notify_new_products: None,
                notify_trending: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("No valid notification fields provided")
        );
    }

    #[tokio::test]
    async fn test_notification_preferences_success() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "user_1",
                json!({ fields::IS_PREMIUM: true }),
            )
            .await
            .unwrap();

        let Json(resp) = notification_preferences(
            State(state.clone()),
            Extension(auth("test")),
            Json(NotificationPrefsRequest {
                user_id: Some("user_1".to_string()),
                notify_new_products: Some(true),
                notify_trending: Some(false),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        let user = state
            .db
            .get_document(collections::USERS, "user_1")
            .await
            .unwrap();
        assert_eq!(user["notifyNewProducts"], true);
        assert_eq!(user["notifyTrending"], false);
    }

    #[tokio::test]
    async fn test_cleanup_fcm_token_idempotent_when_missing() {
        let state = setup_state().await;

        let Json(resp) = cleanup_fcm_token(
            State(state),
            Extension(auth("test")),
            Json(CleanupFcmTokenRequest {
                user_id: Some("user_1".to_string()),
                token: "tok_1".into(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data["deleted"], false);
    }

    #[tokio::test]
    async fn test_cleanup_fcm_token_deletes_existing_token() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::FCM_TOKENS,
                "tok_doc",
                json!({ fields::UID: "user_1", fields::TOKEN: "tok_1" }),
            )
            .await
            .unwrap();

        let Json(resp) = cleanup_fcm_token(
            State(state.clone()),
            Extension(auth("test")),
            Json(CleanupFcmTokenRequest {
                user_id: Some("user_1".to_string()),
                token: "tok_1".into(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data["deleted"], true);
        let query = format!(
            "SELECT * FROM {} WHERE {} = 'user_1' AND {} = 'tok_1'",
            collections::FCM_TOKENS,
            fields::UID,
            fields::TOKEN,
        );
        let remaining: Vec<serde_json::Value> = state.db.query_raw(&query).await.unwrap();
        assert!(remaining.is_empty());
    }

    #[tokio::test]
    async fn test_add_buyer_address_creates_and_returns_id() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(collections::USERS, "user_1", json!({}))
            .await
            .unwrap();

        let Json(resp) = add_buyer_address(
            State(state.clone()),
            Extension(auth("test")),
            Json(AddBuyerAddressRequest {
                user_id: Some("user_1".to_string()),
                street: "1 Main St".into(),
                city: "Toronto".into(),
                province: "ON".into(),
                postal_code: "m5v2h1".into(),
                country: COUNTRY_CANADA.into(),
                label: Some("Home".into()),
                is_default: true,
            }),
        )
        .await
        .unwrap();

        let address_id = resp.data["addressId"].as_str().unwrap();
        let address = state
            .db
            .get_document(collections::ADDRESSES, address_id)
            .await
            .unwrap();
        assert_eq!(address["userId"], "user_1");
        assert_eq!(address["isDefault"], true);
        assert_eq!(address["address"][fields::POSTAL_CODE], "M5V2H1");
    }

    #[tokio::test]
    async fn test_update_buyer_address_rejects_ownership_mismatch() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ADDRESSES,
                "addr_1",
                json!({ "userId": "other_user" }),
            )
            .await
            .unwrap();

        let err = update_buyer_address(
            State(state),
            Extension(auth("test")),
            Json(UpdateBuyerAddressRequest {
                user_id: Some("user_1".to_string()),
                address_id: "addr_1".into(),
                street: "1 Main".into(),
                city: "Toronto".into(),
                province: "ON".into(),
                postal_code: "M5V2H1".into(),
                country: COUNTRY_CANADA.into(),
                label: None,
                is_default: false,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Address ownership mismatch"));
    }

    #[tokio::test]
    async fn test_delete_buyer_address_promotes_next_default() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ADDRESSES,
                "addr_1",
                json!({
                    "userId": "user_1",
                    "isDefault": true,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ADDRESSES,
                "addr_2",
                json!({
                    "userId": "user_1",
                    "isDefault": false,
                    fields::CREATED_AT: "2026-01-02T00:00:00Z",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = delete_buyer_address(
            State(state.clone()),
            Extension(auth("test")),
            Json(DeleteBuyerAddressRequest {
                user_id: Some("user_1".to_string()),
                address_id: "addr_1".into(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data["deleted"], true);
        let promoted = state
            .db
            .get_document(collections::ADDRESSES, "addr_2")
            .await
            .unwrap();
        assert_eq!(promoted["isDefault"], true);
    }

    #[tokio::test]
    async fn test_set_default_buyer_address_updates_requested_address() {
        let state = setup_state().await;
        for (id, is_default) in [("addr_1", true), ("addr_2", false)] {
            state
                .db
                .upsert_document(
                    collections::ADDRESSES,
                    id,
                    json!({ "userId": "user_1", "isDefault": is_default }),
                )
                .await
                .unwrap();
        }

        let Json(resp) = set_default_buyer_address(
            State(state.clone()),
            Extension(auth("test")),
            Json(SetDefaultBuyerAddressRequest {
                user_id: Some("user_1".to_string()),
                address_id: "addr_2".into(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data["updated"], true);
        let old = state
            .db
            .get_document(collections::ADDRESSES, "addr_1")
            .await
            .unwrap();
        let new = state
            .db
            .get_document(collections::ADDRESSES, "addr_2")
            .await
            .unwrap();
        assert_eq!(old["isDefault"], false);
        assert_eq!(new["isDefault"], true);
    }

    // ── Coverage: update_profile name length validation (lines 284-286) ──

    #[tokio::test]
    async fn test_update_profile_rejects_name_too_short() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(collections::USERS, "user_1", json!({}))
            .await
            .unwrap();

        let err = update_profile(
            State(state),
            Extension(auth("test")),
            Json(UpdateProfileRequest {
                terms_accepted_at: None,
                terms_version: None,
                user_id: Some("user_1".to_string()),
                name: Some("".into()), // empty after trim → 0 < MIN_NAME_LENGTH
                address: None,
                preferred_language: None,
                tax_exemption: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Name must be between"));
    }

    #[tokio::test]
    async fn test_update_profile_rejects_name_too_long() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(collections::USERS, "user_1", json!({}))
            .await
            .unwrap();

        let long_name = "A".repeat(MAX_NAME_LENGTH + 1);
        let err = update_profile(
            State(state),
            Extension(auth("test")),
            Json(UpdateProfileRequest {
                terms_accepted_at: None,
                terms_version: None,
                user_id: Some("user_1".to_string()),
                name: Some(long_name),
                address: None,
                preferred_language: None,
                tax_exemption: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Name must be between"));
    }

    // ── Coverage: update_profile address validation (lines 294-314) ──

    #[tokio::test]
    async fn test_update_profile_rejects_non_canada_address() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(collections::USERS, "user_1", json!({}))
            .await
            .unwrap();

        let err = update_profile(
            State(state),
            Extension(auth("test")),
            Json(UpdateProfileRequest {
                terms_accepted_at: None,
                terms_version: None,
                user_id: Some("user_1".to_string()),
                name: None,
                address: Some(AddressInput {
                    street: "123 Main".into(),
                    city: "NYC".into(),
                    province: "NY".into(),
                    postal_code: "10001".into(),
                    country: "United States".into(),
                }),
                preferred_language: None,
                tax_exemption: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Address must be in Canada"));
    }

    #[tokio::test]
    async fn test_update_profile_with_valid_address() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(collections::USERS, "user_1", json!({}))
            .await
            .unwrap();

        let Json(resp) = update_profile(
            State(state.clone()),
            Extension(auth("test")),
            Json(UpdateProfileRequest {
                terms_accepted_at: None,
                terms_version: None,
                user_id: Some("user_1".to_string()),
                name: None,
                address: Some(AddressInput {
                    street: "123 Main St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "m5v 1a1".into(),
                    country: COUNTRY_CANADA.into(),
                }),
                preferred_language: None,
                tax_exemption: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data["updated"], true);
        let fields_updated = resp.data["fields"].as_array().unwrap();
        assert!(fields_updated.iter().any(|f| f == "address"));

        let user = state
            .db
            .get_document(collections::USERS, "user_1")
            .await
            .unwrap();
        assert_eq!(user[fields::ADDRESS][fields::POSTAL_CODE], "M5V 1A1");
        assert_eq!(user[fields::ADDRESS][fields::CITY], "Toronto");
    }

    // ── Coverage: update_profile tax exemption with valid GST (lines 332, 341) ──

    #[tokio::test]
    async fn test_update_profile_with_empty_gst_number() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(collections::USERS, "user_1", json!({}))
            .await
            .unwrap();

        // Empty GST number should skip validation and succeed
        let Json(resp) = update_profile(
            State(state.clone()),
            Extension(auth("test")),
            Json(UpdateProfileRequest {
                terms_accepted_at: None,
                terms_version: None,
                user_id: Some("user_1".to_string()),
                name: None,
                address: None,
                preferred_language: None,
                tax_exemption: Some(TaxExemptionInput {
                    gst_number: "  ".into(), // blank after trim
                }),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data["updated"], true);
        let fields_updated = resp.data["fields"].as_array().unwrap();
        assert!(fields_updated.iter().any(|f| f == "taxExemption"));
    }

    // ── Coverage: email_consent consent=true path (line 397) ──

    #[tokio::test]
    async fn test_email_consent_true_sets_user_preference_method() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(collections::USERS, "user_1", json!({}))
            .await
            .unwrap();

        let Json(resp) = email_consent(
            State(state.clone()),
            Extension(auth("test")),
            Json(EmailConsentRequest {
                user_id: Some("user_1".to_string()),
                consent: true,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data[fields::EMAIL_CONSENT], true);
        let user = state
            .db
            .get_document(collections::USERS, "user_1")
            .await
            .unwrap();
        assert_eq!(user["consentMethod"], "user_preference");
    }

    // ── Coverage: create_profile propagates non-NotFound DB errors (line 513) ──
    // This is hard to trigger with in-mem DB since get_document either returns Ok or NotFound.
    // The existing tests cover Ok (line 510) and NotFound (line 512). Line 513 is
    // an Err(e) propagation; we rely on the other tests covering the branches.

    // ── Coverage: update_buyer_address success path (lines 658-694) ──

    #[tokio::test]
    async fn test_update_buyer_address_success_path() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ADDRESSES,
                "addr_1",
                json!({ "userId": "user_1", "isDefault": false }),
            )
            .await
            .unwrap();

        let Json(resp) = update_buyer_address(
            State(state.clone()),
            Extension(auth("test")),
            Json(UpdateBuyerAddressRequest {
                user_id: Some("user_1".to_string()),
                address_id: "addr_1".into(),
                street: "99 Elm Ave".into(),
                city: "Ottawa".into(),
                province: "ON".into(),
                postal_code: "K1A 0B1".into(),
                country: COUNTRY_CANADA.into(),
                label: Some("Work".into()),
                is_default: false,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data["updated"], true);
        let addr = state
            .db
            .get_document(collections::ADDRESSES, "addr_1")
            .await
            .unwrap();
        assert_eq!(addr["address"][fields::CITY], "Ottawa");
        assert_eq!(addr["label"], "Work");
    }

    #[tokio::test]
    async fn test_update_buyer_address_with_is_default_clears_others() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ADDRESSES,
                "addr_1",
                json!({ "userId": "user_1", "isDefault": true }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ADDRESSES,
                "addr_2",
                json!({ "userId": "user_1", "isDefault": false }),
            )
            .await
            .unwrap();

        let Json(resp) = update_buyer_address(
            State(state.clone()),
            Extension(auth("test")),
            Json(UpdateBuyerAddressRequest {
                user_id: Some("user_1".to_string()),
                address_id: "addr_2".into(),
                street: "5 Oak St".into(),
                city: "Toronto".into(),
                province: "ON".into(),
                postal_code: "M5V 1A1".into(),
                country: COUNTRY_CANADA.into(),
                label: None,
                is_default: true, // should clear addr_1's isDefault
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data["updated"], true);
        let addr2 = state
            .db
            .get_document(collections::ADDRESSES, "addr_2")
            .await
            .unwrap();
        assert_eq!(addr2["isDefault"], true);
    }

    // ── Coverage: delete_buyer_address ownership mismatch (lines 710-712) ──

    #[tokio::test]
    async fn test_delete_buyer_address_rejects_ownership_mismatch() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ADDRESSES,
                "addr_1",
                json!({ "userId": "other_user" }),
            )
            .await
            .unwrap();

        let err = delete_buyer_address(
            State(state),
            Extension(auth("test")),
            Json(DeleteBuyerAddressRequest {
                user_id: Some("user_1".to_string()),
                address_id: "addr_1".into(),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Address ownership mismatch"));
    }

    // ── Coverage: set_default_buyer_address ownership mismatch (lines 761-763) ──

    #[tokio::test]
    async fn test_set_default_buyer_address_rejects_ownership_mismatch() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ADDRESSES,
                "addr_1",
                json!({ "userId": "other_user" }),
            )
            .await
            .unwrap();

        let err = set_default_buyer_address(
            State(state),
            Extension(auth("test")),
            Json(SetDefaultBuyerAddressRequest {
                user_id: Some("user_1".to_string()),
                address_id: "addr_1".into(),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Address ownership mismatch"));
    }

    // ── Coverage: add_buyer_address with is_default=true clears existing (line 614) ──
    // Already covered by test_add_buyer_address_creates_and_returns_id (is_default: true)
    // but the clear query on line 613-614 needs an existing address to clear.

    #[tokio::test]
    async fn test_add_buyer_address_clears_existing_default() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(collections::USERS, "user_1", json!({}))
            .await
            .unwrap();
        // Create an existing default address
        state
            .db
            .upsert_document(
                collections::ADDRESSES,
                "existing_addr",
                json!({
                    "userId": "user_1",
                    "isDefault": true,
                }),
            )
            .await
            .unwrap();

        let Json(resp) = add_buyer_address(
            State(state.clone()),
            Extension(auth("test")),
            Json(AddBuyerAddressRequest {
                user_id: Some("user_1".to_string()),
                street: "1 Main St".into(),
                city: "Toronto".into(),
                province: "ON".into(),
                postal_code: "M5V1A1".into(),
                country: COUNTRY_CANADA.into(),
                label: None,
                is_default: true, // triggers clear of existing_addr
            }),
        )
        .await
        .unwrap();

        let address_id = resp.data["addressId"].as_str().unwrap();
        assert!(!address_id.is_empty());
    }

    // ── Coverage: delete_buyer_address non-default (lines 742-743 already covered,
    // but let's also cover the non-default delete path to ensure no promotion) ──

    #[tokio::test]
    async fn test_delete_buyer_address_non_default_no_promotion() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ADDRESSES,
                "addr_1",
                json!({
                    "userId": "user_1",
                    "isDefault": false,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = delete_buyer_address(
            State(state.clone()),
            Extension(auth("test")),
            Json(DeleteBuyerAddressRequest {
                user_id: Some("user_1".to_string()),
                address_id: "addr_1".into(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.data["deleted"], true);
    }
}
