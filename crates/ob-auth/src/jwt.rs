use chrono::Utc;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};

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
#[derive(Clone)]
pub enum JwtKeys {
    /// RS256 with RSA key pair (recommended for production)
    Rsa {
        encoding: EncodingKey,
        decoding: DecodingKey,
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
        Ok(Self::Rsa { encoding, decoding })
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
pub fn verify_token(token: &str, keys: &JwtKeys) -> Result<Claims> {
    decode::<Claims>(token, &keys.decoding_key(), &keys.validation())
        .map(|data| data.claims)
        .map_err(|e| Error::Auth(format!("Token verification failed: {e}")))
}

/// Auto-generate an RSA key pair and write to the given directory.
/// Returns (private_key_pem, public_key_pem) as byte vectors.
pub fn generate_rsa_keys(keys_dir: &std::path::Path) -> Result<(Vec<u8>, Vec<u8>)> {
    use std::process::Command;

    std::fs::create_dir_all(keys_dir)
        .map_err(|e| Error::Config(format!("Failed to create keys directory: {e}")))?;

    let private_path = keys_dir.join("jwt_private.pem");
    let public_path = keys_dir.join("jwt_public.pem");

    // Generate RSA private key
    let status = Command::new("openssl")
        .args(["genrsa", "-out"])
        .arg(&private_path)
        .arg("2048")
        .output()
        .map_err(|e| Error::Config(format!("Failed to run openssl: {e}")))?;

    if !status.status.success() {
        return Err(Error::Config(format!(
            "openssl genrsa failed: {}",
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
        // Should be able to sign and verify
        let token = issue_access_token("u1", &[], &keys, 60, false).unwrap();
        let claims = verify_token(&token, &keys).unwrap();
        assert_eq!(claims.sub, "u1");
    }
}
