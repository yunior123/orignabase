use axum::{Extension, Json, extract::State, http::HeaderMap};
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::middleware::AuthContext;
use crate::routes::AuthState;

// ── Data Structures ──────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRecord {
    pub user_id: String,
    pub ip: String,
    pub user_agent: String,
    pub device_hash: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct KnownDevice {
    pub user_id: String,
    pub device_hash: String,
    pub device_name: String,
    pub last_used: String,
    pub trusted: bool,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SecurityAlert {
    pub user_id: String,
    pub alert_type: String,
    pub details: String,
    pub acknowledged: bool,
    pub created_at: String,
}

// ── Core Functions ───────────────────────────────────────────────────

/// Compute a SHA-256 hash of the user-agent string for device fingerprinting.
pub fn compute_device_hash(user_agent: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(user_agent.as_bytes());
    hex::encode(hasher.finalize())
}

/// Record a login attempt (success or failure) to the `login_history` table.
pub async fn record_login(
    db: &ob_database::DatabaseClient,
    user_id: &str,
    ip: &str,
    user_agent: &str,
    status: &str,
) -> Result<()> {
    let device_hash = compute_device_hash(user_agent);
    let record = json!({
        "user_id": user_id,
        "ip": ip,
        "user_agent": user_agent,
        "device_hash": device_hash,
        "status": status,
        "created_at": chrono::Utc::now().to_rfc3339(),
    });
    let _ = db.create_document("login_history", record).await;
    Ok(())
}

/// Check if the device is known for this user. Returns an alert type string
/// if the device is new (not in `known_devices`).
pub async fn check_suspicious(
    db: &ob_database::DatabaseClient,
    user_id: &str,
    device_hash: &str,
) -> Option<String> {
    let results = db
        .query_bind(
            "SELECT * FROM known_devices WHERE user_id = $uid AND device_hash = $dh LIMIT 1",
            json!({ "uid": user_id, "dh": device_hash }),
        )
        .await
        .ok()?;

    if results.is_empty() {
        Some("new_device".to_string())
    } else {
        None
    }
}

/// Insert a security alert into the `security_alerts` table.
pub async fn create_security_alert(
    db: &ob_database::DatabaseClient,
    user_id: &str,
    alert_type: &str,
    details: &str,
) -> Result<()> {
    let alert = json!({
        "user_id": user_id,
        "alert_type": alert_type,
        "details": details,
        "acknowledged": false,
        "created_at": chrono::Utc::now().to_rfc3339(),
    });
    let _ = db.create_document("security_alerts", alert).await;
    Ok(())
}

/// Upsert a device into the `known_devices` table for the given user.
/// If it already exists (same user + device_hash), update `last_used`.
/// Otherwise, create a new record.
pub async fn upsert_known_device(
    db: &ob_database::DatabaseClient,
    user_id: &str,
    device_hash: &str,
    device_name: &str,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    // Try to find existing device
    let existing = db
        .query_bind(
            "SELECT * FROM known_devices WHERE user_id = $uid AND device_hash = $dh LIMIT 1",
            json!({ "uid": user_id, "dh": device_hash }),
        )
        .await
        .unwrap_or_default();

    if let Some(device) = existing.first() {
        // Update last_used
        if let Some(id) = device["id"].as_str() {
            let _ = db
                .query_bind(
                    "UPDATE type::thing($id) SET last_used = $now",
                    json!({ "id": id, "now": now }),
                )
                .await;
        }
    } else {
        // Create new known device
        let device = json!({
            "user_id": user_id,
            "device_hash": device_hash,
            "device_name": device_name,
            "last_used": now,
            "trusted": true,
            "created_at": now,
        });
        let _ = db.create_document("known_devices", device).await;
    }

    Ok(())
}

// ── Helper: extract IP and User-Agent from headers ───────────────────

/// Extract client IP from headers (X-Forwarded-For > X-Real-IP > "unknown").
pub fn extract_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .or_else(|| headers.get("x-real-ip").and_then(|v| v.to_str().ok()))
        .unwrap_or("unknown")
        .trim()
        .to_string()
}

/// Extract User-Agent from headers.
pub fn extract_user_agent(headers: &HeaderMap) -> String {
    headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string()
}

/// Truncate user-agent to a short device name for display.
fn short_device_name(user_agent: &str) -> String {
    if user_agent.len() > 80 {
        format!("{}…", &user_agent[..80])
    } else {
        user_agent.to_string()
    }
}

// ── Run suspicious login check after successful auth ─────────────────

/// Called after a successful login to record history, check for suspicious
/// activity, and upsert the known device.
pub async fn on_login_success(
    db: &ob_database::DatabaseClient,
    user_id: &str,
    headers: &HeaderMap,
) {
    let ip = extract_ip(headers);
    let ua = extract_user_agent(headers);
    let device_hash = compute_device_hash(&ua);
    let device_name = short_device_name(&ua);

    // Record login (ignore errors — login tracking is best-effort)
    let _ = record_login(db, user_id, &ip, &ua, "success").await;

    // Check if device is new
    if let Some(alert_type) = check_suspicious(db, user_id, &device_hash).await {
        let details = format!("New device login from IP {ip}: {device_name}");
        let _ = create_security_alert(db, user_id, &alert_type, &details).await;
    }

    // Upsert known device
    let _ = upsert_known_device(db, user_id, &device_hash, &device_name).await;
}

