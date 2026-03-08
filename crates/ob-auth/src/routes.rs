use axum::{Json, extract::State};
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::jwt;
use crate::password;

/// Shared auth state injected into routes.
#[derive(Clone)]
pub struct AuthState {
    pub db: ob_database::DatabaseClient,
    pub jwt_secret: String,
    pub access_ttl: u64,
    pub refresh_ttl: u64,
}

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub user: serde_json::Value,
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

/// POST /auth/register
pub async fn register(
    State(state): State<AuthState>,
    Json(body): Json<RegisterRequest>,
) -> Result<Json<AuthResponse>> {
    // Validate email format (basic)
    if !body.email.contains('@') || body.email.len() < 5 {
        return Err(Error::Validation("Invalid email address".into()));
    }
    if body.password.len() < 8 {
        return Err(Error::Validation(
            "Password must be at least 8 characters".into(),
        ));
    }

    // Check if email already exists (parameterized query — safe from injection)
    let existing = state
        .db
        .query_bind(
            "SELECT id FROM users WHERE email = $email",
            json!({ "email": body.email }),
        )
        .await?;
    if !existing.is_empty() {
        return Err(Error::Validation("Email already registered".into()));
    }

    // Hash password
    let password_hash = password::hash_password(&body.password)?;

    // Create user document
    let user_data = json!({
        "email": body.email,
        "password_hash": password_hash,
        "display_name": body.display_name.unwrap_or_default(),
        "roles": ["user"],
        "created_at": chrono::Utc::now().to_rfc3339(),
    });

    let user = state.db.create_document("users", user_data).await?;
    let user_id = user["id"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| user["id"].to_string());

    // Issue tokens
    let roles = vec!["user".to_string()];
    let access_token =
        jwt::issue_access_token(&user_id, &roles, &state.jwt_secret, state.access_ttl)?;
    let refresh_token = jwt::issue_refresh_token(&user_id, &state.jwt_secret, state.refresh_ttl)?;

    // Strip password hash from response
    let mut safe_user = user.clone();
    if let Some(obj) = safe_user.as_object_mut() {
        obj.remove("password_hash");
    }

    Ok(Json(AuthResponse {
        access_token,
        refresh_token,
        user: safe_user,
    }))
}

