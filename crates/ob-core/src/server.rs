use crate::{AppState, Config, Result};
use axum::Router;
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Build the Axum router with all middleware.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", axum::routing::get(health_check))
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .with_state(state)
}

async fn health_check() -> &'static str {
    "ok"
}

/// Start the HTTP server.
pub async fn serve(config: Config) -> Result<()> {
    let addr = format!("{}:{}", config.host, config.port);
    let state = AppState::new(config);
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

    #[tokio::test]
    async fn test_health_check() {
        let result = health_check().await;
        assert_eq!(result, "ok");
    }
}
