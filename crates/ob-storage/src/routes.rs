use axum::{
    Extension, Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use ob_auth::middleware::AuthContext;
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::resumable::ResumableUploadManager;
use crate::transform::{TransformParams, transform_image};
use crate::{LocalStorage, SignedUrlGenerator, StorageBackend};

/// Allowed MIME types for file uploads.
const ALLOWED_UPLOAD_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "application/pdf",
];

/// Maximum file size for regular uploads: 500MB
const MAX_UPLOAD_SIZE: usize = 500 * 1024 * 1024;

/// Maximum total size for resumable uploads: 5GB
const MAX_RESUMABLE_SIZE: u64 = 5 * 1024 * 1024 * 1024;

/// Validate file signature (magic bytes) against declared Content-Type.
/// Returns 415 Unsupported Media Type if validation fails.
fn validate_file_signature(bytes: &[u8], content_type: &str) -> Result<()> {
    // Check Content-Type is in whitelist
    if !ALLOWED_UPLOAD_TYPES.contains(&content_type) {
        return Err(Error::UnsupportedMediaType(format!(
            "File type '{}' is not allowed",
            content_type
        )));
    }

    // Verify magic bytes match declared Content-Type
    match infer::get(bytes) {
        Some(kind) => {
            let detected_mime = kind.mime_type();
            if detected_mime != content_type {
                return Err(Error::UnsupportedMediaType(format!(
                    "File content ({}) doesn't match declared type ({})",
                    detected_mime, content_type
                )));
            }
        }
        None => {
            return Err(Error::UnsupportedMediaType(
                "Could not verify file type from content".into(),
            ));
        }
    }

    Ok(())
}

/// Sanitize storage path — prevent directory traversal attacks.
/// Uses iterative replacement to handle nested traversal like `....//`.
fn sanitize_storage_path(path: &str) -> Result<String> {
    let mut sanitized = path.replace('\\', "/");
    // Iteratively remove ".." until stable (handles `....//` → `../`)
    loop {
        let next = sanitized.replace("..", "");
        if next == sanitized {
            break;
        }
        sanitized = next;
    }
    let sanitized = sanitized.trim_start_matches('/').to_string();

    if sanitized.is_empty() {
        return Err(Error::Validation("Path cannot be empty".into()));
    }

    // Final safety: reject if any path component is empty or looks traversal-like
    for component in sanitized.split('/') {
        if component == "." || component.is_empty() && sanitized.contains("//") {
            return Err(Error::Validation("Invalid path component".into()));
        }
    }

    Ok(sanitized)
}

/// Shared storage state.
#[derive(Clone)]
pub struct StorageState {
    pub storage: LocalStorage,
    pub url_generator: SignedUrlGenerator,
    pub resumable: ResumableUploadManager,
}

#[derive(Deserialize)]
pub struct SignedParams {
    pub expires: u64,
    pub sig: String,
}

/// Combined query params for download: signed URL fields + optional image transforms.
#[derive(Deserialize)]
pub struct DownloadParams {
    pub expires: u64,
    pub sig: String,
    /// Image transform: target width in pixels.
    #[serde(flatten)]
    pub transform: TransformParams,
}

/// PUT /storage/upload/*path — Upload a file via signed URL.
pub async fn upload_file(
    State(state): State<StorageState>,
    Path(path): Path<String>,
    Query(params): Query<SignedParams>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<crate::ObjectMeta>> {
    // CRITICAL FIX: Enforce max file size
    if body.len() > MAX_UPLOAD_SIZE {
        return Err(Error::Validation(format!(
            "File too large: {} bytes > {} bytes (500MB)",
            body.len(),
            MAX_UPLOAD_SIZE
        )));
    }

    // Verify signed URL
    if !state
        .url_generator
        .verify("PUT", &path, params.expires, &params.sig)?
    {
        return Err(Error::Auth("Invalid signature".into()));
    }

    // Sanitize path to prevent directory traversal
    let safe_path = sanitize_storage_path(&path)?;

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream");

    // Validate file signature (magic bytes) before storing
    validate_file_signature(&body, content_type)?;

    let meta = state
        .storage
        .upload(&safe_path, &body, content_type)
        .await?;
    Ok(Json(meta))
}

/// GET /storage/download/*path — Download a file via signed URL.
/// Supports on-the-fly image transforms via query params: `w`, `h`, `fit`, `q`, `format`.
pub async fn download_file(
    State(state): State<StorageState>,
    Path(path): Path<String>,
    Query(params): Query<DownloadParams>,
) -> Result<Response> {
    // Verify signed URL
    if !state
        .url_generator
        .verify("GET", &path, params.expires, &params.sig)?
    {
        return Err(Error::Auth("Invalid signature".into()));
    }

    // Sanitize path to prevent directory traversal
    let safe_path = sanitize_storage_path(&path)?;

    let data = state.storage.download(&safe_path).await?;
    let meta = state.storage.metadata(&safe_path).await?;

    // Apply image transformations if requested
    let (data, content_type) = if params.transform.has_transforms()
        && meta.content_type.starts_with("image/")
        && !meta.content_type.contains("svg")
    {
        transform_image(&data, &params.transform)?
    } else {
        (data, meta.content_type)
    };

    // Force attachment for potentially dangerous content types (XSS prevention)
    let is_safe_inline = content_type.starts_with("image/") && !content_type.contains("svg");
    let disposition = if is_safe_inline {
        "inline"
    } else {
        "attachment"
    };

    let response = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "{}; filename=\"{}\"",
                    disposition,
                    safe_path.rsplit('/').next().unwrap_or(&safe_path)
                ),
            ),
            (header::CONTENT_LENGTH, data.len().to_string()),
        ],
        data,
    );

    Ok(response.into_response())
}

