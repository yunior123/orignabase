use crate::{fingerprint_public_key, KeyRotationManager};
use chrono::Utc;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID)
    pub sub: String,
    /// Issued at (Unix timestamp)
    pub iat: i64,
    /// Expiration (Unix timestamp)
    pub exp: i64,
    /// User roles
    #[serde(default)]
    pub roles: Vec<String>,
    /// Token type: "access" or "refresh"
    pub typ: String,
    /// Whether the user's email has been verified
    #[serde(default)]
    pub email_verified: bool,
    /// Whether MFA is required (login flow)
    #[serde(default)]
    pub mfa_required: bool,
    /// Custom claims set by admin (replaces Firebase custom claims).
    /// E.g. `{"role": "seller", "plan": "pro", "store_id": "abc"}`
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub custom_claims: serde_json::Value,
}

/// JWT signing strategy: RS256 (asymmetric) or HS256 (symmetric fallback).
/// Supports key rotation with fallback verification.
#[derive(Clone)]
pub enum JwtKeys {
    /// RS256 with RSA key pair (recommended for production)
    Rsa {
        // Current signing key
        encoding: EncodingKey,
        // Current verification key
        decoding: DecodingKey,
        // Previous keys for verification fallback (up to 2 old keys)
        previous_decoding: Vec<DecodingKey>,
    },
    /// HS256 with shared secret (dev/simple deployments)
    Hmac { secret: String },
}

impl JwtKeys {
    /// Create RS256 keys from PEM-encoded RSA private and public keys.
    pub fn from_rsa_pem(private_pem: &[u8], public_pem: &[u8]) -> Result<Self> {
        let encoding = EncodingKey::from_rsa_pem(private_pem)
            .map_err(|e| Error::Auth(format!("Invalid RSA private key: {e}")))?;
        let decoding = DecodingKey::from_rsa_pem(public_pem)
            .map_err(|e| Error::Auth(format!("Invalid RSA public key: {e}")))?;
        Ok(Self::Rsa {
            encoding,
            decoding,
            previous_decoding: Vec::new(),
        })
    }

    /// Create RS256 keys with rotation support.
    /// Requires current private/public keys and previous public keys.
    pub fn from_rsa_pem_with_rotation(
        private_pem: &[u8],
        public_pem: &[u8],
        previous_public_pems: Vec<Vec<u8>>,
    ) -> Result<Self> {
        let encoding = EncodingKey::from_rsa_pem(private_pem)
            .map_err(|e| Error::Auth(format!("Invalid RSA private key: {e}")))?;
        let decoding = DecodingKey::from_rsa_pem(public_pem)
            .map_err(|e| Error::Auth(format!("Invalid RSA public key: {e}")))?;

        let mut previous_decoding = Vec::new();
        for prev_pem in previous_public_pems {
            let prev_key = DecodingKey::from_rsa_pem(&prev_pem)
                .map_err(|e| Error::Auth(format!("Invalid RSA previous public key: {e}")))?;
            previous_decoding.push(prev_key);
        }

        Ok(Self::Rsa {
            encoding,
            decoding,
            previous_decoding,
        })
    }

    /// Create HS256 keys from a shared secret.
    pub fn from_secret(secret: &str) -> Self {
        Self::Hmac {
            secret: secret.to_string(),
        }
    }

    fn header(&self) -> Header {
        match self {
            Self::Rsa { .. } => Header::new(Algorithm::RS256),
            Self::Hmac { .. } => Header::default(), // HS256
        }
    }

    fn encoding_key(&self) -> EncodingKey {
        match self {
            Self::Rsa { encoding, .. } => encoding.clone(),
            Self::Hmac { secret } => EncodingKey::from_secret(secret.as_bytes()),
        }
    }

    fn validation(&self) -> Validation {
        match self {
            Self::Rsa { .. } => Validation::new(Algorithm::RS256),
            Self::Hmac { .. } => Validation::default(), // HS256
        }
    }

    fn decoding_key(&self) -> DecodingKey {
        match self {
            Self::Rsa { decoding, .. } => decoding.clone(),
            Self::Hmac { secret } => DecodingKey::from_secret(secret.as_bytes()),
        }
    }

    fn previous_decoding_keys(&self) -> Vec<DecodingKey> {
        match self {
            Self::Rsa {
                previous_decoding, ..
            } => previous_decoding.clone(),
            Self::Hmac { .. } => Vec::new(),
        }
    }
}

