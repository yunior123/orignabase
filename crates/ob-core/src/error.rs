use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Authorization denied: {0}")]
    Forbidden(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Unsupported media type: {0}")]
    UnsupportedMediaType(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl Error {
    pub fn status_code(&self) -> StatusCode {
        match self {
            Error::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Error::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Error::Auth(_) => StatusCode::UNAUTHORIZED,
            Error::Forbidden(_) => StatusCode::FORBIDDEN,
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::Validation(_) => StatusCode::BAD_REQUEST,
            Error::UnsupportedMediaType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = serde_json::json!({
            "error": {
                "code": status.as_u16(),
                "message": self.to_string(),
            }
        });
        (status, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;

    // ── Display format tests ──

    #[test]
    fn test_display_config() {
        let e = Error::Config("bad toml".into());
        assert_eq!(e.to_string(), "Configuration error: bad toml");
    }

    #[test]
    fn test_display_database() {
        let e = Error::Database("connection refused".into());
        assert_eq!(e.to_string(), "Database error: connection refused");
    }

    #[test]
    fn test_display_auth() {
        let e = Error::Auth("invalid token".into());
        assert_eq!(e.to_string(), "Authentication error: invalid token");
    }

    #[test]
    fn test_display_forbidden() {
        let e = Error::Forbidden("not allowed".into());
        assert_eq!(e.to_string(), "Authorization denied: not allowed");
    }

    #[test]
    fn test_display_not_found() {
        let e = Error::NotFound("user 42".into());
        assert_eq!(e.to_string(), "Not found: user 42");
    }

    #[test]
    fn test_display_validation() {
        let e = Error::Validation("email required".into());
        assert_eq!(e.to_string(), "Validation error: email required");
    }

    #[test]
    fn test_display_internal() {
        let e = Error::Internal("panic".into());
        assert_eq!(e.to_string(), "Internal error: panic");
    }

    // ── status_code() tests ──

    #[test]
    fn test_status_code_config() {
        assert_eq!(
            Error::Config("x".into()).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn test_status_code_database() {
        assert_eq!(
            Error::Database("x".into()).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn test_status_code_auth() {
        assert_eq!(
            Error::Auth("x".into()).status_code(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn test_status_code_forbidden() {
        assert_eq!(
            Error::Forbidden("x".into()).status_code(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn test_status_code_not_found() {
        assert_eq!(
            Error::NotFound("x".into()).status_code(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn test_status_code_validation() {
        assert_eq!(
            Error::Validation("x".into()).status_code(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn test_status_code_internal() {
        assert_eq!(
            Error::Internal("x".into()).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    // ── IntoResponse tests ──

    #[tokio::test]
    async fn test_into_response_auth() {
        let e = Error::Auth("bad creds".into());
        let resp = e.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], 401);
        assert_eq!(json["error"]["message"], "Authentication error: bad creds");
    }

    #[tokio::test]
    async fn test_into_response_not_found() {
        let e = Error::NotFound("item".into());
        let resp = e.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], 404);
        assert_eq!(json["error"]["message"], "Not found: item");
    }

    #[tokio::test]
    async fn test_into_response_validation() {
        let e = Error::Validation("missing field".into());
        let resp = e.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], 400);
    }

    #[tokio::test]
    async fn test_into_response_forbidden() {
        let e = Error::Forbidden("nope".into());
        let resp = e.into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], 403);
    }

    #[tokio::test]
    async fn test_into_response_internal() {
        let e = Error::Internal("boom".into());
        let resp = e.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], 500);
    }

    #[tokio::test]
    async fn test_into_response_config() {
        let e = Error::Config("oops".into());
        let resp = e.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_into_response_database() {
        let e = Error::Database("timeout".into());
        let resp = e.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_display_unsupported_media_type() {
        let e = Error::UnsupportedMediaType("text/html not allowed".into());
        assert_eq!(
            e.to_string(),
            "Unsupported media type: text/html not allowed"
        );
    }

    #[test]
    fn test_status_code_unsupported_media_type() {
        assert_eq!(
            Error::UnsupportedMediaType("x".into()).status_code(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }

    #[tokio::test]
    async fn test_into_response_unsupported_media_type() {
        let e = Error::UnsupportedMediaType("bad type".into());
        let resp = e.into_response();
        assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], 415);
        assert_eq!(
            json["error"]["message"],
            "Unsupported media type: bad type"
        );
    }
}
