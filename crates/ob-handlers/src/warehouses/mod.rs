//! Seller warehouse management handlers.
//! Ported from: functions/handlers/products.py

use axum::{Json, Router, extract::State, routing::post};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::HandlersState;
use crate::shared::schema::{COUNTRY_CANADA, collections, fields};
use crate::shared::validation::{sanitize_html, validate_uid};

const MAX_LABEL_LENGTH: usize = 100;
const VALID_TYPES: &[&str] = &["warehouse", "personal"];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarehouseAddressInput {
    pub street: String,
    #[serde(default)]
    pub apartment: Option<String>,
    pub city: String,
    pub state: String,
    pub postal_code: String,
    #[serde(default = "default_country")]
    pub country: String,
    #[serde(default)]
    pub phone_number: Option<String>,
    #[serde(default)]
    pub latitude: Option<f64>,
    #[serde(default)]
    pub longitude: Option<f64>,
    #[serde(default)]
    pub label: Option<String>,
}

fn default_country() -> String {
    COUNTRY_CANADA.to_string()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWarehouseRequest {
    pub user_id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub warehouse_type: String,
    pub address: WarehouseAddressInput,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWarehouseRequest {
    pub user_id: String,
    pub warehouse_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default, rename = "type")]
    pub warehouse_type: Option<String>,
    #[serde(default)]
    pub address: Option<WarehouseAddressInput>,
    #[serde(default)]
    pub is_default: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteWarehouseRequest {
    pub user_id: String,
    pub warehouse_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListWarehousesRequest {
    pub user_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WarehouseMutationResponse {
    pub success: bool,
    pub warehouse_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListWarehousesResponse {
    pub success: bool,
    pub warehouses: Vec<Value>,
}

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/warehouses/create", post(create_warehouse))
        .route("/api/warehouses/update", post(update_warehouse))
        .route("/api/warehouses/delete", post(delete_warehouse))
        .route("/api/warehouses/list", post(list_warehouses))
        .with_state(state)
}

fn warehouses_collection() -> String {
    format!("{}__{}", collections::USERS, collections::WAREHOUSES)
}

fn warehouse_parent(user_id: &str) -> String {
    format!("{}:{}", collections::USERS, user_id)
}

fn sanitize_label(label: &str) -> ob_core::Result<String> {
    let trimmed = label.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_LABEL_LENGTH {
        return Err(ob_core::Error::Validation(
            "label must be 1-100 characters".into(),
        ));
    }
    Ok(sanitize_html(trimmed))
}

fn sanitize_type(warehouse_type: &str) -> ob_core::Result<String> {
    let trimmed = warehouse_type.trim();
    if !VALID_TYPES.contains(&trimmed) {
        return Err(ob_core::Error::Validation(format!(
            "type must be one of: {:?}",
            VALID_TYPES
        )));
    }
    Ok(trimmed.to_string())
}

fn sanitize_address(address: &WarehouseAddressInput) -> ob_core::Result<Value> {
    if address.street.trim().is_empty()
        || address.city.trim().is_empty()
        || address.state.trim().is_empty()
        || address.postal_code.trim().is_empty()
    {
        return Err(ob_core::Error::Validation(
            "street, city, state, and postalCode are required".into(),
        ));
    }

    if address.country.trim() != COUNTRY_CANADA {
        return Err(ob_core::Error::Validation(
            "Warehouse address must be in Canada".into(),
        ));
    }

    Ok(json!({
        fields::STREET: sanitize_html(address.street.trim()),
        "apartment": sanitize_html(address.apartment.as_deref().unwrap_or("").trim()),
        fields::CITY: sanitize_html(address.city.trim()),
        "state": sanitize_html(address.state.trim()),
        fields::POSTAL_CODE: address.postal_code.trim().to_uppercase(),
        fields::COUNTRY: address.country.trim(),
        "phoneNumber": address.phone_number.as_deref().map(|s| sanitize_html(s.trim())),
        fields::LATITUDE: address.latitude,
        fields::LONGITUDE: address.longitude,
        "label": address.label.as_deref().map(|s| sanitize_html(s.trim())),
    }))
}

async fn clear_other_defaults(
    state: &HandlersState,
    user_id: &str,
    exclude_id: Option<&str>,
) -> ob_core::Result<()> {
    let collection = warehouses_collection();
    let query = format!(
        "SELECT * FROM {} WHERE parent_id = '{}' AND isDefault = true",
        collection,
        ob_core::escape_surreal_string(&warehouse_parent(user_id))
    );
    let docs = state.db.query_raw(&query).await?;
    for doc in docs {
        let id = doc
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ob_core::Error::Database("Warehouse record missing id".into()))?;
        let raw_id = id.strip_prefix(&format!("{collection}:")).unwrap_or(id);
        if exclude_id == Some(raw_id) {
            continue;
        }
        state
            .db
            .update_document(&collection, raw_id, json!({ fields::IS_DEFAULT: false }))
            .await?;
    }
    Ok(())
}

async fn load_owned_warehouse(
    state: &HandlersState,
    user_id: &str,
    warehouse_id: &str,
) -> ob_core::Result<Value> {
    let collection = warehouses_collection();
    let doc = state.db.get_document(&collection, warehouse_id).await?;
    let parent_id = doc
        .get("parent_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if parent_id != warehouse_parent(user_id) {
        return Err(ob_core::Error::NotFound("Warehouse not found".into()));
    }
    Ok(doc)
}

async fn create_warehouse(
    State(state): State<HandlersState>,
    Json(req): Json<CreateWarehouseRequest>,
) -> ob_core::Result<Json<WarehouseMutationResponse>> {
    validate_uid("userId", &req.user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "create_warehouse",
        15, // 15 creations
        60, // per hour
    )
    .await?;

    let label = sanitize_label(&req.label)?;
    let warehouse_type = sanitize_type(&req.warehouse_type)?;
    let address = sanitize_address(&req.address)?;

    if req.is_default {
        clear_other_defaults(&state, &req.user_id, None).await?;
    }

    let collection = warehouses_collection();
    let now = Utc::now().to_rfc3339();
    let created = state
        .db
        .create_document(
            &collection,
            json!({
                "parent_id": warehouse_parent(&req.user_id),
                "parent_collection": collections::USERS,
                "label": label,
                "type": warehouse_type,
                fields::ADDRESS: address,
                fields::IS_DEFAULT: req.is_default,
                fields::CREATED_AT: now,
            }),
        )
        .await?;

    let id = created
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::Database("Warehouse create returned no id".into()))?
        .strip_prefix(&format!("{collection}:"))
        .unwrap_or_default()
        .to_string();

    Ok(Json(WarehouseMutationResponse {
        success: true,
        warehouse_id: id,
    }))
}

async fn update_warehouse(
    State(state): State<HandlersState>,
    Json(req): Json<UpdateWarehouseRequest>,
) -> ob_core::Result<Json<WarehouseMutationResponse>> {
    validate_uid("userId", &req.user_id)?;
    validate_uid("warehouseId", &req.warehouse_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "update_warehouse",
        30, // 30 updates
        60, // per hour
    )
    .await?;

    let _existing = load_owned_warehouse(&state, &req.user_id, &req.warehouse_id).await?;

    let mut patch = serde_json::Map::new();
    if let Some(label) = req.label.as_deref() {
        patch.insert("label".to_string(), json!(sanitize_label(label)?));
    }
    if let Some(warehouse_type) = req.warehouse_type.as_deref() {
        patch.insert("type".to_string(), json!(sanitize_type(warehouse_type)?));
    }
    if let Some(address) = req.address.as_ref() {
        patch.insert(fields::ADDRESS.to_string(), sanitize_address(address)?);
    }
    if let Some(is_default) = req.is_default {
        if is_default {
            clear_other_defaults(&state, &req.user_id, Some(&req.warehouse_id)).await?;
        }
        patch.insert(fields::IS_DEFAULT.to_string(), json!(is_default));
    }

    if patch.is_empty() {
        return Err(ob_core::Error::Validation(
            "No valid fields to update".into(),
        ));
    }

    patch.insert(
        fields::UPDATED_AT.to_string(),
        json!(Utc::now().to_rfc3339()),
    );
    state
        .db
        .update_document(
            &warehouses_collection(),
            &req.warehouse_id,
            Value::Object(patch),
        )
        .await?;

    Ok(Json(WarehouseMutationResponse {
        success: true,
        warehouse_id: req.warehouse_id,
    }))
}

async fn delete_warehouse(
    State(state): State<HandlersState>,
    Json(req): Json<DeleteWarehouseRequest>,
) -> ob_core::Result<Json<WarehouseMutationResponse>> {
    validate_uid("userId", &req.user_id)?;
    validate_uid("warehouseId", &req.warehouse_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "delete_warehouse",
        15, // 15 deletions
        60, // per hour
    )
    .await?;

    let existing = load_owned_warehouse(&state, &req.user_id, &req.warehouse_id).await?;

    let product_guard_query = format!(
        "SELECT id FROM {} WHERE {} = '{}' AND {} CONTAINS '{}' LIMIT 1",
        collections::PRODUCTS,
        fields::SELLER_ID,
        ob_core::escape_surreal_string(&req.user_id),
        "warehouseIds",
        ob_core::escape_surreal_string(&req.warehouse_id),
    );
    if !state.db.query_raw(&product_guard_query).await?.is_empty() {
        return Err(ob_core::Error::Validation(
            "Cannot delete warehouse while products still reference it".into(),
        ));
    }

    if existing
        .get(fields::IS_DEFAULT)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let collection = warehouses_collection();
        let promote_query = format!(
            "SELECT * FROM {} WHERE parent_id = '{}' AND id != type::thing('{}', '{}') ORDER BY createdAt ASC LIMIT 1",
            collection,
            ob_core::escape_surreal_string(&warehouse_parent(&req.user_id)),
            collection,
            ob_core::escape_surreal_string(&req.warehouse_id),
        );
        if let Some(other) = state.db.query_raw(&promote_query).await?.into_iter().next()
            && let Some(id) = other.get("id").and_then(|v| v.as_str())
        {
            let raw_id = id.strip_prefix(&format!("{collection}:")).unwrap_or(id);
            state
                .db
                .update_document(&collection, raw_id, json!({ fields::IS_DEFAULT: true }))
                .await?;
        }
    }

    state
        .db
        .delete_document(&warehouses_collection(), &req.warehouse_id)
        .await?;

    Ok(Json(WarehouseMutationResponse {
        success: true,
        warehouse_id: req.warehouse_id,
    }))
}

