use crate::{AppState, Config, Result};
use axum::Router;
use axum::http::HeaderValue;
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Build the Axum router with all middleware.
pub fn build_router(state: AppState) -> Router {
    let is_test_mode = std::env::var("OB_TEST_MODE").unwrap_or_default() == "1";
    Router::new()
        .route("/health", axum::routing::get(health_check))
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(build_cors_layer(is_test_mode))
        .with_state(state)
}

/// Build CORS layer with explicit origin whitelist.
/// CRITICAL FIX: Replace .allow_origin(Any) with specific production domains.
fn build_cors_layer(is_test_mode: bool) -> CorsLayer {
    let mut allowed_origins = vec![
        "https://orignagta.ca".parse::<HeaderValue>().unwrap(),
        "https://www.orignagta.ca".parse::<HeaderValue>().unwrap(),
        "https://dev.orignagta.ca".parse::<HeaderValue>().unwrap(),
        "https://staging.orignagta.ca".parse::<HeaderValue>().unwrap(),
    ];

    // Allow localhost ONLY in test mode (for local development)
    if is_test_mode {
        allowed_origins.push("http://localhost:3000".parse::<HeaderValue>().unwrap());
        allowed_origins.push("http://localhost:5173".parse::<HeaderValue>().unwrap());
    }

    let mut cors = CorsLayer::new()
        .allow_credentials(true);

    for origin in allowed_origins {
        cors = cors.allow_origin(origin);
    }

    cors
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
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
        AppState::new(config)
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
}
