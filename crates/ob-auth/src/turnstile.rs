//! Cloudflare Turnstile validation module.

use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct TurnstileRequest {
    secret: String,
    response: String,
}

#[derive(Debug, Deserialize)]
pub struct TurnstileResponse {
    pub success: bool,
    #[serde(default)]
    pub error_codes: Vec<String>,
    #[serde(default)]
    pub challenge_ts: String,
    #[serde(default)]
    pub hostname: String,
}

/// Validate a Turnstile token against Cloudflare's API.
/// If OB_TEST_MODE=1, skips validation and returns success.
/// Returns error if validation fails.
pub async fn validate_turnstile_token(token: &str, secret_key: &str) -> Result<()> {
    // Skip validation in test mode
    if std::env::var("OB_TEST_MODE").unwrap_or_default() == "1" {
        return Ok(());
    }

    if token.is_empty() {
        return Err(Error::Validation(
            "Turnstile token is required".into(),
        ));
    }

    let client = reqwest::Client::new();
    let request = TurnstileRequest {
        secret: secret_key.to_string(),
        response: token.to_string(),
    };

    let response = client
        .post("https://challenges.cloudflare.com/turnstile/v0/siteverify")
        .json(&request)
        .send()
        .await
        .map_err(|e| Error::Auth(format!("Turnstile validation request failed: {e}")))?;

    let validation_response = response
        .json::<TurnstileResponse>()
        .await
        .map_err(|e| Error::Auth(format!("Turnstile response parsing failed: {e}")))?;

    if !validation_response.success {
        return Err(Error::Validation(format!(
            "Turnstile validation failed: {}",
            validation_response.error_codes.join(", ")
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_turnstile_skip_in_test_mode() {
        std::env::set_var("OB_TEST_MODE", "1");
        let result = validate_turnstile_token("test_token", "test_secret").await;
        assert!(result.is_ok());
        std::env::remove_var("OB_TEST_MODE");
    }

    #[tokio::test]
    async fn test_turnstile_rejects_empty_token() {
        let result = validate_turnstile_token("", "test_secret").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("required"));
    }
}
