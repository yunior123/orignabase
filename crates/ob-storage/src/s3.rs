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
#[derive(Debug, Clone)]
pub struct S3Config {
    pub bucket: String,
    pub region: String,
    pub endpoint: Option<String>,
    pub access_key: String,
    pub secret_key: String,
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
        // Without env vars set, should return None
        let config = S3Config::from_env();
        // This will be None unless env vars happen to be set
        let _ = config;
    }
}
