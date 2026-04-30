//! ob-handlers: Business logic handlers for the Origna GTA marketplace.
//!
//! Replaces all 114 Python Cloud Functions with native Rust handlers,
//! organized by domain (payments, orders, products, etc.).

pub mod addresses;
pub mod admin;
pub mod chat;
pub mod coupons;
pub mod cron;
pub mod digital;
pub mod email;
pub mod geocoding;
pub mod native_triggers;
pub mod orders;
pub mod payments;
pub mod pdf;
pub mod products;
pub mod push;
pub mod shared;
pub mod shipping_calc;
pub mod users;
pub mod warehouses;

pub mod rest_api;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use ob_auth::middleware::AuthContext;
use ob_core::Config;
use ob_database::DatabaseClient;
use std::sync::Arc;

/// Shared state for all handler routes.
#[derive(Clone)]
pub struct HandlersState {
    pub config: Arc<Config>,
    pub db: DatabaseClient,
    pub http_client: reqwest::Client,
    pub stripe_client: Option<Arc<stripe::Client>>,
    pub stripe_base_url: String,
    pub turnstile_secret_key: Option<String>,
}

impl HandlersState {
    pub fn new(config: Arc<Config>, db: DatabaseClient) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        let stripe_client = config
            .secret("stripe_secret_key")
            .map(|key| Arc::new(stripe::Client::new(key)));
        let stripe_base_url = "https://api.stripe.com/v1".to_string();

        let turnstile_secret_key = config.secret("turnstile_secret_key").map(|s| s.to_string());

        Self {
            config,
            db,
            http_client,
            stripe_client,
            stripe_base_url,
            turnstile_secret_key,
        }
    }
}

async fn enforce_actor_identity_middleware(
    request: Request,
    next: Next,
) -> Result<Response, ob_core::Error> {
    let auth = request
        .extensions()
        .get::<AuthContext>()
        .cloned()
        .unwrap_or_else(AuthContext::anonymous);

    let content_type = request
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");

    if !content_type.starts_with("application/json") {
        return Ok(next.run(request).await);
    }

    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 1024 * 1024)
        .await
        .map_err(|e| ob_core::Error::Validation(format!("Invalid request body: {e}")))?;

    if !bytes.is_empty()
        && let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&bytes)
    {
        for key in ["userId", "sellerId"] {
            let actor_id = payload.get(key).and_then(|v| v.as_str()).unwrap_or("");
            if !actor_id.is_empty() {
                // CRITICAL FIX: ALWAYS require authentication when body specifies userId/sellerId
                // Do NOT allow unauthenticated requests to specify identity
                if !auth.authenticated {
                    return Err(ob_core::Error::Auth("Authentication required".into()));
                }
                if auth.user_id != actor_id && !auth.has_role("admin") {
                    return Err(ob_core::Error::Forbidden(
                        "Cannot act on another user".into(),
                    ));
                }
            }
        }

        for key in ["adminId", "adminUserId"] {
            let admin_id = payload.get(key).and_then(|v| v.as_str()).unwrap_or("");
            if !admin_id.is_empty() {
                // CRITICAL FIX: Check authentication FIRST, then role
                if !auth.authenticated {
                    return Err(ob_core::Error::Auth("Authentication required".into()));
                }
                if !auth.has_role("admin") {
                    return Err(ob_core::Error::Forbidden("Admin access required".into()));
                }
                if auth.user_id != admin_id {
                    return Err(ob_core::Error::Forbidden("Admin identity mismatch".into()));
                }
            }
        }
    }

    let request = Request::from_parts(parts, Body::from(bytes));
    Ok(next.run(request).await)
}

/// Build the combined handlers router with all domain routes.
pub fn handlers_router(state: HandlersState) -> Router {
    Router::new()
        .merge(payments::router(state.clone()))
        .merge(orders::router(state.clone()))
        .merge(products::router(state.clone()))
        .merge(chat::router(state.clone()))
        .merge(digital::router(state.clone()))
        .merge(coupons::router(state.clone()))
        .merge(geocoding::router(state.clone()))
        .merge(users::router(state.clone()))
        .merge(warehouses::router(state.clone()))
        .merge(addresses::router(state.clone()))
        .merge(shipping_calc::router(state.clone()))
        .merge(rest_api::router(state))
        .layer(axum::middleware::from_fn(enforce_actor_identity_middleware))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_handlers_state_new_sets_http_client_and_base_url() {
        let config = Arc::new(Config::load(None).unwrap());
        let db = DatabaseClient::new_mem().await;

        let state = HandlersState::new(config, db);

        assert_eq!(state.stripe_base_url, "https://api.stripe.com/v1");
        let request = state
            .http_client
            .get("https://example.com")
            .build()
            .unwrap();
        assert_eq!(request.url().as_str(), "https://example.com/");
    }

    #[tokio::test]
    async fn test_handlers_state_new_with_stripe_secret() {
        let mut config = Config::load(None).unwrap();
        config.secrets.values.insert(
            "stripe_secret_key".into(),
            "sk_test_fake_key_for_coverage".into(),
        );
        let db = DatabaseClient::new_mem().await;

        let state = HandlersState::new(Arc::new(config), db);
        assert!(
            state.stripe_client.is_some(),
            "Stripe client should be created when secret is present"
        );
    }

    #[tokio::test]
    async fn test_handlers_router_builds() {
        let state = HandlersState::new(
            Arc::new(Config::load(None).unwrap()),
            DatabaseClient::new_mem().await,
        );
        let _router = handlers_router(state);
    }
}
