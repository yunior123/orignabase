use axum::{Extension, Json, extract::State, http::HeaderMap};
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::middleware::AuthContext;
use crate::routes::AuthState;
use ob_database::fields;

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
        if let Some(id) = device[fields::ID].as_str() {
            let _ = db
                .update_document("known_devices", id, json!({ "last_used": now }))
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

/// Extract client IP from headers, respecting X-Forwarded-For/X-Real-IP only
/// when the peer address is 127.0.0.1 (Caddy proxy). When called without
/// peer info (e.g., from login tracking which only has headers), falls back
/// to peer_ip parameter.
///
/// SECURITY: Without a trusted-proxy check, any client can spoof
/// X-Forwarded-For to impersonate arbitrary IPs in login history.
pub fn extract_ip_with_peer(headers: &HeaderMap, peer_ip: Option<std::net::IpAddr>) -> String {
    let trusted_proxy = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
    let peer = peer_ip.unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));

    // Only trust forwarded headers when peer is the trusted Caddy proxy (127.0.0.1)
    if peer == trusted_proxy {
        if let Some(forwarded) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
        {
            return forwarded.trim().to_string();
        }
        if let Some(real_ip) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
            return real_ip.trim().to_string();
        }
    }

    // Not from trusted proxy — use peer IP directly
    if peer.is_unspecified() {
        "unknown".to_string()
    } else {
        peer.to_string()
    }
}

