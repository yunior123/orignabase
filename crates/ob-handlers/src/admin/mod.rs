//! Admin handlers for system operations and configuration

pub mod config {
    use axum::{Json, http::StatusCode, response::IntoResponse};
    use serde_json::json;

    /// Get data retention configuration
    pub async fn get_retention_config() -> impl IntoResponse {
        (
            StatusCode::OK,
            Json(json!({
                "webhookEventRetentionDays": 90,
                "emailLogRetentionDays": 30,
                "auditLogRetentionDays": 365
            })),
        )
    }
}
