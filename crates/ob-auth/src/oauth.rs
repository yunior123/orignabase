use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};

/// User info extracted from an OAuth provider after token verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthUserInfo {
    /// Provider-specific user ID (e.g., Google sub, Apple sub)
    pub provider_id: String,
    /// OAuth provider name
    pub provider: OAuthProvider,
    /// User email (if available)
    pub email: Option<String>,
    /// Display name (if available)
    pub display_name: Option<String>,
    /// Profile picture URL (if available)
    pub picture: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OAuthProvider {
    Google,
    Apple,
    Oidc,
}

impl std::fmt::Display for OAuthProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OAuthProvider::Google => write!(f, "google"),
            OAuthProvider::Apple => write!(f, "apple"),
            OAuthProvider::Oidc => write!(f, "oidc"),
        }
    }
}

// ── Google OAuth ──

#[derive(Deserialize)]
struct GoogleTokenInfo {
    sub: String,
    email: Option<String>,
    name: Option<String>,
    picture: Option<String>,
    aud: String,
}

/// Verify a Google ID token by calling Google's tokeninfo endpoint.
/// In production, you'd verify the JWT signature locally using Google's public keys.
/// This approach is simpler and works for server-side verification.
pub async fn verify_google_id_token(
    id_token: &str,
    expected_client_id: &str,
) -> Result<OAuthUserInfo> {
    let url = format!(
        "https://oauth2.googleapis.com/tokeninfo?id_token={}",
        id_token
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| Error::Auth(format!("Google token verification failed: {e}")))?;

    if !resp.status().is_success() {
        return Err(Error::Auth("Invalid Google ID token".into()));
    }

    let info: GoogleTokenInfo = resp
        .json()
        .await
        .map_err(|e| Error::Auth(format!("Failed to parse Google response: {e}")))?;

    // Verify audience matches our client ID
    if info.aud != expected_client_id {
        return Err(Error::Auth(
            "Google token audience does not match client ID".into(),
        ));
    }

    Ok(OAuthUserInfo {
        provider_id: info.sub,
        provider: OAuthProvider::Google,
        email: info.email,
        display_name: info.name,
        picture: info.picture,
    })
}

// ── Apple Sign In ──

#[derive(Deserialize)]
struct AppleTokenResponse {
    id_token: String,
}

#[derive(Deserialize)]
struct AppleIdTokenClaims {
    sub: String,
    email: Option<String>,
    aud: String,
}