/// Issue a JWT access token.
pub fn issue_access_token(
    user_id: &str,
    roles: &[String],
    keys: &JwtKeys,
    ttl_secs: u64,
    email_verified: bool,
) -> Result<String> {
    issue_access_token_with_claims(
        user_id,
        roles,
        keys,
        ttl_secs,
        email_verified,
        serde_json::Value::Null,
    )
}

/// Issue a JWT access token with custom claims (set by admin via /admin/users/:id/claims).
pub fn issue_access_token_with_claims(
    user_id: &str,
    roles: &[String],
    keys: &JwtKeys,
    ttl_secs: u64,
    email_verified: bool,
    custom_claims: serde_json::Value,
) -> Result<String> {
    let now = Utc::now().timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        iat: now,
        exp: now + ttl_secs as i64,
        roles: roles.to_vec(),
        typ: "access".to_string(),
        email_verified,
        mfa_required: false,
        custom_claims,
    };

    encode(&keys.header(), &claims, &keys.encoding_key())
        .map_err(|e| Error::Auth(format!("Token creation failed: {e}")))
}

/// Issue a JWT refresh token.
pub fn issue_refresh_token(user_id: &str, keys: &JwtKeys, ttl_secs: u64) -> Result<String> {
    let now = Utc::now().timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        iat: now,
        exp: now + ttl_secs as i64,
        roles: vec![],
        typ: "refresh".to_string(),
        email_verified: false,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    encode(&keys.header(), &claims, &keys.encoding_key())
        .map_err(|e| Error::Auth(format!("Refresh token creation failed: {e}")))
}

/// Issue a single-purpose email verification token (24 hours).
pub fn issue_verification_token(user_id: &str, keys: &JwtKeys) -> Result<String> {
    let now = Utc::now().timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        iat: now,
        exp: now + 86400, // 24 hours
        roles: vec![],
        typ: "email_verify".to_string(),
        email_verified: false,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    encode(&keys.header(), &claims, &keys.encoding_key())
        .map_err(|e| Error::Auth(format!("Verification token creation failed: {e}")))
}

/// Issue a single-purpose password reset token (1 hour).
pub fn issue_reset_token(user_id: &str, keys: &JwtKeys) -> Result<String> {
    let now = Utc::now().timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        iat: now,
        exp: now + 3600, // 1 hour
        roles: vec![],
        typ: "password_reset".to_string(),
        email_verified: false,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    encode(&keys.header(), &claims, &keys.encoding_key())
        .map_err(|e| Error::Auth(format!("Reset token creation failed: {e}")))
}

/// Issue a single-purpose magic link token (15 minutes).
pub fn issue_magic_link_token(user_id: &str, keys: &JwtKeys) -> Result<String> {
    let now = Utc::now().timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        iat: now,
        exp: now + 900, // 15 minutes
        roles: vec![],
        typ: "magic_link".to_string(),
        email_verified: false,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    encode(&keys.header(), &claims, &keys.encoding_key())
        .map_err(|e| Error::Auth(format!("Magic link token creation failed: {e}")))
}

/// Issue a short-lived MFA challenge token (5 minutes).
/// This token indicates the user has passed password auth but still needs TOTP.
pub fn issue_challenge_token(user_id: &str, keys: &JwtKeys) -> Result<String> {
    let now = Utc::now().timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        iat: now,
        exp: now + 300, // 5 minutes
        roles: vec![],
        typ: "mfa_challenge".to_string(),
        email_verified: false,
        mfa_required: true,
        custom_claims: serde_json::Value::Null,
    };

    encode(&keys.header(), &claims, &keys.encoding_key())
        .map_err(|e| Error::Auth(format!("Challenge token creation failed: {e}")))
}

/// Verify and decode a JWT token.
/// For RS256 keys: tries current key first, falls back to previous keys.
pub fn verify_token(token: &str, keys: &JwtKeys) -> Result<Claims> {
    let validation = keys.validation();

    // Try current decoding key first
    if let Ok(data) = decode::<Claims>(token, &keys.decoding_key(), &validation) {
        return Ok(data.claims);
    }

    // Fall back to previous keys (only for RS256)
    for prev_key in keys.previous_decoding_keys() {
        if let Ok(data) = decode::<Claims>(token, &prev_key, &validation) {
            return Ok(data.claims);
        }
    }

    Err(Error::Auth("Token verification failed".into()))
}

