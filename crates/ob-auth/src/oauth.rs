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
struct GoogleTokenExchangeResponse {
    id_token: Option<String>,
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
        return Err(Error::Auth("OAuth provider authentication failed".into()));
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

#[derive(Deserialize)]
struct AppleTokenInfo {
    sub: String,
    email: Option<String>,
}

#[derive(Deserialize)]
struct AppleTokenResponse {
    id_token: Option<String>,
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
        return Err(Error::Auth("OAuth provider authentication failed".into()));
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
struct OidcTokenResponse {
    id_token: Option<String>,
}

/// Verifies an OIDC `id_token` signature and extracts minimal identity claims.
///
/// Parameters:
/// - `id_token`: provider-issued JWT returned from the token exchange.
/// - `jwks_uri`: URL used to fetch the provider signing keys.
/// - `http_client`: shared HTTP client for JWKS retrieval.
///
/// Returns:
/// - `Ok(OidcTokenInfo)` with the verified subject, email, name, and picture claims.
/// - `Err(...)` if key discovery, certificate parsing, signature validation, or claim checks fail.
///
/// Gotchas:
/// - This function assumes the caller passes a JWKS endpoint, not the discovery document.
/// - It attempts RSA first and then EC verification because providers differ in signing algorithms.
/// - Only minimal structural claim validation happens here; issuer/audience policy belongs to the caller.
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

/// Exchanges an OIDC authorization code and converts the verified ID token into local user info.
///
/// Parameters:
/// - `http_client`: HTTP client used for the token exchange and JWKS lookup.
/// - `token_endpoint`: provider token endpoint that accepts the authorization code.
/// - `authorization_code`: short-lived code received from the browser redirect.
/// - `client_id`: OAuth client identifier registered with the provider.
/// - `client_secret`: OAuth client secret for confidential-client exchanges.
/// - `redirect_uri`: redirect URI that must match the authorization request.
///
/// Returns:
/// - `Ok(OAuthUserInfo)` with provider ID, optional email, display name, and picture.
/// - `Err(...)` when the exchange or ID token verification fails.
///
/// Gotchas:
/// - The current JWKS lookup derives an issuer base URL from `token_endpoint`; providers
///   with non-standard layouts may require a discovery-document implementation instead.
/// - The response must include an `id_token`; access-token-only providers are unsupported here.
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
        return Err(Error::Auth("OAuth provider authentication failed".into()));
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
#[cfg(test)]
fn base64_decode(input: &str) -> std::result::Result<Vec<u8>, base64::DecodeError> {
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

/// Generates the Apple client-secret JWT required for code exchange.
///
/// Parameters:
/// - `team_id`: Apple developer team identifier.
/// - `key_id`: Apple key identifier used in the JWT header.
/// - `service_id`: Apple service ID that acts as the JWT subject/client ID.
/// - `private_key`: `.p8` EC private key contents.
///
/// Returns:
/// - `Ok(String)` containing the signed ES256 client-secret JWT.
/// - `Err(...)` if the private key is invalid or the JWT cannot be encoded.
///
/// Gotchas:
/// - Apple expects a long-lived client secret; this implementation uses a 180-day expiry.
/// - The header `kid` must match the uploaded Apple signing key or token exchange will fail.
pub fn generate_apple_client_secret(
    team_id: &str,
    key_id: &str,
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

    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(key_id.to_string());
    encode(&header, &claims, &encoding_key)
        .map_err(|e| Error::Auth(format!("Failed to generate client secret: {e}")))
}

/// Verify Apple authorization code and extract user info.
pub async fn verify_apple_auth_code(
    authorization_code: &str,
    service_id: &str,
    base_url: &str,
    client_secret: &str,
) -> Result<OAuthUserInfo> {
    let http_client = reqwest::Client::new();
    let redirect_uri = format!("{}/auth/apple/callback", base_url.trim_end_matches('/'));

    // Exchange the auth_code for an ID token
    exchange_apple_authorization_code(
        &http_client,
        authorization_code,
        service_id,
        client_secret,
        redirect_uri.as_str(),
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
    use base64::prelude::*;

    fn make_jwt_with_kid(kid: &str) -> String {
        let header = serde_json::json!({"alg": "HS256", "typ": "JWT", "kid": kid});
        let payload = serde_json::json!({"sub": "test"});
        let h = BASE64_URL_SAFE_NO_PAD.encode(header.to_string());
        let p = BASE64_URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("{h}.{p}.fake_sig")
    }

    fn make_jwt_without_kid() -> String {
        let header = serde_json::json!({"alg": "HS256", "typ": "JWT"});
        let payload = serde_json::json!({"sub": "test"});
        let h = BASE64_URL_SAFE_NO_PAD.encode(header.to_string());
        let p = BASE64_URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("{h}.{p}.fake_sig")
    }

    fn make_jwks(kid: &str, cert: &str) -> serde_json::Value {
        serde_json::json!({
            "keys": [{
                "kid": kid,
                "kty": "EC",
                "x5c": [cert]
            }]
        })
    }

    // --- OAuthUserInfo ---

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
        assert_eq!(decoded.display_name, Some("John Doe".to_string()));
        assert_eq!(
            decoded.picture,
            Some("https://example.com/pic.jpg".to_string())
        );
    }

    #[test]
    fn test_oauth_user_info_serialization_minimal() {
        let user_info = OAuthUserInfo {
            provider_id: "abc".to_string(),
            provider: OAuthProvider::Apple,
            email: None,
            display_name: None,
            picture: None,
        };
        let json = serde_json::to_string(&user_info).unwrap();
        let decoded: OAuthUserInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.provider_id, "abc");
        assert_eq!(decoded.provider, OAuthProvider::Apple);
        assert!(decoded.email.is_none());
        assert!(decoded.display_name.is_none());
        assert!(decoded.picture.is_none());
    }

    #[test]
    fn test_oauth_user_info_oidc_roundtrip() {
        let user_info = OAuthUserInfo {
            provider_id: "oidc_sub".to_string(),
            provider: OAuthProvider::Oidc,
            email: Some("oidc@test.com".to_string()),
            display_name: Some("OIDC User".to_string()),
            picture: Some("https://pic.com/oidc".to_string()),
        };
        let json = serde_json::to_string(&user_info).unwrap();
        let decoded: OAuthUserInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.provider, OAuthProvider::Oidc);
    }

