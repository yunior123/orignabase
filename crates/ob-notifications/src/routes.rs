use axum::{Json, extract::State};
use ob_core::{Error, Result};
use ob_database::DatabaseClient;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Cached OAuth2 access token with expiry.
#[derive(Clone, Default)]
struct CachedToken {
    token: String,
    expires_at: i64,
}

/// Push notification state.
#[derive(Clone)]
pub struct NotificationsState {
    pub db: DatabaseClient,
    /// FCM project ID (from Firebase Console).
    pub fcm_project_id: Option<String>,
    /// FCM service account JSON (for OAuth2 token).
    pub fcm_service_account: Option<String>,
    /// HTTP client for FCM/APNs calls.
    pub http_client: reqwest::Client,
    /// Cached OAuth2 access token for FCM.
    fcm_token_cache: Arc<RwLock<CachedToken>>,
}

impl NotificationsState {
    /// Create a new NotificationsState.
    pub fn new(
        db: DatabaseClient,
        fcm_project_id: Option<String>,
        fcm_service_account: Option<String>,
        http_client: reqwest::Client,
    ) -> Self {
        Self {
            db,
            fcm_project_id,
            fcm_service_account,
            http_client,
            fcm_token_cache: Arc::new(RwLock::new(CachedToken::default())),
        }
    }

    /// Get a valid OAuth2 access token for FCM HTTP v1 API.
    /// Uses Google service account JWT → OAuth2 token exchange.
    /// Caches the token and refreshes 60s before expiry.
    async fn get_fcm_access_token(&self) -> std::result::Result<String, String> {
        // Check cache first
        {
            let cache = self.fcm_token_cache.read().await;
            let now = chrono::Utc::now().timestamp();
            if !cache.token.is_empty() && cache.expires_at > now + 60 {
                return Ok(cache.token.clone());
            }
        }

        // Need to refresh — parse service account JSON
        let sa_json = self
            .fcm_service_account
            .as_ref()
            .ok_or_else(|| "FCM service account not configured".to_string())?;

        let sa: Value = serde_json::from_str(sa_json)
            .map_err(|e| format!("Invalid service account JSON: {e}"))?;

        let client_email = sa["client_email"]
            .as_str()
            .ok_or("Missing client_email in service account")?;
        let private_key_pem = sa["private_key"]
            .as_str()
            .ok_or("Missing private_key in service account")?;

        // Create JWT for Google OAuth2 token exchange
        let now = chrono::Utc::now().timestamp();
        let claims = json!({
            "iss": client_email,
            "scope": "https://www.googleapis.com/auth/firebase.messaging",
            "aud": "https://oauth2.googleapis.com/token",
            "iat": now,
            "exp": now + 3600,
        });

        let key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key_pem.as_bytes())
            .map_err(|e| format!("Invalid RSA private key: {e}"))?;

        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        let jwt = jsonwebtoken::encode(&header, &claims, &key)
            .map_err(|e| format!("JWT encoding failed: {e}"))?;

        // Exchange JWT for access token
        let resp = self
            .http_client
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .map_err(|e| format!("OAuth2 token request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("OAuth2 token exchange failed ({status}): {body}"));
        }

        let token_resp: Value = resp
            .json()
            .await
            .map_err(|e| format!("Invalid OAuth2 response: {e}"))?;

        let access_token = token_resp["access_token"]
            .as_str()
            .ok_or("Missing access_token in OAuth2 response")?
            .to_string();

        let expires_in = token_resp["expires_in"].as_i64().unwrap_or(3600);

        // Update cache
        {
            let mut cache = self.fcm_token_cache.write().await;
            cache.token = access_token.clone();
            cache.expires_at = now + expires_in;
        }

        Ok(access_token)
    }
}

/// Platform enum for device tokens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Android,
    Ios,
    Web,
}

/// Register a device token for push notifications.
#[derive(Debug, Deserialize)]
pub struct RegisterTokenRequest {
    pub user_id: String,
    pub token: String,
    pub platform: Platform,
}

