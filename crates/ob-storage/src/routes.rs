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
    "application/octet-stream",
];

/// Maximum file size for regular uploads: 500MB
const MAX_UPLOAD_SIZE: usize = 500 * 1024 * 1024;

/// Maximum total size for resumable uploads: 500MB (same as regular)
const MAX_RESUMABLE_SIZE: u64 = 500 * 1024 * 1024; // 500MB, same as regular uploads

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

    // Generic private binary uploads intentionally use octet-stream.
    // They are always downloaded as attachments, so we skip format sniffing here.
    if content_type == "application/octet-stream" {
        return Ok(());
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

    if path.starts_with("products/") || path.starts_with("products/videos/") {
        // Require the seller's user_id to appear in the path to prevent
        // horizontal privilege escalation between sellers.
        // Expected path format: products/{user_id}/... or products/videos/{user_id}/...
        return path.contains(&format!("/{}/", auth.user_id)) || auth.has_role("admin");
    }

    path.starts_with(&format!("users/{}/", auth.user_id))
        || path.starts_with(&format!("reviews/{}/", auth.user_id))
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
            "Total size {} exceeds limit {} (500MB)",
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
        let ttl_secs = req.ttl_secs.clamp(60, 86400); // 1min to 24h max
        let upload_url = state.url_generator.sign_upload(&safe, ttl_secs)?;
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
        let ttl_secs = req.ttl_secs.clamp(60, 86400); // 1min to 24h max
        let download_url = state.url_generator.sign_download(&safe, ttl_secs)?;
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
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    fn test_validate_octet_stream_allowed() {
        let bytes = [0xFF, 0xD8, 0xFF, 0xE0];
        let result = validate_file_signature(&bytes, "application/octet-stream");
        assert!(result.is_ok());
    }

    #[test]
    fn test_max_upload_size_enforced() {
        assert_eq!(MAX_UPLOAD_SIZE, 500 * 1024 * 1024); // 500MB
    }

    #[test]
    fn test_max_resumable_size_enforced() {
        assert_eq!(MAX_RESUMABLE_SIZE, 500 * 1024 * 1024); // 500MB
    }

    #[test]
    fn test_max_batch_paths_enforced() {
        assert_eq!(MAX_BATCH_PATHS, 100);
    }

    // ── sanitize_storage_path tests ──

    #[test]
    fn test_sanitize_normal_path() {
        let result = sanitize_storage_path("users/123/avatar.jpg").unwrap();
        assert_eq!(result, "users/123/avatar.jpg");
    }

    #[test]
    fn test_sanitize_rejects_empty_path() {
        let result = sanitize_storage_path("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_sanitize_strips_leading_dotdot() {
        let result = sanitize_storage_path("../../../etc/passwd");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "etc/passwd");
    }

    #[test]
    fn test_sanitize_removes_dotdot_in_middle() {
        // ".." is stripped, leaving "users//etc/passwd" which has empty components
        // and double slashes — rejected by validation
        let result = sanitize_storage_path("users/../etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitize_strips_leading_slash() {
        let result = sanitize_storage_path("/users/123/file.jpg").unwrap();
        assert_eq!(result, "users/123/file.jpg");
    }

    #[test]
    fn test_sanitize_normalizes_backslashes() {
        let result = sanitize_storage_path("users\\123\\file.jpg").unwrap();
        assert_eq!(result, "users/123/file.jpg");
    }

    #[test]
    fn test_sanitize_handles_nested_traversal() {
        // "....//" → after first replace ".." → ".." → after second replace → ""
        let result = sanitize_storage_path("....//");
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitize_single_dot_component_rejected() {
        let result = sanitize_storage_path("./file.txt");
        // "." component is rejected
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitize_preserves_valid_paths() {
        let result = sanitize_storage_path("products/img/photo.jpg").unwrap();
        assert_eq!(result, "products/img/photo.jpg");
    }

    #[test]
    fn test_sanitize_all_dots_removed() {
        // ".." sequences removed iteratively
        let result = sanitize_storage_path("a..b/c..d");
        // ".." gets removed → "ab/cd"
        assert_eq!(result.unwrap(), "ab/cd");
    }

    // ── is_public_read_path tests ──

    #[test]
    fn test_is_public_read_products() {
        assert!(is_public_read_path("products/img.jpg"));
        assert!(is_public_read_path("products/videos/vid.mp4"));
    }

    #[test]
    fn test_is_public_read_not_public() {
        assert!(!is_public_read_path("users/123/avatar.jpg"));
        assert!(!is_public_read_path("reviews/123/photo.jpg"));
        assert!(!is_public_read_path("other/path"));
    }

    // ── can_user_write_path tests ──

    #[test]
    fn test_can_write_user_own_path() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("OB_TEST_MODE", "1");
        }
        let auth = AuthContext {
            user_id: "user_123".to_string(),
            authenticated: true,
            ..AuthContext::anonymous()
        };
        assert!(can_user_write_path(&auth, "users/user_123/avatar.jpg"));
        assert!(can_user_write_path(&auth, "reviews/user_123/photo.jpg"));
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
    }

    #[test]
    fn test_can_write_products_path() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("OB_TEST_MODE", "1");
        }
        let auth = AuthContext {
            user_id: "user_123".to_string(),
            authenticated: true,
            ..AuthContext::anonymous()
        };
        // Products path requires user_id in path for ownership
        assert!(can_user_write_path(&auth, "products/user_123/img.jpg"));
        assert!(can_user_write_path(
            &auth,
            "products/videos/user_123/vid.mp4"
        ));
        // Without user_id in path, should be rejected
        assert!(!can_user_write_path(&auth, "products/img.jpg"));
        assert!(!can_user_write_path(&auth, "products/other_user/img.jpg"));
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
    }

    #[test]
    fn test_can_write_rejects_other_user_path() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Without test mode, unauthenticated should fail
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
        let auth = AuthContext {
            user_id: "user_123".to_string(),
            authenticated: false,
            ..AuthContext::anonymous()
        };
        assert!(!can_user_write_path(&auth, "users/user_456/avatar.jpg"));
    }

    #[test]
    fn test_can_write_rejects_unauthenticated() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
        let auth = AuthContext::anonymous();
        assert!(!can_user_write_path(&auth, "users/user_123/avatar.jpg"));
    }

    #[test]
    fn test_can_write_rejects_empty_user_id() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
        let auth = AuthContext {
            user_id: "".to_string(),
            authenticated: true,
            ..AuthContext::anonymous()
        };
        assert!(!can_user_write_path(&auth, "users/someone/avatar.jpg"));
    }

    // ── require_authenticated_user tests ──

    #[test]
    fn test_require_auth_in_test_mode_still_requires_auth() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("OB_TEST_MODE", "1");
        }
        // Authenticated user works normally
        let auth = AuthContext {
            user_id: "user_123".to_string(),
            authenticated: true,
            ..AuthContext::anonymous()
        };
        let result = require_authenticated_user(&auth);
        assert_eq!(result.unwrap(), "user_123");
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
    }

    #[test]
    fn test_require_auth_test_mode_no_longer_bypasses() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("OB_TEST_MODE", "1");
        }
        // Anonymous user is rejected even in test mode
        let auth = AuthContext::anonymous();
        let result = require_authenticated_user(&auth);
        assert!(result.is_err());
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
    }

    #[test]
    fn test_require_auth_fails_unauthenticated() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
        let auth = AuthContext::anonymous();
        let result = require_authenticated_user(&auth);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Authentication required")
        );
    }

    // ── Deserialization tests ──

    #[test]
    fn test_download_params_deserialize() {
        let json = r#"{"expires": 1700000000, "sig": "abc", "w": 200, "h": 100, "fit": "contain", "q": 90}"#;
        let params: DownloadParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.expires, 1700000000);
        assert_eq!(params.sig, "abc");
        assert_eq!(params.transform.w, Some(200));
        assert_eq!(params.transform.h, Some(100));
        assert_eq!(params.transform.fit, "contain");
        assert_eq!(params.transform.q, Some(90));
    }

    #[test]
    fn test_download_params_without_transform() {
        let json = r#"{"expires": 1700000000, "sig": "abc"}"#;
        let params: DownloadParams = serde_json::from_str(json).unwrap();
        assert!(!params.transform.has_transforms());
    }

    #[test]
    fn test_init_resumable_params_deserialize() {
        let json =
            r#"{"path": "users/123/file.mp4", "content_type": "video/mp4", "total_size": 1000000}"#;
        let params: InitResumableParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.path, "users/123/file.mp4");
        assert_eq!(params.content_type, "video/mp4");
        assert_eq!(params.total_size, 1000000);
    }

    #[test]
    fn test_init_resumable_params_default_content_type() {
        let json = r#"{"path": "users/123/file.mp4", "total_size": 1000000}"#;
        let params: InitResumableParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.content_type, "application/octet-stream");
    }

    #[test]
    fn test_batch_presign_request_deserialize() {
        let json = r#"{"paths": ["users/123/a.jpg", "users/123/b.jpg"], "ttl_secs": 7200}"#;
        let req: BatchPresignRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.paths.len(), 2);
        assert_eq!(req.ttl_secs, 7200);
    }

    #[test]
    fn test_batch_presign_request_default_ttl() {
        let json = r#"{"paths": ["users/123/a.jpg"]}"#;
        let req: BatchPresignRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.ttl_secs, 3600);
    }

    #[test]
    fn test_batch_delete_request_deserialize() {
        let json = r#"{"paths": ["a.jpg", "b.jpg"]}"#;
        let req: BatchDeleteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.paths.len(), 2);
    }

    #[test]
    fn test_presigned_upload_entry_serializes() {
        let entry = PresignedUploadEntry {
            path: "test.jpg".to_string(),
            upload_url: "https://example.com/upload".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["path"], "test.jpg");
        assert_eq!(parsed["upload_url"], "https://example.com/upload");
    }

    #[test]
    fn test_presigned_download_entry_serializes() {
        let entry = PresignedDownloadEntry {
            path: "test.jpg".to_string(),
            download_url: "https://example.com/download".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["path"], "test.jpg");
        assert_eq!(parsed["download_url"], "https://example.com/download");
    }

    #[test]
    fn test_default_content_type_is_octet_stream() {
        assert_eq!(default_content_type(), "application/octet-stream");
    }

    #[test]
    fn test_default_ttl_is_one_hour() {
        assert_eq!(default_ttl(), 3600);
    }

    #[test]
    fn test_allowed_upload_types_contains_expected() {
        assert!(ALLOWED_UPLOAD_TYPES.contains(&"image/jpeg"));
        assert!(ALLOWED_UPLOAD_TYPES.contains(&"image/png"));
        assert!(ALLOWED_UPLOAD_TYPES.contains(&"image/gif"));
        assert!(ALLOWED_UPLOAD_TYPES.contains(&"image/webp"));
        assert!(ALLOWED_UPLOAD_TYPES.contains(&"application/pdf"));
        assert!(ALLOWED_UPLOAD_TYPES.contains(&"application/octet-stream"));
        assert!(!ALLOWED_UPLOAD_TYPES.contains(&"text/html"));
        assert!(!ALLOWED_UPLOAD_TYPES.contains(&"video/mp4"));
    }

    #[test]
    fn test_validate_webp_valid() {
        // WebP magic bytes: RIFF....WEBP
        let mut webp_bytes = b"RIFF".to_vec();
        webp_bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // file size placeholder
        webp_bytes.extend_from_slice(b"WEBP");
        let result = validate_file_signature(&webp_bytes, "image/webp");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_gif_valid() {
        let gif_bytes = b"GIF89a\x01\x00\x01\x00";
        assert!(validate_file_signature(gif_bytes, "image/gif").is_ok());
    }

    #[test]
    fn test_validate_gif87_valid() {
        let gif_bytes = b"GIF87a\x01\x00\x01\x00";
        assert!(validate_file_signature(gif_bytes, "image/gif").is_ok());
    }

    #[test]
    fn test_sanitize_only_slashes() {
        let result = sanitize_storage_path("///");
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitize_backslash_traversal() {
        let result = sanitize_storage_path("..\\..\\etc\\passwd");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "etc/passwd");
    }

    #[test]
    fn test_sanitize_path_with_spaces() {
        let result = sanitize_storage_path("users/123/my file.jpg").unwrap();
        assert_eq!(result, "users/123/my file.jpg");
    }

    #[test]
    fn test_sanitize_path_with_unicode() {
        let result = sanitize_storage_path("users/123/файл.jpg").unwrap();
        assert_eq!(result, "users/123/файл.jpg");
    }

    #[test]
    fn test_sanitize_double_slash_in_path() {
        let result = sanitize_storage_path("users//file.jpg");
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitize_deeply_nested_traversal() {
        let result = sanitize_storage_path("../../../../etc/passwd");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "etc/passwd");
    }

    #[test]
    fn test_sanitize_mixed_traversal_and_normal() {
        let result = sanitize_storage_path("a/../b/../c");
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitize_path_with_leading_dot() {
        let result = sanitize_storage_path(".hidden/file.txt");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ".hidden/file.txt");
    }

    // ── more is_public_read_path tests ──

    #[test]
    fn test_is_public_read_products_exact() {
        assert!(is_public_read_path("products/"));
    }

    #[test]
    fn test_is_public_read_empty_string() {
        assert!(!is_public_read_path(""));
    }

    #[test]
    fn test_is_public_read_similar_prefix() {
        assert!(!is_public_read_path("products_backup/file.jpg"));
    }

    // ── more can_user_write_path tests ──

    #[test]
    fn test_can_write_empty_path() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
        let auth = AuthContext {
            user_id: "user_123".to_string(),
            authenticated: true,
            ..AuthContext::anonymous()
        };
        assert!(!can_user_write_path(&auth, ""));
    }

    #[test]
    fn test_can_write_user_exact_prefix() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("OB_TEST_MODE", "1");
        }
        let auth = AuthContext {
            user_id: "u1".to_string(),
            authenticated: true,
            ..AuthContext::anonymous()
        };
        assert!(can_user_write_path(&auth, "users/u1/"));
        assert!(can_user_write_path(&auth, "reviews/u1/photo.jpg"));
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
    }

    #[test]
    fn test_can_write_rejects_similar_user_prefix() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
        let auth = AuthContext {
            user_id: "u1".to_string(),
            authenticated: true,
            ..AuthContext::anonymous()
        };
        assert!(!can_user_write_path(&auth, "users/u10/avatar.jpg"));
    }

    // ── more require_authenticated_user tests ──

    #[test]
    fn test_require_auth_fails_empty_user_id_not_authenticated() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
        let auth = AuthContext {
            user_id: "".to_string(),
            authenticated: false,
            ..AuthContext::anonymous()
        };
        assert!(require_authenticated_user(&auth).is_err());
    }

    // ── more deserialization tests ──

    #[test]
    fn test_batch_presign_request_empty_paths() {
        let json = r#"{"paths": []}"#;
        let req: BatchPresignRequest = serde_json::from_str(json).unwrap();
        assert!(req.paths.is_empty());
    }

    #[test]
    fn test_batch_delete_request_empty_paths() {
        let json = r#"{"paths": []}"#;
        let req: BatchDeleteRequest = serde_json::from_str(json).unwrap();
        assert!(req.paths.is_empty());
    }

    #[test]
    fn test_download_params_with_format() {
        let json = r#"{"expires": 100, "sig": "x", "w": 100, "format": "webp"}"#;
        let params: DownloadParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.transform.format, Some("webp".to_string()));
        assert!(params.transform.has_transforms());
    }

    #[test]
    fn test_validate_gif_mismatch() {
        let gif_bytes = b"GIF89a\x01\x00\x01\x00";
        let result = validate_file_signature(gif_bytes, "image/png");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("doesn't match"));
    }

    #[test]
    fn test_validate_webp_mismatch() {
        let mut webp_bytes = b"RIFF".to_vec();
        webp_bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        webp_bytes.extend_from_slice(b"WEBP");
        let result = validate_file_signature(&webp_bytes, "image/jpeg");
        assert!(result.is_err());
    }

    // ── constant value tests ──

    #[test]
    fn test_allowed_upload_types_count() {
        assert_eq!(ALLOWED_UPLOAD_TYPES.len(), 6);
    }

    #[test]
    fn test_validate_file_signature_octet_stream_allowed() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("OB_TEST_MODE", "1");
        }
        let result = validate_file_signature(b"random bytes", "application/octet-stream");
        assert!(
            result.is_ok(),
            "octet-stream should be allowed for binary uploads"
        );
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
    }

    #[test]
    fn test_validate_file_signature_test_mode_still_validates_others() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("OB_TEST_MODE", "1");
        }
        // In test mode, non-octet-stream types still go through normal validation
        let result = validate_file_signature(b"not a jpeg", "image/jpeg");
        // This should fail because magic bytes don't match
        assert!(result.is_err());
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
    }

    #[test]
    fn test_can_write_test_mode_no_longer_bypasses_auth() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("OB_TEST_MODE", "1");
        }
        let auth = AuthContext::anonymous();
        // Test mode no longer bypasses auth — anonymous users cannot write
        assert!(!can_user_write_path(&auth, "any/random/path.jpg"));
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
    }

    #[test]
    fn test_can_write_authenticated_user_own_reviews_path() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
        let auth = AuthContext {
            user_id: "user_abc".to_string(),
            authenticated: true,
            ..AuthContext::anonymous()
        };
        assert!(can_user_write_path(&auth, "reviews/user_abc/photo.jpg"));
        assert!(!can_user_write_path(&auth, "reviews/other_user/photo.jpg"));
    }

    #[test]
    fn test_require_auth_authenticated_with_user_id() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
        let auth = AuthContext {
            user_id: "real_user_123".to_string(),
            authenticated: true,
            ..AuthContext::anonymous()
        };
        let result = require_authenticated_user(&auth);
        assert_eq!(result.unwrap(), "real_user_123");
    }

    #[test]
    fn test_require_auth_authenticated_but_empty_user_id() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("OB_TEST_MODE");
        }
        let auth = AuthContext {
            user_id: "".to_string(),
            authenticated: true,
            ..AuthContext::anonymous()
        };
        let result = require_authenticated_user(&auth);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_public_read_path_products_videos() {
        assert!(is_public_read_path("products/videos/clip.mp4"));
        assert!(is_public_read_path("products/image.jpg"));
    }

    #[test]
    fn test_is_public_read_path_case_sensitive() {
        assert!(!is_public_read_path("Products/file.jpg"));
        assert!(!is_public_read_path("PRODUCTS/file.jpg"));
    }

    #[test]
    fn test_sanitize_triple_dots() {
        // "..." contains ".." so it gets removed
        let result = sanitize_storage_path("...file.txt");
        // ".." removed → ".file.txt"
        assert!(result.is_ok());
    }

    #[test]
    fn test_sanitize_path_single_file() {
        let result = sanitize_storage_path("file.txt").unwrap();
        assert_eq!(result, "file.txt");
    }

    #[test]
    fn test_storage_router_builds() {
        let storage = crate::LocalStorage::new("./test_storage_data").unwrap();
        let url_gen = crate::SignedUrlGenerator::new("test-secret-key", "/api/storage");
        let resumable = crate::resumable::ResumableUploadManager::new("./test_resumable").unwrap();
        let state = StorageState {
            storage,
            url_generator: url_gen,
            resumable,
        };
        let _router = storage_router(state);
    }
}
