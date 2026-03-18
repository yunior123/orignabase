//! addresses/mod.rs: Address suggestions (Geoapify) and buyer address management.

use crate::HandlersState;
use crate::shared::schema::{collections, fields};
use crate::shared::validation::validate_uid;
use axum::{
    Extension, Json, Router,
    extract::State,
    routing::{delete, post, put},
};
use ob_auth::middleware::AuthContext;
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

const GEOAPIFY_SECRET_KEY: &str = "geoapify_api_key";
const COUNTRY_FILTER: &str = "countrycode:ca";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressSuggestionsRequest {
    pub query: String,
    pub limit: Option<usize>,
    pub country: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AddressSuggestionsResponse {
    pub features: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AddressPayload {
    pub label: String,
    pub street: String,
    pub city: String,
    pub province: String,
    pub postal_code: String,
    pub country: String,
    pub apartment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddBuyerAddressRequest {
    pub user_id: String,
    pub address: AddressPayload,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddBuyerAddressResponse {
    pub success: bool,
    pub address_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBuyerAddressRequest {
    pub user_id: String,
    pub address_id: String,
    pub address: AddressPayload,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteBuyerAddressRequest {
    pub user_id: String,
    pub address_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDefaultBuyerAddressRequest {
    pub user_id: String,
    pub address_id: String,
}

#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub success: bool,
}

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/suggestions", post(get_suggestions))
        .route("/buyer", post(add_buyer_address))
        .route("/buyer", put(update_buyer_address))
        .route("/buyer", delete(delete_buyer_address))
        .route("/buyer/default", post(set_default_buyer_address))
        .with_state(state)
}

/// GET /suggestions: Get address suggestions from Geoapify.
async fn get_suggestions(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<AddressSuggestionsRequest>,
) -> Result<Json<AddressSuggestionsResponse>> {
    if !auth.authenticated {
        return Err(Error::Auth("Must be authenticated".into()));
    }

    if req.query.trim().is_empty() {
        return Err(Error::Validation("query is required".into()));
    }

    let api_key = match state.config.secret(GEOAPIFY_SECRET_KEY) {
        Some(key) => key,
        None => {
            tracing::warn!("Geoapify API key not configured");
            return Ok(Json(AddressSuggestionsResponse { features: vec![] }));
        }
    };

    let base_url = std::env::var("GEOAPIFY_API_URL")
        .unwrap_or_else(|_| "https://api.geoapify.com/v1/geocode/autocomplete".to_string());

    let limit = req.limit.unwrap_or(5).min(20);
    let country = req.country.as_deref().unwrap_or(COUNTRY_FILTER);

    let resp = state
        .http_client
        .get(base_url)
        .query(&[
            ("text", &req.query),
            ("apiKey", &api_key.to_string()),
            ("limit", &limit.to_string()),
            ("filter", &country.to_string()),
        ])
        .send()
        .await
        .map_err(|e| Error::Internal(format!("Geoapify request failed: {e}")))?;

    if !resp.status().is_success() {
        return Err(Error::Internal(format!(
            "Geoapify returned status {}",
            resp.status()
        )));
    }

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| Error::Internal(format!("Failed to parse Geoapify response: {e}")))?;

    let features = data["features"].as_array().cloned().unwrap_or_default();

    Ok(Json(AddressSuggestionsResponse { features }))
}

/// POST /buyer: Add a new address for a buyer.
async fn add_buyer_address(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<AddBuyerAddressRequest>,
) -> Result<Json<AddBuyerAddressResponse>> {
    if !auth.authenticated || auth.user_id != req.user_id {
        return Err(Error::Auth("Unauthorized".into()));
    }

    validate_address_payload(&req.address)?;

    let address_id = format!("addr_{}", uuid::Uuid::new_v4().simple());
    let mut address_doc = json!(req.address);
    address_doc[fields::UID] = req.user_id.clone().into();
    address_doc["id"] = address_id.clone().into();
    address_doc[fields::CREATED_AT] = chrono::Utc::now().to_rfc3339().into();

    state
        .db
        .create_document(collections::BUYER_ADDRESSES, address_doc)
        .await?;

    Ok(Json(AddBuyerAddressResponse {
        success: true,
        address_id,
    }))
}

/// PUT /buyer: Update an existing buyer address.
async fn update_buyer_address(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<UpdateBuyerAddressRequest>,
) -> Result<Json<SuccessResponse>> {
    if !auth.authenticated || auth.user_id != req.user_id {
        return Err(Error::Auth("Unauthorized".into()));
    }

    validate_uid("addressId", &req.address_id)?;
    validate_address_payload(&req.address)?;

    let mut address_doc = json!(req.address);
    address_doc[fields::UPDATED_AT] = chrono::Utc::now().to_rfc3339().into();

    state
        .db
        .update_document(collections::BUYER_ADDRESSES, &req.address_id, address_doc)
        .await?;

    Ok(Json(SuccessResponse { success: true }))
}

/// DELETE /buyer: Delete a buyer address.
async fn delete_buyer_address(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<DeleteBuyerAddressRequest>,
) -> Result<Json<SuccessResponse>> {
    if !auth.authenticated || auth.user_id != req.user_id {
        return Err(Error::Auth("Unauthorized".into()));
    }

    validate_uid("addressId", &req.address_id)?;

    state
        .db
        .delete_document(collections::BUYER_ADDRESSES, &req.address_id)
        .await?;

    Ok(Json(SuccessResponse { success: true }))
}

/// POST /buyer/default: Set an address as default for a buyer.
async fn set_default_buyer_address(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<SetDefaultBuyerAddressRequest>,
) -> Result<Json<SuccessResponse>> {
    if !auth.authenticated || auth.user_id != req.user_id {
        return Err(Error::Auth("Unauthorized".into()));
    }

    validate_uid("addressId", &req.address_id)?;

    // Update user document to point to the default address
    state
        .db
        .update_document(
            collections::USERS,
            &req.user_id,
            json!({
                "defaultAddressId": req.address_id
            }),
        )
        .await?;

    Ok(Json(SuccessResponse { success: true }))
}

fn validate_address_payload(addr: &AddressPayload) -> Result<()> {
    if addr.label.trim().is_empty() {
        return Err(Error::Validation("label is required".into()));
    }
    if addr.street.trim().is_empty() {
        return Err(Error::Validation("street is required".into()));
    }
    if addr.city.trim().is_empty() {
        return Err(Error::Validation("city is required".into()));
    }
    if addr.province.trim().is_empty() {
        return Err(Error::Validation("province is required".into()));
    }
    if addr.country.to_uppercase() != "CA" {
        return Err(Error::Validation(
            "Only Canadian addresses are currently supported".into(),
        ));
    }
    validate_postal_code(&addr.postal_code)?;

    if let Some(apt) = &addr.apartment {
        if apt.len() > 50 {
            return Err(Error::Validation("apartment too long".into()));
        }
    }

    Ok(())
}

fn validate_postal_code(pc: &str) -> Result<()> {
    let clean = pc.replace(' ', "").to_uppercase();
    if clean.len() != 6 {
        return Err(Error::Validation(
            "Canadian postal code must be 6 characters".into(),
        ));
    }
    // Very basic check: A1A1A1
    for (i, c) in clean.chars().enumerate() {
        if i % 2 == 0 {
            if !c.is_alphabetic() {
                return Err(Error::Validation("Invalid postal code format".into()));
            }
        } else {
            if !c.is_numeric() {
                return Err(Error::Validation("Invalid postal code format".into()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use ob_auth::middleware::AuthContext;
    use std::sync::Mutex;
    use std::time::Duration;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn test_get_suggestions_success() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;
        use wiremock::matchers::{method, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let _lock = ENV_MUTEX.lock().unwrap();
        let server = MockServer::start().await;
        unsafe { std::env::set_var("GEOAPIFY_API_URL", server.uri()) };

        let mock_response = json!({
            "features": [
                {"type": "Feature", "properties": {"formatted": "123 Main St, Toronto, ON"}}
            ]
        });

        Mock::given(method("GET"))
            .and(query_param("text", "Toronto"))
            .respond_with(ResponseTemplate::new(200).set_body_json(mock_response))
            .mount(&server)
            .await;

        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert(GEOAPIFY_SECRET_KEY.to_string(), "fake_key".into());

        let state = HandlersState {
            config: Arc::new(config),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let auth = AuthContext {
            user_id: "user_1".into(),
            authenticated: true,
            ..AuthContext::anonymous()
        };

        let req = AddressSuggestionsRequest {
            query: "Toronto".into(),
            limit: Some(1),
            country: None,
        };

        let result = get_suggestions(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert_eq!(resp.features.len(), 1);
        assert_eq!(
            resp.features[0]["properties"]["formatted"],
            "123 Main St, Toronto, ON"
        );

        unsafe { std::env::remove_var("GEOAPIFY_API_URL") };
    }

    #[tokio::test]
    async fn test_get_suggestions_unauthenticated() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let auth = AuthContext::anonymous();

        let req = AddressSuggestionsRequest {
            query: "Toronto".into(),
            limit: None,
            country: None,
        };

        let result = get_suggestions(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_suggestions_no_api_key() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()), // No key
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let auth = AuthContext {
            user_id: "user_1".into(),
            authenticated: true,
            ..AuthContext::anonymous()
        };

        let req = AddressSuggestionsRequest {
            query: "Toronto".into(),
            limit: None,
            country: None,
        };

        let result = get_suggestions(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.features.is_empty());
    }

    #[tokio::test]
    async fn test_get_suggestions_api_error() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let _lock = ENV_MUTEX.lock().unwrap();
        let server = MockServer::start().await;
        unsafe { std::env::set_var("GEOAPIFY_API_URL", server.uri()) };

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert(GEOAPIFY_SECRET_KEY.to_string(), "key".into());

        let state = HandlersState {
            config: Arc::new(config),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let auth = AuthContext {
            user_id: "user_1".into(),
            authenticated: true,
            ..AuthContext::anonymous()
        };

        let req = AddressSuggestionsRequest {
            query: "fail".into(),
            limit: None,
            country: None,
        };

        let result = get_suggestions(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());

        unsafe { std::env::remove_var("GEOAPIFY_API_URL") };
    }

    #[tokio::test]
    async fn test_get_suggestions_timeout() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let _lock = ENV_MUTEX.lock().unwrap();
        let server = MockServer::start().await;
        unsafe { std::env::set_var("GEOAPIFY_API_URL", server.uri()) };

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(10)))
            .mount(&server)
            .await;

        let config = Config::load(None).unwrap();
        let mut secrets = config.secrets.clone();
        secrets
            .values
            .insert(GEOAPIFY_SECRET_KEY.to_string(), "key".into());

        let state = HandlersState {
            config: Arc::new(Config { secrets, ..config }),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let auth = AuthContext {
            user_id: "user_1".into(),
            authenticated: true,
            ..AuthContext::anonymous()
        };

        let req = AddressSuggestionsRequest {
            query: "timeout".into(),
            limit: None,
            country: None,
        };

        // To force a timeout quickly, I can use a mock client with a very short timeout.
        let state_short_timeout = HandlersState {
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_millis(10))
                .build()
                .unwrap(),
            config: state.config.clone(),
            db: state.db.clone(),
            stripe_client: None,
            stripe_base_url: state.stripe_base_url.clone(),
        };

        let result = get_suggestions(State(state_short_timeout), Extension(auth), Json(req)).await;
        assert!(result.is_err());

        unsafe { std::env::remove_var("GEOAPIFY_API_URL") };
    }

    #[tokio::test]
    async fn test_get_suggestions_empty_query() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let auth = AuthContext {
            user_id: "user_1".into(),
            authenticated: true,
            ..AuthContext::anonymous()
        };

        let req = AddressSuggestionsRequest {
            query: "   ".into(),
            limit: None,
            country: None,
        };

        let result = get_suggestions(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_country_filter_is_canada() {
        assert_eq!(COUNTRY_FILTER, "countrycode:ca");
    }

    #[tokio::test]
    async fn test_addresses_router() {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };

        let r = router(state);
        let _ = r;
    }

    #[test]
    fn test_validate_postal_code_valid() {
        assert!(validate_postal_code("M5V 2T6").is_ok());
    }

    #[test]
    fn test_validate_address_payload_valid() {
        let addr = AddressPayload {
            label: "Home".into(),
            street: "123 Main St".into(),
            city: "Toronto".into(),
            province: "ON".into(),
            postal_code: "M5V 2T6".into(),
            country: "CA".into(),
            apartment: None,
        };
        assert!(validate_address_payload(&addr).is_ok());
    }

    // --- Coverage tests for uncovered lines ---

    fn make_valid_address() -> AddressPayload {
        AddressPayload {
            label: "Home".into(),
            street: "123 Main St".into(),
            city: "Toronto".into(),
            province: "ON".into(),
            postal_code: "M5V 2T6".into(),
            country: "CA".into(),
            apartment: None,
        }
    }

    async fn setup_state() -> HandlersState {
        use ob_core::Config;
        use ob_database::DatabaseClient;
        use std::sync::Arc;

        HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        }
    }

    fn auth_for(user_id: &str) -> AuthContext {
        AuthContext {
            user_id: user_id.into(),
            authenticated: true,
            ..AuthContext::anonymous()
        }
    }

    // Lines 154-180: add_buyer_address happy path
    #[tokio::test]
    async fn test_add_buyer_address_success() {
        let state = setup_state().await;
        let auth = auth_for("user_1");
        let req = AddBuyerAddressRequest {
            user_id: "user_1".into(),
            address: make_valid_address(),
        };

        let result = add_buyer_address(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.success);
        assert!(resp.address_id.starts_with("addr_"));
    }

    // Lines 159-161: add_buyer_address unauthorized (wrong user)
    #[tokio::test]
    async fn test_add_buyer_address_unauthorized() {
        let state = setup_state().await;
        let auth = auth_for("user_2");
        let req = AddBuyerAddressRequest {
            user_id: "user_1".into(),
            address: make_valid_address(),
        };

        let result = add_buyer_address(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    // Lines 159-161: add_buyer_address unauthenticated
    #[tokio::test]
    async fn test_add_buyer_address_unauthenticated() {
        let state = setup_state().await;
        let auth = AuthContext::anonymous();
        let req = AddBuyerAddressRequest {
            user_id: "".into(),
            address: make_valid_address(),
        };

        let result = add_buyer_address(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    // Lines 163: add_buyer_address validation failure (empty label)
    #[tokio::test]
    async fn test_add_buyer_address_invalid_payload() {
        let state = setup_state().await;
        let auth = auth_for("user_1");
        let mut addr = make_valid_address();
        addr.label = "".into();
        let req = AddBuyerAddressRequest {
            user_id: "user_1".into(),
            address: addr,
        };

        let result = add_buyer_address(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    // Lines 183-204: update_buyer_address
    #[tokio::test]
    async fn test_update_buyer_address_success() {
        use ob_core::Config;
        use ob_database::DatabaseClient;

        let state = setup_state().await;
        // First create an address
        let address_id = "addr_test123";
        state
            .db
            .create_document(
                collections::BUYER_ADDRESSES,
                json!({
                    "id": address_id,
                    fields::UID: "user_1",
                    "label": "Home",
                    "street": "123 Main",
                    "city": "Toronto",
                    "province": "ON",
                    "postalCode": "M5V 2T6",
                    "country": "CA",
                }),
            )
            .await
            .unwrap();

        let auth = auth_for("user_1");
        let req = UpdateBuyerAddressRequest {
            user_id: "user_1".into(),
            address_id: address_id.into(),
            address: make_valid_address(),
        };

        let result = update_buyer_address(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.success);
    }

    // Lines 188-190: update_buyer_address unauthorized
    #[tokio::test]
    async fn test_update_buyer_address_unauthorized() {
        let state = setup_state().await;
        let auth = auth_for("user_2");
        let req = UpdateBuyerAddressRequest {
            user_id: "user_1".into(),
            address_id: "addr_123".into(),
            address: make_valid_address(),
        };

        let result = update_buyer_address(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    // Lines 192-193: update_buyer_address invalid address_id
    #[tokio::test]
    async fn test_update_buyer_address_invalid_address_id() {
        let state = setup_state().await;
        let auth = auth_for("user_1");
        let req = UpdateBuyerAddressRequest {
            user_id: "user_1".into(),
            address_id: "".into(),
            address: make_valid_address(),
        };

        let result = update_buyer_address(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    // Lines 207-224: delete_buyer_address
    #[tokio::test]
    async fn test_delete_buyer_address_success() {
        let state = setup_state().await;
        let address_id = "addr_del123";
        state
            .db
            .create_document(
                collections::BUYER_ADDRESSES,
                json!({
                    "id": address_id,
                    fields::UID: "user_1",
                }),
            )
            .await
            .unwrap();

        let auth = auth_for("user_1");
        let req = DeleteBuyerAddressRequest {
            user_id: "user_1".into(),
            address_id: address_id.into(),
        };

        let result = delete_buyer_address(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.success);
    }

    // Lines 212-214: delete unauthorized
    #[tokio::test]
    async fn test_delete_buyer_address_unauthorized() {
        let state = setup_state().await;
        let auth = auth_for("user_2");
        let req = DeleteBuyerAddressRequest {
            user_id: "user_1".into(),
            address_id: "addr_123".into(),
        };

        let result = delete_buyer_address(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    // Lines 216: delete invalid address_id
    #[tokio::test]
    async fn test_delete_buyer_address_invalid_id() {
        let state = setup_state().await;
        let auth = auth_for("user_1");
        let req = DeleteBuyerAddressRequest {
            user_id: "user_1".into(),
            address_id: "".into(),
        };

        let result = delete_buyer_address(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    // Lines 227-251: set_default_buyer_address
    #[tokio::test]
    async fn test_set_default_buyer_address_success() {
        let state = setup_state().await;
        // Create user doc
        state
            .db
            .create_document(
                collections::USERS,
                json!({
                    "id": "user_1",
                    fields::UID: "user_1",
                }),
            )
            .await
            .unwrap();

        let auth = auth_for("user_1");
        let req = SetDefaultBuyerAddressRequest {
            user_id: "user_1".into(),
            address_id: "addr_default".into(),
        };

        let result = set_default_buyer_address(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.success);
    }

    // Lines 232-234: set_default unauthorized
    #[tokio::test]
    async fn test_set_default_buyer_address_unauthorized() {
        let state = setup_state().await;
        let auth = auth_for("user_2");
        let req = SetDefaultBuyerAddressRequest {
            user_id: "user_1".into(),
            address_id: "addr_123".into(),
        };

        let result = set_default_buyer_address(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    // Lines 236: set_default invalid address_id
    #[tokio::test]
    async fn test_set_default_buyer_address_invalid_id() {
        let state = setup_state().await;
        let auth = auth_for("user_1");
        let req = SetDefaultBuyerAddressRequest {
            user_id: "user_1".into(),
            address_id: "".into(),
        };

        let result = set_default_buyer_address(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    // Lines 253-276: validate_address_payload edge cases
    #[test]
    fn test_validate_address_payload_empty_label() {
        let addr = AddressPayload {
            label: "  ".into(),
            ..make_valid_address()
        };
        assert!(validate_address_payload(&addr).is_err());
    }

    #[test]
    fn test_validate_address_payload_empty_street() {
        let addr = AddressPayload {
            street: "".into(),
            ..make_valid_address()
        };
        assert!(validate_address_payload(&addr).is_err());
    }

    #[test]
    fn test_validate_address_payload_empty_city() {
        let addr = AddressPayload {
            city: " ".into(),
            ..make_valid_address()
        };
        assert!(validate_address_payload(&addr).is_err());
    }

    #[test]
    fn test_validate_address_payload_empty_province() {
        let addr = AddressPayload {
            province: "".into(),
            ..make_valid_address()
        };
        assert!(validate_address_payload(&addr).is_err());
    }

    #[test]
    fn test_validate_address_payload_non_ca_country() {
        let addr = AddressPayload {
            country: "US".into(),
            ..make_valid_address()
        };
        let err = validate_address_payload(&addr);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("Canadian"));
    }

    #[test]
    fn test_validate_address_payload_apartment_too_long() {
        let addr = AddressPayload {
            apartment: Some("A".repeat(51)),
            ..make_valid_address()
        };
        assert!(validate_address_payload(&addr).is_err());
    }

    #[test]
    fn test_validate_address_payload_with_apartment_ok() {
        let addr = AddressPayload {
            apartment: Some("Unit 4B".into()),
            ..make_valid_address()
        };
        assert!(validate_address_payload(&addr).is_ok());
    }

    // Line 285: validate_postal_code wrong length
    #[test]
    fn test_validate_postal_code_wrong_length() {
        assert!(validate_postal_code("M5V").is_err());
        assert!(validate_postal_code("M5V2T6X").is_err());
    }

    // Lines 288-293: validate_postal_code invalid format
    #[test]
    fn test_validate_postal_code_invalid_format_alpha_where_digit_expected() {
        assert!(validate_postal_code("MMVVTT").is_err()); // digit positions have letters
    }

    #[test]
    fn test_validate_postal_code_invalid_format_digit_where_alpha_expected() {
        assert!(validate_postal_code("152346").is_err()); // alpha positions have digits
    }

    #[test]
    fn test_validate_postal_code_valid_with_space() {
        assert!(validate_postal_code("K1A 0B1").is_ok());
    }

    #[test]
    fn test_validate_postal_code_valid_lowercase() {
        assert!(validate_postal_code("k1a0b1").is_ok());
    }
}