/// Auto-generate an RSA key pair and write to the given directory.
/// Returns (private_key_pem, public_key_pem) as byte vectors.
pub fn generate_rsa_keys(keys_dir: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    use std::process::Command;

    std::fs::create_dir_all(keys_dir)
        .map_err(|e| Error::Config(format!("Failed to create keys directory: {e}")))?;

    let private_path = keys_dir.join("jwt_private.pem");
    let public_path = keys_dir.join("jwt_public.pem");

    // Generate RSA private key
    let status = Command::new("openssl")
        .args(["genpkey", "-algorithm", "RSA", "-out"])
        .arg(&private_path)
        .args(["-pkeyopt", "rsa_keygen_bits:2048"])
        .output()
        .map_err(|e| Error::Config(format!("Failed to run openssl: {e}")))?;

    if !status.status.success() {
        return Err(Error::Config(format!(
            "openssl genpkey failed: {}",
            String::from_utf8_lossy(&status.stderr)
        )));
    }

    // Extract public key
    let status = Command::new("openssl")
        .args(["rsa", "-in"])
        .arg(&private_path)
        .args(["-pubout", "-out"])
        .arg(&public_path)
        .output()
        .map_err(|e| Error::Config(format!("Failed to run openssl: {e}")))?;

    if !status.status.success() {
        return Err(Error::Config(format!(
            "openssl rsa failed: {}",
            String::from_utf8_lossy(&status.stderr)
        )));
    }

    let private_pem = std::fs::read(&private_path)
        .map_err(|e| Error::Config(format!("Failed to read private key: {e}")))?;
    let public_pem = std::fs::read(&public_path)
        .map_err(|e| Error::Config(format!("Failed to read public key: {e}")))?;

    Ok((private_pem, public_pem))
}

/// Rotate JWT keys: generate new key pair, archive old one, update metadata.
/// Returns the fingerprint of the new key.
pub fn rotate_keys(keys_dir: &Path) -> Result<String> {
    use std::fs;

    std::fs::create_dir_all(keys_dir)
        .map_err(|e| Error::Config(format!("Failed to create keys directory: {e}")))?;

    let private_path = keys_dir.join("jwt_private.pem");
    let public_path = keys_dir.join("jwt_public.pem");
    let rotation_metadata_path = keys_dir.join("key_rotation.json");

    // Load current rotation metadata if exists
    let mut rotation_mgr = if rotation_metadata_path.exists() {
        KeyRotationManager::load_from_file(&rotation_metadata_path)?
    } else {
        // Initialize with current key
        let current_public_pem = fs::read(&public_path)
            .map_err(|e| Error::Config(format!("Failed to read current public key: {e}")))?;
        let current_fp = fingerprint_public_key(&current_public_pem)?;
        KeyRotationManager::new(current_fp)
    };

    // Archive current keys with timestamp
    let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
    let archived_private = keys_dir.join(format!("jwt_private_{}.pem.bak", timestamp));
    let archived_public = keys_dir.join(format!("jwt_public_{}.pem.bak", timestamp));

    if private_path.exists() && public_path.exists() {
        fs::copy(&private_path, &archived_private)
            .map_err(|e| Error::Config(format!("Failed to archive private key: {e}")))?;
        fs::copy(&public_path, &archived_public)
            .map_err(|e| Error::Config(format!("Failed to archive public key: {e}")))?;
    }

    // Generate new RSA key pair
    let (new_private_pem, new_public_pem) = generate_rsa_keys(keys_dir)?;

    // Update rotation metadata
    let new_fp = fingerprint_public_key(&new_public_pem)?;
    rotation_mgr.rotate(new_fp.clone());
    rotation_mgr.save_to_file(&rotation_metadata_path)?;

    // Cleanup: keep only last 4 backups
    cleanup_old_backups(keys_dir, 4)?;

    Ok(new_fp)
}