/// DELETE /storage/delete/*path — Delete a file via signed URL.
pub async fn delete_file(
    State(state): State<StorageState>,
    Path(path): Path<String>,
    Query(params): Query<SignedParams>,
) -> Result<Json<serde_json::Value>> {
    // Verify signed URL — prevents unauthenticated deletion
    if !state
        .url_generator
        .verify("DELETE", &path, params.expires, &params.sig)?
    {
        return Err(Error::Auth("Invalid signature".into()));
    }

    // Sanitize path to prevent directory traversal
    let safe_path = sanitize_storage_path(&path)?;

    state.storage.delete(&safe_path).await?;
    Ok(Json(serde_json::json!({ "deleted": safe_path })))
}

// ── Resumable upload handlers ────────────────────────────────────────────

#[derive(Deserialize)]
pub struct InitResumableParams {
    pub path: String,
    #[serde(default = "default_content_type")]
    pub content_type: String,
    pub total_size: u64,
}

fn default_content_type() -> String {
    "application/octet-stream".into()
}

fn is_public_read_path(path: &str) -> bool {
    path.starts_with("products/") || path.starts_with("products/videos/")
}

fn can_user_write_path(auth: &AuthContext, path: &str) -> bool {
    if !auth.authenticated || auth.user_id.is_empty() {
        return false;
    }

    path.starts_with(&format!("users/{}/", auth.user_id))
        || path.starts_with(&format!("reviews/{}/", auth.user_id))
        || path.starts_with("products/")
        || path.starts_with("products/videos/")
}

fn require_authenticated_user(auth: &AuthContext) -> Result<&str> {
    if !auth.authenticated || auth.user_id.is_empty() {
        return Err(Error::Auth("Authentication required".into()));
    }
    Ok(&auth.user_id)
}

/// POST /storage/upload/resumable — Initiate a resumable upload session.
pub async fn init_resumable(
    State(state): State<StorageState>,
    Extension(auth): Extension<AuthContext>,
    Json(params): Json<InitResumableParams>,
) -> Result<Json<crate::resumable::UploadSession>> {
    let owner = require_authenticated_user(&auth)?;

    // CRITICAL FIX: Validate total_size doesn't exceed limit
    if params.total_size > MAX_RESUMABLE_SIZE {
        return Err(Error::Validation(format!(
            "Total size {} exceeds limit {} (5GB)",
            params.total_size, MAX_RESUMABLE_SIZE
        )));
    }

    let safe_path = sanitize_storage_path(&params.path)?;
    if !can_user_write_path(&auth, &safe_path) {
        return Err(Error::Forbidden(
            "Not allowed to write this storage path".into(),
        ));
    }
    let session = state.resumable.create_session(
        &safe_path,
        &params.content_type,
        params.total_size,
        owner,
    )?;
    Ok(Json(session))
}

