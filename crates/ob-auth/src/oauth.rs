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

#[derive(Deserialize)]
struct GoogleTokenExchangeResponse {
    id_token: Option<String>,
    access_token: Option<String>,
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

/// Exchange a Google authorization code for tokens, then verify the returned ID token.
/// Uses the provided http_client instead of creating a new one.
pub async fn exchange_google_authorization_code(
    http_client: &reqwest::Client,
    authorization_code: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
) -> Result<OAuthUserInfo> {
    let resp = http_client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", authorization_code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .map_err(|e| Error::Auth(format!("Google token exchange failed: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Auth(format!("Google auth failed: {body}")));
    }

    let token_response: GoogleTokenExchangeResponse = resp
        .json()
        .await
        .map_err(|e| Error::Auth(format!("Failed to parse Google token response: {e}")))?;

    let id_token = token_response
        .id_token
        .ok_or_else(|| Error::Auth("No ID token in Google response".into()))?;

    verify_google_id_token(&id_token, client_id).await
}

// ── Apple OAuth ──

#[derive(Serialize)]
struct AppleTokenRequest {
    grant_type: String,
    code: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
}

#[derive(Deserialize)]
struct AppleTokenInfo {
    sub: String,
    email: Option<String>,
}

#[derive(Deserialize)]
struct AppleTokenResponse {
    id_token: Option<String>,
    access_token: Option<String>,
}

/// Exchange an Apple authorization code for tokens, then extract user info.
/// Uses the provided http_client instead of creating a new one.
pub async fn exchange_apple_authorization_code(
    http_client: &reqwest::Client,
    authorization_code: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
) -> Result<OAuthUserInfo> {
    let resp = http_client
        .post("https://appleid.apple.com/auth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", authorization_code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .map_err(|e| Error::Auth(format!("Apple token exchange failed: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Auth(format!("Apple auth failed: {body}")));
    }

    let token_response: AppleTokenResponse = resp
        .json()
        .await
        .map_err(|e| Error::Auth(format!("Failed to parse Apple token response: {e}")))?;

    let id_token = token_response
        .id_token
        .ok_or_else(|| Error::Auth("No ID token in Apple response".into()))?;

    // Decode JWT claims (Apple tokens can be verified without signature check in dev)
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return Err(Error::Auth("Invalid Apple ID token format".into()));
    }

    let payload = parts[1];
    let decoded = base64_decode(payload)
        .map_err(|_| Error::Auth("Failed to decode Apple token payload".into()))?;
    let claims: AppleTokenInfo = serde_json::from_slice(&decoded)
        .map_err(|_| Error::Auth("Failed to parse Apple token claims".into()))?;

    Ok(OAuthUserInfo {
        provider_id: claims.sub,
        provider: OAuthProvider::Apple,
        email: claims.email,
        display_name: None,
        picture: None,
    })
}

// ── OIDC (Generic OpenID Connect) ──

#[derive(Deserialize)]
struct OidcTokenInfo {
    sub: String,
    email: Option<String>,
    name: Option<String>,
    picture: Option<String>,
}

#[derive(Deserialize)]
struct OidcTokenResponse {
    id_token: Option<String>,
    access_token: Option<String>,
}

/// Exchange an OIDC authorization code for tokens, then extract user info.
/// Uses the provided http_client instead of creating a new one.
pub async fn exchange_oidc_authorization_code(
    http_client: &reqwest::Client,
    token_endpoint: &str,
    authorization_code: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
) -> Result<OAuthUserInfo> {
    let resp = http_client
        .post(token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", authorization_code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .map_err(|e| Error::Auth(format!("OIDC token exchange failed: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Auth(format!("OIDC auth failed: {body}")));
    }

    let token_response: OidcTokenResponse = resp
        .json()
        .await
        .map_err(|e| Error::Auth(format!("Failed to parse OIDC token response: {e}")))?;

    let id_token = token_response
        .id_token
        .ok_or_else(|| Error::Auth("No ID token in OIDC response".into()))?;

    // Decode JWT claims
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return Err(Error::Auth("Invalid OIDC ID token format".into()));
    }

    let payload = parts[1];
    let decoded = base64_decode(payload)
        .map_err(|_| Error::Auth("Failed to decode OIDC token payload".into()))?;
    let claims: OidcTokenInfo = serde_json::from_slice(&decoded)
        .map_err(|_| Error::Auth("Failed to parse OIDC token claims".into()))?;

    Ok(OAuthUserInfo {
        provider_id: claims.sub,
        provider: OAuthProvider::Oidc,
        email: claims.email,
        display_name: claims.name,
        picture: claims.picture,
    })
}

// ── Utilities ──

/// Base64 URL decode with padding
fn base64_decode(input: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::prelude::*;
    let mut input = input.to_string();
    // Add padding
    while input.len() % 4 != 0 {
        input.push('=');
    }
    // Replace URL-safe characters
    let input = input.replace('-', "+").replace('_', "/");
    BASE64_STANDARD.decode(&input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_user_info_serialization() {
        let user_info = OAuthUserInfo {
            provider_id: "sub_123".to_string(),
            provider: OAuthProvider::Google,
            email: Some("user@example.com".to_string()),
            display_name: Some("John Doe".to_string()),
            picture: Some("https://example.com/pic.jpg".to_string()),
        };

        let json = serde_json::to_string(&user_info).unwrap();
        let decoded: OAuthUserInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.provider_id, "sub_123");
        assert_eq!(decoded.provider, OAuthProvider::Google);
        assert_eq!(decoded.email, Some("user@example.com".to_string()));
    }

    #[test]
    fn test_oauth_provider_display() {
        assert_eq!(OAuthProvider::Google.to_string(), "google");
        assert_eq!(OAuthProvider::Apple.to_string(), "apple");
        assert_eq!(OAuthProvider::Oidc.to_string(), "oidc");
    }

    #[test]
    fn test_base64_decode() {
        let encoded = "eyJzdWIiOiIxMjM0NTY3ODkwIn0"; // {"sub":"1234567890"}
        let decoded = base64_decode(encoded).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(claims["sub"], "1234567890");
    }
}