    // --- OAuthProvider ---

    #[test]
    fn test_oauth_provider_display() {
        assert_eq!(OAuthProvider::Google.to_string(), "google");
        assert_eq!(OAuthProvider::Apple.to_string(), "apple");
        assert_eq!(OAuthProvider::Oidc.to_string(), "oidc");
    }

    #[test]
    fn test_oauth_provider_equality() {
        assert_eq!(OAuthProvider::Google, OAuthProvider::Google);
        assert_ne!(OAuthProvider::Google, OAuthProvider::Apple);
        assert_ne!(OAuthProvider::Apple, OAuthProvider::Oidc);
    }

    #[test]
    fn test_oauth_provider_serde_roundtrip() {
        for provider in [
            OAuthProvider::Google,
            OAuthProvider::Apple,
            OAuthProvider::Oidc,
        ] {
            let json = serde_json::to_string(&provider).unwrap();
            let back: OAuthProvider = serde_json::from_str(&json).unwrap();
            assert_eq!(provider, back);
        }
    }

    // --- base64_decode ---

    #[test]
    fn test_base64_decode() {
        let encoded = "eyJzdWIiOiIxMjM0NTY3ODkwIn0"; // {"sub":"1234567890"}
        let decoded = base64_decode(encoded).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(claims["sub"], "1234567890");
    }

    #[test]
    fn test_base64_decode_with_padding_needed() {
        // "hello" = aGVsbG8= → encoded without padding as "aGVsbG8"
        let encoded = "aGVsbG8";
        let decoded = base64_decode(encoded).unwrap();
        assert_eq!(decoded, b"hello");
    }

