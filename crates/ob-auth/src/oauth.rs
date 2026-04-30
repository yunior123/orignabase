use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
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
#[allow(dead_code)]
struct GoogleTokenExchangeResponse {
    id_token: Option<String>,
    #[allow(dead_code)]
    access_token: Option<String>,
}

/// Verify a Google ID token by calling Google's tokeninfo endpoint.
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
#[allow(dead_code)]
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
#[allow(dead_code)]
struct AppleTokenResponse {
    id_token: Option<String>,
    #[allow(dead_code)]
    access_token: Option<String>,
}

/// CRITICAL FIX: Fetch JWKS from a remote URL
async fn fetch_jwks(url: &str, http_client: &reqwest::Client) -> Result<serde_json::Value> {
    let resp = http_client
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Auth(format!("Failed to fetch JWKS: {e}")))?;

    if !resp.status().is_success() {
        return Err(Error::Auth("JWKS endpoint returned error".into()));
    }

    resp.json()
        .await
        .map_err(|e| Error::Auth(format!("Failed to parse JWKS: {e}")))
}

/// CRITICAL FIX: Extract kid (key ID) from JWT header
fn get_kid_from_token(token: &str) -> Result<String> {
    let header =
        decode_header(token).map_err(|_| Error::Auth("Failed to decode JWT header".into()))?;

    header
        .kid
        .ok_or_else(|| Error::Auth("JWT missing 'kid' header".into()))
}

/// CRITICAL FIX: Find public key in JWKS by kid
fn find_public_key_in_jwks(jwks: &serde_json::Value, kid: &str) -> Result<String> {
    let keys = jwks["keys"]
        .as_array()
        .ok_or_else(|| Error::Auth("Invalid JWKS format".into()))?;

    for key_data in keys {
        if key_data["kid"].as_str() == Some(kid) {
            // Return PEM-formatted public key from x5c (certificate chain)
            return key_data["x5c"]
                .as_array()
                .and_then(|certs| certs.first())
                .and_then(|cert| cert.as_str())
                .ok_or_else(|| Error::Auth("Certificate not found in JWKS".into()))
                .map(|cert| {
                    format!(
                        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----",
                        cert
                    )
                });
        }
    }

    Err(Error::Auth("Public key not found in JWKS".into()))
}

/// CRITICAL FIX: Verify Apple ID token signature using Apple's JWKS
async fn verify_apple_id_token_with_signature(
    id_token: &str,
    http_client: &reqwest::Client,
) -> Result<AppleTokenInfo> {
    // Fetch Apple's JWKS
    let jwks = fetch_jwks("https://appleid.apple.com/auth/keys", http_client).await?;

    // Extract kid from JWT header
    let kid = get_kid_from_token(id_token)?;

    // Find matching public key in JWKS
    let cert_pem = find_public_key_in_jwks(&jwks, &kid)?;

    // Create decoding key from certificate
    let decoding_key = DecodingKey::from_ec_pem(cert_pem.as_bytes())
        .map_err(|e| Error::Auth(format!("Invalid EC certificate: {e}")))?;

    // Verify JWT signature with ES256 algorithm
    let validation = Validation::new(Algorithm::ES256);

    let token_data = decode::<AppleTokenInfo>(id_token, &decoding_key, &validation)
        .map_err(|e| Error::Auth(format!("JWT verification failed: {e}")))?;

    let claims = token_data.claims;

    // Validate required claims
    if claims.sub.is_empty() {
        return Err(Error::Auth("Empty 'sub' claim in Apple token".into()));
    }

    Ok(claims)
}

/// Exchange an Apple authorization code for tokens, then extract user info.
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

    // CRITICAL FIX: Verify JWT signature instead of base64 decode
    let claims = verify_apple_id_token_with_signature(&id_token, http_client).await?;

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
#[allow(dead_code)]
struct OidcTokenResponse {
    id_token: Option<String>,
    #[allow(dead_code)]
    access_token: Option<String>,
}