/// Verify an Apple authorization code by exchanging it for tokens.
/// Apple requires server-side code exchange — the client sends the authorization code.
pub async fn verify_apple_auth_code(
    authorization_code: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<OAuthUserInfo> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://appleid.apple.com/auth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", authorization_code),
            ("client_id", client_id),
            ("client_secret", client_secret),
        ])
        .send()
        .await
        .map_err(|e| Error::Auth(format!("Apple token exchange failed: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Auth(format!("Apple auth failed: {body}")));
    }

    let token_resp: AppleTokenResponse = resp
        .json()
        .await
        .map_err(|e| Error::Auth(format!("Failed to parse Apple response: {e}")))?;

    // Decode the id_token (without verification — Apple's endpoint already validated it)
    // In production, verify the JWT signature using Apple's public keys from
    // https://appleid.apple.com/auth/keys
    let parts: Vec<&str> = token_resp.id_token.split('.').collect();
    if parts.len() != 3 {
        return Err(Error::Auth("Invalid Apple ID token format".into()));
    }

    let payload = base64_decode_url_safe(parts[1])?;
    let claims: AppleIdTokenClaims = serde_json::from_slice(&payload)
        .map_err(|e| Error::Auth(format!("Failed to decode Apple ID token: {e}")))?;

    if claims.aud != client_id {
        return Err(Error::Auth(
            "Apple token audience does not match client ID".into(),
        ));
    }

    Ok(OAuthUserInfo {
        provider_id: claims.sub,
        provider: OAuthProvider::Apple,
        email: claims.email,
        display_name: None, // Apple only gives name on first sign-in via the client
        picture: None,
    })
}

/// Generate an Apple client secret JWT.
/// Apple requires a JWT signed with the team's private key as the client_secret.
pub fn generate_apple_client_secret(
    team_id: &str,
    key_id: &str,
    client_id: &str,
    private_key_pem: &str,
) -> Result<String> {
    use jsonwebtoken::{Algorithm, EncodingKey, Header};

    let now = chrono::Utc::now().timestamp();
    let claims = serde_json::json!({
        "iss": team_id,
        "iat": now,
        "exp": now + 15777000, // ~6 months
        "aud": "https://appleid.apple.com",
        "sub": client_id,
    });

    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(key_id.to_string());

    let key = EncodingKey::from_ec_pem(private_key_pem.as_bytes())
        .map_err(|e| Error::Auth(format!("Invalid Apple private key: {e}")))?;

    jsonwebtoken::encode(&header, &claims, &key)
        .map_err(|e| Error::Auth(format!("Failed to generate Apple client secret: {e}")))
}

// ── Generic OIDC ──

#[derive(Deserialize)]
struct OidcDiscovery {
    userinfo_endpoint: String,
}

#[derive(Deserialize)]
struct OidcUserInfo {
    sub: String,
    email: Option<String>,
    name: Option<String>,
    picture: Option<String>,
}

/// Verify an OIDC access token by calling the provider's userinfo endpoint.
pub async fn verify_oidc_token(
    access_token: &str,
    issuer_url: &str,
) -> Result<OAuthUserInfo> {
    // Discover the userinfo endpoint
    let discovery_url = format!(
        "{}/.well-known/openid-configuration",
        issuer_url.trim_end_matches('/')
    );

    let discovery: OidcDiscovery = reqwest::get(&discovery_url)
        .await
        .map_err(|e| Error::Auth(format!("OIDC discovery failed: {e}")))?
        .json()
        .await
        .map_err(|e| Error::Auth(format!("Invalid OIDC discovery response: {e}")))?;

    // Call userinfo with the access token
    let client = reqwest::Client::new();
    let resp = client
        .get(&discovery.userinfo_endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| Error::Auth(format!("OIDC userinfo request failed: {e}")))?;

    if !resp.status().is_success() {
        return Err(Error::Auth("Invalid OIDC access token".into()));
    }

    let info: OidcUserInfo = resp
        .json()
        .await
        .map_err(|e| Error::Auth(format!("Failed to parse OIDC userinfo: {e}")))?;

    Ok(OAuthUserInfo {
        provider_id: info.sub,
        provider: OAuthProvider::Oidc,
        email: info.email,
        display_name: info.name,
        picture: info.picture,
    })
}

fn base64_decode_url_safe(input: &str) -> Result<Vec<u8>> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    URL_SAFE_NO_PAD
        .decode(input)
        .map_err(|e| Error::Auth(format!("Base64 decode failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_provider_display() {
        assert_eq!(OAuthProvider::Google.to_string(), "google");
        assert_eq!(OAuthProvider::Apple.to_string(), "apple");
        assert_eq!(OAuthProvider::Oidc.to_string(), "oidc");
    }

    #[test]
    fn test_oauth_provider_serde() {
        let google = serde_json::to_string(&OAuthProvider::Google).unwrap();
        assert_eq!(google, "\"google\"");
        let back: OAuthProvider = serde_json::from_str(&google).unwrap();
        assert_eq!(back, OAuthProvider::Google);

        let apple = serde_json::to_string(&OAuthProvider::Apple).unwrap();
        assert_eq!(apple, "\"apple\"");

        let oidc = serde_json::to_string(&OAuthProvider::Oidc).unwrap();
        assert_eq!(oidc, "\"oidc\"");
    }

    #[test]
    fn test_oauth_user_info_construction() {
        let info = OAuthUserInfo {
            provider_id: "12345".into(),
            provider: OAuthProvider::Google,
            email: Some("user@example.com".into()),
            display_name: Some("Test User".into()),
            picture: Some("https://example.com/photo.jpg".into()),
        };
        assert_eq!(info.provider_id, "12345");
        assert_eq!(info.provider, OAuthProvider::Google);
        assert_eq!(info.email.as_deref(), Some("user@example.com"));
    }

    #[test]
    fn test_oauth_user_info_serde_roundtrip() {
        let info = OAuthUserInfo {
            provider_id: "apple_sub_123".into(),
            provider: OAuthProvider::Apple,
            email: Some("user@icloud.com".into()),
            display_name: None,
            picture: None,
        };

        let json = serde_json::to_string(&info).unwrap();
        let back: OAuthUserInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider_id, "apple_sub_123");
        assert_eq!(back.provider, OAuthProvider::Apple);
        assert_eq!(back.email, Some("user@icloud.com".into()));
        assert!(back.display_name.is_none());
        assert!(back.picture.is_none());
    }

    #[test]
    fn test_oauth_user_info_minimal() {
        let info = OAuthUserInfo {
            provider_id: "oidc_sub".into(),
            provider: OAuthProvider::Oidc,
            email: None,
            display_name: None,
            picture: None,
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["provider_id"], "oidc_sub");
        assert_eq!(json["provider"], "oidc");
        assert!(json["email"].is_null());
    }

    #[test]
    fn test_base64_url_safe_decode_valid() {
        // "hello" in base64url
        let encoded = "aGVsbG8";
        let decoded = base64_decode_url_safe(encoded).unwrap();
        assert_eq!(decoded, b"hello");
    }

    #[test]
    fn test_base64_url_safe_decode_json() {
        use base64::Engine;
        // {"sub":"123","email":"a@b.com"} in base64url
        let payload = r#"{"sub":"123","email":"a@b.com"}"#;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(payload.as_bytes());
        let decoded = base64_decode_url_safe(&encoded).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(claims["sub"], "123");
        assert_eq!(claims["email"], "a@b.com");
    }

    #[test]
    fn test_base64_url_safe_decode_invalid() {
        let result = base64_decode_url_safe("!!!invalid!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_apple_client_secret_invalid_key() {
        let result = generate_apple_client_secret(
            "TEAM123",
            "KEY456",
            "com.example.app",
            "not-a-valid-pem-key",
        );
        assert!(result.is_err());
    }
}