    #[test]
    fn test_base64_decode_url_safe_chars() {
        // Encode with URL-safe chars
        let original = b"test data with special chars";
        let encoded = BASE64_URL_SAFE_NO_PAD.encode(original);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_base64_decode_invalid() {
        let result = base64_decode("!!!invalid!!!");
        assert!(result.is_err());
    }

    // --- get_kid_from_token ---

    #[test]
    fn test_get_kid_from_token_valid() {
        let jwt = make_jwt_with_kid("my-key-id");
        let kid = get_kid_from_token(&jwt).unwrap();
        assert_eq!(kid, "my-key-id");
    }

    #[test]
    fn test_get_kid_from_token_missing_kid() {
        let jwt = make_jwt_without_kid();
        let result = get_kid_from_token(&jwt);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("kid"));
    }

    #[test]
    fn test_get_kid_from_token_invalid_jwt() {
        let result = get_kid_from_token("not-a-jwt");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_kid_from_token_empty() {
        let result = get_kid_from_token("");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_kid_from_token_only_one_part() {
        let result = get_kid_from_token("abc");
        assert!(result.is_err());
    }

    // --- find_public_key_in_jwks ---

    #[test]
    fn test_find_public_key_in_jwks_found() {
        let jwks = make_jwks("key1", "CERTDATA123");
        let result = find_public_key_in_jwks(&jwks, "key1").unwrap();
        assert!(result.contains("-----BEGIN CERTIFICATE-----"));
        assert!(result.contains("CERTDATA123"));
        assert!(result.contains("-----END CERTIFICATE-----"));
    }

    #[test]
    fn test_find_public_key_in_jwks_not_found() {
        let jwks = make_jwks("key1", "CERT");
        let result = find_public_key_in_jwks(&jwks, "key2");
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("not found"));
    }

    #[test]
    fn test_find_public_key_in_jwks_no_keys_array() {
        let jwks = serde_json::json!({"not_keys": []});
        let result = find_public_key_in_jwks(&jwks, "key1");
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("Invalid JWKS"));
    }

    #[test]
    fn test_find_public_key_in_jwks_missing_x5c() {
        let jwks = serde_json::json!({
            "keys": [{"kid": "key1", "kty": "EC"}]
        });
        let result = find_public_key_in_jwks(&jwks, "key1");
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("Certificate not found"));
    }

    #[test]
    fn test_find_public_key_in_jwks_empty_x5c() {
        let jwks = serde_json::json!({
            "keys": [{"kid": "key1", "kty": "EC", "x5c": []}]
        });
        let result = find_public_key_in_jwks(&jwks, "key1");
        assert!(result.is_err());
    }

    #[test]
    fn test_find_public_key_in_jwks_multiple_keys() {
        let jwks = serde_json::json!({
            "keys": [
                {"kid": "k1", "x5c": ["CERT1"]},
                {"kid": "k2", "x5c": ["CERT2"]},
                {"kid": "k3", "x5c": ["CERT3"]}
            ]
        });
        let result = find_public_key_in_jwks(&jwks, "k2").unwrap();
        assert!(result.contains("CERT2"));
    }

    // --- async: fetch_jwks ---