/// PATCH /storage/upload/resumable/{id} — Append a chunk.
/// Client sends `Upload-Offset` header with the byte offset.
pub async fn append_resumable(
    State(state): State<StorageState>,
    Extension(auth): Extension<AuthContext>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<crate::resumable::UploadSession>> {
    let owner = require_authenticated_user(&auth)?;
    let offset: u64 = headers
        .get("Upload-Offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| Error::Validation("Missing or invalid Upload-Offset header".into()))?;

    let session = state
        .resumable
        .append_chunk(&session_id, offset, &body, owner)
        .await?;

    // CRITICAL FIX: Re-validate path ownership on append (not just session owner)
    if !can_user_write_path(&auth, &session.path) {
        return Err(Error::Forbidden("Path access denied".into()));
    }

    // If complete, finalize: validate then move assembled data into storage backend
    if session.status == crate::resumable::UploadStatus::Complete {
        let (data, final_session) = state.resumable.finalize(&session_id).await?;
        // Validate file signature (magic bytes) before storing
        validate_file_signature(&data, &final_session.content_type)?;
        let _meta = state
            .storage
            .upload(&final_session.path, &data, &final_session.content_type)
            .await?;
        return Ok(Json(final_session));
    }

    Ok(Json(session))
}

/// GET /storage/upload/resumable/{id} — Query upload progress (for resume).
pub async fn get_resumable_status(
    State(state): State<StorageState>,
    Path(session_id): Path<String>,
    Extension(auth): Extension<AuthContext>,
    _headers: HeaderMap,
) -> Result<Json<crate::resumable::UploadSession>> {
    let owner = require_authenticated_user(&auth)?;
    let session = state.resumable.get_session(&session_id, owner)?;
    Ok(Json(session))
}

/// DELETE /storage/upload/resumable/{id} — Cancel a resumable upload.
pub async fn cancel_resumable(
    State(state): State<StorageState>,
    Path(session_id): Path<String>,
    Extension(auth): Extension<AuthContext>,
    _headers: HeaderMap,
) -> Result<Json<serde_json::Value>> {
    let owner = require_authenticated_user(&auth)?;
    state.resumable.cancel(&session_id, owner).await?;
    Ok(Json(serde_json::json!({ "cancelled": session_id })))
}

// ── Batch presigned URL & delete handlers ──────────────────────────────

/// Max paths in a single batch request (prevents abuse).
const MAX_BATCH_PATHS: usize = 100;

#[derive(Deserialize)]
pub struct BatchPresignRequest {
    pub paths: Vec<String>,
    #[serde(default = "default_ttl")]
    pub ttl_secs: u64,
}

fn default_ttl() -> u64 {
    3600
}

#[derive(Serialize)]
pub struct PresignedUploadEntry {
    pub path: String,
    pub upload_url: String,
}

#[derive(Serialize)]
pub struct PresignedDownloadEntry {
    pub path: String,
    pub download_url: String,
}

#[derive(Deserialize)]
pub struct BatchDeleteRequest {
    pub paths: Vec<String>,
}

/// POST /storage/presign/upload — Generate presigned upload URLs in batch.
pub async fn batch_presign_upload(
    State(state): State<StorageState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<BatchPresignRequest>,
) -> Result<Json<serde_json::Value>> {
    require_authenticated_user(&auth)?;
    if req.paths.is_empty() {
        return Err(Error::Validation("paths must not be empty".into()));
    }
    if req.paths.len() > MAX_BATCH_PATHS {
        return Err(Error::Validation(format!(
            "Too many paths (max {MAX_BATCH_PATHS})"
        )));
    }

    let mut urls = Vec::with_capacity(req.paths.len());
    for raw_path in &req.paths {
        let safe = sanitize_storage_path(raw_path)?;
        if !can_user_write_path(&auth, &safe) {
            return Err(Error::Forbidden(format!(
                "Not allowed to upload to path '{safe}'"
            )));
        }
        let upload_url = state.url_generator.sign_upload(&safe, req.ttl_secs)?;
        urls.push(PresignedUploadEntry {
            path: safe,
            upload_url,
        });
    }

    Ok(Json(serde_json::json!({ "urls": urls })))
}

/// POST /storage/presign/download — Generate presigned download URLs in batch.
pub async fn batch_presign_download(
    State(state): State<StorageState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<BatchPresignRequest>,
) -> Result<Json<serde_json::Value>> {
    if req.paths.is_empty() {
        return Err(Error::Validation("paths must not be empty".into()));
    }
    if req.paths.len() > MAX_BATCH_PATHS {
        return Err(Error::Validation(format!(
            "Too many paths (max {MAX_BATCH_PATHS})"
        )));
    }

    let mut urls = Vec::with_capacity(req.paths.len());
    for raw_path in &req.paths {
        let safe = sanitize_storage_path(raw_path)?;
        let allowed =
            is_public_read_path(&safe) || (auth.authenticated && can_user_write_path(&auth, &safe));
        if !allowed {
            return Err(Error::Forbidden(format!(
                "Not allowed to download path '{safe}'"
            )));
        }
        let download_url = state.url_generator.sign_download(&safe, req.ttl_secs)?;
        urls.push(PresignedDownloadEntry {
            path: safe,
            download_url,
        });
    }

    Ok(Json(serde_json::json!({ "urls": urls })))
}

/// POST /storage/batch-delete — Delete multiple files (requires auth).
pub async fn batch_delete(
    State(state): State<StorageState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<BatchDeleteRequest>,
) -> Result<Json<serde_json::Value>> {
    require_authenticated_user(&auth)?;

    if req.paths.is_empty() {
        return Err(Error::Validation("paths must not be empty".into()));
    }
    if req.paths.len() > MAX_BATCH_PATHS {
        return Err(Error::Validation(format!(
            "Too many paths (max {MAX_BATCH_PATHS})"
        )));
    }

    let mut deleted = Vec::with_capacity(req.paths.len());
    let mut errors = Vec::new();
    for raw_path in &req.paths {
        let safe = match sanitize_storage_path(raw_path) {
            Ok(p) => p,
            Err(e) => {
                errors.push(serde_json::json!({ "path": raw_path, "error": e.to_string() }));
                continue;
            }
        };
        if !can_user_write_path(&auth, &safe) {
            errors.push(serde_json::json!({ "path": safe, "error": "forbidden" }));
            continue;
        }
        match state.storage.delete(&safe).await {
            Ok(()) => deleted.push(safe),
            Err(e) => {
                errors.push(serde_json::json!({ "path": safe, "error": e.to_string() }));
            }
        }
    }

    Ok(Json(
        serde_json::json!({ "deleted": deleted, "errors": errors }),
    ))
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
        // Batch presigned URL routes
        .route(
            "/storage/presign/upload",
            axum::routing::post(batch_presign_upload),
        )
        .route(
            "/storage/presign/download",
            axum::routing::post(batch_presign_download),
        )
        .route("/storage/batch-delete", axum::routing::post(batch_delete))
        // Resumable upload routes
        .route(
            "/storage/upload/resumable",
            axum::routing::post(init_resumable),
        )
        .route(
            "/storage/upload/resumable/{id}",
            axum::routing::patch(append_resumable)
                .get(get_resumable_status)
                .delete(cancel_resumable),
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

    // ── File signature validation tests ──

    #[test]
    fn test_validate_jpeg_valid() {
        // JPEG magic bytes: FF D8 FF
        let jpeg_bytes = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46];
        assert!(validate_file_signature(&jpeg_bytes, "image/jpeg").is_ok());
    }

    #[test]
    fn test_validate_png_valid() {
        // PNG magic bytes: 89 50 4E 47 0D 0A 1A 0A
        let png_bytes = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
        ];
        assert!(validate_file_signature(&png_bytes, "image/png").is_ok());
    }

    #[test]
    fn test_validate_disallowed_type() {
        let bytes = b"not relevant";
        let result = validate_file_signature(bytes, "text/html");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not allowed"));
    }

    #[test]
    fn test_validate_mime_mismatch() {
        // PNG magic bytes but claiming to be JPEG
        let png_bytes = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
        ];
        let result = validate_file_signature(&png_bytes, "image/jpeg");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("doesn't match"));
    }

    #[test]
    fn test_validate_undetectable_content() {
        // Random bytes that don't match any known file signature
        let random_bytes = [0x00, 0x01, 0x02, 0x03];
        let result = validate_file_signature(&random_bytes, "image/jpeg");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Could not verify"));
    }

    #[test]
    fn test_validate_empty_bytes() {
        let result = validate_file_signature(&[], "image/jpeg");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_pdf_valid() {
        // PDF magic bytes: %PDF
        let pdf_bytes = b"%PDF-1.4 some content here";
        assert!(validate_file_signature(pdf_bytes, "application/pdf").is_ok());
    }

    #[test]
    fn test_validate_octet_stream_rejected() {
        let bytes = [0xFF, 0xD8, 0xFF, 0xE0];
        let result = validate_file_signature(&bytes, "application/octet-stream");
        assert!(result.is_err());
    }

    #[test]
    fn test_max_upload_size_enforced() {
        // Verify that MAX_UPLOAD_SIZE constant is set
        assert!(MAX_UPLOAD_SIZE > 0);
        assert_eq!(MAX_UPLOAD_SIZE, 500 * 1024 * 1024); // 500MB
    }

    #[test]
    fn test_max_resumable_size_enforced() {
        // Verify that MAX_RESUMABLE_SIZE constant is set
        assert!(MAX_RESUMABLE_SIZE > 0);
        assert_eq!(MAX_RESUMABLE_SIZE, 5 * 1024 * 1024 * 1024); // 5GB
    }
}