/// Remove old backup keys, keeping only the last N.
fn cleanup_old_backups(keys_dir: &Path, keep_count: usize) -> Result<()> {
    use std::fs;

    let mut backups: Vec<_> = fs::read_dir(keys_dir)
        .map_err(|e| Error::Config(format!("Failed to read keys directory: {e}")))?
        .filter_map(|entry| {
            entry.ok().and_then(|e| {
                let path = e.path();
                if path.extension().map_or(false, |ext| ext == "bak") {
                    e.metadata()
                        .ok()
                        .and_then(|meta| meta.modified().ok().map(|modified| (path, modified)))
                } else {
                    None
                }
            })
        })
        .collect();

    // Sort by modification time (newest first)
    backups.sort_by(|a, b| b.1.cmp(&a.1));

    // Remove old backups beyond keep_count
    for (path, _) in backups.iter().skip(keep_count) {
        fs::remove_file(path)
            .map_err(|e| Error::Config(format!("Failed to delete backup: {e}")))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_keys() -> JwtKeys {
        JwtKeys::from_secret("test_secret_key_12345")
    }

    #[test]
    fn test_issue_and_verify_access_token() {
        let keys = test_keys();
        let roles = vec!["user".to_string()];
        let token = issue_access_token("user123", &roles, &keys, 3600, true).unwrap();
        let claims = verify_token(&token, &keys).unwrap();

        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.typ, "access");
        assert_eq!(claims.roles, vec!["user"]);
        assert!(claims.email_verified);
    }

    #[test]
    fn test_issue_and_verify_refresh_token() {
        let keys = test_keys();
        let token = issue_refresh_token("user123", &keys, 604800).unwrap();
        let claims = verify_token(&token, &keys).unwrap();

        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.typ, "refresh");
    }

    #[test]
    fn test_wrong_secret_fails() {
        let keys1 = JwtKeys::from_secret("secret1");
        let keys2 = JwtKeys::from_secret("wrong_secret");
        let token = issue_access_token("user123", &[], &keys1, 3600, false).unwrap();
        let result = verify_token(&token, &keys2);
        assert!(result.is_err());
    }

    #[test]
    fn test_expired_token_fails() {
        let keys = JwtKeys::from_secret("test_secret");
        let now = chrono::Utc::now().timestamp();
        let claims = Claims {
            sub: "user123".to_string(),
            iat: now - 7200,
            exp: now - 3600,
            roles: vec![],
            typ: "access".to_string(),
            email_verified: false,
            mfa_required: false,
            custom_claims: serde_json::Value::Null,
        };
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(b"test_secret"),
        )
        .unwrap();
        let result = verify_token(&token, &keys);
        assert!(result.is_err());
    }

    #[test]
    fn test_jwt_keys_from_secret() {
        let keys = JwtKeys::from_secret("my_secret");
        matches!(keys, JwtKeys::Hmac { .. });
        let token = issue_access_token("u1", &[], &keys, 60, false).unwrap();
        let claims = verify_token(&token, &keys).unwrap();
        assert_eq!(claims.sub, "u1");
    }
}

#[test]
fn test_custom_claims_serialization() {
    let keys = test_keys();

    // Create claims with custom claims
    let custom = serde_json::json!({
        "role": "seller",
        "store_id": "store_123",
        "plan": "pro"
    });

    let token =
        issue_access_token_with_claims("user123", &[], &keys, 3600, true, false, custom.clone())
            .unwrap();

    let claims = verify_token(&token, &keys).unwrap();
    assert_eq!(claims.custom_claims, custom);
}

#[test]
fn test_custom_claims_null() {
    let keys = test_keys();
    let token = issue_access_token("user123", &[], &keys, 3600, false).unwrap();
    let claims = verify_token(&token, &keys).unwrap();
    assert_eq!(claims.custom_claims, serde_json::Value::Null);
}

#[test]
fn test_mfa_required_flag() {
    let keys = test_keys();
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: "user123".to_string(),
        iat: now,
        exp: now + 3600,
        roles: vec![],
        typ: "access".to_string(),
        email_verified: false,
        mfa_required: true,
        custom_claims: serde_json::Value::Null,
    };

    let token = jsonwebtoken::encode(&keys.header(), &claims, &keys.encoding_key()).unwrap();

    let decoded = verify_token(&token, &keys).unwrap();
    assert!(decoded.mfa_required);
    assert!(!decoded.email_verified);
}

#[test]
fn test_email_verified_flag() {
    let keys = test_keys();
    let token = issue_access_token("user123", &[], &keys, 3600, true).unwrap();
    let claims = verify_token(&token, &keys).unwrap();
    assert!(claims.email_verified);
}

#[test]
fn test_multiple_roles() {
    let keys = test_keys();
    let roles = vec![
        "user".to_string(),
        "seller".to_string(),
        "admin".to_string(),
    ];

    let token = issue_access_token("user123", &roles, &keys, 3600, false).unwrap();
    let claims = verify_token(&token, &keys).unwrap();

    assert_eq!(claims.roles.len(), 3);
    assert!(claims.roles.contains(&"seller".to_string()));
    assert!(claims.roles.contains(&"admin".to_string()));
}