/// Called after a failed login attempt.
pub async fn on_login_failure(
    db: &ob_database::DatabaseClient,
    user_id: &str,
    headers: &HeaderMap,
) {
    let ip = extract_ip(headers);
    let ua = extract_user_agent(headers);
    let _ = record_login(db, user_id, &ip, &ua, "failed").await;
}

// ── API Endpoint Handlers ────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PaginationQuery {
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

fn default_limit() -> u32 {
    20
}

/// GET /api/security/login-history — paginated login history for the current user.
pub async fn get_login_history(
    State(state): State<AuthState>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Query(pagination): axum::extract::Query<PaginationQuery>,
) -> Result<Json<serde_json::Value>> {
    if !auth.authenticated {
        return Err(Error::Auth("Authentication required".into()));
    }

    let records = state
        .db
        .query_bind(
            "SELECT * FROM login_history WHERE user_id = $uid ORDER BY created_at DESC LIMIT $lim START $off",
            json!({
                "uid": auth.user_id,
                "lim": pagination.limit,
                "off": pagination.offset,
            }),
        )
        .await?;

    Ok(Json(json!({ "data": records })))
}

/// GET /api/security/known-devices — list known devices for the current user.
pub async fn get_known_devices(
    State(state): State<AuthState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<serde_json::Value>> {
    if !auth.authenticated {
        return Err(Error::Auth("Authentication required".into()));
    }

    let devices = state
        .db
        .query_bind(
            "SELECT * FROM known_devices WHERE user_id = $uid ORDER BY last_used DESC",
            json!({ "uid": auth.user_id }),
        )
        .await?;

    Ok(Json(json!({ "data": devices })))
}

#[derive(Deserialize)]
pub struct DeviceIdPath {
    pub id: String,
}

/// DELETE /api/security/known-devices/{id} — remove a known device (user-scoped).
pub async fn delete_known_device(
    State(state): State<AuthState>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(path): axum::extract::Path<DeviceIdPath>,
) -> Result<Json<serde_json::Value>> {
    if !auth.authenticated {
        return Err(Error::Auth("Authentication required".into()));
    }

    // Verify the device belongs to the current user before deleting
    let devices = state
        .db
        .query_bind(
            "SELECT * FROM type::thing($id) WHERE user_id = $uid",
            json!({ "id": path.id, "uid": auth.user_id }),
        )
        .await?;

    if devices.is_empty() {
        return Err(Error::NotFound("Device not found".into()));
    }

    // Extract just the record key from the full SurrealDB id
    let record_id = devices[0]["id"]
        .as_str()
        .unwrap_or(&path.id);

    // Delete — use the collection + short id
    let parts: Vec<&str> = record_id.splitn(2, ':').collect();
    if parts.len() == 2 {
        let _ = state.db.delete_document(parts[0], parts[1]).await;
    } else {
        let _ = state.db.delete_document("known_devices", record_id).await;
    }

    Ok(Json(json!({ "ok": true })))
}

/// GET /api/security/alerts — list security alerts for the current user.
pub async fn get_security_alerts(
    State(state): State<AuthState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<serde_json::Value>> {
    if !auth.authenticated {
        return Err(Error::Auth("Authentication required".into()));
    }

    let alerts = state
        .db
        .query_bind(
            "SELECT * FROM security_alerts WHERE user_id = $uid ORDER BY created_at DESC",
            json!({ "uid": auth.user_id }),
        )
        .await?;

    Ok(Json(json!({ "data": alerts })))
}

#[derive(Deserialize)]
pub struct AlertIdPath {
    pub id: String,
}

/// POST /api/security/alerts/{id}/acknowledge — acknowledge a security alert.
pub async fn acknowledge_alert(
    State(state): State<AuthState>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(path): axum::extract::Path<AlertIdPath>,
) -> Result<Json<serde_json::Value>> {
    if !auth.authenticated {
        return Err(Error::Auth("Authentication required".into()));
    }

    // Verify the alert belongs to the current user
    let alerts = state
        .db
        .query_bind(
            "SELECT * FROM type::thing($id) WHERE user_id = $uid",
            json!({ "id": path.id, "uid": auth.user_id }),
        )
        .await?;

    if alerts.is_empty() {
        return Err(Error::NotFound("Alert not found".into()));
    }

    // Update acknowledged flag
    let _ = state
        .db
        .query_bind(
            "UPDATE type::thing($id) SET acknowledged = true",
            json!({ "id": path.id }),
        )
        .await;

    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_device_hash_deterministic() {
        let h1 = compute_device_hash("Mozilla/5.0 Chrome/120");
        let h2 = compute_device_hash("Mozilla/5.0 Chrome/120");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_compute_device_hash_different_agents() {
        let h1 = compute_device_hash("Chrome/120");
        let h2 = compute_device_hash("Firefox/121");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_compute_device_hash_is_hex() {
        let h = compute_device_hash("test-agent");
        assert_eq!(h.len(), 64); // SHA-256 = 32 bytes = 64 hex chars
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_short_device_name_short() {
        let name = short_device_name("Chrome/120");
        assert_eq!(name, "Chrome/120");
    }

    #[test]
    fn test_short_device_name_long() {
        let long = "A".repeat(200);
        let name = short_device_name(&long);
        assert!(name.len() < 200);
        assert!(name.ends_with('…'));
    }

    #[test]
    fn test_default_limit() {
        assert_eq!(default_limit(), 20);
    }
}
