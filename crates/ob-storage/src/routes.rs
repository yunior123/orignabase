use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use ob_core::{Error, Result};
use serde::Deserialize;

use crate::{LocalStorage, SignedUrlGenerator, StorageBackend};

/// Shared storage state.
#[derive(Clone)]
pub struct StorageState {
    pub storage: LocalStorage,
    pub url_generator: SignedUrlGenerator,
}

#[derive(Deserialize)]
pub struct SignedParams {
    pub expires: u64,
    pub sig: String,
}

/// PUT /storage/upload/*path — Upload a file via signed URL.
pub async fn upload_file(
    State(state): State<StorageState>,
    Path(path): Path<String>,
    Query(params): Query<SignedParams>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<crate::ObjectMeta>> {
    // Verify signed URL
    if !state
        .url_generator
        .verify("PUT", &path, params.expires, &params.sig)?
    {
        return Err(Error::Auth("Invalid signature".into()));
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream");

    let meta = state.storage.upload(&path, &body, content_type).await?;
    Ok(Json(meta))
}

/// GET /storage/download/*path — Download a file via signed URL.
pub async fn download_file(
    State(state): State<StorageState>,
    Path(path): Path<String>,
    Query(params): Query<SignedParams>,
) -> Result<Response> {
    // Verify signed URL
    if !state
        .url_generator
        .verify("GET", &path, params.expires, &params.sig)?
    {
        return Err(Error::Auth("Invalid signature".into()));
    }

    let data = state.storage.download(&path).await?;
    let meta = state.storage.metadata(&path).await?;

    let response = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, meta.content_type),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "inline; filename=\"{}\"",
                    path.rsplit('/').next().unwrap_or(&path)
                ),
            ),
            (header::CONTENT_LENGTH, data.len().to_string()),
        ],
        data,
    );

    Ok(response.into_response())
}

/// DELETE /storage/delete/*path — Delete a file (requires auth, no signed URL).
pub async fn delete_file(
    State(state): State<StorageState>,
    Path(path): Path<String>,
) -> Result<Json<serde_json::Value>> {
    state.storage.delete(&path).await?;
    Ok(Json(serde_json::json!({ "deleted": path })))
}

/// Build the storage router.
pub fn storage_router(state: StorageState) -> axum::Router {
    axum::Router::new()
        .route("/storage/upload/{*path}", axum::routing::put(upload_file))
        .route(
            "/storage/download/{*path}",
            axum::routing::get(download_file),
        )
        .route(
            "/storage/delete/{*path}",
            axum::routing::delete(delete_file),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signed_params_deserialize() {
        let json = r#"{"expires": 1700000000, "sig": "abc123"}"#;
        let params: SignedParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.expires, 1700000000);
        assert_eq!(params.sig, "abc123");
    }

    #[test]
    fn test_signed_params_missing_field_fails() {
        let json = r#"{"expires": 1700000000}"#;
        let result = serde_json::from_str::<SignedParams>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_signed_params_wrong_type_fails() {
        let json = r#"{"expires": "not_a_number", "sig": "abc"}"#;
        let result = serde_json::from_str::<SignedParams>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_storage_state_is_clone() {
        // Compile-time check: StorageState must be Clone (required by axum State)
        fn assert_clone<T: Clone>() {}
        assert_clone::<StorageState>();
    }
}