async fn list_warehouses(
    State(state): State<HandlersState>,
    Json(req): Json<ListWarehousesRequest>,
) -> ob_core::Result<Json<ListWarehousesResponse>> {
    validate_uid("userId", &req.user_id)?;
    let collection = warehouses_collection();
    let query = format!(
        "SELECT * FROM {} WHERE parent_id = '{}' ORDER BY isDefault DESC, createdAt ASC",
        collection,
        ob_core::escape_surreal_string(&warehouse_parent(&req.user_id)),
    );
    let rows = state.db.query_raw(&query).await?;
    let warehouses = rows
        .into_iter()
        .map(|mut row| {
            if let Some(id) = row.get("id").and_then(|v| v.as_str()) {
                let raw_id = id
                    .strip_prefix(&format!("{collection}:"))
                    .unwrap_or(id)
                    .to_string();
                if let Some(obj) = row.as_object_mut() {
                    obj.insert("warehouseId".to_string(), json!(raw_id));
                }
            }
            row
        })
        .collect();

    Ok(Json(ListWarehousesResponse {
        success: true,
        warehouses,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use std::sync::Arc;

    async fn setup_state() -> HandlersState { HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
            http_client: reqwest::Client::new(),
        }
    }

    fn sample_address() -> WarehouseAddressInput {
        WarehouseAddressInput {
            street: "123 Main".into(),
            apartment: Some("Unit 4".into()),
            city: "Toronto".into(),
            state: "ON".into(),
            postal_code: "m5v3a8".into(),
            country: COUNTRY_CANADA.into(),
            phone_number: Some("555-1234".into()),
            latitude: Some(43.0),
            longitude: Some(-79.0),
            label: Some("Receiving".into()),
        }
    }

    #[test]
    fn test_sanitize_type_rejects_unknown_values() {
        assert!(sanitize_type("warehouse").is_ok());
        assert!(sanitize_type("personal").is_ok());
        assert!(sanitize_type("storefront").is_err());
    }

    #[test]
    fn test_sanitize_label_rejects_empty() {
        assert!(sanitize_label("").is_err());
        assert!(sanitize_label(" Main ").is_ok());
    }

    #[test]
    fn test_sanitize_address_requires_canada() {
        let address = WarehouseAddressInput {
            street: "123 Main".into(),
            apartment: None,
            city: "Toronto".into(),
            state: "ON".into(),
            postal_code: "M5V3A8".into(),
            country: "USA".into(),
            phone_number: None,
            latitude: None,
            longitude: None,
            label: None,
        };
        assert!(sanitize_address(&address).is_err());
    }

    #[test]
    fn test_sanitize_address_normalizes_values() {
        let address = sanitize_address(&sample_address()).unwrap();
        assert_eq!(address[fields::POSTAL_CODE], "M5V3A8");
        assert_eq!(address[fields::COUNTRY], COUNTRY_CANADA);
        assert_eq!(address["apartment"], "Unit 4");
    }

    #[tokio::test]
    async fn test_create_warehouse_default_true_clears_existing_default() {
        let state = setup_state().await;
        let collection = warehouses_collection();
        state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Old Default",
                    "type": "warehouse",
                    fields::IS_DEFAULT: true,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = create_warehouse(
            State(state.clone()),
            Json(CreateWarehouseRequest {
                user_id: "seller_1".into(),
                label: "New Default".into(),
                warehouse_type: "warehouse".into(),
                address: sample_address(),
                is_default: true,
            }),
        )
        .await
        .unwrap();

        let rows = state
            .db
            .query_bind_value(
                "SELECT * FROM $collection WHERE parent_id = $parent_id",
                json!({"collection": collection, "parent_id": warehouse_parent("seller_1")})
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        let defaults = rows
            .iter()
            .filter(|row| row.get(fields::IS_DEFAULT).and_then(|v| v.as_bool()) == Some(true))
            .count();
        assert_eq!(defaults, 1);
        assert!(!resp.warehouse_id.is_empty());
    }

    #[tokio::test]
    async fn test_update_warehouse_requires_at_least_one_field() {
        let state = setup_state().await;
        let collection = warehouses_collection();
        let created = state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Primary",
                    "type": "warehouse",
                    fields::ADDRESS: sanitize_address(&sample_address()).unwrap(),
                    fields::IS_DEFAULT: false,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        let warehouse_id = created["id"]
            .as_str()
            .unwrap()
            .strip_prefix(&format!("{collection}:"))
            .unwrap()
            .to_string();

        let err = update_warehouse(
            State(state),
            Json(UpdateWarehouseRequest {
                user_id: "seller_1".into(),
                warehouse_id,
                label: None,
                warehouse_type: None,
                address: None,
                is_default: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("No valid fields"));
    }

    #[tokio::test]
    async fn test_delete_default_warehouse_promotes_oldest_remaining() {
        let state = setup_state().await;
        let collection = warehouses_collection();
        let first = state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Default",
                    "type": "warehouse",
                    fields::IS_DEFAULT: true,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        let second = state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Backup",
                    "type": "warehouse",
                    fields::IS_DEFAULT: false,
                    fields::CREATED_AT: "2026-01-02T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        let first_id = first["id"]
            .as_str()
            .unwrap()
            .strip_prefix(&format!("{collection}:"))
            .unwrap()
            .to_string();
        let second_id = second["id"]
            .as_str()
            .unwrap()
            .strip_prefix(&format!("{collection}:"))
            .unwrap()
            .to_string();

        let _ = delete_warehouse(
            State(state.clone()),
            Json(DeleteWarehouseRequest {
                user_id: "seller_1".into(),
                warehouse_id: first_id,
            }),
        )
        .await
        .unwrap();

        let promoted = state
            .db
            .get_document(&collection, &second_id)
            .await
            .unwrap();
        assert_eq!(promoted[fields::IS_DEFAULT], true);
    }

    #[tokio::test]
    async fn test_delete_warehouse_blocked_when_products_reference_it() {
        let state = setup_state().await;
        let collection = warehouses_collection();
        let created = state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Primary",
                    "type": "warehouse",
                    fields::IS_DEFAULT: false,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        let warehouse_id = created["id"]
            .as_str()
            .unwrap()
            .strip_prefix(&format!("{collection}:"))
            .unwrap()
            .to_string();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                json!({
                    fields::SELLER_ID: "seller_1",
                    "warehouseIds": [warehouse_id.clone()],
                }),
            )
            .await
            .unwrap();

        let err = delete_warehouse(
            State(state),
            Json(DeleteWarehouseRequest {
                user_id: "seller_1".into(),
                warehouse_id,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("products still reference"));
    }

    // --- Coverage tests for uncovered lines ---

    // Lines 37-39: default_country
    #[test]
    fn test_default_country_is_canada() {
        assert_eq!(default_country(), COUNTRY_CANADA);
    }

    // Lines 139-141: sanitize_address rejects empty fields
    #[test]
    fn test_sanitize_address_rejects_empty_street() {
        let addr = WarehouseAddressInput {
            street: "".into(),
            apartment: None,
            city: "Toronto".into(),
            state: "ON".into(),
            postal_code: "M5V3A8".into(),
            country: COUNTRY_CANADA.into(),
            phone_number: None,
            latitude: None,
            longitude: None,
            label: None,
        };
        assert!(sanitize_address(&addr).is_err());
    }

    #[test]
    fn test_sanitize_address_rejects_empty_city() {
        let addr = WarehouseAddressInput {
            street: "123 Main".into(),
            apartment: None,
            city: "  ".into(),
            state: "ON".into(),
            postal_code: "M5V3A8".into(),
            country: COUNTRY_CANADA.into(),
            phone_number: None,
            latitude: None,
            longitude: None,
            label: None,
        };
        assert!(sanitize_address(&addr).is_err());
    }

    #[test]
    fn test_sanitize_address_rejects_empty_state() {
        let addr = WarehouseAddressInput {
            street: "123 Main".into(),
            apartment: None,
            city: "Toronto".into(),
            state: "".into(),
            postal_code: "M5V3A8".into(),
            country: COUNTRY_CANADA.into(),
            phone_number: None,
            latitude: None,
            longitude: None,
            label: None,
        };
        assert!(sanitize_address(&addr).is_err());
    }

    #[test]
    fn test_sanitize_address_rejects_empty_postal_code() {
        let addr = WarehouseAddressInput {
            street: "123 Main".into(),
            apartment: None,
            city: "Toronto".into(),
            state: "ON".into(),
            postal_code: "".into(),
            country: COUNTRY_CANADA.into(),
            phone_number: None,
            latitude: None,
            longitude: None,
            label: None,
        };
        assert!(sanitize_address(&addr).is_err());
    }

    // Line 183: load_owned_warehouse not found (wrong user)
    #[tokio::test]
    async fn test_load_owned_warehouse_wrong_user() {
        let state = setup_state().await;
        let collection = warehouses_collection();
        let created = state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Test",
                    "type": "warehouse",
                    fields::IS_DEFAULT: false,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        let wid = created["id"]
            .as_str()
            .unwrap()
            .strip_prefix(&format!("{collection}:"))
            .unwrap();

        let err = load_owned_warehouse(&state, "seller_2", wid).await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("not found"));
    }

    // Line 205: load_owned_warehouse success
    #[tokio::test]
    async fn test_load_owned_warehouse_success() {
        let state = setup_state().await;
        let collection = warehouses_collection();
        let created = state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Test",
                    "type": "warehouse",
                    fields::IS_DEFAULT: false,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        let wid = created["id"]
            .as_str()
            .unwrap()
            .strip_prefix(&format!("{collection}:"))
            .unwrap();

        let result = load_owned_warehouse(&state, "seller_1", wid).await;
        assert!(result.is_ok());
    }

    // Line 230: create_warehouse without is_default
    #[tokio::test]
    async fn test_create_warehouse_not_default() {
        let state = setup_state().await;
        let Json(resp) = create_warehouse(
            State(state),
            Json(CreateWarehouseRequest {
                user_id: "seller_1".into(),
                label: "My Warehouse".into(),
                warehouse_type: "personal".into(),
                address: sample_address(),
                is_default: false,
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(!resp.warehouse_id.is_empty());
    }

    // Lines 283, 286, 289: update_warehouse with label, type, and address
    #[tokio::test]
    async fn test_update_warehouse_all_fields() {
        let state = setup_state().await;
        let collection = warehouses_collection();
        let created = state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Primary",
                    "type": "warehouse",
                    fields::ADDRESS: sanitize_address(&sample_address()).unwrap(),
                    fields::IS_DEFAULT: false,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        let warehouse_id = created["id"]
            .as_str()
            .unwrap()
            .strip_prefix(&format!("{collection}:"))
            .unwrap()
            .to_string();

        let Json(resp) = update_warehouse(
            State(state),
            Json(UpdateWarehouseRequest {
                user_id: "seller_1".into(),
                warehouse_id: warehouse_id.clone(),
                label: Some("Updated Label".into()),
                warehouse_type: Some("personal".into()),
                address: Some(sample_address()),
                is_default: Some(true),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.warehouse_id, warehouse_id);
    }

    // Lines 292-295: update_warehouse set is_default = true clears others
    #[tokio::test]
    async fn test_update_warehouse_set_default_clears_others() {
        let state = setup_state().await;
        let collection = warehouses_collection();
        let first = state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "First",
                    "type": "warehouse",
                    fields::IS_DEFAULT: true,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        let second = state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Second",
                    "type": "warehouse",
                    fields::IS_DEFAULT: false,
                    fields::CREATED_AT: "2026-01-02T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        let second_id = second["id"]
            .as_str()
            .unwrap()
            .strip_prefix(&format!("{collection}:"))
            .unwrap()
            .to_string();

        let Json(resp) = update_warehouse(
            State(state.clone()),
            Json(UpdateWarehouseRequest {
                user_id: "seller_1".into(),
                warehouse_id: second_id.clone(),
                label: None,
                warehouse_type: None,
                address: None,
                is_default: Some(true),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);

        // Check only second is default now
        let first_id = first["id"]
            .as_str()
            .unwrap()
            .strip_prefix(&format!("{collection}:"))
            .unwrap();
        let first_doc = state.db.get_document(&collection, first_id).await.unwrap();
        assert_eq!(first_doc[fields::IS_DEFAULT], false);
    }

    // Lines 302-320: update_warehouse with empty patch
    // Already tested in test_update_warehouse_requires_at_least_one_field

    // Lines 304-315, 317-320: update_warehouse successful patch write
    #[tokio::test]
    async fn test_update_warehouse_label_only() {
        let state = setup_state().await;
        let collection = warehouses_collection();
        let created = state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Old",
                    "type": "warehouse",
                    fields::IS_DEFAULT: false,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        let warehouse_id = created["id"]
            .as_str()
            .unwrap()
            .strip_prefix(&format!("{collection}:"))
            .unwrap()
            .to_string();

        let Json(resp) = update_warehouse(
            State(state),
            Json(UpdateWarehouseRequest {
                user_id: "seller_1".into(),
                warehouse_id: warehouse_id.clone(),
                label: Some("New Label".into()),
                warehouse_type: None,
                address: None,
                is_default: None,
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.warehouse_id, warehouse_id);
    }

    // Lines 374-375: delete default warehouse, no other warehouse to promote
    #[tokio::test]
    async fn test_delete_default_warehouse_no_other_to_promote() {
        let state = setup_state().await;
        let collection = warehouses_collection();
        let created = state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Only Default",
                    "type": "warehouse",
                    fields::IS_DEFAULT: true,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        let warehouse_id = created["id"]
            .as_str()
            .unwrap()
            .strip_prefix(&format!("{collection}:"))
            .unwrap()
            .to_string();

        let Json(resp) = delete_warehouse(
            State(state),
            Json(DeleteWarehouseRequest {
                user_id: "seller_1".into(),
                warehouse_id: warehouse_id.clone(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.warehouse_id, warehouse_id);
    }

    // Lines 388-420: list_warehouses
    #[tokio::test]
    async fn test_list_warehouses_success() {
        let state = setup_state().await;
        let collection = warehouses_collection();
        state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Warehouse A",
                    "type": "warehouse",
                    fields::IS_DEFAULT: true,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Warehouse B",
                    "type": "personal",
                    fields::IS_DEFAULT: false,
                    fields::CREATED_AT: "2026-01-02T00:00:00Z",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = list_warehouses(
            State(state),
            Json(ListWarehousesRequest {
                user_id: "seller_1".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.warehouses.len(), 2);
        // Each warehouse should have warehouseId field added
        for w in &resp.warehouses {
            assert!(w.get("warehouseId").is_some());
        }
    }

    // Lines 397: list_warehouses empty
    #[tokio::test]
    async fn test_list_warehouses_empty() {
        let state = setup_state().await;

        let Json(resp) = list_warehouses(
            State(state),
            Json(ListWarehousesRequest {
                user_id: "seller_no_warehouses".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(resp.warehouses.is_empty());
    }

    // Lines 399-414: list_warehouses with id stripping
    #[tokio::test]
    async fn test_list_warehouses_strips_collection_prefix() {
        let state = setup_state().await;
        let collection = warehouses_collection();
        state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Test",
                    "type": "warehouse",
                    fields::IS_DEFAULT: false,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = list_warehouses(
            State(state),
            Json(ListWarehousesRequest {
                user_id: "seller_1".into(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.warehouses.len(), 1);
        let wid = resp.warehouses[0]["warehouseId"].as_str().unwrap();
        // Should not contain the collection prefix
        assert!(!wid.contains(&collection));
    }

    // Line 392: list_warehouses with invalid uid
    #[tokio::test]
    async fn test_list_warehouses_invalid_uid() {
        let state = setup_state().await;

        let result = list_warehouses(
            State(state),
            Json(ListWarehousesRequest { user_id: "".into() }),
        )
        .await;

        assert!(result.is_err());
    }

    // Delete non-default warehouse (lines 354-375 non-default branch)
    #[tokio::test]
    async fn test_delete_non_default_warehouse() {
        let state = setup_state().await;
        let collection = warehouses_collection();
        let created = state
            .db
            .create_document(
                &collection,
                json!({
                    "parent_id": warehouse_parent("seller_1"),
                    "label": "Not Default",
                    "type": "warehouse",
                    fields::IS_DEFAULT: false,
                    fields::CREATED_AT: "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        let warehouse_id = created["id"]
            .as_str()
            .unwrap()
            .strip_prefix(&format!("{collection}:"))
            .unwrap()
            .to_string();

        let Json(resp) = delete_warehouse(
            State(state),
            Json(DeleteWarehouseRequest {
                user_id: "seller_1".into(),
                warehouse_id: warehouse_id.clone(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
    }

    // Sanitize label too long
    #[test]
    fn test_sanitize_label_too_long() {
        let long_label = "A".repeat(101);
        assert!(sanitize_label(&long_label).is_err());
    }

    // Sanitize label with HTML
    #[test]
    fn test_sanitize_label_strips_html() {
        let result = sanitize_label("<b>Warehouse</b>").unwrap();
        assert!(!result.contains("<b>"));
    }
}
