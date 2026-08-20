//! FCM push notification service.
//!
//! Provides `send_push()` using the FCM HTTP v1 API with a real RS256 service
//! account JWT exchange. Tests can override the FCM base URL via
//! `OB_FCM_API_BASE_URL`.

use chrono::Utc;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::shared::schema::business_rules;

mod fcm_fields {
    pub const MESSAGE: &str = "message";
    pub const TOKEN: &str = "token";
    pub const NOTIFICATION: &str = "notification";
    pub const TITLE: &str = "title";
    pub const BODY: &str = "body";
    pub const DATA: &str = "data";
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum PushError {
    #[error("FCM API error (status {status}): {body}")]
    FcmApi { status: u16, body: String },
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Missing service account JSON")]
    MissingServiceAccount,
    #[error("Invalid service account JSON: {0}")]
    InvalidServiceAccount(String),
    #[error("OAuth2 token fetch failed: {0}")]
    OAuth2(String),
    #[error("Rate limit exceeded for user (max {0}/day)")]
    RateLimitExceeded(u32),
}

pub type Result<T> = std::result::Result<T, PushError>;

// ---------------------------------------------------------------------------
// OAuth2 JWT for service account
// ---------------------------------------------------------------------------

/// Minimal JWT claims for Google OAuth2.
#[derive(Serialize)]
struct JwtClaims {
    iss: String,
    scope: String,
    aud: String,
    iat: i64,
    exp: i64,
}

/// Service account key fields we need.
#[derive(Deserialize)]
struct ServiceAccountKey {
    client_email: String,
    private_key: String,
    token_uri: String,
}

/// Build a signed JWT and exchange it for an OAuth2 access token.
async fn get_access_token(
    http_client: &reqwest::Client,
    service_account_json: &str,
) -> Result<String> {
    let sa: ServiceAccountKey = serde_json::from_str(service_account_json)
        .map_err(|e| PushError::InvalidServiceAccount(e.to_string()))?;

    let now = Utc::now().timestamp();
    let claims = JwtClaims {
        iss: sa.client_email.clone(),
        scope: "https://www.googleapis.com/auth/firebase.messaging".into(),
        aud: sa.token_uri.clone(),
        iat: now,
        exp: now + 3600,
    };

    let key = EncodingKey::from_rsa_pem(sa.private_key.as_bytes())
        .map_err(|e| PushError::InvalidServiceAccount(format!("Invalid RSA private key: {e}")))?;
    let header = Header::new(Algorithm::RS256);
    let jwt = encode(&header, &claims, &key)
        .map_err(|e| PushError::OAuth2(format!("JWT encoding failed: {e}")))?;

    let resp = http_client
        .post(&sa.token_uri)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &jwt),
        ])
        .send()
        .await
        .map_err(|e| PushError::OAuth2(e.to_string()))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(PushError::OAuth2(format!(
            "Token endpoint returned error: {body}"
        )));
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
    }

    let token: TokenResponse = resp
        .json()
        .await
        .map_err(|e| PushError::OAuth2(e.to_string()))?;

    Ok(token.access_token)
}

fn fcm_send_url(project_id: &str) -> String {
    let base = std::env::var("OB_FCM_API_BASE_URL")
        .unwrap_or_else(|_| "https://fcm.googleapis.com".to_string());
    fcm_send_url_for_base(project_id, &base)
}

fn fcm_send_url_for_base(project_id: &str, base: &str) -> String {
    format!(
        "{}/v1/projects/{project_id}/messages:send",
        base.trim_end_matches('/')
    )
}

// ---------------------------------------------------------------------------
// Core send function — FCM HTTP v1 API
// ---------------------------------------------------------------------------

/// Send a push notification via FCM HTTP v1 API.
///
/// # Arguments
/// - `http_client` — shared reqwest client
/// - `project_id` — GCP project ID (e.g., "orignagta-dev")
/// - `service_account_json` — JSON string of the service account key
/// - `token` — device FCM registration token
/// - `title` — notification title
/// - `body` — notification body text
/// - `data` — optional custom data payload (key-value string pairs)
pub async fn send_push(
    http_client: &reqwest::Client,
    project_id: &str,
    service_account_json: &str,
    token: &str,
    title: &str,
    body: &str,
    data: Option<&std::collections::HashMap<String, String>>,
) -> Result<()> {
    send_push_internal(PushRequest {
        http_client,
        project_id,
        service_account_json,
        token,
        title,
        body,
        data,
        fcm_base_url: None,
    })
    .await
}

struct PushRequest<'a> {
    http_client: &'a reqwest::Client,
    project_id: &'a str,
    service_account_json: &'a str,
    token: &'a str,
    title: &'a str,
    body: &'a str,
    data: Option<&'a std::collections::HashMap<String, String>>,
    fcm_base_url: Option<&'a str>,
}

