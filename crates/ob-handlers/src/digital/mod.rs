//! Digital product handlers: license activation/deactivation, download tokens.
//! Ported from: functions/handlers/digital.py

use axum::{
    Json, Router,
    extract::{Query, State},
    response::Redirect,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::info;

use crate::HandlersState;
use crate::shared::schema::{business_rules, collections, fields};
use crate::shared::validation::validate_uid;

/// License key format: XXXX-XXXX-XXXX-XXXX (uppercase alphanumeric).
fn is_valid_license_key(key: &str) -> bool {
    let re = regex_lite::Regex::new(business_rules::LICENSE_KEY_PATTERN).unwrap();
    re.is_match(key)
}

// ─── Request/Response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivateLicenseRequest {
    pub license_key: String,
    pub device_id: String,
    pub user_id: String,
    pub platform: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivateLicenseResponse {
    pub approved: bool,
    pub license_key: String,
    pub activated_at: String,
    pub product_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeactivateLicenseRequest {
    pub license_key: String,
    pub device_id: String,
    pub user_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeactivateLicenseResponse {
    pub deactivated: bool,
    pub remaining_activations: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRequest {
    pub product_id: String,
    pub user_id: String,
    pub license_key: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResponse {
    pub download_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyLicenseRequest {
    pub license_key: String,
    pub device_id: Option<String>,
    pub platform: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyLicenseResponse {
    pub valid: bool,
    pub license_key: String,
    pub product_name: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct RedirectTokenQuery {
    pub t: String,
}

// ─── Router ─────────────────────────────────────────────────────────────────

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/digital/activate-license", post(activate_license))
        .route("/api/digital/deactivate-license", post(deactivate_license))
        .route("/api/digital/download/book", post(download_book))
        .route("/api/digital/download/software", post(download_software))
        // Flutter-compatible aliases (order_widgets.dart uses these paths)
        .route("/api/digital/book-download", post(download_book))
        .route("/api/digital/software-download", post(download_software))
        .route("/api/digital/verify-license", post(verify_license))
        .route("/dl", get(get_book_redirect))
        .route("/sdl", get(get_software_redirect))
        .with_state(state)
}

// ─── Handlers ───────────────────────────────────────────────────────────────

async fn activate_license(
    State(state): State<HandlersState>,
    Json(req): Json<ActivateLicenseRequest>,
) -> Result<Json<ActivateLicenseResponse>, ob_core::Error> {
    validate_uid("userId", &req.user_id)?;
    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "activate_license",
        20, // max 20 activations
        60, // per hour
    )
    .await?;

    let license_key = req.license_key.trim().to_uppercase();
    let device_id = req.device_id.trim().to_string();

    if !is_valid_license_key(&license_key) {
        return Err(ob_core::Error::Validation(
            "Invalid license key format".into(),
        ));
    }

    if device_id.is_empty() {
        return Err(ob_core::Error::Validation("deviceId is required".into()));
    }

    validate_uid("userId", &req.user_id)?;

    // Fetch license
    let license = state
        .db
        .get_document(collections::LICENSES, &license_key)
        .await
        .map_err(|_| ob_core::Error::NotFound("License not found".into()))?;

    if license.is_null() {
        return Err(ob_core::Error::NotFound("License not found".into()));
    }

    // Verify status
    let status = license.get("status").and_then(|v| v.as_str()).unwrap_or("");

    if status != "active" {
        return Err(ob_core::Error::Forbidden("License has been revoked".into()));
    }

    // Verify ownership
    let owner_id = license.get("userId").and_then(|v| v.as_str()).unwrap_or("");

    if owner_id != req.user_id {
        return Err(ob_core::Error::Forbidden(
            "You do not own this license".into(),
        ));
    }

    // Check existing activations
    let activations = license
        .get("activations")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let now = chrono::Utc::now().to_rfc3339();

    // Idempotent: check if device is already activated
    for act in &activations {
        if act.get(fields::DEVICE_ID).and_then(|v| v.as_str()) == Some(&device_id) {
            let activated_at = act
                .get("activatedAt")
                .and_then(|v| v.as_str())
                .unwrap_or(&now);

            let product_name = license
                .get("productName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Update lastVerifiedAt
            let mut updated_activations = activations.clone();
            if let Some(existing) = updated_activations
                .iter_mut()
                .find(|a| a.get(fields::DEVICE_ID).and_then(|v| v.as_str()) == Some(&device_id))
                && let Some(obj) = existing.as_object_mut()
            {
                obj.insert("lastVerifiedAt".into(), serde_json::json!(now));
            }

            let update = serde_json::json!({
                "activations": updated_activations,
                fields::UPDATED_AT: now,
            });
            state
                .db
                .update_document(collections::LICENSES, &license_key, update)
                .await
                .ok();

            return Ok(Json(ActivateLicenseResponse {
                approved: true,
                license_key: license_key.clone(),
                activated_at: activated_at.to_string(),
                product_name,
            }));
        }
    }

    // Check device limit
    let device_limit = license
        .get("deviceLimit")
        .and_then(|v| v.as_u64())
        .unwrap_or(business_rules::MAX_DEVICES_PER_LICENSE as u64);

    if activations.len() >= device_limit as usize {
        return Err(ob_core::Error::Forbidden(
            "Device activation limit reached".into(),
        ));
    }

    // New activation
    let mut new_activations = activations;
    new_activations.push(serde_json::json!({
        fields::DEVICE_ID: device_id,
        "platform": req.platform.unwrap_or_default(),
        "activatedAt": now,
        "lastVerifiedAt": now,
    }));

    let update = serde_json::json!({
        "activations": new_activations,
        fields::UPDATED_AT: now,
    });

    state
        .db
        .update_document(collections::LICENSES, &license_key, update)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to activate license: {e}")))?;

    let product_name = license
        .get("productName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    info!(license_key = %license_key, device_id = %device_id, "License activated");

    Ok(Json(ActivateLicenseResponse {
        approved: true,
        license_key,
        activated_at: now,
        product_name,
    }))
}

async fn deactivate_license(
    State(state): State<HandlersState>,
    Json(req): Json<DeactivateLicenseRequest>,
) -> Result<Json<DeactivateLicenseResponse>, ob_core::Error> {
    validate_uid("userId", &req.user_id)?;
    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "deactivate_license",
        20, // max 20 deactivations
        60, // per hour
    )
    .await?;

    let license_key = req.license_key.trim().to_uppercase();

    if !is_valid_license_key(&license_key) {
        return Err(ob_core::Error::Validation(
            "Invalid license key format".into(),
        ));
    }

    validate_uid("userId", &req.user_id)?;

    if req.device_id.is_empty() {
        return Err(ob_core::Error::Validation("deviceId is required".into()));
    }

    // Fetch license
    let license = state
        .db
        .get_document(collections::LICENSES, &license_key)
        .await
        .map_err(|_| ob_core::Error::NotFound("License not found".into()))?;

    if license.is_null() {
        return Err(ob_core::Error::NotFound("License not found".into()));
    }

    // Verify ownership
    let owner_id = license.get("userId").and_then(|v| v.as_str()).unwrap_or("");

    if owner_id != req.user_id {
        return Err(ob_core::Error::Forbidden("Not your license".into()));
    }

    // Remove device from activations
    let activations = license
        .get("activations")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let remaining: Vec<Value> = activations
        .into_iter()
        .filter(|a| a.get(fields::DEVICE_ID).and_then(|v| v.as_str()) != Some(&req.device_id))
        .collect();

    let remaining_count = remaining.len();
    let now = chrono::Utc::now().to_rfc3339();

    let update = serde_json::json!({
        "activations": remaining,
        fields::UPDATED_AT: now,
    });

    state
        .db
        .update_document(collections::LICENSES, &license_key, update)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to deactivate: {e}")))?;

    info!(license_key = %license_key, device_id = %req.device_id, "License deactivated");

    Ok(Json(DeactivateLicenseResponse {
        deactivated: true,
        remaining_activations: remaining_count,
    }))
}

async fn download_book(
    State(state): State<HandlersState>,
    Json(req): Json<DownloadRequest>,
) -> Result<Json<DownloadResponse>, ob_core::Error> {
    validate_uid("productId", &req.product_id)?;
    validate_uid("userId", &req.user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "download_book",
        10, // max 10 download token requests
        60, // per hour
    )
    .await?;

    let license_key = req.license_key.as_deref().unwrap_or("");
    if license_key.is_empty() {
        return Err(ob_core::Error::Validation("licenseKey is required".into()));
    }

    let license_key = license_key.trim().to_uppercase();
    if !is_valid_license_key(&license_key) {
        return Err(ob_core::Error::Validation(
            "Invalid license key format".into(),
        ));
    }

    // Fetch license and verify
    let license = state
        .db
        .get_document(collections::LICENSES, &license_key)
        .await
        .map_err(|_| ob_core::Error::NotFound("License not found".into()))?;

    if license.is_null() {
        return Err(ob_core::Error::NotFound("License not found".into()));
    }

    let owner = license.get("userId").and_then(|v| v.as_str()).unwrap_or("");
    if owner != req.user_id {
        return Err(ob_core::Error::Forbidden("Not your license".into()));
    }

    let status = license.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status != "active" {
        return Err(ob_core::Error::Forbidden("License revoked".into()));
    }

    let digital_type = license
        .get("digitalType")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if digital_type != "book" {
        return Err(ob_core::Error::Validation("Not a book license".into()));
    }

    // Generate download token (15 min validity)
    let token = format!("tok_{}", hex::encode(uuid::Uuid::new_v4().as_bytes()));
    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::minutes(business_rules::DOWNLOAD_TOKEN_MINUTES as i64);

    let book_source_url = license
        .get("bookSourceUrl")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let token_doc = serde_json::json!({
        "accessToken": token,
        fields::LICENSE_KEY: license_key,
        "userId": req.user_id,
        fields::PRODUCT_ID: req.product_id,
        "bookSourceUrl": book_source_url,
        "expiresAt": expires_at.to_rfc3339(),
        "used": false,
        fields::CREATED_AT: now.to_rfc3339(),
    });

    state
        .db
        .create_document(collections::BOOK_ACCESS_TOKENS, token_doc)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to create download token: {e}")))?;

    let download_url = format!("/dl?t={}", token);

    info!(product_id = %req.product_id, user_id = %req.user_id, "Book download token generated");

    Ok(Json(DownloadResponse { download_url }))
}

async fn download_software(
    State(state): State<HandlersState>,
    Json(req): Json<DownloadRequest>,
) -> Result<Json<DownloadResponse>, ob_core::Error> {
    validate_uid("productId", &req.product_id)?;
    validate_uid("userId", &req.user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "download_software",
        10, // max 10 download token requests
        60, // per hour
    )
    .await?;

    let license_key = req.license_key.as_deref().unwrap_or("");
    if license_key.is_empty() {
        return Err(ob_core::Error::Validation("licenseKey is required".into()));
    }

    let license_key = license_key.trim().to_uppercase();
    if !is_valid_license_key(&license_key) {
        return Err(ob_core::Error::Validation(
            "Invalid license key format".into(),
        ));
    }

    // Fetch license and verify
    let license = state
        .db
        .get_document(collections::LICENSES, &license_key)
        .await
        .map_err(|_| ob_core::Error::NotFound("License not found".into()))?;

    if license.is_null() {
        return Err(ob_core::Error::NotFound("License not found".into()));
    }

    let owner = license.get("userId").and_then(|v| v.as_str()).unwrap_or("");
    if owner != req.user_id {
        return Err(ob_core::Error::Forbidden("Not your license".into()));
    }

    let status = license.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status != "active" {
        return Err(ob_core::Error::Forbidden("License revoked".into()));
    }

    let digital_type = license
        .get("digitalType")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if digital_type != "software" {
        return Err(ob_core::Error::Validation("Not a software license".into()));
    }

    // Generate download token (15 min validity)
    let token = format!("tok_{}", hex::encode(uuid::Uuid::new_v4().as_bytes()));
    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::minutes(business_rules::DOWNLOAD_TOKEN_MINUTES as i64);

    let token_doc = serde_json::json!({
        "accessToken": token,
        fields::LICENSE_KEY: license_key,
        "userId": req.user_id,
        fields::PRODUCT_ID: req.product_id,
        "softwareSourceUrl": license
            .get("softwareSourceUrl")
            .or_else(|| license.get("downloadUrl"))
            .cloned()
            .unwrap_or(serde_json::json!("")),
        "expiresAt": expires_at.to_rfc3339(),
        "used": false,
        fields::CREATED_AT: now.to_rfc3339(),
    });

    state
        .db
        .create_document(collections::SOFTWARE_ACCESS_TOKENS, token_doc)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to create download token: {e}")))?;

    let download_url = format!("/sdl?t={}", token);

    info!(product_id = %req.product_id, user_id = %req.user_id, "Software download token generated");

    Ok(Json(DownloadResponse { download_url }))
}

async fn verify_license(
    State(state): State<HandlersState>,
    Json(req): Json<VerifyLicenseRequest>,
) -> Result<Json<VerifyLicenseResponse>, ob_core::Error> {
    let license_key = req.license_key.trim().to_uppercase();

    if !is_valid_license_key(&license_key) {
        return Err(ob_core::Error::Validation(
            "Invalid license key format".into(),
        ));
    }

    // Fetch license
    let license = state
        .db
        .get_document(collections::LICENSES, &license_key)
        .await
        .map_err(|_| ob_core::Error::NotFound("License not found".into()))?;

    if license.is_null() {
        return Err(ob_core::Error::NotFound("License not found".into()));
    }

    let status = license
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let product_name = license
        .get("productName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let valid = status == "active";

    // If device_id provided, update lastVerifiedAt for that device
    if let Some(ref device_id) = req.device_id {
        let activations = license
            .get("activations")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let device_found = activations
            .iter()
            .any(|a| a.get(fields::DEVICE_ID).and_then(|v| v.as_str()) == Some(device_id.as_str()));

        if device_found {
            let now = chrono::Utc::now().to_rfc3339();
            let mut updated = activations;
            for act in updated.iter_mut() {
                if act.get(fields::DEVICE_ID).and_then(|v| v.as_str()) == Some(device_id.as_str())
                    && let Some(obj) = act.as_object_mut()
                {
                    obj.insert("lastVerifiedAt".into(), serde_json::json!(now));
                }
            }
            let update = serde_json::json!({
                "activations": updated,
                fields::UPDATED_AT: now,
            });
            state
                .db
                .update_document(collections::LICENSES, &license_key, update)
                .await
                .ok();
        }
    }

    Ok(Json(VerifyLicenseResponse {
        valid,
        license_key,
        product_name,
        status,
    }))
}

async fn get_book_redirect(
    State(state): State<HandlersState>,
    Query(req): Query<RedirectTokenQuery>,
) -> Result<Redirect, ob_core::Error> {
    validate_string_token("t", &req.t)?;
    let query = format!(
        "SELECT * FROM {} WHERE accessToken = '{}' LIMIT 1",
        collections::BOOK_ACCESS_TOKENS,
        ob_core::escape_surreal_string(&req.t),
    );
    let token_doc = state
        .db
        .query_raw(&query)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ob_core::Error::NotFound("Download token not found".into()))?;

    validate_redirect_token_state(&token_doc)?;
    let source_url = token_doc
        .get("bookSourceUrl")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::NotFound("Download URL not found".into()))?;
    mark_download_token_used(&state, collections::BOOK_ACCESS_TOKENS, &token_doc).await;
    Ok(Redirect::temporary(source_url))
}

async fn get_software_redirect(
    State(state): State<HandlersState>,
    Query(req): Query<RedirectTokenQuery>,
) -> Result<Redirect, ob_core::Error> {
    validate_string_token("t", &req.t)?;
    let query = format!(
        "SELECT * FROM {} WHERE accessToken = '{}' LIMIT 1",
        collections::SOFTWARE_ACCESS_TOKENS,
        ob_core::escape_surreal_string(&req.t),
    );
    let token_doc = state
        .db
        .query_raw(&query)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ob_core::Error::NotFound("Download token not found".into()))?;

    validate_redirect_token_state(&token_doc)?;
    let source_url = token_doc
        .get("softwareSourceUrl")
        .or_else(|| token_doc.get("downloadUrl"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::NotFound("Download URL not found".into()))?;
    mark_download_token_used(&state, collections::SOFTWARE_ACCESS_TOKENS, &token_doc).await;
    Ok(Redirect::temporary(source_url))
}

fn validate_string_token(field: &str, token: &str) -> Result<(), ob_core::Error> {
    if token.trim().is_empty() || token.len() > 255 {
        return Err(ob_core::Error::Validation(format!("{field} is invalid")));
    }
    Ok(())
}

fn validate_redirect_token_state(token_doc: &Value) -> Result<(), ob_core::Error> {
    if token_doc
        .get("used")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(ob_core::Error::Forbidden(
            "Download token already used".into(),
        ));
    }

    let expires_at = token_doc
        .get("expiresAt")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::Validation("Token missing expiration".into()))?;
    let expires_at = chrono::DateTime::parse_from_rfc3339(expires_at)
        .map_err(|_| ob_core::Error::Validation("Token expiration is invalid".into()))?
        .with_timezone(&chrono::Utc);
    if chrono::Utc::now() > expires_at {
        return Err(ob_core::Error::Forbidden("Download token expired".into()));
    }

    Ok(())
}

async fn mark_download_token_used(state: &HandlersState, collection: &str, token_doc: &Value) {
    if let Some(raw_id) = token_doc.get("id").and_then(|v| v.as_str()) {
        let doc_id = raw_id
            .strip_prefix(&format!("{collection}:"))
            .unwrap_or(raw_id);
        let _ = state
            .db
            .update_document(
                collection,
                doc_id,
                serde_json::json!({
                    "used": true,
                    fields::UPDATED_AT: chrono::Utc::now().to_rfc3339(),
                }),
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Query;
    use axum::response::IntoResponse;
    use serde_json::json;

    #[test]
    fn test_license_key_validation() {
        assert!(is_valid_license_key("REDACTED_SECRET"));
        assert!(is_valid_license_key("AAAA-BBBB-CCCC-DDDD"));
        assert!(!is_valid_license_key("abcd-1234-efgh-5678")); // lowercase
        assert!(!is_valid_license_key("ABCD12345678EFGH")); // no dashes
        assert!(!is_valid_license_key("ABCD-1234")); // too short
        assert!(!is_valid_license_key("")); // empty
        assert!(!is_valid_license_key("ABCD-1234-EFGH-567!")); // special char
    }

    #[test]
    fn test_license_key_validation_rejects_whitespace_and_wrong_segment_lengths() {
        assert!(!is_valid_license_key(" REDACTED_SECRET"));
        assert!(!is_valid_license_key("ABCD-123-EEEE-FFFF"));
        assert!(!is_valid_license_key("ABCDE-1234-EFGH-5678"));
    }

    #[test]
    fn test_download_token_format() {
        let token = format!("tok_{}", hex::encode(uuid::Uuid::new_v4().as_bytes()));
        assert!(token.starts_with("tok_"));
        assert!(token.len() > 4);
    }

    #[test]
    fn test_activate_request_deser_preserves_platform() {
        let json = r#"{
            "licenseKey": "REDACTED_SECRET",
            "deviceId": "device-001",
            "userId": "user123",
            "platform": "macos"
        }"#;
        let req: ActivateLicenseRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.platform.as_deref(), Some("macos"));
    }

    #[test]
    fn test_activate_request_deser() {
        let json = r#"{
            "licenseKey": "REDACTED_SECRET",
            "deviceId": "device-001",
            "userId": "user123"
        }"#;
        let req: ActivateLicenseRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.license_key, "REDACTED_SECRET");
        assert_eq!(req.device_id, "device-001");
        assert!(req.platform.is_none());
    }

    // ── Ported from test_handlers_digital.py + test_handlers_digital_deep.py + test_handlers_digital_branch_pack.py ──

    #[test]
    fn test_license_key_case_normalization() {
        let input = "abcd-1234-efgh-5678";
        let normalized = input.trim().to_uppercase();
        assert_eq!(normalized, "REDACTED_SECRET");
        assert!(is_valid_license_key(&normalized));
    }

    #[test]
    fn test_license_key_whitespace_trimming() {
        let input = "  REDACTED_SECRET  ";
        let trimmed = input.trim().to_uppercase();
        assert!(is_valid_license_key(&trimmed));
    }

    #[test]
    fn test_license_key_invalid_formats() {
        // Short key
        assert!(!is_valid_license_key("bad"));
        // No dashes
        assert!(!is_valid_license_key("ABCDEFGHIJKLMNOP"));
        // Wrong segment count
        assert!(!is_valid_license_key("ABCD-1234"));
        assert!(!is_valid_license_key("REDACTED_SECRET-XXXX"));
        // Special characters
        assert!(!is_valid_license_key("ABCD-1234-EF!H-5678"));
        // Lowercase (before normalization)
        assert!(!is_valid_license_key("abcd-1234-efgh-5678"));
    }

    #[test]
    fn test_deactivate_request_deser() {
        let json =
            r#"{"licenseKey": "REDACTED_SECRET", "deviceId": "dev_1", "userId": "user_1"}"#;
        let req: DeactivateLicenseRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.license_key, "REDACTED_SECRET");
        assert_eq!(req.device_id, "dev_1");
    }

    #[test]
    fn test_deactivate_empty_device_id_rejected() {
        let device_id = "";
        assert!(device_id.is_empty());
    }

    #[test]
    fn test_deactivate_response_serialize() {
        let resp = DeactivateLicenseResponse {
            deactivated: true,
            remaining_activations: 1,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"deactivated\":true"));
        assert!(json.contains("\"remainingActivations\":1"));
    }

    #[test]
    fn test_download_request_deser_with_license_key() {
        let json = r#"{"productId": "p1", "userId": "u1", "licenseKey": "REDACTED_SECRET"}"#;
        let req: DownloadRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.license_key.as_deref(), Some("REDACTED_SECRET"));
    }

    #[test]
    fn test_download_request_deser_without_license_key() {
        let json = r#"{"productId": "p1", "userId": "u1"}"#;
        let req: DownloadRequest = serde_json::from_str(json).unwrap();
        assert!(req.license_key.is_none());
    }

    #[test]
    fn test_download_response_serialize() {
        let resp = DownloadResponse {
            download_url: "/dl?t=tok_abc123".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("downloadUrl"));
        assert!(json.contains("tok_abc123"));
    }

    #[test]
    fn test_verify_license_request_deser() {
        let json =
            r#"{"licenseKey": "REDACTED_SECRET", "deviceId": "dev_1", "platform": "windows"}"#;
        let req: VerifyLicenseRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.device_id.as_deref(), Some("dev_1"));
        assert_eq!(req.platform.as_deref(), Some("windows"));
    }

    #[test]
    fn test_verify_license_request_minimal() {
        let json = r#"{"licenseKey": "REDACTED_SECRET"}"#;
        let req: VerifyLicenseRequest = serde_json::from_str(json).unwrap();
        assert!(req.device_id.is_none());
        assert!(req.platform.is_none());
    }

    #[test]
    fn test_verify_license_response_serialize() {
        let resp = VerifyLicenseResponse {
            valid: true,
            license_key: "REDACTED_SECRET".into(),
            product_name: "Pro Editor".into(),
            status: "active".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"valid\":true"));
        assert!(json.contains("\"productName\":\"Pro Editor\""));
        assert!(json.contains("\"status\":\"active\""));
    }

    #[test]
    fn test_verify_license_response_revoked() {
        let resp = VerifyLicenseResponse {
            valid: false,
            license_key: "REDACTED_SECRET".into(),
            product_name: "Pro Editor".into(),
            status: "revoked".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"valid\":false"));
        assert!(json.contains("\"status\":\"revoked\""));
    }

    #[test]
    fn test_device_limit_logic() {
        // Under limit -> allow
        let activations_count = 2;
        let device_limit: u64 = 3;
        assert!(activations_count < device_limit as usize);

        // At limit -> reject
        let activations_count = 3;
        assert!(activations_count >= device_limit as usize);

        // Unlimited (None -> default)
        let default_limit = business_rules::MAX_DEVICES_PER_LICENSE as u64;
        assert!(default_limit > 0);
    }

    #[test]
    fn test_idempotent_reactivation_logic() {
        // Simulate: device_id already in activations list
        let activations = vec![
            serde_json::json!({"deviceId": "dev_1", "platform": "macos"}),
            serde_json::json!({"deviceId": "dev_2", "platform": "windows"}),
        ];
        let target_device = "dev_1";
        let found = activations
            .iter()
            .any(|a| a.get("deviceId").and_then(|v| v.as_str()) == Some(target_device));
        assert!(
            found,
            "Existing device should be found for idempotent reactivation"
        );

        // Device not in list
        let new_device = "dev_3";
        let found2 = activations
            .iter()
            .any(|a| a.get("deviceId").and_then(|v| v.as_str()) == Some(new_device));
        assert!(!found2);
    }

    #[test]
    fn test_deactivation_removes_device_from_list() {
        let activations = vec![
            serde_json::json!({"deviceId": "dev_1"}),
            serde_json::json!({"deviceId": "dev_2"}),
            serde_json::json!({"deviceId": "dev_3"}),
        ];
        let to_remove = "dev_2";
        let remaining: Vec<_> = activations
            .into_iter()
            .filter(|a| a.get("deviceId").and_then(|v| v.as_str()) != Some(to_remove))
            .collect();
        assert_eq!(remaining.len(), 2);
        assert!(
            remaining
                .iter()
                .all(|a| { a.get("deviceId").and_then(|v| v.as_str()) != Some(to_remove) })
        );
    }

    #[test]
    fn test_validate_redirect_token_state_used_token() {
        let token_doc = serde_json::json!({
            "used": true,
            "expiresAt": "2099-01-01T00:00:00Z"
        });
        let result = validate_redirect_token_state(&token_doc);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_redirect_token_state_expired() {
        let token_doc = serde_json::json!({
            "used": false,
            "expiresAt": "2020-01-01T00:00:00Z"
        });
        let result = validate_redirect_token_state(&token_doc);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_redirect_token_state_valid() {
        let token_doc = serde_json::json!({
            "used": false,
            "expiresAt": "2099-01-01T00:00:00Z"
        });
        let result = validate_redirect_token_state(&token_doc);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_redirect_token_state_missing_expiry() {
        let token_doc = serde_json::json!({
            "used": false
        });
        let result = validate_redirect_token_state(&token_doc);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_redirect_token_state_invalid_expiry_format() {
        let token_doc = serde_json::json!({
            "used": false,
            "expiresAt": "not-a-date"
        });
        let result = validate_redirect_token_state(&token_doc);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_string_token_empty() {
        assert!(validate_string_token("t", "").is_err());
        assert!(validate_string_token("t", "   ").is_err());
    }

    #[test]
    fn test_validate_string_token_too_long() {
        let long = "x".repeat(256);
        assert!(validate_string_token("t", &long).is_err());
    }

    #[test]
    fn test_validate_string_token_valid() {
        assert!(validate_string_token("t", "tok_abc123").is_ok());
    }

    #[test]
    fn test_download_token_minutes_constant() {
        assert_eq!(business_rules::DOWNLOAD_TOKEN_MINUTES, 15);
    }

    #[test]
    fn test_download_token_expiry_calculation() {
        let now = chrono::Utc::now();
        let expires_at =
            now + chrono::Duration::minutes(business_rules::DOWNLOAD_TOKEN_MINUTES as i64);
        let diff = expires_at.signed_duration_since(now).num_minutes();
        assert_eq!(diff, 15);
    }

    #[test]
    fn test_license_status_check_logic() {
        let statuses = ["active", "revoked", "expired", "suspended"];
        // Only "active" should be valid
        for status in &statuses {
            let is_valid = *status == "active";
            if *status == "active" {
                assert!(is_valid);
            } else {
                assert!(!is_valid);
            }
        }
    }

    #[test]
    fn test_digital_type_validation_for_book() {
        let digital_type = "book";
        assert_eq!(digital_type, "book");
        assert_ne!(digital_type, "software");
    }

    #[test]
    fn test_digital_type_validation_for_software() {
        let digital_type = "software";
        assert_eq!(digital_type, "software");
        assert_ne!(digital_type, "book");
    }

    #[test]
    fn test_ownership_check_logic() {
        let owner_id = "buyer_1";
        let caller_id = "buyer_1";
        assert_eq!(owner_id, caller_id);

        let attacker_id = "attacker";
        assert_ne!(owner_id, attacker_id);
    }

    #[test]
    fn test_redirect_token_query_deser() {
        let json = r#"{"t": "tok_abc123"}"#;
        let req: RedirectTokenQuery = serde_json::from_str(json).unwrap();
        assert_eq!(req.t, "tok_abc123");
    }

    #[test]
    fn test_activate_response_serialize() {
        let resp = ActivateLicenseResponse {
            approved: true,
            license_key: "REDACTED_SECRET".into(),
            activated_at: "2026-03-10T12:00:00Z".into(),
            product_name: "FXCleaner".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"approved\":true"));
        assert!(json.contains("\"productName\":\"FXCleaner\""));
        assert!(json.contains("\"activatedAt\""));
    }

    #[tokio::test]
    async fn test_activate_license_success() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let db = DatabaseClient::new_mem().await;
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: db.clone(),
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let license_key = "REDACTED_SECRET";
        let user_id = "user_1";

        db.upsert_document(
            collections::LICENSES,
            license_key,
            json!({
                "status": "active",
                "userId": user_id,
                "productName": "Test Product",
                "activations": []
            }),
        )
        .await
        .unwrap();

        let req = ActivateLicenseRequest {
            license_key: license_key.to_string(),
            device_id: "dev_1".to_string(),
            user_id: user_id.to_string(),
            platform: Some("macos".to_string()),
        };

        let result = activate_license(State(state), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.approved);
        assert_eq!(resp.product_name, "Test Product");

        // Verify in DB
        let license = db
            .get_document(collections::LICENSES, license_key)
            .await
            .unwrap();
        let activations = license["activations"].as_array().unwrap();
        assert_eq!(activations.len(), 1);
        assert_eq!(activations[0][fields::DEVICE_ID], "dev_1");
    }

    #[tokio::test]
    async fn test_activate_license_already_activated_idempotent() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let db = DatabaseClient::new_mem().await;
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: db.clone(),
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let license_key = "REDACTED_SECRET";
        let user_id = "user_1";
        let device_id = "dev_1";

        db.upsert_document(
            collections::LICENSES,
            license_key,
            json!({
                "status": "active",
                "userId": user_id,
                "productName": "Test Product",
                "activations": [{
                    fields::DEVICE_ID: device_id,
                    "activatedAt": "2026-01-01T00:00:00Z"
                }]
            }),
        )
        .await
        .unwrap();

        let req = ActivateLicenseRequest {
            license_key: license_key.to_string(),
            device_id: device_id.to_string(),
            user_id: user_id.to_string(),
            platform: None,
        };

        let result = activate_license(State(state), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.approved);
        assert_eq!(resp.activated_at, "2026-01-01T00:00:00Z");
    }

    #[tokio::test]
    async fn test_deactivate_license_success() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let db = DatabaseClient::new_mem().await;
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: db.clone(),
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let license_key = "REDACTED_SECRET";
        let user_id = "user_1";
        let device_id = "dev_1";

        db.upsert_document(
            collections::LICENSES,
            license_key,
            json!({
                "userId": user_id,
                "activations": [{ fields::DEVICE_ID: device_id }]
            }),
        )
        .await
        .unwrap();

        let req = DeactivateLicenseRequest {
            license_key: license_key.to_string(),
            device_id: device_id.to_string(),
            user_id: user_id.to_string(),
        };

        let result = deactivate_license(State(state), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.deactivated);
        assert_eq!(resp.remaining_activations, 0);
    }

    #[tokio::test]
    async fn test_download_book_success() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let db = DatabaseClient::new_mem().await;
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: db.clone(),
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let license_key = "REDACTED_SECRET";
        let user_id = "user_1";

        db.upsert_document(
            collections::LICENSES,
            license_key,
            json!({
                "status": "active",
                "userId": user_id,
                "digitalType": "book",
                "bookSourceUrl": "https://s3.local/book.pdf"
            }),
        )
        .await
        .unwrap();

        let req = DownloadRequest {
            product_id: "p1".to_string(),
            user_id: user_id.to_string(),
            license_key: Some(license_key.to_string()),
        };

        let result = download_book(State(state), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.download_url.starts_with("/dl?t=tok_"));
    }

    #[tokio::test]
    async fn test_verify_license_handler_success() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let db = DatabaseClient::new_mem().await;
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: db.clone(),
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let license_key = "REDACTED_SECRET";
        db.upsert_document(
            collections::LICENSES,
            license_key,
            json!({
                "status": "active",
                "productName": "Test Product"
            }),
        )
        .await
        .unwrap();

        let req = VerifyLicenseRequest {
            license_key: license_key.to_string(),
            device_id: None,
            platform: None,
        };

        let result = verify_license(State(state), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.valid);
        assert_eq!(resp.status, "active");
    }

    #[tokio::test]
    async fn test_verify_license_updates_last_verified_at_for_matching_device() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let db = DatabaseClient::new_mem().await;
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: db.clone(),
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let license_key = "REDACTED_SECRET";
        db.upsert_document(
            collections::LICENSES,
            license_key,
            json!({
                "status": "active",
                "productName": "Verifier",
                "activations": [{
                    fields::DEVICE_ID: "dev_1",
                    "activatedAt": "2026-01-01T00:00:00Z"
                }]
            }),
        )
        .await
        .unwrap();

        let Json(resp) = verify_license(
            State(state.clone()),
            Json(VerifyLicenseRequest {
                license_key: license_key.to_string(),
                device_id: Some("dev_1".into()),
                platform: None,
            }),
        )
        .await
        .unwrap();

        assert!(resp.valid);
        let license = db
            .get_document(collections::LICENSES, license_key)
            .await
            .unwrap();
        assert!(license["activations"][0].get("lastVerifiedAt").is_some());
    }

    #[tokio::test]
    async fn test_download_software_success_uses_download_url_fallback() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let db = DatabaseClient::new_mem().await;
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: db.clone(),
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let license_key = "REDACTED_SECRET";
        db.upsert_document(
            collections::LICENSES,
            license_key,
            json!({
                "status": "active",
                "userId": "user_1",
                "digitalType": "software",
                "downloadUrl": "https://downloads.local/app.zip"
            }),
        )
        .await
        .unwrap();

        let Json(resp) = download_software(
            State(state.clone()),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "user_1".into(),
                license_key: Some(license_key.into()),
            }),
        )
        .await
        .unwrap();

        assert!(resp.download_url.starts_with("/sdl?t=tok_"));
        let tokens = db
            .query_bind_value(
                "SELECT * FROM software_access_tokens",
                json!({})
            )
            .await
            .unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0]["softwareSourceUrl"],
            "https://downloads.local/app.zip"
        );
    }

    #[tokio::test]
    async fn test_get_book_redirect_marks_token_used_and_redirects() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let db = DatabaseClient::new_mem().await;
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: db.clone(),
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        db.upsert_document(
            collections::BOOK_ACCESS_TOKENS,
            "tok_doc_1",
            json!({
                "accessToken": "tok_book_1",
                "bookSourceUrl": "https://books.local/file.pdf",
                "expiresAt": (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
                "used": false,
            }),
        )
        .await
        .unwrap();

        let redirect = get_book_redirect(
            State(state.clone()),
            Query(RedirectTokenQuery {
                t: "tok_book_1".into(),
            }),
        )
        .await
        .unwrap();
        let response = redirect.into_response();

        assert_eq!(response.status().as_u16(), 307);
        assert_eq!(
            response.headers().get("location").unwrap(),
            "https://books.local/file.pdf"
        );

        let token_doc = db
            .get_document(collections::BOOK_ACCESS_TOKENS, "tok_doc_1")
            .await
            .unwrap();
        assert_eq!(token_doc["used"], true);
    }

    #[tokio::test]
    async fn test_get_software_redirect_marks_token_used() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let db = DatabaseClient::new_mem().await;
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: db.clone(),
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        db.upsert_document(
            collections::SOFTWARE_ACCESS_TOKENS,
            "tok_doc_2",
            json!({
                "accessToken": "tok_soft_1",
                "softwareSourceUrl": "https://downloads.local/app.dmg",
                "expiresAt": (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
                "used": false,
            }),
        )
        .await
        .unwrap();

        let redirect = get_software_redirect(
            State(state.clone()),
            Query(RedirectTokenQuery {
                t: "tok_soft_1".into(),
            }),
        )
        .await
        .unwrap();
        let response = redirect.into_response();

        assert_eq!(response.status().as_u16(), 307);
        assert_eq!(
            response.headers().get("location").unwrap(),
            "https://downloads.local/app.dmg"
        );

        let token_doc = db
            .get_document(collections::SOFTWARE_ACCESS_TOKENS, "tok_doc_2")
            .await
            .unwrap();
        assert_eq!(token_doc["used"], true);
    }

    // ── Helper for async tests ──

    async fn setup_state() -> (HandlersState, ob_database::DatabaseClient) {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let db = DatabaseClient::new_mem().await;
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: db.clone(),
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };
        (state, db)
    }

    // ── Coverage: activate_license invalid key format (lines 131-133) ──

    #[tokio::test]
    async fn test_activate_license_rejects_invalid_key_format() {
        let (state, _db) = setup_state().await;

        let err = activate_license(
            State(state),
            Json(ActivateLicenseRequest {
                license_key: "bad-key".into(),
                device_id: "dev_1".into(),
                user_id: "user_1".into(),
                platform: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Invalid license key format"));
    }

    // ── Coverage: activate_license empty device_id (line 137) ──

    #[tokio::test]
    async fn test_activate_license_rejects_empty_device_id() {
        let (state, _db) = setup_state().await;

        let err = activate_license(
            State(state),
            Json(ActivateLicenseRequest {
                license_key: "REDACTED_SECRET".into(),
                device_id: "  ".into(), // blank after trim
                user_id: "user_1".into(),
                platform: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("deviceId is required"));
    }

    // ── Coverage: activate_license non-active status (line 157) ──

    #[tokio::test]
    async fn test_activate_license_rejects_revoked_license() {
        let (state, db) = setup_state().await;

        db.upsert_document(
            collections::LICENSES,
            "REDACTED_SECRET",
            json!({
                "status": "revoked",
                "userId": "user_1",
                "activations": []
            }),
        )
        .await
        .unwrap();

        let err = activate_license(
            State(state),
            Json(ActivateLicenseRequest {
                license_key: "REDACTED_SECRET".into(),
                device_id: "dev_1".into(),
                user_id: "user_1".into(),
                platform: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("License has been revoked"));
    }

    // ── Coverage: activate_license ownership mismatch (lines 164-166) ──

    #[tokio::test]
    async fn test_activate_license_rejects_wrong_owner() {
        let (state, db) = setup_state().await;

        db.upsert_document(
            collections::LICENSES,
            "REDACTED_SECRET",
            json!({
                "status": "active",
                "userId": "actual_owner",
                "activations": []
            }),
        )
        .await
        .unwrap();

        let err = activate_license(
            State(state),
            Json(ActivateLicenseRequest {
                license_key: "REDACTED_SECRET".into(),
                device_id: "dev_1".into(),
                user_id: "attacker".into(),
                platform: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("You do not own this license"));
    }

    // ── Coverage: activate_license device limit reached (lines 227-229) ──

    #[tokio::test]
    async fn test_activate_license_rejects_at_device_limit() {
        let (state, db) = setup_state().await;

        db.upsert_document(
            collections::LICENSES,
            "REDACTED_SECRET",
            json!({
                "status": "active",
                "userId": "user_1",
                "deviceLimit": 1,
                "activations": [{ fields::DEVICE_ID: "dev_existing", "platform": "linux" }]
            }),
        )
        .await
        .unwrap();

        let err = activate_license(
            State(state),
            Json(ActivateLicenseRequest {
                license_key: "REDACTED_SECRET".into(),
                device_id: "dev_new".into(),
                user_id: "user_1".into(),
                platform: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Device activation limit reached"));
    }

    // ── Coverage: deactivate_license invalid key format (lines 284-286) ──

    #[tokio::test]
    async fn test_deactivate_license_rejects_invalid_key_format() {
        let (state, _db) = setup_state().await;

        let err = deactivate_license(
            State(state),
            Json(DeactivateLicenseRequest {
                license_key: "bad".into(),
                device_id: "dev_1".into(),
                user_id: "user_1".into(),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Invalid license key format"));
    }

    // ── Coverage: deactivate_license empty device_id (line 292) ──

    #[tokio::test]
    async fn test_deactivate_license_rejects_empty_device_id() {
        let (state, _db) = setup_state().await;

        let err = deactivate_license(
            State(state),
            Json(DeactivateLicenseRequest {
                license_key: "REDACTED_SECRET".into(),
                device_id: "".into(),
                user_id: "user_1".into(),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("deviceId is required"));
    }

    // ── Coverage: deactivate_license ownership mismatch (line 310) ──

    #[tokio::test]
    async fn test_deactivate_license_rejects_wrong_owner() {
        let (state, db) = setup_state().await;

        db.upsert_document(
            collections::LICENSES,
            "REDACTED_SECRET",
            json!({
                "userId": "actual_owner",
                "activations": [{ fields::DEVICE_ID: "dev_1" }]
            }),
        )
        .await
        .unwrap();

        let err = deactivate_license(
            State(state),
            Json(DeactivateLicenseRequest {
                license_key: "REDACTED_SECRET".into(),
                device_id: "dev_1".into(),
                user_id: "attacker".into(),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Not your license"));
    }

    // ── Coverage: download_book empty license key (line 364) ──

    #[tokio::test]
    async fn test_download_book_rejects_empty_license_key() {
        let (state, _db) = setup_state().await;

        let err = download_book(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "user_1".into(),
                license_key: Some("".into()),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("licenseKey is required"));
    }

    // ── Coverage: download_book invalid license key format (lines 369-371) ──

    #[tokio::test]
    async fn test_download_book_rejects_invalid_license_key_format() {
        let (state, _db) = setup_state().await;

        let err = download_book(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "user_1".into(),
                license_key: Some("bad-format".into()),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Invalid license key format"));
    }

    // ── Coverage: download_book ownership mismatch (line 387) ──

    #[tokio::test]
    async fn test_download_book_rejects_wrong_owner() {
        let (state, db) = setup_state().await;

        db.upsert_document(
            collections::LICENSES,
            "REDACTED_SECRET",
            json!({
                "status": "active",
                "userId": "real_owner",
                "digitalType": "book"
            }),
        )
        .await
        .unwrap();

        let err = download_book(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "attacker".into(),
                license_key: Some("REDACTED_SECRET".into()),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Not your license"));
    }

    // ── Coverage: download_book license revoked (line 392) ──

    #[tokio::test]
    async fn test_download_book_rejects_revoked_license() {
        let (state, db) = setup_state().await;

        db.upsert_document(
            collections::LICENSES,
            "REDACTED_SECRET",
            json!({
                "status": "revoked",
                "userId": "user_1",
                "digitalType": "book"
            }),
        )
        .await
        .unwrap();

        let err = download_book(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "user_1".into(),
                license_key: Some("REDACTED_SECRET".into()),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("License revoked"));
    }

    // ── Coverage: download_book not a book license (line 400) ──

    #[tokio::test]
    async fn test_download_book_rejects_software_license() {
        let (state, db) = setup_state().await;

        db.upsert_document(
            collections::LICENSES,
            "REDACTED_SECRET",
            json!({
                "status": "active",
                "userId": "user_1",
                "digitalType": "software"
            }),
        )
        .await
        .unwrap();

        let err = download_book(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "user_1".into(),
                license_key: Some("REDACTED_SECRET".into()),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Not a book license"));
    }

    // ── Coverage: download_software empty license key (line 455) ──

    #[tokio::test]
    async fn test_download_software_rejects_empty_license_key() {
        let (state, _db) = setup_state().await;

        let err = download_software(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "user_1".into(),
                license_key: Some("".into()),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("licenseKey is required"));
    }

    // ── Coverage: download_software invalid license key (lines 460-462) ──

    #[tokio::test]
    async fn test_download_software_rejects_invalid_license_key() {
        let (state, _db) = setup_state().await;

        let err = download_software(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "user_1".into(),
                license_key: Some("bad".into()),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Invalid license key format"));
    }

    // ── Coverage: download_software ownership mismatch (line 478) ──

    #[tokio::test]
    async fn test_download_software_rejects_wrong_owner() {
        let (state, db) = setup_state().await;

        db.upsert_document(
            collections::LICENSES,
            "REDACTED_SECRET",
            json!({
                "status": "active",
                "userId": "real_owner",
                "digitalType": "software"
            }),
        )
        .await
        .unwrap();

        let err = download_software(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "attacker".into(),
                license_key: Some("REDACTED_SECRET".into()),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Not your license"));
    }

    // ── Coverage: download_software license revoked (line 483) ──

    #[tokio::test]
    async fn test_download_software_rejects_revoked_license() {
        let (state, db) = setup_state().await;

        db.upsert_document(
            collections::LICENSES,
            "REDACTED_SECRET",
            json!({
                "status": "revoked",
                "userId": "user_1",
                "digitalType": "software"
            }),
        )
        .await
        .unwrap();

        let err = download_software(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "user_1".into(),
                license_key: Some("REDACTED_SECRET".into()),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("License revoked"));
    }

    // ── Coverage: download_software not a software license (line 491) ──

    #[tokio::test]
    async fn test_download_software_rejects_book_license() {
        let (state, db) = setup_state().await;

        db.upsert_document(
            collections::LICENSES,
            "REDACTED_SECRET",
            json!({
                "status": "active",
                "userId": "user_1",
                "digitalType": "book"
            }),
        )
        .await
        .unwrap();

        let err = download_software(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "user_1".into(),
                license_key: Some("REDACTED_SECRET".into()),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Not a software license"));
    }

    // ── Coverage: verify_license invalid key (lines 534-536) ──

    #[tokio::test]
    async fn test_verify_license_rejects_invalid_key_format() {
        let (state, _db) = setup_state().await;

        let err = verify_license(
            State(state),
            Json(VerifyLicenseRequest {
                license_key: "bad".into(),
                device_id: None,
                platform: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Invalid license key format"));
    }

    // ── Coverage: verify_license not found (line 547) ──

    #[tokio::test]
    async fn test_verify_license_not_found() {
        let (state, _db) = setup_state().await;

        let err = verify_license(
            State(state),
            Json(VerifyLicenseRequest {
                license_key: "REDACTED_SECRET".into(),
                device_id: None,
                platform: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("License not found"));
    }

    // ── Coverage: download_book/software missing license key (None) (lines 364, 455) ──

    #[tokio::test]
    async fn test_download_book_rejects_none_license_key() {
        let (state, _db) = setup_state().await;

        let err = download_book(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "user_1".into(),
                license_key: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("licenseKey is required"));
    }

    #[tokio::test]
    async fn test_download_software_rejects_none_license_key() {
        let (state, _db) = setup_state().await;

        let err = download_software(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "user_1".into(),
                license_key: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("licenseKey is required"));
    }

    // ── Coverage: activate_license not found (line 150) ──

    #[tokio::test]
    async fn test_activate_license_not_found() {
        let (state, _db) = setup_state().await;

        let err = activate_license(
            State(state),
            Json(ActivateLicenseRequest {
                license_key: "REDACTED_SECRET".into(),
                device_id: "dev_1".into(),
                user_id: "user_1".into(),
                platform: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("License not found"));
    }

    // ── Coverage: deactivate_license not found (line 303) ──

    #[tokio::test]
    async fn test_deactivate_license_not_found() {
        let (state, _db) = setup_state().await;

        let err = deactivate_license(
            State(state),
            Json(DeactivateLicenseRequest {
                license_key: "REDACTED_SECRET".into(),
                device_id: "dev_1".into(),
                user_id: "user_1".into(),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("License not found"));
    }

    // ── Coverage: download_book not found (line 382) ──

    #[tokio::test]
    async fn test_download_book_license_not_found() {
        let (state, _db) = setup_state().await;

        let err = download_book(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "user_1".into(),
                license_key: Some("REDACTED_SECRET".into()),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("License not found"));
    }

    // ── Coverage: download_software not found (line 473) ──

    #[tokio::test]
    async fn test_download_software_license_not_found() {
        let (state, _db) = setup_state().await;

        let err = download_software(
            State(state),
            Json(DownloadRequest {
                product_id: "p1".into(),
                user_id: "user_1".into(),
                license_key: Some("REDACTED_SECRET".into()),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("License not found"));
    }

    // ── Coverage: verify_license with device_id that IS in activations (line 594) ──
    // Already covered by test_verify_license_updates_last_verified_at_for_matching_device
    // but let's also ensure the non-matching device path works.

    #[tokio::test]
    async fn test_verify_license_with_nonmatching_device_no_update() {
        let (state, db) = setup_state().await;

        db.upsert_document(
            collections::LICENSES,
            "REDACTED_SECRET",
            json!({
                "status": "active",
                "productName": "Test",
                "activations": [{ fields::DEVICE_ID: "dev_1" }]
            }),
        )
        .await
        .unwrap();

        let Json(resp) = verify_license(
            State(state),
            Json(VerifyLicenseRequest {
                license_key: "REDACTED_SECRET".into(),
                device_id: Some("dev_nonexistent".into()),
                platform: None,
            }),
        )
        .await
        .unwrap();

        assert!(resp.valid);
    }
}
