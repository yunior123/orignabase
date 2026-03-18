use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use ob_core::{Error, Result};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Generates and verifies HMAC-signed URLs for temporary file access.
#[derive(Clone)]
pub struct SignedUrlGenerator {
    secret: Vec<u8>,
    base_url: String,
}

impl SignedUrlGenerator {
    pub fn new(secret: &str, base_url: &str) -> Self {
        Self {
            secret: secret.as_bytes().to_vec(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Generate a signed download URL that expires after `ttl_secs`.
    pub fn sign_download(&self, path: &str, ttl_secs: u64) -> Result<String> {
        let expires = chrono::Utc::now().timestamp() as u64 + ttl_secs;
        let message = format!("GET:{path}:{expires}");

        let signature = self.compute_signature(&message)?;
        Ok(format!(
            "{}/storage/download/{}?expires={}&sig={}",
            self.base_url, path, expires, signature
        ))
    }

    /// Generate a signed upload URL.
    pub fn sign_upload(&self, path: &str, ttl_secs: u64) -> Result<String> {
        let expires = chrono::Utc::now().timestamp() as u64 + ttl_secs;
        let message = format!("PUT:{path}:{expires}");

        let signature = self.compute_signature(&message)?;
        Ok(format!(
            "{}/storage/upload/{}?expires={}&sig={}",
            self.base_url, path, expires, signature
        ))
    }

    /// Verify a signed URL's signature and check expiration.
    /// Uses constant-time comparison to prevent timing attacks.
    pub fn verify(&self, method: &str, path: &str, expires: u64, signature: &str) -> Result<bool> {
        // Check expiration
        let now = chrono::Utc::now().timestamp() as u64;
        if now > expires {
            return Err(Error::Auth("Signed URL has expired".into()));
        }

        // Verify signature using HMAC's built-in constant-time verification
        let message = format!("{method}:{path}:{expires}");
        let mut mac = HmacSha256::new_from_slice(&self.secret)
            .map_err(|e| Error::Internal(format!("HMAC init failed: {e}")))?;
        mac.update(message.as_bytes());

        // Decode the provided signature from base64
        let sig_bytes = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| Error::Auth("Invalid signature encoding".into()))?;

        // constant-time comparison via hmac::Mac::verify_slice
        Ok(mac.verify_slice(&sig_bytes).is_ok())
    }

    fn compute_signature(&self, message: &str) -> Result<String> {
        let mut mac = HmacSha256::new_from_slice(&self.secret)
            .map_err(|e| Error::Internal(format!("HMAC init failed: {e}")))?;
        mac.update(message.as_bytes());
        let result = mac.finalize();
        Ok(URL_SAFE_NO_PAD.encode(result.into_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_and_verify_download() {
        let signer = SignedUrlGenerator::new("test_secret", "http://localhost:8080");

        let url = signer.sign_download("users/123/avatar.jpg", 3600).unwrap();
        assert!(url.contains("/storage/download/users/123/avatar.jpg"));
        assert!(url.contains("expires="));
        assert!(url.contains("sig="));

        // Extract params
        let parts: Vec<&str> = url.split('?').collect();
        let params: Vec<&str> = parts[1].split('&').collect();
        let expires: u64 = params[0].strip_prefix("expires=").unwrap().parse().unwrap();
        let sig = params[1].strip_prefix("sig=").unwrap();

        assert!(
            signer
                .verify("GET", "users/123/avatar.jpg", expires, sig)
                .unwrap()
        );
    }

    #[test]
    fn test_wrong_signature_fails() {
        let signer = SignedUrlGenerator::new("test_secret", "http://localhost:8080");
        let expires = chrono::Utc::now().timestamp() as u64 + 3600;

        assert!(
            !signer
                .verify("GET", "file.txt", expires, "bad_sig")
                .unwrap()
        );
    }

    #[test]
    fn test_expired_url_fails() {
        let signer = SignedUrlGenerator::new("test_secret", "http://localhost:8080");
        let expired = chrono::Utc::now().timestamp() as u64 - 100; // already expired

        let result = signer.verify("GET", "file.txt", expired, "any");
        assert!(result.is_err());
    }

    #[test]
    fn test_different_secrets_produce_different_sigs() {
        let gen1 = SignedUrlGenerator::new("secret1", "http://localhost");
        let gen2 = SignedUrlGenerator::new("secret2", "http://localhost");

        let url1 = gen1.sign_download("test.txt", 3600).unwrap();
        let url2 = gen2.sign_download("test.txt", 3600).unwrap();

        // Signatures should differ
        let sig1 = url1.split("sig=").nth(1).unwrap();
        let sig2 = url2.split("sig=").nth(1).unwrap();
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_sign_and_verify_upload() {
        let signer = SignedUrlGenerator::new("upload_secret", "http://localhost:9000");

        let url = signer.sign_upload("docs/report.pdf", 600).unwrap();
        assert!(url.contains("/storage/upload/docs/report.pdf"));
        assert!(url.contains("expires="));
        assert!(url.contains("sig="));

        // Extract and verify
        let parts: Vec<&str> = url.split('?').collect();
        let params: Vec<&str> = parts[1].split('&').collect();
        let expires: u64 = params[0].strip_prefix("expires=").unwrap().parse().unwrap();
        let sig = params[1].strip_prefix("sig=").unwrap();

        assert!(
            signer
                .verify("PUT", "docs/report.pdf", expires, sig)
                .unwrap()
        );
    }

    #[test]
    fn test_method_mismatch_fails_verification() {
        let signer = SignedUrlGenerator::new("test_secret", "http://localhost:8080");

        let url = signer.sign_download("file.txt", 3600).unwrap();

        let parts: Vec<&str> = url.split('?').collect();
        let params: Vec<&str> = parts[1].split('&').collect();
        let expires: u64 = params[0].strip_prefix("expires=").unwrap().parse().unwrap();
        let sig = params[1].strip_prefix("sig=").unwrap();

        // Signed as GET (download), verifying as PUT should fail
        assert!(!signer.verify("PUT", "file.txt", expires, sig).unwrap());
    }

    #[test]
    fn test_path_mismatch_fails_verification() {
        let signer = SignedUrlGenerator::new("test_secret", "http://localhost:8080");

        let url = signer.sign_download("original.txt", 3600).unwrap();

        let parts: Vec<&str> = url.split('?').collect();
        let params: Vec<&str> = parts[1].split('&').collect();
        let expires: u64 = params[0].strip_prefix("expires=").unwrap().parse().unwrap();
        let sig = params[1].strip_prefix("sig=").unwrap();

        // Different path should fail
        assert!(!signer.verify("GET", "tampered.txt", expires, sig).unwrap());
    }

    #[test]
    fn test_base_url_trailing_slash_normalized() {
        let gen1 = SignedUrlGenerator::new("s", "http://example.com/");
        let gen2 = SignedUrlGenerator::new("s", "http://example.com");

        let url1 = gen1.sign_download("f.txt", 3600).unwrap();
        let url2 = gen2.sign_download("f.txt", 3600).unwrap();

        // Both should produce the same base URL prefix (no double slash)
        assert!(url1.starts_with("http://example.com/storage/"));
        assert!(url2.starts_with("http://example.com/storage/"));
    }

    #[test]
    fn test_compute_signature_deterministic() {
        let signer = SignedUrlGenerator::new("deterministic", "http://localhost");
        let sig1 = signer.compute_signature("same:message:123").unwrap();
        let sig2 = signer.compute_signature("same:message:123").unwrap();
        assert_eq!(sig1, sig2);

        let sig3 = signer.compute_signature("different:message:456").unwrap();
        assert_ne!(sig1, sig3);
    }

    #[test]
    fn test_expired_url_returns_auth_error() {
        let signer = SignedUrlGenerator::new("test", "http://localhost");
        let expired = 0u64; // epoch = always expired
        let result = signer.verify("GET", "file.txt", expired, "any");
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("expired"),
            "Error should mention expiration: {err_msg}"
        );
    }

    #[test]
    fn test_empty_secret_still_produces_valid_hmac() {
        let signer = SignedUrlGenerator::new("", "http://localhost");
        let url = signer.sign_download("test.txt", 3600);
        assert!(url.is_ok());
    }
}