async fn send_push_internal(request: PushRequest<'_>) -> Result<()> {
    if request.service_account_json.is_empty() {
        return Err(PushError::MissingServiceAccount);
    }

    let access_token = get_access_token(request.http_client, request.service_account_json).await?;

    let url = request
        .fcm_base_url
        .map(|base| fcm_send_url_for_base(request.project_id, base))
        .unwrap_or_else(|| fcm_send_url(request.project_id));

    let mut message = json!({
        fcm_fields::TOKEN: request.token,
        fcm_fields::NOTIFICATION: {
            fcm_fields::TITLE: request.title,
            fcm_fields::BODY: request.body,
        },
    });

    if let Some(data_map) = request.data {
        message[fcm_fields::DATA] = serde_json::to_value(data_map).unwrap_or(Value::Null);
    }

    let payload = json!({ fcm_fields::MESSAGE: message });

    let resp = request
        .http_client
        .post(&url)
        .bearer_auth(&access_token)
        .json(&payload)
        .send()
        .await?;

    let status = resp.status().as_u16();
    if status >= 400 {
        let resp_body = resp.text().await.unwrap_or_default();
        return Err(PushError::FcmApi {
            status,
            body: resp_body,
        });
    }

    Ok(())
}

/// Check if a user has exceeded the daily push limit.
///
/// Returns `true` if the user can receive more pushes today.
pub fn check_daily_limit(push_count_today: u32) -> bool {
    push_count_today < business_rules::MAX_PUSH_PER_DAY
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn service_account_json(token_uri: &str) -> String {
        serde_json::json!({
            "client_email": "test@project.iam.gserviceaccount.com",
            "private_key": "REDACTED_SECRET\n",
            "token_uri": token_uri,
        })
        .to_string()
    }

    #[test]
    fn test_check_daily_limit() {
        assert!(check_daily_limit(0));
        assert!(check_daily_limit(19));
        assert!(!check_daily_limit(20));
        assert!(!check_daily_limit(100));
    }

    #[test]
    fn test_max_push_per_day_constant() {
        assert_eq!(business_rules::MAX_PUSH_PER_DAY, 20);
    }

    #[tokio::test]
    async fn test_send_push_missing_service_account() {
        let client = reqwest::Client::new();
        let result = send_push(&client, "my-project", "", "token123", "Title", "Body", None).await;
        assert!(matches!(result, Err(PushError::MissingServiceAccount)));
    }

    #[tokio::test]
    async fn test_send_push_invalid_service_account() {
        let client = reqwest::Client::new();
        let result = send_push(
            &client,
            "my-project",
            "not-valid-json",
            "token123",
            "Title",
            "Body",
            None,
        )
        .await;
        assert!(matches!(result, Err(PushError::InvalidServiceAccount(_))));
    }

    #[test]
    fn test_service_account_key_parsing() {
        let json = r#"{
            "client_email": "test@project.iam.gserviceaccount.com",
            "private_key": "REDACTED_PRIVATE_KEY\n",
            "token_uri": "https://oauth2.googleapis.com/token"
        }"#;
        let sa: ServiceAccountKey = serde_json::from_str(json).unwrap();
        assert_eq!(sa.client_email, "test@project.iam.gserviceaccount.com");
        assert!(sa.private_key.contains("RSA PRIVATE KEY"));
    }

    #[test]
    fn test_push_error_display() {
        let err1 = PushError::MissingServiceAccount;
        assert_eq!(err1.to_string(), "Missing service account JSON");

        let err2 = PushError::FcmApi {
            status: 403,
            body: "Forbidden".into(),
        };
        assert_eq!(err2.to_string(), "FCM API error (status 403): Forbidden");

        let err3 = PushError::InvalidServiceAccount("bad json".into());
        assert_eq!(err3.to_string(), "Invalid service account JSON: bad json");

        let err4 = PushError::OAuth2("timeout".into());
        assert_eq!(err4.to_string(), "OAuth2 token fetch failed: timeout");

        let err5 = PushError::RateLimitExceeded(20);
        assert_eq!(
            err5.to_string(),
            "Rate limit exceeded for user (max 20/day)"
        );
    }

    #[tokio::test]
    async fn test_send_push_oauth_network_error() {
        let client = reqwest::Client::new();
        let sa_json = service_account_json("http://127.0.0.1:1");
        let result = send_push(
            &client,
            "my-project",
            &sa_json,
            "token123",
            "Title",
            "Body",
            None,
        )
        .await;
        assert!(result.is_err());
    }

    // --- Ported from Python test_services_push_service_batch.py ---

    #[test]
    fn test_daily_limit_boundary_values() {
        // 0 pushes: allowed
        assert!(check_daily_limit(0));
        // 1 push: allowed
        assert!(check_daily_limit(1));
        // 19 pushes: allowed (one below limit)
        assert!(check_daily_limit(19));
        // 20 pushes: blocked (at limit)
        assert!(!check_daily_limit(20));
        // 21 pushes: blocked (over limit)
        assert!(!check_daily_limit(21));
        // Far over limit
        assert!(!check_daily_limit(u32::MAX));
    }

    #[test]
    fn test_push_error_is_send_and_sync() {
        // Ensures PushError can be sent across threads (required for async handlers)
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PushError>();
    }

    #[test]
    fn test_rate_limit_exceeded_error_includes_max() {
        let err = PushError::RateLimitExceeded(20);
        let msg = err.to_string();
        assert!(msg.contains("20"), "Error should include the max limit");
        assert!(
            msg.contains("Rate limit"),
            "Error should mention rate limit"
        );
    }

    #[test]
    fn test_service_account_key_missing_fields_fails() {
        // Missing private_key
        let json = r#"{"client_email": "test@test.com", "token_uri": "https://oauth2.googleapis.com/token"}"#;
        let result: std::result::Result<ServiceAccountKey, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_fcm_api_error_preserves_status_and_body() {
        let err = PushError::FcmApi {
            status: 404,
            body: "Not Found".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("404"));
        assert!(msg.contains("Not Found"));
    }

    #[tokio::test]
    async fn test_send_push_with_custom_data() {
        let client = reqwest::Client::new();
        let mut data = std::collections::HashMap::new();
        data.insert("type".to_string(), "order_update".to_string());
        data.insert("orderId".to_string(), "order_123".to_string());
        // This will fail at OAuth but verifies the data parameter path compiles and runs
        let result = send_push(
            &client,
            "my-project",
            &service_account_json("http://127.0.0.1:0"),
            "token",
            "Title",
            "Body",
            Some(&data),
        )
        .await;
        assert!(result.is_err()); // Expected: OAuth failure, not a panic
    }

    // --- Coverage: OAuth2 token endpoint error response (lines 112-117) ---

    #[tokio::test]
    async fn test_get_access_token_endpoint_returns_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/token"))
            .respond_with(wiremock::ResponseTemplate::new(400).set_body_string("invalid_grant"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let sa_json = service_account_json(&format!("{}/token", server.uri()));
        let result = get_access_token(&client, &sa_json).await;
        assert!(matches!(result, Err(PushError::OAuth2(_))));
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("Token endpoint returned error")
                || err_str.contains("invalid_grant")
                || err_str.contains("OAuth2"),
            "Got: {err_str}"
        );
    }

    // --- Coverage: OAuth2 token response parse error (lines 124-127, 129) ---

    #[tokio::test]
    async fn test_get_access_token_invalid_json_response() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/token"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let sa_json = service_account_json(&format!("{}/token", server.uri()));
        let result = get_access_token(&client, &sa_json).await;
        assert!(matches!(result, Err(PushError::OAuth2(_))));
    }

    // --- Coverage: Full send_push success + FCM error paths (lines 161-193) ---

    #[tokio::test]
    async fn test_send_push_full_success() {
        let token_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/token"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(json!({"access_token": "ya29.test_token"})),
            )
            .mount(&token_server)
            .await;

        let fcm_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(json!({"name": "projects/test-project/messages/123"})),
            )
            .mount(&fcm_server)
            .await;

        let client = reqwest::Client::new();
        let sa_json = service_account_json(&format!("{}/token", token_server.uri()));
        let result = send_push_internal(PushRequest {
            http_client: &client,
            project_id: "test-project",
            service_account_json: &sa_json,
            token: "device_token",
            title: "Test Title",
            body: "Test Body",
            data: None,
            fcm_base_url: Some(&fcm_server.uri()),
        })
        .await;
        assert!(result.is_ok(), "expected push send to succeed: {result:?}");
    }

    #[tokio::test]
    async fn test_send_push_fcm_api_error() {
        let token_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/token"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(json!({"access_token": "ya29.test_token"})),
            )
            .mount(&token_server)
            .await;

        let fcm_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("Unauthorized"))
            .mount(&fcm_server)
            .await;

        let client = reqwest::Client::new();
        let sa_json = service_account_json(&format!("{}/token", token_server.uri()));
        let result = send_push_internal(PushRequest {
            http_client: &client,
            project_id: "test-project",
            service_account_json: &sa_json,
            token: "device_token",
            title: "Title",
            body: "Body",
            data: None,
            fcm_base_url: Some(&fcm_server.uri()),
        })
        .await;
        assert!(matches!(result, Err(PushError::FcmApi { status: 401, .. })));
    }

    #[tokio::test]
    async fn test_send_push_with_data_map() {
        let client = reqwest::Client::new();
        let mut data = std::collections::HashMap::new();
        data.insert("key1".to_string(), "value1".to_string());

        let token_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/token"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(json!({"access_token": "ya29.test_token"})),
            )
            .mount(&token_server)
            .await;
        let fcm_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&fcm_server)
            .await;

        let sa_json = service_account_json(&format!("{}/token", token_server.uri()));
        let result = send_push_internal(PushRequest {
            http_client: &client,
            project_id: "test-proj",
            service_account_json: &sa_json,
            token: "device_token",
            title: "Test Title",
            body: "Test Body",
            data: Some(&data),
            fcm_base_url: Some(&fcm_server.uri()),
        })
        .await;
        assert!(result.is_ok(), "expected push send to succeed: {result:?}");
    }
}