/// Send a push notification.
#[derive(Debug, Deserialize)]
pub struct SendNotificationRequest {
    /// Target: user_id, device token, or topic name.
    pub to: String,
    /// "user", "token", or "topic"
    #[serde(default = "default_target_type")]
    pub target_type: String,
    pub title: String,
    pub body: String,
    /// Custom data payload (key-value pairs).
    #[serde(default)]
    pub data: Value,
}

fn default_target_type() -> String {
    "user".to_string()
}

/// Subscribe a device to a topic.
#[derive(Debug, Deserialize)]
pub struct TopicSubscription {
    pub token: String,
    pub topic: String,
}

/// POST /push/register — Register a device token.
async fn register_token(
    State(state): State<NotificationsState>,
    Json(body): Json<RegisterTokenRequest>,
) -> Result<Json<Value>> {
    if body.token.is_empty() || body.token.len() > 1024 {
        return Err(Error::Validation("Invalid device token".into()));
    }
    if body.user_id.is_empty() || body.user_id.len() > 256 {
        return Err(Error::Validation("Invalid user_id".into()));
    }
    let now = chrono::Utc::now().to_rfc3339();

    // Upsert: if this token already exists, update user_id and platform
    state
        .db
        .query_bind(
            "UPSERT _push_tokens SET user_id = $user_id, token = $push_token, \
             platform = $platform, updated_at = $now WHERE token = $push_token",
            json!({
                "user_id": body.user_id,
                "push_token": body.token,
                "platform": body.platform,
                "now": now,
            }),
        )
        .await?;

    Ok(Json(json!({ "registered": true })))
}

/// DELETE /push/register — Unregister a device token.
async fn unregister_token(
    State(state): State<NotificationsState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let token = body
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Validation("Missing 'token' field".into()))?;

    state
        .db
        .query_bind(
            "DELETE FROM _push_tokens WHERE token = $push_token",
            json!({ "push_token": token }),
        )
        .await?;

    Ok(Json(json!({ "unregistered": true })))
}

/// POST /push/send — Send a push notification.
///
/// Supports three target types:
/// - "user": send to all devices registered to a user_id
/// - "token": send to a specific device token
/// - "topic": send to all devices subscribed to a topic
async fn send_notification(
    State(state): State<NotificationsState>,
    Json(body): Json<SendNotificationRequest>,
) -> Result<Json<Value>> {
    let tokens = match body.target_type.as_str() {
        "user" => {
            // Get all device tokens for this user
            let results = state
                .db
                .query_bind(
                    "SELECT token FROM _push_tokens WHERE user_id = $user_id",
                    json!({ "user_id": body.to }),
                )
                .await?;
            results
                .iter()
                .filter_map(|r| r.get("token").and_then(|v| v.as_str()).map(String::from))
                .collect::<Vec<_>>()
        }
        "token" => vec![body.to.clone()],
        "topic" => {
            // Get all tokens subscribed to this topic
            let results = state
                .db
                .query_bind(
                    "SELECT token FROM _push_subscriptions WHERE topic = $topic",
                    json!({ "topic": body.to }),
                )
                .await?;
            results
                .iter()
                .filter_map(|r| r.get("token").and_then(|v| v.as_str()).map(String::from))
                .collect::<Vec<_>>()
        }
        _ => {
            return Err(Error::Validation(
                "Invalid target_type. Use: user, token, or topic".into(),
            ));
        }
    };

    if tokens.is_empty() {
        return Ok(Json(json!({ "sent": 0, "message": "No devices found" })));
    }

    let mut sent = 0u64;
    let mut failed = 0u64;

    // Send via FCM HTTP v1 API if configured
    if let (Some(project_id), Some(_service_account)) =
        (&state.fcm_project_id, &state.fcm_service_account)
    {
        let fcm_url = format!("https://fcm.googleapis.com/v1/projects/{project_id}/messages:send");

        for token in &tokens {
            let fcm_message = json!({
                "message": {
                    "token": token,
                    "notification": {
                        "title": body.title,
                        "body": body.body,
                    },
                    "data": body.data,
                }
            });

            let bearer = match state.get_fcm_access_token().await {
                Ok(token) => token,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to get FCM access token");
                    // Fallback to env var for backwards compatibility
                    std::env::var("OB_FCM_BEARER_TOKEN").unwrap_or_default()
                }
            };

            match state
                .http_client
                .post(&fcm_url)
                .header("Authorization", format!("Bearer {bearer}"))
                .json(&fcm_message)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => sent += 1,
                Ok(resp) => {
                    tracing::warn!(
                        token = %token,
                        status = %resp.status(),
                        "FCM send failed"
                    );
                    failed += 1;
                }
                Err(e) => {
                    tracing::error!(error = %e, "FCM request error");
                    failed += 1;
                }
            }
        }
    } else {
        // No FCM configured — store as pending notifications
        for token in &tokens {
            let notif = json!({
                "token": token,
                "title": body.title,
                "body": body.body,
                "data": body.data,
                "status": "pending",
                "created_at": chrono::Utc::now().to_rfc3339(),
            });
            let _ = state
                .db
                .create_document("_pending_notifications", notif)
                .await;
            sent += 1;
        }
        tracing::info!("FCM not configured — stored {} pending notifications", sent);
    }

    Ok(Json(json!({
        "sent": sent,
        "failed": failed,
        "total_devices": tokens.len(),
    })))
}

