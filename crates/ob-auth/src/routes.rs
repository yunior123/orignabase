use axum::{Json, extract::State};
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::jwt;
use crate::oauth::{self, OAuthUserInfo};
use crate::password;

/// Shared auth state injected into routes.
#[derive(Clone)]
pub struct AuthState {
    pub db: ob_database::DatabaseClient,
    pub jwt_secret: String,
    pub access_ttl: u64,
    pub refresh_ttl: u64,
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,
    pub apple_team_id: Option<String>,
    pub apple_key_id: Option<String>,
    pub apple_service_id: Option<String>,
    pub apple_private_key: Option<String>,
    pub oidc_issuer_url: Option<String>,
    pub oidc_client_id: Option<String>,
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

// ── OAuth Requests ──

#[derive(Deserialize)]
pub struct GoogleSignInRequest {
    /// Google ID token from the client (obtained via Google Sign-In SDK)
    pub id_token: String,
}

#[derive(Deserialize)]
pub struct AppleSignInRequest {
    /// Authorization code from Apple Sign-In
    pub authorization_code: String,
    /// Optional: user's name (Apple only sends this on first sign-in)
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Deserialize)]
pub struct OidcSignInRequest {
    /// OIDC access token
    pub access_token: String,
}

/// POST /auth/google — Sign in with Google.
pub async fn google_sign_in(
    State(state): State<AuthState>,
    Json(body): Json<GoogleSignInRequest>,
) -> Result<Json<AuthResponse>> {
    let client_id = state
        .google_client_id
        .as_deref()
        .ok_or_else(|| Error::Config("Google OAuth not configured".into()))?;

    let user_info = oauth::verify_google_id_token(&body.id_token, client_id).await?;
    oauth_find_or_create_user(&state, user_info).await
}

/// POST /auth/apple — Sign in with Apple.
pub async fn apple_sign_in(
    State(state): State<AuthState>,
    Json(body): Json<AppleSignInRequest>,
) -> Result<Json<AuthResponse>> {
    let service_id = state
        .apple_service_id
        .as_deref()
        .ok_or_else(|| Error::Config("Apple OAuth not configured".into()))?;
    let team_id = state
        .apple_team_id
        .as_deref()
        .ok_or_else(|| Error::Config("Apple team_id not configured".into()))?;
    let key_id = state
        .apple_key_id
        .as_deref()
        .ok_or_else(|| Error::Config("Apple key_id not configured".into()))?;
    let private_key = state
        .apple_private_key
        .as_deref()
        .ok_or_else(|| Error::Config("Apple private key not configured".into()))?;

    // Generate Apple client secret JWT
    let client_secret =
        oauth::generate_apple_client_secret(team_id, key_id, service_id, private_key)?;

    let mut user_info =
        oauth::verify_apple_auth_code(&body.authorization_code, service_id, &client_secret)
            .await?;

    // Apple only sends display_name on first sign-in (from client)
    if user_info.display_name.is_none() {
        user_info.display_name = body.display_name;
    }

    oauth_find_or_create_user(&state, user_info).await
}

/// POST /auth/oidc — Sign in with a generic OIDC provider.
pub async fn oidc_sign_in(
    State(state): State<AuthState>,
    Json(body): Json<OidcSignInRequest>,
) -> Result<Json<AuthResponse>> {
    let issuer_url = state
        .oidc_issuer_url
        .as_deref()
        .ok_or_else(|| Error::Config("OIDC not configured".into()))?;

    let user_info = oauth::verify_oidc_token(&body.access_token, issuer_url).await?;
    oauth_find_or_create_user(&state, user_info).await
}

/// Shared logic: find existing user by provider+provider_id, or create new one.
/// Returns auth tokens.
async fn oauth_find_or_create_user(
    state: &AuthState,
    info: OAuthUserInfo,
) -> Result<Json<AuthResponse>> {
    let provider = info.provider.to_string();

    // Look up by provider + provider_id
    let existing = state
        .db
        .query_bind(
            "SELECT * FROM users WHERE oauth_provider = $provider AND oauth_provider_id = $pid",
            json!({ "provider": provider, "pid": info.provider_id }),
        )
        .await?;

    let user = if let Some(user) = existing.first() {
        user.clone()
    } else {
        // Check if email exists (link accounts)
        let email_user = if let Some(ref email) = info.email {
            let results = state
                .db
                .query_bind(
                    "SELECT * FROM users WHERE email = $email",
                    json!({ "email": email }),
                )
                .await?;
            results.into_iter().next()
        } else {
            None
        };

        if let Some(mut user) = email_user {
            // Link OAuth to existing email account
            let user_id = user["id"]
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| user["id"].to_string());

            state
                .db
                .update_document(
                    "users",
                    &user_id,
                    json!({
                        "oauth_provider": provider,
                        "oauth_provider_id": info.provider_id,
                    }),
                )
                .await?;

            user["oauth_provider"] = json!(provider);
            user["oauth_provider_id"] = json!(info.provider_id);
            user
        } else {
            // Create new user
            let user_data = json!({
                "email": info.email,
                "display_name": info.display_name.unwrap_or_default(),
                "oauth_provider": provider,
                "oauth_provider_id": info.provider_id,
                "picture": info.picture,
                "roles": ["user"],
                "created_at": chrono::Utc::now().to_rfc3339(),
            });
            state.db.create_document("users", user_data).await?
        }
    };

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
    let refresh_token =
        jwt::issue_refresh_token(&user_id, &state.jwt_secret, state.refresh_ttl)?;

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

/// Build the auth router.
pub fn auth_router(state: AuthState) -> axum::Router {
    axum::Router::new()
        .route("/auth/register", axum::routing::post(register))
        .route("/auth/login", axum::routing::post(login))
        .route("/auth/refresh", axum::routing::post(refresh))
        .route("/auth/google", axum::routing::post(google_sign_in))
        .route("/auth/apple", axum::routing::post(apple_sign_in))
        .route("/auth/oidc", axum::routing::post(oidc_sign_in))
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
            let _ = &s.google_client_id;
            let _ = &s.apple_team_id;
            let _ = &s.oidc_issuer_url;
        }
    }

    // ── OAuth request deserialization ──

    #[test]
    fn test_google_sign_in_request_deserialize() {
        let json = json!({ "id_token": "REDACTED_SECRET" });
        let req: GoogleSignInRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.id_token, "REDACTED_SECRET");
    }

    #[test]
    fn test_apple_sign_in_request_deserialize() {
        let json = json!({
            "authorization_code": "auth_code_123",
            "display_name": "John Appleseed"
        });
        let req: AppleSignInRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.authorization_code, "auth_code_123");
        assert_eq!(req.display_name, Some("John Appleseed".to_string()));
    }

    #[test]
    fn test_apple_sign_in_request_without_name() {
        let json = json!({ "authorization_code": "code" });
        let req: AppleSignInRequest = serde_json::from_value(json).unwrap();
        assert!(req.display_name.is_none());
    }

    #[test]
    fn test_oidc_sign_in_request_deserialize() {
        let json = json!({ "access_token": "oidc_token_abc" });
        let req: OidcSignInRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.access_token, "oidc_token_abc");
    }
}
