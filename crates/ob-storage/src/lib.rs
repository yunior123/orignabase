pub mod local;
pub mod resumable;
pub mod routes;
pub mod s3;
pub mod signed_url;
pub mod transform;

pub use local::LocalStorage;
pub use resumable::ResumableUploadManager;
pub use s3::{S3Config, S3Storage};
pub use signed_url::SignedUrlGenerator;

use ob_core::Result;
use serde::{Deserialize, Serialize};

/// Metadata about a stored object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMeta {
    pub path: String,
    pub size: u64,
    pub content_type: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Trait for storage backends (local filesystem, S3, R2, etc.)
pub trait StorageBackend: Send + Sync {
    /// Upload bytes to a path. Returns metadata.
    fn upload(
        &self,
        path: &str,
        data: &[u8],
        content_type: &str,
    ) -> impl std::future::Future<Output = Result<ObjectMeta>> + Send;

    /// Download bytes from a path.
    fn download(&self, path: &str) -> impl std::future::Future<Output = Result<Vec<u8>>> + Send;

    /// Delete an object at a path.
    fn delete(&self, path: &str) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Check if an object exists.
    fn exists(&self, path: &str) -> impl std::future::Future<Output = Result<bool>> + Send;

    /// Get metadata for an object.
    fn metadata(&self, path: &str) -> impl std::future::Future<Output = Result<ObjectMeta>> + Send;

    /// List objects under a prefix.
    fn list(
        &self,
        prefix: &str,
    ) -> impl std::future::Future<Output = Result<Vec<ObjectMeta>>> + Send;
}