/// Legacy extract_ip for callers that don't have ConnectInfo.
/// Defaults to untrusted (peer_ip = None), which means forwarded headers
/// are NOT trusted. This is the safe default.
pub fn extract_ip(headers: &HeaderMap) -> String {
    extract_ip_with_peer(headers, None)
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

/// Records post-login telemetry and raises best-effort suspicious-device alerts.
///
/// Parameters:
/// - `db`: database client used for login history, alerts, and known-device records.
/// - `user_id`: authenticated user ID that just completed login.
/// - `headers`: request headers used to derive IP address and user-agent details.
///
/// Returns:
/// - Nothing. All persistence is best-effort and ignored on failure.
///
/// Gotchas:
/// - This function must never block or fail the primary login flow.
/// - Device identity is derived from a user-agent hash, so browser upgrades may
///   look like new devices and trigger alerts.
pub async fn on_login_success(
    db: &ob_database::DatabaseClient,
    user_id: &str,
    headers: &HeaderMap,
) {
    let ip = extract_ip_with_peer(headers, None);
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
    let ip = extract_ip_with_peer(headers, None);
    let ua = extract_user_agent(headers);
    let _ = record_login(db, user_id, &ip, &ua, "failed").await;
}

/// Record a failed login attempt by email for lockout tracking.
/// Called from the login handler for both "user not found" and "wrong password" cases.
pub async fn record_failed_login_for_lockout(db: &ob_database::DatabaseClient, email: &str) {
    let _ = record_failed_login_attempt(db, email).await;
}

// ── Account Lockout ──────────────────────────────────────────────────

/// Maximum failed login attempts before lockout.
const LOCKOUT_MAX_ATTEMPTS: i64 = 5;
/// Lockout window in seconds (15 minutes).
const LOCKOUT_WINDOW_SECS: i64 = 15 * 60;

/// Record a failed login attempt for lockout tracking.
/// Uses the `login_lockout` collection with Unix timestamps.
async fn record_failed_login_attempt(db: &ob_database::DatabaseClient, email: &str) -> Result<()> {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let result = db
        .create_document(
            "login_lockout",
            json!({
                "email": email,
                "timestamp": now_secs,
            }),
        )
        .await;
    let _ = result;
    Ok(())
}

/// Check if an email is currently locked out due to too many failed login attempts.
/// Returns `Ok(())` if the account is NOT locked out, or an `Error::Auth` if it is.
///
/// Skipped when `OB_TEST_MODE=1` to allow tests to run freely.
pub async fn check_account_lockout(db: &ob_database::DatabaseClient, email: &str) -> Result<()> {
    // Skip lockout in test mode
    if std::env::var("OB_TEST_MODE").unwrap_or_default() == "1" {
        return Ok(());
    }

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let window_start = now_secs - LOCKOUT_WINDOW_SECS;

    let results = db
        .query_bind(
            "SELECT * FROM login_lockout WHERE email = $email AND timestamp >= $window_start LIMIT 10",
            json!({
                "email": email,
                "window_start": window_start,
            }),
        )
        .await
        .unwrap_or_default();

    let count = results.len() as i64;

    if count >= LOCKOUT_MAX_ATTEMPTS {
        return Err(Error::Auth(
            "Account temporarily locked. Try again in 15 minutes.".into(),
        ));
    }

    Ok(())
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
            "SELECT * FROM login_history WHERE user_id = $uid ORDER BY created_at DESC LIMIT $lim OFFSET $off",
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
    let device = state.db.get_document("known_devices", &path.id).await?;
    if device.get("user_id").and_then(|v| v.as_str()) != Some(auth.user_id.as_str()) {
        return Err(Error::NotFound("Device not found".into()));
    }
    let _ = state.db.delete_document("known_devices", &path.id).await;

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
    let alert = state.db.get_document("security_alerts", &path.id).await?;
    if alert.get("user_id").and_then(|v| v.as_str()) != Some(auth.user_id.as_str()) {
        return Err(Error::NotFound("Alert not found".into()));
    }

    // Update acknowledged flag
    let _ = state
        .db
        .update_document("security_alerts", &path.id, json!({ "acknowledged": true }))
        .await;

    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn ob_test_mode_guard() -> MutexGuard<'static, ()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("OB_TEST_MODE test guard poisoned")
    }

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
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_compute_device_hash_empty_string() {
        let h = compute_device_hash("");
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn test_compute_device_hash_unicode() {
        let h = compute_device_hash("Mozilla/5.0 (日本語)");
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn test_short_device_name_short() {
        let name = short_device_name("Chrome/120");
        assert_eq!(name, "Chrome/120");
    }

    #[test]
    fn test_short_device_name_exactly_80() {
        let s = "A".repeat(80);
        let name = short_device_name(&s);
        assert_eq!(name, s);
    }

    #[test]
    fn test_short_device_name_81_chars() {
        let s = "A".repeat(81);
        let name = short_device_name(&s);
        assert!(name.ends_with('…'));
        assert_eq!(name.len(), 80 + '…'.len_utf8());
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

    #[test]
    fn test_extract_ip_from_x_forwarded_for_trusted_proxy() {
        // X-Forwarded-For trusted only when peer is 127.0.0.1 (Caddy)
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("1.2.3.4, 5.6.7.8"),
        );
        let trusted = Some(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        assert_eq!(extract_ip_with_peer(&headers, trusted), "1.2.3.4");
    }

    #[test]
    fn test_extract_ip_from_x_forwarded_for_untrusted_peer() {
        // X-Forwarded-For NOT trusted when peer is not localhost
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("1.2.3.4, 5.6.7.8"),
        );
        let untrusted = Some("203.0.113.50".parse::<std::net::IpAddr>().unwrap());
        assert_eq!(extract_ip_with_peer(&headers, untrusted), "203.0.113.50");
    }

    #[test]
    fn test_extract_ip_from_x_real_ip_trusted() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("10.0.0.1"));
        let trusted = Some(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        assert_eq!(extract_ip_with_peer(&headers, trusted), "10.0.0.1");
    }

    #[test]
    fn test_extract_ip_x_forwarded_for_takes_precedence() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("1.1.1.1"));
        headers.insert("x-real-ip", HeaderValue::from_static("2.2.2.2"));
        let trusted = Some(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        assert_eq!(extract_ip_with_peer(&headers, trusted), "1.1.1.1");
    }

    #[test]
    fn test_extract_ip_unknown_when_no_headers() {
        let headers = HeaderMap::new();
        assert_eq!(extract_ip(&headers), "unknown");
    }

    #[test]
    fn test_extract_ip_trims_whitespace() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("  10.0.0.1  "));
        let trusted = Some(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        assert_eq!(extract_ip_with_peer(&headers, trusted), "10.0.0.1");
    }

    #[test]
    fn test_extract_ip_no_peer_ignores_forwarded_headers() {
        // Without peer info, forwarded headers are NOT trusted (safe default)
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("1.2.3.4"));
        assert_eq!(extract_ip(&headers), "unknown");
    }

    #[test]
    fn test_extract_user_agent_present() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("Mozilla/5.0"));
        assert_eq!(extract_user_agent(&headers), "Mozilla/5.0");
    }

    #[test]
    fn test_extract_user_agent_missing() {
        let headers = HeaderMap::new();
        assert_eq!(extract_user_agent(&headers), "unknown");
    }

    #[test]
    fn test_login_record_serialization() {
        let record = LoginRecord {
            user_id: "u1".into(),
            ip: "1.2.3.4".into(),
            user_agent: "Chrome".into(),
            device_hash: "abc".into(),
            status: "success".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["user_id"], "u1");
        assert_eq!(json[fields::STATUS], "success");

        let deserialized: LoginRecord = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.user_id, "u1");
    }

    #[test]
    fn test_known_device_serialization() {
        let device = KnownDevice {
            user_id: "u1".into(),
            device_hash: "hash1".into(),
            device_name: "Chrome".into(),
            last_used: "2026-01-01T00:00:00Z".into(),
            trusted: true,
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_value(&device).unwrap();
        assert_eq!(json["trusted"], true);
        let deserialized: KnownDevice = serde_json::from_value(json).unwrap();
        assert!(deserialized.trusted);
    }

    #[test]
    fn test_security_alert_serialization() {
        let alert = SecurityAlert {
            user_id: "u1".into(),
            alert_type: "new_device".into(),
            details: "New login".into(),
            acknowledged: false,
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_value(&alert).unwrap();
        assert_eq!(json["alert_type"], "new_device");
        assert_eq!(json["acknowledged"], false);
    }

    #[test]
    fn test_pagination_query_defaults() {
        let json = serde_json::json!({});
        let query: PaginationQuery = serde_json::from_value(json).unwrap();
        assert_eq!(query.limit, 20);
        assert_eq!(query.offset, 0);
    }

    #[test]
    fn test_pagination_query_custom() {
        let json = serde_json::json!({"limit": 50, "offset": 10});
        let query: PaginationQuery = serde_json::from_value(json).unwrap();
        assert_eq!(query.limit, 50);
        assert_eq!(query.offset, 10);
    }

    #[tokio::test]
    async fn test_record_login_creates_document() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let result = record_login(&db, "user1", "1.2.3.4", "Chrome/120", "success").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_record_login_failure_status() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let result = record_login(&db, "user1", "1.2.3.4", "Chrome/120", "failed").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_upsert_known_device_creates_new() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let result = upsert_known_device(&db, "user1", "hash1", "Chrome").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_upsert_known_device_updates_existing() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let _ = upsert_known_device(&db, "user1", "hash1", "Chrome").await;
        let result = upsert_known_device(&db, "user1", "hash1", "Chrome Updated").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_upsert_known_device_different_users() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let r1 = upsert_known_device(&db, "user1", "hash1", "Chrome").await;
        let r2 = upsert_known_device(&db, "user2", "hash1", "Chrome").await;
        assert!(r1.is_ok());
        assert!(r2.is_ok());
    }

    #[tokio::test]
    async fn test_on_login_success_records_and_upserts() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("1.2.3.4"));
        headers.insert("user-agent", HeaderValue::from_static("Chrome/120"));
        on_login_success(&db, "user1", &headers).await;
    }

    #[tokio::test]
    async fn test_on_login_success_creates_alert_for_new_device() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("1.2.3.4"));
        headers.insert("user-agent", HeaderValue::from_static("NewBrowser/1.0"));
        on_login_success(&db, "user1", &headers).await;
        on_login_success(&db, "user1", &headers).await;
    }

    #[tokio::test]
    async fn test_on_login_failure_records() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("1.2.3.4"));
        headers.insert("user-agent", HeaderValue::from_static("Chrome/120"));
        on_login_failure(&db, "user1", &headers).await;
    }

    #[tokio::test]
    async fn test_on_login_failure_no_headers() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let headers = HeaderMap::new();
        on_login_failure(&db, "user1", &headers).await;
    }

    // ── Account Lockout Tests ────────────────────────────────────────

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_lockout_not_triggered_below_threshold() {
        let _guard = ob_test_mode_guard();
        // Ensure OB_TEST_MODE is NOT set for this test
        unsafe { std::env::remove_var("OB_TEST_MODE") };

        let db = ob_database::DatabaseClient::new_mem().await;
        let email = format!("lockout-test-{}@example.com", uuid::Uuid::new_v4());

        // 4 failures should NOT trigger lockout
        for _ in 0..4 {
            record_failed_login_for_lockout(&db, &email).await;
        }

        let result = check_account_lockout(&db, &email).await;
        assert!(
            result.is_ok(),
            "Account should NOT be locked after 4 failures"
        );
    }

    async fn wait_for_lockout_attempts(
        db: &ob_database::DatabaseClient,
        email: &str,
        expected_min: usize,
    ) -> usize {
        for _ in 0..20 {
            let attempts = db
                .query_bind(
                    "SELECT * FROM login_lockout WHERE email = $email",
                    json!({ "email": email }),
                )
                .await
                .unwrap_or_default();
            if attempts.len() >= expected_min {
                return attempts.len();
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        db.query_bind(
            "SELECT * FROM login_lockout WHERE email = $email",
            json!({ "email": email }),
        )
        .await
        .unwrap_or_default()
        .len()
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_lockout_triggered_after_5_failures() {
        let _guard = ob_test_mode_guard();
        unsafe { std::env::remove_var("OB_TEST_MODE") };

        let db = ob_database::DatabaseClient::new_mem().await;
        let email = format!("lockout-5-{}@example.com", uuid::Uuid::new_v4());

        // 5 failures should trigger lockout
        for _ in 0..5 {
            record_failed_login_for_lockout(&db, &email).await;
        }
        let visible = wait_for_lockout_attempts(&db, &email, 5).await;
        assert!(
            visible >= 5,
            "Expected at least 5 persisted lockout attempts, saw {visible}"
        );

        let result = check_account_lockout(&db, &email).await;
        assert!(result.is_err(), "Account SHOULD be locked after 5 failures");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Account temporarily locked"),
            "Error message should mention lockout, got: {err_msg}"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_lockout_different_emails_independent() {
        let _guard = ob_test_mode_guard();
        unsafe { std::env::remove_var("OB_TEST_MODE") };

        let db = ob_database::DatabaseClient::new_mem().await;
        let locked_email = format!("locked-{}@example.com", uuid::Uuid::new_v4());
        let unlocked_email = format!("unlocked-{}@example.com", uuid::Uuid::new_v4());

        // Lock out email_a
        for _ in 0..5 {
            record_failed_login_for_lockout(&db, &locked_email).await;
        }

        // email_b should still be fine
        let result = check_account_lockout(&db, &unlocked_email).await;
        assert!(result.is_ok(), "Different email should NOT be locked");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_lockout_expires_after_window() {
        let _guard = ob_test_mode_guard();
        unsafe { std::env::remove_var("OB_TEST_MODE") };

        let db = ob_database::DatabaseClient::new_mem().await;
        let email = format!("lockout-expire-{}@example.com", uuid::Uuid::new_v4());

        // Insert 5 failures with timestamps older than 15 minutes
        let old_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            - (16 * 60); // 16 minutes ago

        for _ in 0..5 {
            let _ = db
                .create_document(
                    "login_lockout",
                    json!({
                        "email": email,
                        "timestamp": old_timestamp,
                    }),
                )
                .await;
        }

        // Should NOT be locked — all failures are outside the 15-min window
        let result = check_account_lockout(&db, &email).await;
        assert!(result.is_ok(), "Lockout should expire after 15 minutes");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_lockout_exactly_at_threshold() {
        let _guard = ob_test_mode_guard();
        unsafe { std::env::remove_var("OB_TEST_MODE") };

        let db = ob_database::DatabaseClient::new_mem().await;
        let email = format!("lockout-exact-{}@example.com", uuid::Uuid::new_v4());

        // Exactly 5 failures should trigger lockout (threshold is >= 5)
        for _ in 0..5 {
            record_failed_login_for_lockout(&db, &email).await;
        }
        let visible = wait_for_lockout_attempts(&db, &email, 5).await;
        assert!(
            visible >= 5,
            "Expected at least 5 persisted lockout attempts, saw {visible}"
        );
        let result = check_account_lockout(&db, &email).await;
        assert!(result.is_err(), "Exactly 5 failures should trigger lockout");

        // 6 failures should also be locked
        record_failed_login_for_lockout(&db, &email).await;
        let visible = wait_for_lockout_attempts(&db, &email, 6).await;
        assert!(
            visible >= 6,
            "Expected at least 6 persisted lockout attempts, saw {visible}"
        );
        let result = check_account_lockout(&db, &email).await;
        assert!(result.is_err(), "6 failures should still be locked");
    }

    #[tokio::test]
    async fn test_record_failed_login_for_lockout_creates_document() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let email = format!("record-test-{}@example.com", uuid::Uuid::new_v4());
        record_failed_login_for_lockout(&db, &email).await;

        // Verify the document was created
        let results = db
            .query_bind(
                "SELECT * FROM login_lockout WHERE email = $email",
                json!({ "email": email }),
            )
            .await
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "Should have created exactly one lockout record"
        );
    }
}