#[test]
fn test_access_token_type() {
    let keys = test_keys();
    let token = issue_access_token("user123", &[], &keys, 3600, false).unwrap();
    let claims = verify_token(&token, &keys).unwrap();
    assert_eq!(claims.typ, "access");
}

#[test]
fn test_refresh_token_type() {
    let keys = test_keys();
    let token = issue_refresh_token("user123", &keys, 604800).unwrap();
    let claims = verify_token(&token, &keys).unwrap();
    assert_eq!(claims.typ, "refresh");
}

#[test]
fn test_token_with_very_long_ttl() {
    let keys = test_keys();
    let ttl_secs = 30 * 24 * 60 * 60; // 30 days
    let token = issue_access_token("user123", &[], &keys, ttl_secs, false).unwrap();
    let claims = verify_token(&token, &keys).unwrap();

    // Verify exp is roughly 30 days in future
    let now = chrono::Utc::now().timestamp();
    let diff = claims.exp - now;
    assert!(diff > ttl_secs as i64 - 10); // Allow 10 sec variance
    assert!(diff <= ttl_secs as i64 + 10);
}

#[test]
fn test_token_iat_is_recent() {
    let keys = test_keys();
    let token = issue_access_token("user123", &[], &keys, 3600, false).unwrap();
    let claims = verify_token(&token, &keys).unwrap();

    let now = chrono::Utc::now().timestamp();
    let age = now - claims.iat;
    assert!(age >= 0 && age <= 5); // Issued within last 5 seconds
}

#[test]
fn test_hs256_algorithm_hs256_keys() {
    let keys = JwtKeys::from_secret("test_secret_hmac");
    let token = issue_access_token("user123", &[], &keys, 3600, false).unwrap();
    let claims = verify_token(&token, &keys).unwrap();
    assert_eq!(claims.sub, "user123");
}

#[test]
fn test_malformed_token_missing_signature() {
    let keys = test_keys();
    let malformed = "REDACTED_SECRET.eyJzdWIiOiJ1c2VyMTIzIiwiaWF0IjoxNjk4NTAwMDAwLCJleHAiOjE2OTg1MDM2MDB9";
    let result = verify_token(malformed, &keys);
    assert!(result.is_err());
}

#[test]
fn test_malformed_token_invalid_base64() {
    let keys = test_keys();
    let malformed = "not.valid.base64!!!";
    let result = verify_token(malformed, &keys);
    assert!(result.is_err());
}

#[test]
fn test_token_empty_roles() {
    let keys = test_keys();
    let token = issue_access_token("user123", &[], &keys, 3600, false).unwrap();
    let claims = verify_token(&token, &keys).unwrap();
    assert!(claims.roles.is_empty());
}

#[test]
fn test_user_id_preserved() {
    let keys = test_keys();
    let user_ids = vec!["user:abc123", "users:xyz789", "admin:super"];

    for uid in user_ids {
        let token = issue_access_token(uid, &[], &keys, 3600, false).unwrap();
        let claims = verify_token(&token, &keys).unwrap();
        assert_eq!(claims.sub, uid);
    }
}

#[test]
fn test_expiry_edge_case_just_expired() {
    let keys = test_keys();
    let now = chrono::Utc::now().timestamp();

    // Token that expired 1 second ago
    let claims = Claims {
        sub: "user123".to_string(),
        iat: now - 3600,
        exp: now - 1,
        roles: vec![],
        typ: "access".to_string(),
        email_verified: false,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    let token = jsonwebtoken::encode(&keys.header(), &claims, &keys.encoding_key()).unwrap();

    let result = verify_token(&token, &keys);
    assert!(result.is_err()); // Should be rejected
}

#[test]
fn test_expiry_edge_case_just_valid() {
    let keys = test_keys();
    let now = chrono::Utc::now().timestamp();

    // Token expiring in 2 seconds (should still be valid now)
    let claims = Claims {
        sub: "user123".to_string(),
        iat: now,
        exp: now + 2,
        roles: vec![],
        typ: "access".to_string(),
        email_verified: false,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    let token = jsonwebtoken::encode(&keys.header(), &claims, &keys.encoding_key()).unwrap();

    let result = verify_token(&token, &keys);
    assert!(result.is_ok()); // Should be valid
}

#[test]
fn test_refresh_token_has_no_roles() {
    let keys = test_keys();
    let token = issue_refresh_token("user123", &keys, 604800).unwrap();
    let claims = verify_token(&token, &keys).unwrap();

    assert!(claims.roles.is_empty());
    assert!(!claims.email_verified);
    assert!(!claims.mfa_required);
}
