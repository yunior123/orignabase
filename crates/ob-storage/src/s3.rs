use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client,
    config::{Builder as S3ConfigBuilder, Region},
    primitives::ByteStream,
};
use ob_core::{Error, Result};

use crate::{ObjectMeta, StorageBackend};

/// Configuration for S3-compatible storage (AWS S3, Cloudflare R2, MinIO).
#[derive(Clone)]
pub struct S3Config {
    pub bucket: String,
    pub region: String,
    pub endpoint: Option<String>,
    pub access_key: String,
    pub secret_key: String,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("access_key", &self.access_key)
            .field("secret_key", &"[REDACTED]")
            .finish()
    }
}

impl S3Config {
    /// Create from environment variables (OB_STORAGE__*).
    pub fn from_env() -> Option<Self> {
        Some(Self {
            bucket: std::env::var("OB_STORAGE__S3_BUCKET").ok()?,
            region: std::env::var("OB_STORAGE__S3_REGION").unwrap_or_else(|_| "auto".to_string()),
            endpoint: std::env::var("OB_STORAGE__S3_ENDPOINT").ok(),
            access_key: std::env::var("OB_STORAGE__S3_ACCESS_KEY").ok()?,
            secret_key: std::env::var("OB_STORAGE__S3_SECRET_KEY").ok()?,
        })
    }
}

/// S3-compatible storage backend.
///
/// Works with AWS S3, Cloudflare R2, MinIO, and other S3-compatible services.
pub struct S3Storage {
    client: Client,
    bucket: String,
}

impl S3Storage {
    pub async fn new(config: S3Config) -> Result<Self> {
        let creds = Credentials::new(
            &config.access_key,
            &config.secret_key,
            None,
            None,
            "orignabase",
        );

        let mut s3_config = S3ConfigBuilder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(config.region))
            .credentials_provider(creds)
            .force_path_style(true);

        if let Some(endpoint) = &config.endpoint {
            s3_config = s3_config.endpoint_url(endpoint);
        }

        let client = Client::from_conf(s3_config.build());

        Ok(Self {
            client,
            bucket: config.bucket,
        })
    }

    /// Generate a presigned URL for downloading an object.
    pub async fn presign_download(&self, path: &str, expires_secs: u64) -> Result<String> {
        use aws_sdk_s3::presigning::PresigningConfig;
        use std::time::Duration;

        let presigning_config = PresigningConfig::builder()
            .expires_in(Duration::from_secs(expires_secs))
            .build()
            .map_err(|e| Error::Internal(format!("Presign config error: {e}")))?;

        let presigned = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(path)
            .presigned(presigning_config)
            .await
            .map_err(|e| Error::Internal(format!("Presign failed: {e}")))?;

        Ok(presigned.uri().to_string())
    }

    /// Generate a presigned URL for uploading an object.
    pub async fn presign_upload(
        &self,
        path: &str,
        content_type: &str,
        expires_secs: u64,
    ) -> Result<String> {
        use aws_sdk_s3::presigning::PresigningConfig;
        use std::time::Duration;

        let presigning_config = PresigningConfig::builder()
            .expires_in(Duration::from_secs(expires_secs))
            .build()
            .map_err(|e| Error::Internal(format!("Presign config error: {e}")))?;

        let presigned = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(path)
            .content_type(content_type)
            .presigned(presigning_config)
            .await
            .map_err(|e| Error::Internal(format!("Presign failed: {e}")))?;

        Ok(presigned.uri().to_string())
    }
}

