use axum::{Json, extract::State, http::HeaderMap};
use ob_core::Result;
use ob_database::DatabaseClient;
use serde::Deserialize;

use crate::event::{AnalyticsEvent, extract_domain, hash_ip};

/// Shared analytics state.
#[derive(Clone)]
pub struct AnalyticsState {
    /// Salt for IP hashing (rotated daily for extra privacy).
    pub ip_salt: String,
    /// Database client for persisting analytics events.
    pub db: DatabaseClient,
}

#[derive(Deserialize)]
pub struct IngestRequest {
    pub event: String,
    #[serde(default)]
    pub properties: serde_json::Value,
    pub path: Option<String>,
    pub referrer: Option<String>,
    pub device: Option<String>,
    pub browser: Option<String>,
}

/// POST /analytics/event — Ingest an analytics event.
pub async fn ingest_event(
    State(state): State<AnalyticsState>,
    headers: HeaderMap,
    Json(request): Json<IngestRequest>,
) -> Result<Json<serde_json::Value>> {
    // Extract IP from headers (privacy: hash immediately, never store raw)
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .or_else(|| headers.get("x-real-ip").and_then(|v| v.to_str().ok()))
        .unwrap_or("unknown");

    let visitor_hash = hash_ip(ip, &state.ip_salt);
    let referrer_domain = request.referrer.as_deref().and_then(extract_domain);

    let event = AnalyticsEvent {
        id: uuid::Uuid::new_v4().to_string(),
        event: request.event,
        visitor_hash,
        properties: request.properties,
        timestamp: chrono::Utc::now().to_rfc3339(),
        path: request.path,
        referrer: referrer_domain,
        country: None, // Would need GeoIP lookup
        device: request.device,
        browser: request.browser,
    };

    // Persist to SurrealDB
    let event_json =
        serde_json::to_value(&event).map_err(|e| ob_core::Error::Internal(e.to_string()))?;
    state
        .db
        .create_document("_analytics_events", event_json)
        .await?;

    tracing::debug!(event_id = %event.id, event_name = %event.event, "Analytics event ingested");

    Ok(Json(serde_json::json!({
        "status": "ok",
        "event_id": event.id
    })))
}

/// GET /analytics/stats — Get basic analytics stats.
/// In production, this queries the daily rollup tables.
pub async fn get_stats() -> Result<Json<serde_json::Value>> {
    // Placeholder — would query _analytics_rollup table
    Ok(Json(serde_json::json!({
        "message": "Analytics stats endpoint. Connect SurrealDB for data."
    })))
}

/// Build the analytics router.
pub fn analytics_router(state: AnalyticsState) -> axum::Router {
    axum::Router::new()
        .route("/analytics/event", axum::routing::post(ingest_event))
        .route("/analytics/stats", axum::routing::get(get_stats))
        .with_state(state)
}
