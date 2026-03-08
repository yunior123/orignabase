use chrono::Utc;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
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
}

/// Issue a JWT access token.
pub fn issue_access_token(
    user_id: &str,
    roles: &[String],
    secret: &str,
    ttl_secs: u64,
) -> Result<String> {
    let now = Utc::now().timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        iat: now,
        exp: now + ttl_secs as i64,
        roles: roles.to_vec(),
        typ: "access".to_string(),
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| Error::Auth(format!("Token creation failed: {e}")))
}

/// Issue a JWT refresh token.
pub fn issue_refresh_token(user_id: &str, secret: &str, ttl_secs: u64) -> Result<String> {
    let now = Utc::now().timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        iat: now,
        exp: now + ttl_secs as i64,
        roles: vec![],
        typ: "refresh".to_string(),
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| Error::Auth(format!("Refresh token creation failed: {e}")))
}

/// Verify and decode a JWT token.
pub fn verify_token(token: &str, secret: &str) -> Result<Claims> {
    let validation = Validation::default();

    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|e| Error::Auth(format!("Token verification failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_issue_and_verify_access_token() {
        let secret = "test_secret_key_12345";
        let roles = vec!["user".to_string()];
        let token = issue_access_token("user123", &roles, secret, 3600).unwrap();
        let claims = verify_token(&token, secret).unwrap();

        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.typ, "access");
        assert_eq!(claims.roles, vec!["user"]);
    }

    #[test]
    fn test_issue_and_verify_refresh_token() {
        let secret = "test_secret_key_12345";
        let token = issue_refresh_token("user123", secret, 604800).unwrap();
        let claims = verify_token(&token, secret).unwrap();

        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.typ, "refresh");
    }

    #[test]
    fn test_wrong_secret_fails() {
        let token = issue_access_token("user123", &[], "secret1", 3600).unwrap();
        let result = verify_token(&token, "wrong_secret");
        assert!(result.is_err());
    }

    #[test]
    fn test_expired_token_fails() {
        let secret = "test_secret";
        // Manually create an already-expired token
        let now = chrono::Utc::now().timestamp();
        let claims = Claims {
            sub: "user123".to_string(),
            iat: now - 7200,
            exp: now - 3600, // expired 1 hour ago
            roles: vec![],
            typ: "access".to_string(),
        };
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();
        let result = verify_token(&token, secret);
        assert!(result.is_err());
    }
}