/// CRITICAL FIX: Verify OIDC ID token signature using provider's JWKS
async fn verify_oidc_id_token_with_signature(
    id_token: &str,
    jwks_uri: &str,
    http_client: &reqwest::Client,
) -> Result<OidcTokenInfo> {
    // Fetch OIDC provider's JWKS
    let jwks = fetch_jwks(jwks_uri, http_client).await?;

    // Extract kid from JWT header
    let kid = get_kid_from_token(id_token)?;

    // Find matching public key in JWKS
    let cert_pem = find_public_key_in_jwks(&jwks, &kid)?;

    // Try both RSA and EC keys depending on algorithm
    let decoding_key = DecodingKey::from_rsa_pem(cert_pem.as_bytes())
        .or_else(|_| DecodingKey::from_ec_pem(cert_pem.as_bytes()))
        .map_err(|e| Error::Auth(format!("Invalid certificate: {e}")))?;

    // Verify JWT signature (try RS256 first, then ES256)
    let validation = Validation::new(Algorithm::RS256);

    let token_data = decode::<OidcTokenInfo>(id_token, &decoding_key, &validation)
        .or_else(|_| {
            let validation_ec = Validation::new(Algorithm::ES256);
            decode::<OidcTokenInfo>(id_token, &decoding_key, &validation_ec)
        })
        .map_err(|e| Error::Auth(format!("JWT verification failed: {e}")))?;

    let claims = token_data.claims;

    // Validate required claims
    if claims.sub.is_empty() {
        return Err(Error::Auth("Empty 'sub' claim in OIDC token".into()));
    }

    Ok(claims)
}

/// Exchange an OIDC authorization code for tokens, then extract user info.
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

    // Extract JWKS URI from OIDC discovery (simplified: assume standard path)
    // In production, fetch from /.well-known/openid-configuration
    let issuer = token_endpoint
        .split("/token")
        .next()
        .ok_or_else(|| Error::Auth("Invalid token endpoint".into()))?;
    let jwks_uri = format!("{}/.well-known/openid-configuration", issuer);

    // CRITICAL FIX: Verify JWT signature instead of base64 decode
    let claims = verify_oidc_id_token_with_signature(&id_token, &jwks_uri, http_client).await?;

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
#[allow(dead_code)]
fn _base64_decode(input: &str) -> std::result::Result<Vec<u8>, base64::DecodeError> {
    use base64::prelude::*;
    let mut input = input.to_string();
    // Add padding
    while !input.len().is_multiple_of(4) {
        input.push('=');
    }
    // Replace URL-safe characters
    let input = input.replace('-', "+").replace('_', "/");
    BASE64_STANDARD.decode(&input)
}

/// Takes team_id, key_id, service_id (client_id), and private_key (p8 format).
pub fn generate_apple_client_secret(
    team_id: &str,
    _key_id: &str,
    service_id: &str,
    private_key: &str,
) -> Result<String> {
    use chrono::Utc;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize)]
    struct AppleClientSecretClaims {
        iss: String, // team_id
        iat: i64,    // issued at
        exp: i64,    // expiration (6 months)
        aud: String, // "https://appleid.apple.com"
        sub: String, // service_id (client_id)
    }

    let now = Utc::now().timestamp();
    let claims = AppleClientSecretClaims {
        iss: team_id.to_string(),
        iat: now,
        exp: now + (180 * 24 * 60 * 60), // 6 months
        aud: "https://appleid.apple.com".to_string(),
        sub: service_id.to_string(),
    };

    let encoding_key = EncodingKey::from_ec_pem(private_key.as_bytes())
        .map_err(|e| Error::Auth(format!("Invalid Apple private key: {e}")))?;

    let header = Header::new(Algorithm::ES256);
    encode(&header, &claims, &encoding_key)
        .map_err(|e| Error::Auth(format!("Failed to generate client secret: {e}")))
}

/// Verify Apple authorization code and extract user info.
/// Note: redirect_uri is hardcoded to https://orignagta.ca/auth/apple/callback for now.
pub async fn verify_apple_auth_code(
    authorization_code: &str,
    service_id: &str,
    client_secret: &str,
) -> Result<OAuthUserInfo> {
    let http_client = reqwest::Client::new();
    let redirect_uri = "https://orignagta.ca/auth/apple/callback";

    // Exchange the auth_code for an ID token
    exchange_apple_authorization_code(
        &http_client,
        authorization_code,
        service_id,
        client_secret,
        redirect_uri,
    )
    .await
}

/// Verify OIDC access token and extract user info.
pub async fn verify_oidc_token(access_token: &str, issuer_url: &str) -> Result<OAuthUserInfo> {
    let http_client = reqwest::Client::new();

    // Construct the token endpoint from issuer URL
    let token_endpoint = format!("{}/token", issuer_url);

    // For generic OIDC, we treat the access_token as an authorization code for token exchange
    // This is a simplification - in reality you'd need client_id, client_secret, and redirect_uri
    // For now, exchange using dummy values (this should be improved)
    let client_id = "origna-gta";
    let client_secret = "";
    let redirect_uri = "https://orignagta.ca/auth/oidc/callback";

    exchange_oidc_authorization_code(
        &http_client,
        &token_endpoint,
        access_token,
        client_id,
        client_secret,
        redirect_uri,
    )
    .await
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
        let decoded = _base64_decode(encoded).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(claims["sub"], "1234567890");
    }
}