/// POST /push/subscribe — Subscribe a device to a topic.
async fn subscribe_topic(
    State(state): State<NotificationsState>,
    Json(body): Json<TopicSubscription>,
) -> Result<Json<Value>> {
    if body.token.is_empty() || body.topic.is_empty() {
        return Err(Error::Validation(
            "Token and topic must not be empty".into(),
        ));
    }
    if body.topic.len() > 256 {
        return Err(Error::Validation("Topic name too long".into()));
    }
    state
        .db
        .query_bind(
            "UPSERT _push_subscriptions SET token = $push_token, topic = $topic, \
             created_at = time::now() WHERE token = $push_token AND topic = $topic",
            json!({ "push_token": body.token, "topic": body.topic }),
        )
        .await?;

    Ok(Json(json!({ "subscribed": body.topic })))
}

/// DELETE /push/subscribe — Unsubscribe a device from a topic.
async fn unsubscribe_topic(
    State(state): State<NotificationsState>,
    Json(body): Json<TopicSubscription>,
) -> Result<Json<Value>> {
    state
        .db
        .query_bind(
            "DELETE FROM _push_subscriptions WHERE token = $push_token AND topic = $topic",
            json!({ "push_token": body.token, "topic": body.topic }),
        )
        .await?;

    Ok(Json(json!({ "unsubscribed": body.topic })))
}

