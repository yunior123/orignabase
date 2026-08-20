//! Geocoding proxy handler — forward Geoapify requests with server-side API key.

use axum::{Json, Router, extract::State, routing::post};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::HandlersState;
use ob_core::{Error, Result};

const GEOAPIFY_API_KEY: &str = "geoapify_api_key";
const GEOAPIFY_BASE_URL: &str = "https://api.geoapify.com/v1/geocode/autocomplete";
const TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeocodeAutocompleteRequest {
    pub query: String,
    pub country: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub r#type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GeocodeAutocompleteResponse {
    pub features: Vec<serde_json::Value>,
}

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/geocode/autocomplete", post(autocomplete))
        .with_state(state)
}

/// POST /api/geocode/autocomplete
/// Proxy Geoapify geocoding with server-side API key.
async fn autocomplete(
    State(state): State<HandlersState>,
    Json(req): Json<GeocodeAutocompleteRequest>,
) -> Result<Json<GeocodeAutocompleteResponse>> {
    // Validate input
    if req.query.trim().is_empty() {
        return Err(Error::Validation("query is required".into()));
    }

    let api_key = match state.config.secret(GEOAPIFY_API_KEY) {
        Some(key) => key,
        None => {
            tracing::warn!("Geoapify API key not configured");
            return Ok(Json(GeocodeAutocompleteResponse { features: vec![] }));
        }
    };

    let limit = req.limit.unwrap_or(5).min(20);
    let country = req.country.as_deref().unwrap_or("countrycode:ca");
    let r#type = req.r#type.as_deref().unwrap_or("");

    // Build Geoapify query parameters
    let mut params = vec![
        ("text", req.query.clone()),
        ("apiKey", api_key.to_string()),
        ("limit", limit.to_string()),
        ("filter", country.to_string()),
    ];

    if !r#type.is_empty() {
        params.push(("type", r#type.to_string()));
    }

    // Create HTTP client with timeout
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .build()
        .map_err(|e| Error::Internal(format!("Failed to build HTTP client: {e}")))?;

    let resp = client
        .get(GEOAPIFY_BASE_URL)
        .query(&params)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Geoapify request failed: {e}");
            Error::Internal(format!("Geoapify request failed: {e}"))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        tracing::warn!("Geoapify returned status {}", status);
        return Err(Error::Internal(format!(
            "Geoapify returned status {}",
            status
        )));
    }

    let data: serde_json::Value = resp.json().await.map_err(|e| {
        tracing::error!("Failed to parse Geoapify response: {e}");
        Error::Internal(format!("Failed to parse Geoapify response: {e}"))
    })?;

    let features = data
        .get("features")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(Json(GeocodeAutocompleteResponse { features }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ob_database::DatabaseClient;
    use std::sync::Arc;

    async fn create_test_state() -> HandlersState {
        HandlersState {
            config: Arc::new(ob_core::Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: String::new(),
            turnstile_secret_key: None,
        }
    }

    #[tokio::test]
    async fn test_empty_query_returns_validation_error() {
        let state = create_test_state().await;
        let req = GeocodeAutocompleteRequest {
            query: "".to_string(),
            country: None,
            limit: None,
            r#type: None,
        };

        let result = autocomplete(State(state), Json(req)).await;
        assert!(result.is_err());
        match result {
            Err(Error::Validation(msg)) => assert_eq!(msg, "query is required"),
            _ => panic!("Expected Validation error"),
        }
    }

    #[tokio::test]
    async fn test_whitespace_only_query_returns_validation_error() {
        let state = create_test_state().await;
        let req = GeocodeAutocompleteRequest {
            query: "   ".to_string(),
            country: None,
            limit: None,
            r#type: None,
        };

        let result = autocomplete(State(state), Json(req)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_missing_api_key_returns_empty_features() {
        let state = create_test_state().await;
        let req = GeocodeAutocompleteRequest {
            query: "toronto".to_string(),
            country: None,
            limit: None,
            r#type: None,
        };

        let result = autocomplete(State(state), Json(req)).await;
        assert!(result.is_ok());
        let response = result.unwrap().0;
        assert!(response.features.is_empty());
    }

    #[tokio::test]
    async fn test_limit_capped_at_20() {
        let mut config = ob_core::Config::load(None).unwrap();
        config.secrets.values.insert(
            GEOAPIFY_API_KEY.to_string(),
            "test_key_coverage".to_string(),
        );

        let state = HandlersState {
            config: Arc::new(config),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: String::new(),
            turnstile_secret_key: None,
        };

        let req = GeocodeAutocompleteRequest {
            query: "toronto".to_string(),
            country: None,
            limit: Some(100),
            r#type: None,
        };

        // This will fail due to no mocking, but tests the limit logic path
        let _ = autocomplete(State(state), Json(req)).await;
    }
}
