use crate::{AppState, Config, Environment, Result};
use axum::Router;
use axum::http::{HeaderValue, Method, header};
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Build the Axum router with all middleware.
pub fn build_router(state: AppState) -> Router {
    let is_test_mode = std::env::var("OB_TEST_MODE").unwrap_or_default() == "1";
    let cors_layer = build_cors_layer(&state.config, is_test_mode);
    Router::new()
        .route("/health", axum::routing::get(health_check))
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(cors_layer)
        .with_state(state)
}

/// Build CORS layer with explicit origin whitelist.
/// CRITICAL FIX: Replace .allow_origin(Any) with specific production domains.
fn build_cors_layer(config: &Config, is_test_mode: bool) -> CorsLayer {
    let allowed_origins: Vec<HeaderValue> = config
        .cors
        .allowed_origins
        .iter()
        .filter_map(|origin| match origin.parse::<HeaderValue>() {
            Ok(value) => Some(value),
            Err(err) => {
                tracing::warn!(origin, %err, "Skipping invalid CORS origin from config");
                None
            }
        })
        .collect();

    let allow_loopback_origins = is_test_mode || Environment::from_env().is_dev();

    if allowed_origins.is_empty() && !allow_loopback_origins {
        tracing::warn!(
            "CORS allowed_origins is empty and not in test mode — all cross-origin requests will be denied"
        );
    }

    let allow_origin = AllowOrigin::predicate(move |origin, _request_parts| {
        allowed_origins.contains(origin) || (allow_loopback_origins && is_loopback_origin(origin))
    });

    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::ACCEPT,
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ORIGIN,
            header::CACHE_CONTROL,
            "x-requested-with"
                .parse()
                .expect("static header should parse"),
            "x-tenant-id".parse().expect("static header should parse"),
        ])
}

fn is_loopback_origin(origin: &HeaderValue) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };

    let Ok(uri) = origin.parse::<axum::http::Uri>() else {
        return false;
    };

    if !matches!(uri.scheme_str(), Some("http" | "https")) {
        return false;
    }

    matches!(
        uri.host(),
        Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
    )
}

async fn health_check() -> &'static str {
    "ok"
}

/// Start the HTTP server.
pub async fn serve(config: Config) -> Result<()> {
    let addr = format!("{}:{}", config.host, config.port);
    let http_client = reqwest::Client::new();
    let state = AppState::new(config, http_client);
    let app = build_router(state);

    tracing::info!("OrignaBase listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| crate::Error::Internal(format!("Failed to bind {addr}: {e}")))?;

    axum::serve(listener, app)
        .await
        .map_err(|e| crate::Error::Internal(e.to_string()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn make_state() -> AppState {
        let config: Config = toml::from_str(
            r#"
            [database]
            endpoint = "localhost:8000"
            "#,
        )
        .unwrap();
        AppState::new(config, reqwest::Client::new())
    }

    #[tokio::test]
    async fn test_health_check() {
        let result = health_check().await;
        assert_eq!(result, "ok");
    }

    #[tokio::test]
    async fn test_router_health_endpoint() {
        let app = build_router(make_state());
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"ok");
    }

    #[tokio::test]
    async fn test_router_unknown_route_404() {
        let app = build_router(make_state());
        let req = Request::builder()
            .uri("/nonexistent")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_router_health_post_method_not_allowed() {
        let app = build_router(make_state());
        let req = Request::builder()
            .method("POST")
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn test_is_loopback_origin_allows_random_localhost_port() {
        let origin = HeaderValue::from_static("http://localhost:54643");
        assert!(is_loopback_origin(&origin));
    }

    #[test]
    fn test_is_loopback_origin_rejects_public_host() {
        let origin = HeaderValue::from_static("https://evil.example");
        assert!(!is_loopback_origin(&origin));
    }

    #[tokio::test]
    async fn test_router_options_allows_random_localhost_origin_in_dev() {
        let app = build_router(make_state());
        let req = Request::builder()
            .method(Method::OPTIONS)
            .uri("/health")
            .header(header::ORIGIN, "http://localhost:54643")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("http://localhost:54643")
        );
    }
}