/// Build the notifications router.
pub fn notifications_router(state: NotificationsState) -> axum::Router {
    axum::Router::new()
        .route(
            "/push/register",
            axum::routing::post(register_token).delete(unregister_token),
        )
        .route("/push/send", axum::routing::post(send_notification))
        .route(
            "/push/subscribe",
            axum::routing::post(subscribe_topic).delete(unsubscribe_topic),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_token_request_deser() {
        let json = json!({
            "user_id": "user_123",
            "token": "fcm_token_abc",
            "platform": "android"
        });
        let req: RegisterTokenRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.user_id, "user_123");
        assert_eq!(req.token, "fcm_token_abc");
        assert_eq!(req.platform, Platform::Android);
    }

    #[test]
    fn test_platform_variants() {
        let android: Platform = serde_json::from_value(json!("android")).unwrap();
        let ios: Platform = serde_json::from_value(json!("ios")).unwrap();
        let web: Platform = serde_json::from_value(json!("web")).unwrap();
        assert_eq!(android, Platform::Android);
        assert_eq!(ios, Platform::Ios);
        assert_eq!(web, Platform::Web);
    }

    #[test]
    fn test_send_notification_request_defaults() {
        let json = json!({
            "to": "user_123",
            "title": "Hello",
            "body": "World"
        });
        let req: SendNotificationRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.target_type, "user");
        assert_eq!(req.data, Value::Null);
    }

    #[test]
    fn test_send_notification_with_data() {
        let json = json!({
            "to": "token_abc",
            "target_type": "token",
            "title": "New Order",
            "body": "You have a new order",
            "data": { "order_id": "ord_123", "amount": "29.99" }
        });
        let req: SendNotificationRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.target_type, "token");
        assert_eq!(req.data["order_id"], "ord_123");
    }

    #[test]
    fn test_topic_subscription_deser() {
        let json = json!({
            "token": "device_token",
            "topic": "promotions"
        });
        let req: TopicSubscription = serde_json::from_value(json).unwrap();
        assert_eq!(req.token, "device_token");
        assert_eq!(req.topic, "promotions");
    }

    #[test]
    fn test_platform_serialization() {
        assert_eq!(
            serde_json::to_string(&Platform::Android).unwrap(),
            "\"android\""
        );
        assert_eq!(serde_json::to_string(&Platform::Ios).unwrap(), "\"ios\"");
        assert_eq!(serde_json::to_string(&Platform::Web).unwrap(), "\"web\"");
    }

    #[test]
    fn test_send_to_topic_deser() {
        let json = json!({
            "to": "promotions",
            "target_type": "topic",
            "title": "Sale!",
            "body": "50% off everything"
        });
        let req: SendNotificationRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.to, "promotions");
        assert_eq!(req.target_type, "topic");
        assert_eq!(req.title, "Sale!");
        assert_eq!(req.body, "50% off everything");
    }

    #[test]
    fn test_send_to_topic_with_data() {
        let json = json!({
            "to": "news",
            "target_type": "topic",
            "title": "Breaking",
            "body": "Check this out",
            "data": { "url": "https://example.com", "category": "tech" }
        });
        let req: SendNotificationRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.target_type, "topic");
        assert_eq!(req.data["url"], "https://example.com");
        assert_eq!(req.data["category"], "tech");
    }

    #[test]
    fn test_register_token_all_platforms() {
        for (platform_str, expected) in [
            ("android", Platform::Android),
            ("ios", Platform::Ios),
            ("web", Platform::Web),
        ] {
            let json = json!({
                "user_id": "user_1",
                "token": format!("token_{platform_str}"),
                "platform": platform_str
            });
            let req: RegisterTokenRequest = serde_json::from_value(json).unwrap();
            assert_eq!(req.platform, expected);
            assert_eq!(req.token, format!("token_{platform_str}"));
        }
    }

    #[test]
    fn test_register_token_invalid_platform() {
        let json = json!({
            "user_id": "user_1",
            "token": "tok",
            "platform": "blackberry"
        });
        let result = serde_json::from_value::<RegisterTokenRequest>(json);
        assert!(
            result.is_err(),
            "Invalid platform should fail deserialization"
        );
    }

    #[test]
    fn test_topic_subscription_roundtrip() {
        let original = TopicSubscription {
            token: "device_abc".to_string(),
            topic: "sports".to_string(),
        };
        // Serialize manually since TopicSubscription only derives Deserialize
        let json = json!({
            "token": original.token,
            "topic": original.topic,
        });
        let deserialized: TopicSubscription = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.token, original.token);
        assert_eq!(deserialized.topic, original.topic);
    }

    #[test]
    fn test_topic_subscription_empty_strings() {
        let json = json!({
            "token": "",
            "topic": ""
        });
        let req: TopicSubscription = serde_json::from_value(json).unwrap();
        assert_eq!(req.token, "");
        assert_eq!(req.topic, "");
    }

    #[test]
    fn test_default_target_type() {
        assert_eq!(default_target_type(), "user");
    }

    #[test]
    fn test_notifications_state_fields() {
        // Compile-time check that NotificationsState has all expected fields
        fn _assert_fields(s: &NotificationsState) {
            let _ = &s.db;
            let _ = &s.fcm_project_id;
            let _ = &s.fcm_service_account;
            let _ = &s.http_client;
        }
    }
}
