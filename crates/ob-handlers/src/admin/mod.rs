//! Admin handlers for system operations and configuration

pub mod jwt_rotation {
    use axum::{http::StatusCode, response::IntoResponse, Json};
    use serde_json::json;

    /// Rotate JWT signing keys (admin-only)
    pub async fn rotate_keys() -> impl IntoResponse {
        // TODO: Implement key rotation with secret manager integration
        (StatusCode::OK, Json(json!({
            "status": "key rotation initiated",
            "rotatedAt": chrono::Utc::now().to_rfc3339()
        })))
    }
}

pub mod config {
    use axum::{http::StatusCode, response::IntoResponse, Json};
    use serde_json::json;

    /// Get data retention configuration
    pub async fn get_retention_config() -> impl IntoResponse {
        (StatusCode::OK, Json(json!({
            "webhookEventRetentionDays": 90,
            "emailLogRetentionDays": 30,
            "auditLogRetentionDays": 365
        })))
    }
}