impl StorageBackend for S3Storage {
    async fn upload(&self, path: &str, data: &[u8], content_type: &str) -> Result<ObjectMeta> {
        let body = ByteStream::from(data.to_vec());

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(path)
            .body(body)
            .content_type(content_type)
            .send()
            .await
            .map_err(|e| Error::Internal(format!("S3 upload failed: {e}")))?;

        Ok(ObjectMeta {
            path: path.to_string(),
            size: data.len() as u64,
            content_type: content_type.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    async fn download(&self, path: &str) -> Result<Vec<u8>> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(path)
            .send()
            .await
            .map_err(|e| Error::NotFound(format!("S3 download failed: {e}")))?;

        let bytes = resp
            .body
            .collect()
            .await
            .map_err(|e| Error::Internal(format!("S3 read body failed: {e}")))?
            .into_bytes()
            .to_vec();

        Ok(bytes)
    }

    async fn delete(&self, path: &str) -> Result<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(path)
            .send()
            .await
            .map_err(|e| Error::Internal(format!("S3 delete failed: {e}")))?;

        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(path)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    async fn metadata(&self, path: &str) -> Result<ObjectMeta> {
        let resp = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(path)
            .send()
            .await
            .map_err(|e| Error::NotFound(format!("S3 metadata failed: {e}")))?;

        Ok(ObjectMeta {
            path: path.to_string(),
            size: resp.content_length().unwrap_or(0) as u64,
            content_type: resp
                .content_type()
                .unwrap_or("application/octet-stream")
                .to_string(),
            created_at: resp
                .last_modified()
                .map(|t| t.to_string())
                .unwrap_or_default(),
            updated_at: resp
                .last_modified()
                .map(|t| t.to_string())
                .unwrap_or_default(),
        })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<ObjectMeta>> {
        let resp = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix)
            .send()
            .await
            .map_err(|e| Error::Internal(format!("S3 list failed: {e}")))?;

        let objects = resp
            .contents()
            .iter()
            .map(|obj| ObjectMeta {
                path: obj.key().unwrap_or_default().to_string(),
                size: obj.size().unwrap_or(0) as u64,
                content_type: "application/octet-stream".to_string(),
                created_at: obj
                    .last_modified()
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
                updated_at: obj
                    .last_modified()
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
            })
            .collect();

        Ok(objects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_s3_config_fields() {
        let config = S3Config {
            bucket: "test-bucket".into(),
            region: "us-east-1".into(),
            endpoint: Some("http://localhost:9000".into()),
            access_key: "access".into(),
            secret_key: "secret".into(),
        };
        assert_eq!(config.bucket, "test-bucket");
        assert_eq!(config.endpoint.as_deref(), Some("http://localhost:9000"));
    }

    #[test]
    fn test_s3_config_from_env_missing() {
        let config = S3Config::from_env();
        let _ = config;
    }

    #[test]
    fn test_s3_config_clone() {
        let config = S3Config {
            bucket: "bucket".into(),
            region: "us-east-1".into(),
            endpoint: None,
            access_key: "key".into(),
            secret_key: "secret".into(),
        };
        let cloned = config.clone();
        assert_eq!(cloned.bucket, config.bucket);
        assert_eq!(cloned.region, config.region);
        assert_eq!(cloned.endpoint, config.endpoint);
    }

    #[test]
    fn test_s3_config_debug() {
        let config = S3Config {
            bucket: "my-bucket".into(),
            region: "eu-west-1".into(),
            endpoint: Some("http://localhost:9000".into()),
            access_key: "ak".into(),
            secret_key: "sk".into(),
        };
        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("my-bucket"));
        assert!(debug_str.contains("eu-west-1"));
        assert!(debug_str.contains("localhost:9000"));
    }

    #[test]
    fn test_s3_config_from_env_with_all_vars() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("OB_STORAGE__S3_BUCKET", "test-bucket");
        }
        unsafe {
            std::env::set_var("OB_STORAGE__S3_REGION", "us-west-2");
        }
        unsafe {
            std::env::set_var("OB_STORAGE__S3_ENDPOINT", "http://localhost:9000");
        }
        unsafe {
            std::env::set_var("OB_STORAGE__S3_ACCESS_KEY", "test-key");
        }
        unsafe {
            std::env::set_var("OB_STORAGE__S3_SECRET_KEY", "test-secret");
        }

        let config = S3Config::from_env().unwrap();
        assert_eq!(config.bucket, "test-bucket");
        assert_eq!(config.region, "us-west-2");
        assert_eq!(config.endpoint, Some("http://localhost:9000".to_string()));
        assert_eq!(config.access_key, "test-key");
        assert_eq!(config.secret_key, "test-secret");

        unsafe {
            std::env::remove_var("OB_STORAGE__S3_BUCKET");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_REGION");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_ENDPOINT");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_ACCESS_KEY");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_SECRET_KEY");
        }
    }

    #[test]
    fn test_s3_config_from_env_region_defaults_to_auto() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("OB_STORAGE__S3_BUCKET", "test-bucket-rto");
        }
        unsafe {
            std::env::set_var("OB_STORAGE__S3_ACCESS_KEY", "test-key-rto");
        }
        unsafe {
            std::env::set_var("OB_STORAGE__S3_SECRET_KEY", "test-secret-rto");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_REGION");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_ENDPOINT");
        }

        let config = S3Config::from_env().unwrap();
        assert_eq!(config.region, "auto");
        assert!(config.endpoint.is_none());

        unsafe {
            std::env::remove_var("OB_STORAGE__S3_BUCKET");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_ACCESS_KEY");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_SECRET_KEY");
        }
    }

    #[test]
    fn test_s3_config_from_env_missing_bucket() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_BUCKET");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_ACCESS_KEY");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_SECRET_KEY");
        }
        assert!(S3Config::from_env().is_none());
    }

    #[test]
    fn test_s3_config_from_env_missing_access_key() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("OB_STORAGE__S3_BUCKET", "bucket");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_ACCESS_KEY");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_SECRET_KEY");
        }
        assert!(S3Config::from_env().is_none());
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_BUCKET");
        }
    }

    #[test]
    fn test_s3_config_from_env_missing_secret_key() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("OB_STORAGE__S3_BUCKET", "bucket");
        }
        unsafe {
            std::env::set_var("OB_STORAGE__S3_ACCESS_KEY", "key");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_SECRET_KEY");
        }
        assert!(S3Config::from_env().is_none());
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_BUCKET");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_ACCESS_KEY");
        }
    }

    #[test]
    fn test_s3_config_empty_strings() {
        let config = S3Config {
            bucket: "".into(),
            region: "".into(),
            endpoint: None,
            access_key: "".into(),
            secret_key: "".into(),
        };
        assert!(config.bucket.is_empty());
        assert!(config.region.is_empty());
    }

    #[test]
    fn test_s3_config_with_endpoint_none() {
        let config = S3Config {
            bucket: "b".into(),
            region: "r".into(),
            endpoint: None,
            access_key: "a".into(),
            secret_key: "s".into(),
        };
        assert!(config.endpoint.is_none());
    }

    #[test]
    fn test_s3_config_debug_shows_all_fields() {
        let config = S3Config {
            bucket: "b".into(),
            region: "r".into(),
            endpoint: None,
            access_key: "ak".into(),
            secret_key: "sk".into(),
        };
        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("bucket"));
        assert!(debug_str.contains("region"));
        assert!(debug_str.contains("endpoint"));
        assert!(debug_str.contains("access_key"));
        assert!(debug_str.contains("secret_key"));
        assert!(debug_str.contains("ak"));
        // Secret key should be redacted for security
        assert!(debug_str.contains("[REDACTED]"));
        assert!(!debug_str.contains("sk"));
    }

    #[test]
    fn test_s3_config_clone_preserves_all_fields() {
        let config = S3Config {
            bucket: "b".into(),
            region: "r".into(),
            endpoint: Some("http://s3:9000".into()),
            access_key: "ak".into(),
            secret_key: "sk".into(),
        };
        let clone = config.clone();
        assert_eq!(clone.bucket, config.bucket);
        assert_eq!(clone.region, config.region);
        assert_eq!(clone.endpoint, config.endpoint);
        assert_eq!(clone.access_key, config.access_key);
        assert_eq!(clone.secret_key, config.secret_key);
    }

    #[test]
    fn test_s3_config_partial_env_missing_access_key() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("OB_STORAGE__S3_BUCKET", "b");
        }
        unsafe {
            std::env::set_var("OB_STORAGE__S3_REGION", "us-east-1");
        }
        unsafe {
            std::env::set_var("OB_STORAGE__S3_SECRET_KEY", "sk");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_ACCESS_KEY");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_ENDPOINT");
        }
        assert!(S3Config::from_env().is_none());
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_BUCKET");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_REGION");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_SECRET_KEY");
        }
    }

    #[test]
    fn test_s3_config_region_empty_string_is_used() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("OB_STORAGE__S3_BUCKET", "b");
        }
        unsafe {
            std::env::set_var("OB_STORAGE__S3_REGION", "");
        }
        unsafe {
            std::env::set_var("OB_STORAGE__S3_ACCESS_KEY", "ak");
        }
        unsafe {
            std::env::set_var("OB_STORAGE__S3_SECRET_KEY", "sk");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_ENDPOINT");
        }
        let config = S3Config::from_env().unwrap();
        assert_eq!(config.region, "");
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_BUCKET");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_REGION");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_ACCESS_KEY");
        }
        unsafe {
            std::env::remove_var("OB_STORAGE__S3_SECRET_KEY");
        }
    }

    #[test]
    fn test_s3_config_field_access() {
        let config = S3Config {
            bucket: "photos".into(),
            region: "eu-central-1".into(),
            endpoint: Some("https://r2.cloudflarestorage.com".into()),
            access_key: "key1".into(),
            secret_key: "secret1".into(),
        };
        assert_eq!(config.bucket.len(), 6);
        assert_eq!(config.region, "eu-central-1");
        assert!(config.endpoint.as_ref().unwrap().contains("cloudflare"));
    }

    /// Helper to create S3Storage pointing at a non-existent endpoint for error path testing
    async fn test_s3_storage() -> S3Storage {
        S3Storage::new(S3Config {
            bucket: "test-bucket".into(),
            region: "us-east-1".into(),
            endpoint: Some("http://127.0.0.1:19999".into()), // no server here
            access_key: "test-access".into(),
            secret_key: "test-secret".into(),
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_s3_storage_new_creates_client() {
        let storage = test_s3_storage().await;
        assert_eq!(storage.bucket, "test-bucket");
    }

    #[tokio::test]
    async fn test_s3_storage_upload_error_on_unreachable() {
        let storage = test_s3_storage().await;
        let result = storage
            .upload("test/file.txt", b"hello", "text/plain")
            .await;
        assert!(result.is_err(), "Upload to unreachable S3 should fail");
    }

    #[tokio::test]
    async fn test_s3_storage_download_error_on_unreachable() {
        let storage = test_s3_storage().await;
        let result = storage.download("test/file.txt").await;
        assert!(result.is_err(), "Download from unreachable S3 should fail");
    }

    #[tokio::test]
    async fn test_s3_storage_delete_error_on_unreachable() {
        let storage = test_s3_storage().await;
        let result = storage.delete("test/file.txt").await;
        assert!(result.is_err(), "Delete on unreachable S3 should fail");
    }

    #[tokio::test]
    async fn test_s3_storage_exists_returns_false_on_unreachable() {
        let storage = test_s3_storage().await;
        let result = storage.exists("test/file.txt").await;
        // exists() returns Ok(false) on error, not Err
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[tokio::test]
    async fn test_s3_storage_metadata_error_on_unreachable() {
        let storage = test_s3_storage().await;
        let result = storage.metadata("test/file.txt").await;
        assert!(result.is_err(), "Metadata on unreachable S3 should fail");
    }

    #[tokio::test]
    async fn test_s3_storage_list_error_on_unreachable() {
        let storage = test_s3_storage().await;
        let result = storage.list("test/").await;
        assert!(result.is_err(), "List on unreachable S3 should fail");
    }

    #[tokio::test]
    async fn test_s3_storage_presign_download_generates_url() {
        let storage = test_s3_storage().await;
        let result = storage.presign_download("test/file.txt", 3600).await;
        // Presigning doesn't need network — it's computed locally
        assert!(
            result.is_ok(),
            "Presign download should succeed without network"
        );
        let url = result.unwrap();
        assert!(
            url.contains("test-bucket"),
            "URL should contain bucket name"
        );
        assert!(
            url.contains("test/file.txt"),
            "URL should contain object key"
        );
    }

    #[tokio::test]
    async fn test_s3_storage_presign_upload_generates_url() {
        let storage = test_s3_storage().await;
        let result = storage
            .presign_upload("uploads/img.jpg", "image/jpeg", 3600)
            .await;
        assert!(
            result.is_ok(),
            "Presign upload should succeed without network"
        );
        let url = result.unwrap();
        assert!(
            url.contains("test-bucket"),
            "URL should contain bucket name"
        );
        assert!(
            url.contains("uploads/img.jpg"),
            "URL should contain object key"
        );
    }

    #[tokio::test]
    async fn test_s3_storage_new_without_endpoint() {
        // Tests the branch where config.endpoint is None (no custom endpoint)
        let storage = S3Storage::new(S3Config {
            bucket: "no-endpoint-bucket".into(),
            region: "us-west-2".into(),
            endpoint: None,
            access_key: "access".into(),
            secret_key: "secret".into(),
        })
        .await
        .unwrap();
        assert_eq!(storage.bucket, "no-endpoint-bucket");
    }

    #[tokio::test]
    async fn test_s3_storage_presign_download_different_keys() {
        let storage = test_s3_storage().await;
        // Test with nested path
        let result = storage.presign_download("a/b/c/d.txt", 600).await;
        assert!(result.is_ok());
        let url = result.unwrap();
        assert!(url.contains("a/b/c/d.txt"));
    }

    #[tokio::test]
    async fn test_s3_storage_presign_upload_different_content_types() {
        let storage = test_s3_storage().await;
        let result = storage
            .presign_upload("docs/report.pdf", "application/pdf", 1800)
            .await;
        assert!(result.is_ok());
        let url = result.unwrap();
        assert!(url.contains("docs/report.pdf"));
    }

    #[tokio::test]
    async fn test_s3_storage_upload_empty_data() {
        let storage = test_s3_storage().await;
        // Empty data upload to unreachable server should still fail
        let result = storage.upload("empty.txt", b"", "text/plain").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_s3_storage_download_nested_path() {
        let storage = test_s3_storage().await;
        let result = storage.download("deep/nested/path/file.bin").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_s3_storage_delete_nested_path() {
        let storage = test_s3_storage().await;
        let result = storage.delete("some/path/to/delete.jpg").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_s3_storage_list_with_prefix() {
        let storage = test_s3_storage().await;
        let result = storage.list("users/123/").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_s3_storage_metadata_with_path() {
        let storage = test_s3_storage().await;
        let result = storage.metadata("path/to/check.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_s3_storage_exists_different_paths() {
        let storage = test_s3_storage().await;
        // exists() returns Ok(false) for unreachable server
        assert!(!storage.exists("any/path").await.unwrap());
        assert!(!storage.exists("another/path.jpg").await.unwrap());
    }
}
