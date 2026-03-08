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

    // Check if email already exists
    let existing = state
        .db
        .query_raw(&format!(
            "SELECT id FROM users WHERE email = '{}'",
            body.email.replace('\'', "''")
        ))
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
    // Find user by email
    let users = state
        .db
        .query_raw(&format!(
            "SELECT * FROM users WHERE email = '{}'",
            body.email.replace('\'', "''")
        ))
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
    let users = state
        .db
        .query_raw(&format!("SELECT * FROM users WHERE id = {}", claims.sub))
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