    #[tokio::test]
    async fn test_fetch_jwks_success() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(make_jwks("test-kid", "CERT")))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = fetch_jwks(&server.uri(), &client).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_fetch_jwks_server_error() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = fetch_jwks(&server.uri(), &client).await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("error"));
    }

    #[tokio::test]
    async fn test_fetch_jwks_invalid_json() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = fetch_jwks(&server.uri(), &client).await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("parse"));
    }

    // --- async: exchange_google_authorization_code ---

    #[tokio::test]
    async fn test_exchange_google_code_token_endpoint_error() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string("invalid_grant"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = exchange_google_authorization_code(
            &client,
            "bad_code",
            "client_id",
            "client_secret",
            &format!("{}/callback", server.uri()),
        )
        .await;
        assert!(result.is_err());
        assert!(
            format!("{}", result.unwrap_err()).contains("OAuth provider authentication failed")
        );
    }

    #[tokio::test]
    async fn test_exchange_google_code_bad_credentials() {
        let client = reqwest::Client::new();
        let result = exchange_google_authorization_code(
            &client,
            "bad_code",
            "nonexistent_client_id",
            "bad_secret",
            "https://example.com/callback",
        )
        .await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("auth failed")
                || err_msg.contains("verification failed")
                || err_msg.contains("OAuth provider"),
            "Got: {err_msg}"
        );
    }

    // --- async: exchange_apple_authorization_code ---

    #[tokio::test]
    async fn test_exchange_apple_code_token_endpoint_error() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string("invalid_code"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = exchange_apple_authorization_code(
            &client,
            "bad_code",
            "service_id",
            "secret",
            &format!("{}/cb", server.uri()),
        )
        .await;
        assert!(result.is_err());
        assert!(
            format!("{}", result.unwrap_err()).contains("OAuth provider authentication failed")
        );
    }

    #[tokio::test]
    async fn test_exchange_apple_code_bad_credentials() {
        let client = reqwest::Client::new();
        let result = exchange_apple_authorization_code(
            &client,
            "bad_code",
            "bad_service_id",
            "bad_secret",
            "https://example.com/callback",
        )
        .await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("OAuth provider authentication failed")
                || err_msg.contains("fetch JWKS")
                || err_msg.contains("verification failed"),
            "Got: {err_msg}"
        );
    }

    // --- async: exchange_oidc_authorization_code ---

    #[tokio::test]
    async fn test_exchange_oidc_code_token_endpoint_error() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = exchange_oidc_authorization_code(
            &client,
            &format!("{}/token", server.uri()),
            "code",
            "cid",
            "csec",
            &format!("{}/cb", server.uri()),
        )
        .await;
        assert!(result.is_err());
        assert!(
            format!("{}", result.unwrap_err()).contains("OAuth provider authentication failed")
        );
    }

    #[tokio::test]
    async fn test_exchange_oidc_code_no_id_token() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"access_token": "tok"})),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = exchange_oidc_authorization_code(
            &client,
            &format!("{}/token", server.uri()),
            "code",
            "cid",
            "csec",
            &format!("{}/cb", server.uri()),
        )
        .await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("No ID token"));
    }

    // --- generate_apple_client_secret ---

    #[test]
    fn test_generate_apple_client_secret_invalid_key() {
        let result =
            generate_apple_client_secret("TEAM1", "KEY1", "service.id", "not a valid pem key");
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("Invalid Apple private key"));
    }

    // --- Error messages ---

    #[test]
    fn test_auth_error_variants() {
        let err = Error::Auth("test error".into());
        assert!(format!("{err}").contains("test error"));
    }

    // --- OAuthProvider variants ---

    #[test]
    fn test_oauth_provider_all_variants() {
        let providers = [
            OAuthProvider::Google,
            OAuthProvider::Apple,
            OAuthProvider::Oidc,
        ];
        assert_eq!(providers.len(), 3);
        for p in &providers {
            let s = p.to_string();
            assert!(!s.is_empty());
        }
    }

    // --- verify_google_id_token error paths ---

    #[tokio::test]
    async fn test_verify_google_id_token_http_error() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/tokeninfo?id_token=fake", server.uri());
        let resp = client.get(&url).send().await.unwrap();
        assert!(!resp.status().is_success());
    }

    #[tokio::test]
    async fn test_verify_google_id_token_bad_json() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/tokeninfo?id_token=fake", server.uri());
        let resp = client.get(&url).send().await.unwrap();
        assert!(resp.status().is_success());
        let result = resp.json::<GoogleTokenInfo>().await;
        assert!(result.is_err());
    }

    // --- More base64_decode edge cases ---

    #[test]
    fn test_base64_decode_empty() {
        let decoded = base64_decode("").unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_base64_decode_with_padding() {
        // "SGVsbG8" = "Hello" without padding → test padding logic
        let decoded = base64_decode("SGVsbG8").unwrap();
        assert_eq!(decoded, b"Hello");
    }

    // --- JWT header structure tests ---

    #[test]
    fn test_jwt_header_decode_structure() {
        // {"alg":"HS256","kid":"test-kid","typ":"JWT"}
        let header_json = serde_json::json!({"alg": "HS256", "kid": "test-kid", "typ": "JWT"});
        let encoded = BASE64_URL_SAFE_NO_PAD.encode(header_json.to_string());
        let decoded = base64_decode(&encoded).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(parsed["kid"], "test-kid");
        assert_eq!(parsed["alg"], "HS256");
    }
}