/// POST /auth/login
pub async fn login(
    State(state): State<AuthState>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<AuthResponse>> {
    // Find user by email (parameterized query — safe from injection)
    let users = state
        .db
        .query_bind(
            "SELECT * FROM users WHERE email = $email",
            json!({ "email": body.email }),
        )
        .await?;

    let user = users
        .first()
        .ok_or_else(|| Error::Auth("Invalid email or password".into()))?;

    // Verify password
    let hash = user["password_hash"]
        .as_str()
        .ok_or_else(|| Error::Auth("Invalid user record".into()))?;

    if !password::verify_password(&body.password, hash)? {
        return Err(Error::Auth("Invalid email or password".into()));
    }

    let user_id = user["id"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| user["id"].to_string());

    let roles: Vec<String> = user["roles"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let access_token =
        jwt::issue_access_token(&user_id, &roles, &state.jwt_secret, state.access_ttl)?;
    let refresh_token = jwt::issue_refresh_token(&user_id, &state.jwt_secret, state.refresh_ttl)?;

    let mut safe_user = user.clone();
    if let Some(obj) = safe_user.as_object_mut() {
        obj.remove("password_hash");
    }

    Ok(Json(AuthResponse {
        access_token,
        refresh_token,
        user: safe_user,
    }))
}

/// POST /auth/refresh
pub async fn refresh(
    State(state): State<AuthState>,
    Json(body): Json<RefreshRequest>,
) -> Result<Json<serde_json::Value>> {
    let claims = jwt::verify_token(&body.refresh_token, &state.jwt_secret)?;

    if claims.typ != "refresh" {
        return Err(Error::Auth("Invalid token type".into()));
    }

    // Look up user to get current roles
    // Use type::thing() to convert the string record ID back to a RecordId
    let users = state
        .db
        .query_bind(
            "SELECT * FROM type::thing($uid)",
            json!({ "uid": claims.sub }),
        )
        .await?;

    let user = users
        .first()
        .ok_or_else(|| Error::Auth("User not found".into()))?;

    let roles: Vec<String> = user["roles"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let access_token =
        jwt::issue_access_token(&claims.sub, &roles, &state.jwt_secret, state.access_ttl)?;
    let refresh_token =
        jwt::issue_refresh_token(&claims.sub, &state.jwt_secret, state.refresh_ttl)?;

    Ok(Json(json!({
        "access_token": access_token,
        "refresh_token": refresh_token,
    })))
}

/// Build the auth router.
pub fn auth_router(state: AuthState) -> axum::Router {
    axum::Router::new()
        .route("/auth/register", axum::routing::post(register))
        .route("/auth/login", axum::routing::post(login))
        .route("/auth/refresh", axum::routing::post(refresh))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── RegisterRequest deserialization ──────────────────────────────

    #[test]
    fn test_register_request_with_display_name() {
        let json = json!({
            "email": "user@example.com",
            "password": "secret123",
            "display_name": "Alice"
        });
        let req: RegisterRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.email, "user@example.com");
        assert_eq!(req.password, "secret123");
        assert_eq!(req.display_name, Some("Alice".to_string()));
    }

    #[test]
    fn test_register_request_without_display_name() {
        let json = json!({
            "email": "user@example.com",
            "password": "secret123"
        });
        let req: RegisterRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.display_name, None);
    }

    #[test]
    fn test_register_request_missing_email_fails() {
        let json = json!({ "password": "secret123" });
        let result = serde_json::from_value::<RegisterRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_register_request_missing_password_fails() {
        let json = json!({ "email": "a@b.com" });
        let result = serde_json::from_value::<RegisterRequest>(json);
        assert!(result.is_err());
    }

    // ── LoginRequest deserialization ─────────────────────────────────

    #[test]
    fn test_login_request_deserialize() {
        let json = json!({ "email": "a@b.com", "password": "pass1234" });
        let req: LoginRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.email, "a@b.com");
        assert_eq!(req.password, "pass1234");
    }

    #[test]
    fn test_login_request_extra_fields_ignored() {
        let raw = r#"{"email":"a@b.com","password":"pass1234","extra":"ignored"}"#;
        let req: LoginRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.email, "a@b.com");
    }

    // ── AuthResponse serialization ──────────────────────────────────

    #[test]
    fn test_auth_response_serialize() {
        let resp = AuthResponse {
            access_token: "at_123".into(),
            refresh_token: "rt_456".into(),
            user: json!({"id": "u1", "email": "a@b.com"}),
        };
        let val = serde_json::to_value(&resp).unwrap();
        assert_eq!(val["access_token"], "at_123");
        assert_eq!(val["refresh_token"], "rt_456");
        assert_eq!(val["user"]["id"], "u1");
    }

    #[test]
    fn test_auth_response_has_all_fields() {
        let resp = AuthResponse {
            access_token: "a".into(),
            refresh_token: "r".into(),
            user: json!(null),
        };
        let val = serde_json::to_value(&resp).unwrap();
        let obj = val.as_object().unwrap();
        assert!(obj.contains_key("access_token"));
        assert!(obj.contains_key("refresh_token"));
        assert!(obj.contains_key("user"));
        assert_eq!(obj.len(), 3);
    }

    // ── RefreshRequest deserialization ───────────────────────────────

    #[test]
    fn test_refresh_request_deserialize() {
        let json = json!({ "refresh_token": "rt_abc" });
        let req: RefreshRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.refresh_token, "rt_abc");
    }

    #[test]
    fn test_refresh_request_missing_token_fails() {
        let json = json!({});
        let result = serde_json::from_value::<RefreshRequest>(json);
        assert!(result.is_err());
    }

    // ── Email validation logic ──────────────────────────────────────

    /// Documents the inline validation: `!email.contains('@') || email.len() < 5`
    fn is_valid_email(e: &str) -> bool {
        e.contains('@') && e.len() >= 5
    }

    #[test]
    fn test_email_valid_basic() {
        assert!(is_valid_email("a@b.c"));   // exactly 5 chars
        assert!(is_valid_email("user@example.com"));
    }

    #[test]
    fn test_email_no_at_sign() {
        assert!(!is_valid_email("abcdef"));
    }

    #[test]
    fn test_email_too_short() {
        assert!(!is_valid_email("a@b"));    // 3 chars
        assert!(!is_valid_email("a@bc"));   // 4 chars
    }

    #[test]
    fn test_email_at_boundary() {
        assert!(is_valid_email("a@b.c"));   // 5 chars — passes
        assert!(!is_valid_email("a@bc"));   // 4 chars — fails
    }

    #[test]
    fn test_email_empty() {
        assert!(!is_valid_email(""));
    }

    #[test]
    fn test_email_only_at() {
        assert!(!is_valid_email("@"));       // 1 char
        assert!(!is_valid_email("@@@@"));    // 4 chars, has @
    }

    #[test]
    fn test_email_at_at_five_chars() {
        assert!(is_valid_email("@@@@a"));   // 5 chars, has @ — passes validation
    }

    // ── Password validation logic ───────────────────────────────────

    /// Documents the inline validation: `password.len() < 8`
    fn is_valid_password(p: &str) -> bool {
        p.len() >= 8
    }

    #[test]
    fn test_password_too_short() {
        assert!(!is_valid_password(""));
        assert!(!is_valid_password("1234567"));  // 7 chars
    }

    #[test]
    fn test_password_exact_boundary() {
        assert!(is_valid_password("12345678"));  // 8 chars — passes
        assert!(!is_valid_password("1234567"));  // 7 chars — fails
    }

    #[test]
    fn test_password_long() {
        assert!(is_valid_password("a]very$long!password?"));
    }

    // ── AuthState clone ─────────────────────────────────────────────

    #[test]
    fn test_auth_state_fields_exist() {
        // Compile-time check that AuthState has the expected fields.
        fn _assert_fields(s: &AuthState) {
            let _ = &s.db;
            let _ = &s.jwt_secret;
            let _ = &s.access_ttl;
            let _ = &s.refresh_ttl;
        }
    }
}
