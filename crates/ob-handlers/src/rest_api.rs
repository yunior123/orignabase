//! REST API endpoints for MCP server integration
//! Provides GET-based endpoints that wrap existing business logic handlers

use crate::HandlersState;
use crate::shared::schema::{collections, fields, lifecycle_status};
use axum::{
    Json, Router,
    extract::{Extension, Path, Query, State},
    routing::get,
};
use ob_auth::middleware::AuthContext;
use ob_database::fields as db_fields;
use serde::Deserialize;
use serde_json::json;

fn validate_image_url(url: &str) -> Result<(), ob_core::Error> {
    if url.trim().is_empty() {
        return Err(ob_core::Error::Validation(
            "Image URL cannot be empty".into(),
        ));
    }

    if !url.starts_with("https://") {
        return Err(ob_core::Error::Validation(
            "Image URLs must use HTTPS protocol".into(),
        ));
    }

    let allowed_domains = [
        "r2.cloudflarestorage.com",
        "orignagta.ca",
        "cdn.orignagta.ca",
        "pub-",
    ];

    let is_allowed = allowed_domains.iter().any(|domain| url.contains(domain));
    if !is_allowed {
        return Err(ob_core::Error::Validation(
            "Image URL must be from Cloudflare R2 or OrignaGTA CDN".into(),
        ));
    }

    Ok(())
}

pub fn router(state: HandlersState) -> Router {
    Router::new()
        // Products
        .route("/products", get(get_products).post(create_product))
        .route("/products/{id}", get(get_product))
        .route(
            "/products/{id}/recommendations",
            get(get_product_recommendations),
        )
        // Cart
        .route("/cart", get(get_cart))
        // Orders
        .route("/orders", get(list_orders))
        .route("/orders/{id}", get(get_order))
        // User profile
        .route("/user/profile", get(get_user_profile))
        .with_state(state)
}

// ───────────────────────────────────────────────────────────────────────────
// PRODUCTS
// ───────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SearchProductsQuery {
    q: Option<String>,
    category: Option<String>,
    #[serde(alias = "categoryId")]
    category_id: Option<String>,
    min_price: Option<i64>,
    max_price: Option<i64>,
    sort: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 {
    20
}

async fn get_products(
    State(state): State<HandlersState>,
    Query(qs): Query<SearchProductsQuery>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    let mut query = format!(
        "SELECT * FROM {} WHERE data->>'{}' = '{}'",
        collections::PRODUCTS,
        db_fields::LIFECYCLE_STATUS,
        lifecycle_status::ACTIVE,
    );

    if let Some(search) = qs.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let escaped_search = ob_core::escape_sql_string(search);
        query.push_str(&format!(
            " AND (data->>'{}' ~~* '%{}%' OR data->>'{}' ~~* '%{}%')",
            fields::TITLE,
            escaped_search,
            fields::DESCRIPTION,
            escaped_search,
        ));
    }
    if let Some(category) = qs
        .category
        .as_deref()
        .or(qs.category_id.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let escaped_category = ob_core::escape_sql_string(category);
        query.push_str(&format!(
            " AND data->>'{}' = '{}'",
            fields::CATEGORY,
            escaped_category,
        ));
    }
    if let Some(min) = qs.min_price {
        query.push_str(&format!(
            " AND NULLIF(data->>'{}', '')::numeric >= {}",
            db_fields::PRICE_CENTS,
            min,
        ));
    }
    if let Some(max) = qs.max_price {
        query.push_str(&format!(
            " AND NULLIF(data->>'{}', '')::numeric <= {}",
            db_fields::PRICE_CENTS,
            max,
        ));
    }

    match qs.sort.as_deref() {
        Some("price_asc") => query.push_str(&format!(
            " ORDER BY (data->>'{}')::\"numeric\" ASC, data->>'{}' DESC",
            db_fields::PRICE_CENTS,
            db_fields::CREATED_AT,
        )),
        Some("price_desc") => query.push_str(&format!(
            " ORDER BY (data->>'{}')::\"numeric\" DESC, data->>'{}' DESC",
            db_fields::PRICE_CENTS,
            db_fields::CREATED_AT,
        )),
        Some("oldest") => {
            query.push_str(&format!(" ORDER BY data->>'{}' ASC", db_fields::CREATED_AT))
        }
        Some("newest") | None => query.push_str(&format!(
            " ORDER BY data->>'{}' DESC",
            db_fields::CREATED_AT
        )),
        Some(_) => query.push_str(&format!(
            " ORDER BY data->>'{}' DESC",
            db_fields::CREATED_AT
        )),
    }

    let limit = qs.limit.clamp(1, 100);
    let offset = qs.offset.max(0);
    query.push_str(&format!(" LIMIT {} OFFSET {}", limit, offset));

    let results = state.db.query_raw(&query).await?;

    Ok(Json(serde_json::Value::Array(results)))
}

async fn get_product(
    State(state): State<HandlersState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    let doc = state
        .db
        .get_document(collections::PRODUCTS, &id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Product not found".into()))?;

    Ok(Json(doc))
}

/// GET /products/{id}/recommendations — Get product recommendations.
/// Tries co-purchase data first, then seller-curated bundles, then same-category fallback.
async fn get_product_recommendations(
    State(state): State<HandlersState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    // 1. Try co-purchase recommendations from precomputed table
    let sanitized_id = id.replace(':', "_");
    let rec_result = state
        .db
        .get_document(collections::PRODUCT_RECOMMENDATIONS, &sanitized_id)
        .await;

    if let Ok(rec_doc) = &rec_result
        && let Some(recs) = rec_doc.get(fields::RECOMMENDATIONS)
        && recs.as_array().is_some_and(|a| !a.is_empty())
    {
        return Ok(Json(
            json!({ "recommendations": recs, "source": "co_purchase" }),
        ));
    }

    // 2. Fallback: bundledProductIds from product itself
    let product = state
        .db
        .get_document(collections::PRODUCTS, &id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Product not found".into()))?;

    if let Some(bundled) = product.get(fields::BUNDLED_PRODUCT_IDS)
        && bundled.as_array().is_some_and(|a| !a.is_empty())
    {
        return Ok(Json(
            json!({ "recommendations": bundled, "source": "seller_curated" }),
        ));
    }

    // 3. Fallback: same category products
    let category_id = product
        .get(fields::CATEGORY)
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let similar: Vec<serde_json::Value> = state
        .db
        .query_bind_value(
            &format!(
                "SELECT data->>'productId' FROM {} WHERE data->>'categoryId' = $cid AND data->>'productId' != $pid AND data->>'{}' = $status ORDER BY (data->>'purchaseCount')::bigint DESC LIMIT 5",
                collections::PRODUCTS,
                db_fields::LIFECYCLE_STATUS,
            ),
            json!({
                "cid": category_id,
                "pid": &id,
                "status": lifecycle_status::ACTIVE,
            }),
        )
        .await
        .unwrap_or_default();

    let ids: Vec<serde_json::Value> = similar
        .iter()
        .filter_map(|v| v.get(fields::PRODUCT_ID).cloned())
        .collect();

    Ok(Json(
        json!({ "recommendations": ids, "source": "category" }),
    ))
}

/// POST /products — Create a product with validation.
/// Requires authentication and seller role.
async fn create_product(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    // Require authentication
    let user_id = require_authenticated(&auth)?;

    // Require seller role
    if !auth.roles.iter().any(|r| r == "seller" || r == "admin") {
        return Err(ob_core::Error::Forbidden(
            "Only sellers can create products".into(),
        ));
    }

    let obj = body
        .as_object()
        .ok_or_else(|| ob_core::Error::Validation("Request body must be a JSON object".into()))?;

    // Require name
    match obj.get(db_fields::NAME).and_then(|v| v.as_str()) {
        Some(name) if name.trim().is_empty() => {
            return Err(ob_core::Error::Validation(
                "Product name cannot be empty".into(),
            ));
        }
        None => {
            return Err(ob_core::Error::Validation(
                "Product name is required".into(),
            ));
        }
        _ => {}
    }

    // Require priceCents: must be > 0 and <= 10,000,000
    match obj.get(db_fields::PRICE_CENTS).and_then(|v| v.as_i64()) {
        Some(price) if price <= 0 => {
            return Err(ob_core::Error::Validation(
                "Product price must be greater than 0 cents".into(),
            ));
        }
        Some(price) if price > 10_000_000 => {
            return Err(ob_core::Error::Validation(
                "Product price cannot exceed $100,000 CAD".into(),
            ));
        }
        None => {
            return Err(ob_core::Error::Validation(
                "Product priceCents is required".into(),
            ));
        }
        _ => {}
    }

    // Validate stockQuantity: must be >= 0
    if let Some(stock) = obj.get(fields::STOCK_QUANTITY).and_then(|v| v.as_i64())
        && stock < 0
    {
        return Err(ob_core::Error::Validation(
            "Stock quantity cannot be negative".into(),
        ));
    }

    // Validate lifecycleStatus if present (draft -> active -> inactive -> deleted)
    if let Some(status) = obj
        .get(db_fields::LIFECYCLE_STATUS)
        .and_then(|v| v.as_str())
        && !lifecycle_status::ALL.contains(&status)
    {
        return Err(ob_core::Error::Validation(format!(
            "Invalid lifecycle status: {status}. Valid values: {}",
            lifecycle_status::ALL.join(", ")
        )));
    }

    let mut normalized_image_urls = Vec::new();
    if let Some(image_urls) = obj.get(fields::IMAGE_URLS).and_then(|v| v.as_array()) {
        for url in image_urls {
            let url = url.as_str().ok_or_else(|| {
                ob_core::Error::Validation("imageUrls entries must be strings".into())
            })?;
            validate_image_url(url)?;
            normalized_image_urls.push(url.to_string());
        }
    } else if let Some(image_url) = obj.get("imageUrl").and_then(|v| v.as_str()) {
        validate_image_url(image_url)?;
        normalized_image_urls.push(image_url.to_string());
    }

    // Build product document
    let mut product = body.clone();
    let product_obj = product
        .as_object_mut()
        .ok_or_else(|| ob_core::Error::Validation("Product body must be a JSON object".into()))?;
    product_obj.insert(db_fields::SELLER_ID.into(), json!(user_id));
    // Products use dateCreated (not createdAt) per schema
    product_obj.insert(
        db_fields::DATE_CREATED.into(),
        json!(chrono::Utc::now().to_rfc3339()),
    );
    product_obj.insert(
        db_fields::UPDATED_AT.into(),
        json!(chrono::Utc::now().to_rfc3339()),
    );
    product_obj.insert(fields::IMAGE_URLS.into(), json!(normalized_image_urls));
    product_obj.remove("imageUrl");

    let created = state
        .db
        .create_document(collections::PRODUCTS, product)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to create product: {e}")))?;

    Ok(Json(json!({
        "success": true,
        "product": created
    })))
}

/// GET /user/profile — Get authenticated user's profile.
/// Strips sensitive fields before returning.
async fn get_user_profile(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    let user_id = require_authenticated(&auth)?;

    let doc = state
        .db
        .get_document(collections::USERS, &user_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("User not found".into()))?;

    // Strip sensitive fields
    let mut profile = doc;
    if let Some(obj) = profile.as_object_mut() {
        // ignore-magic: security-sensitive field names stripped before API response
        obj.remove("passwordHash");
        obj.remove("password_hash");
        obj.remove("mfaSecret");
        obj.remove("mfa_secret");
        obj.remove("backupCodes");
        obj.remove("backup_codes");
        obj.remove("refreshTokens");
        obj.remove("refresh_tokens");
        obj.remove("totpSecret");
        obj.remove("totp_secret");
    }

    Ok(Json(profile))
}

// ───────────────────────────────────────────────────────────────────────────
// CART
// ───────────────────────────────────────────────────────────────────────────

async fn get_cart(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    let user_id = require_authenticated(&auth)?;

    // Get cart from user document
    let user = state
        .db
        .get_document(collections::USERS, &user_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("User not found".into()))?;

    let cart = user
        .get(collections::CART)
        .cloned()
        .unwrap_or_else(|| json!([]));

    Ok(Json(cart))
}
// ───────────────────────────────────────────────────────────────────────────
// ORDERS
// ───────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ListOrdersQuery {
    status: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

async fn list_orders(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Query(qs): Query<ListOrdersQuery>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    let user_id = require_authenticated(&auth)?;

    // Use parameterized query to prevent injection
    let mut query = format!(
        "SELECT * FROM {} WHERE data->>'buyerId' = $user_id",
        collections::ORDERS,
    );
    let mut bind_params = serde_json::Map::new();
    bind_params.insert("user_id".into(), json!(user_id));

    if qs.status.is_some() {
        query.push_str(" AND status = $status");
        bind_params.insert(db_fields::STATUS.into(), json!(qs.status));
    }

    let limit = qs.limit.clamp(1, 100);
    let offset = qs.offset.max(0);
    query.push_str(" ORDER BY created_at DESC LIMIT $limit OFFSET $offset");
    bind_params.insert("limit".into(), json!(limit));
    bind_params.insert("offset".into(), json!(offset));

    let results = state
        .db
        .query_bind_value(&query, serde_json::Value::Object(bind_params))
        .await?;

    Ok(Json(serde_json::Value::Array(results)))
}

async fn get_order(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    let user_id = require_authenticated(&auth)?;

    let order = state
        .db
        .get_document(collections::ORDERS, &id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Order not found".into()))?;

    // Verify user owns this order (buyer or seller)
    let buyer_id = order
        .get(db_fields::BUYER_ID)
        .and_then(|b| b.as_str())
        .unwrap_or("");
    let seller_id = order
        .get(db_fields::SELLER_ID)
        .and_then(|s| s.as_str())
        .unwrap_or("");

    if buyer_id != user_id && seller_id != user_id {
        return Err(ob_core::Error::Forbidden(
            "You do not own this order".into(),
        ));
    }

    Ok(Json(order))
}

// ───────────────────────────────────────────────────────────────────────────
// HELPERS
// ───────────────────────────────────────────────────────────────────────────

fn require_authenticated(auth: &AuthContext) -> Result<String, ob_core::Error> {
    if auth.authenticated {
        Ok(auth.user_id.clone())
    } else {
        Err(ob_core::Error::Auth("Authentication required".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_rest_api_router_builds() {
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
        };
        let _router = router(state);
    }

    #[test]
    fn test_search_products_query_deserialize_all_fields() {
        let qs: SearchProductsQuery = serde_json::from_value(serde_json::json!({
            "q": "widget",
            "category": "electronics",
            "min_price": 100,
            "max_price": 5000,
            "sort": "price_asc",
            "limit": 10,
            "offset": 20,
        }))
        .unwrap();
        assert_eq!(qs.q.as_deref(), Some("widget"));
        assert_eq!(qs.category.as_deref(), Some("electronics"));
        assert_eq!(qs.min_price, Some(100));
        assert_eq!(qs.max_price, Some(5000));
        assert_eq!(qs.sort.as_deref(), Some("price_asc"));
        assert_eq!(qs.limit, 10);
        assert_eq!(qs.offset, 20);
    }

    #[test]
    fn test_search_products_query_accepts_category_id_alias() {
        let qs: SearchProductsQuery =
            serde_json::from_value(serde_json::json!({ "categoryId": "1" })).unwrap();
        assert_eq!(qs.category_id.as_deref(), Some("1"));
        assert!(qs.category.is_none());
    }

    #[test]
    fn test_search_products_query_defaults() {
        let qs: SearchProductsQuery = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(qs.q.is_none());
        assert!(qs.category.is_none());
        assert!(qs.category_id.is_none());
        assert!(qs.min_price.is_none());
        assert!(qs.max_price.is_none());
        assert!(qs.sort.is_none());
        assert_eq!(qs.limit, 20);
        assert_eq!(qs.offset, 0);
    }

    #[test]
    fn test_list_orders_query_deserialize() {
        let qs: ListOrdersQuery = serde_json::from_value(serde_json::json!({
            "status": "shipped",
            "limit": 5,
            "offset": 10,
        }))
        .unwrap();
        assert_eq!(qs.status.as_deref(), Some("shipped"));
        assert_eq!(qs.limit, 5);
        assert_eq!(qs.offset, 10);
    }

    #[test]
    fn test_list_orders_query_defaults() {
        let qs: ListOrdersQuery = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(qs.status.is_none());
        assert_eq!(qs.limit, 20);
        assert_eq!(qs.offset, 0);
    }

    #[test]
    fn test_default_limit() {
        assert_eq!(super::default_limit(), 20);
    }

    #[test]
    fn test_require_authenticated_valid() {
        let auth = AuthContext {
            user_id: "user_123".into(),
            roles: vec!["user".into()],
            authenticated: true,
            email_verified: false,
            custom_claims: serde_json::Value::Null,
        };
        let result = require_authenticated(&auth);
        assert_eq!(result.unwrap(), "user_123");
    }

    #[test]
    fn test_require_authenticated_not_auth() {
        let auth = AuthContext::anonymous();
        let result = require_authenticated(&auth);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_products_filters_active_category_and_search() {
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
        };
        let test_id = uuid::Uuid::new_v4().to_string();
        let category_id = 91001;
        let phone_active = format!("phone_active_{test_id}");
        let book_active = format!("book_active_{test_id}");
        let phone_draft = format!("phone_draft_{test_id}");
        let search_term = format!("phone-{test_id}");

        state
            .db
            .upsert_document(
                "products",
                &phone_active,
                json!({
                    "productId": phone_active,
                    "name": format!("Phone Max {search_term}"),
                    "description": format!("Premium {search_term} with OLED display"),
                    "categoryId": category_id,
                    "lifecycleStatus": "active",
                    "priceCents": 129900,
                    "createdAt": "2026-04-22T10:00:00Z",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                "products",
                &book_active,
                json!({
                    "productId": book_active,
                    "name": "Design Book",
                    "description": "A design systems handbook",
                    "categoryId": category_id + 1,
                    "lifecycleStatus": "active",
                    "priceCents": 4900,
                    "createdAt": "2026-04-21T10:00:00Z",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                "products",
                &phone_draft,
                json!({
                    "productId": phone_draft,
                    "name": format!("Phone Draft {search_term}"),
                    "description": format!("Hidden draft {search_term}"),
                    "categoryId": category_id,
                    "lifecycleStatus": "draft",
                    "priceCents": 99900,
                    "createdAt": "2026-04-23T10:00:00Z",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = get_products(
            State(state),
            Query(SearchProductsQuery {
                q: Some(search_term.clone()),
                category: Some(category_id.to_string()),
                category_id: None,
                min_price: Some(100000),
                max_price: Some(140000),
                sort: Some("price_desc".into()),
                limit: 10,
                offset: 0,
            }),
        )
        .await
        .unwrap();

        let products = resp.as_array().unwrap();
        assert_eq!(products.len(), 1);
        assert_eq!(products[0]["productId"], phone_active);
    }

    #[tokio::test]
    async fn test_get_products_accepts_category_id_alias() {
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
        };
        let test_id = uuid::Uuid::new_v4().to_string();
        let category_id = 92001;
        let electronics_active = format!("electronics_active_{test_id}");
        let digital_active = format!("digital_active_{test_id}");

        state
            .db
            .upsert_document(
                "products",
                &electronics_active,
                json!({
                    "productId": electronics_active,
                    "name": "Phone Max",
                    "description": "Premium phone with OLED display",
                    "categoryId": category_id,
                    "lifecycleStatus": "active",
                    "priceCents": 129900,
                    "createdAt": "2026-04-22T10:00:00Z",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                "products",
                &digital_active,
                json!({
                    "productId": digital_active,
                    "name": "Digital Download",
                    "description": "Should be filtered out",
                    "categoryId": category_id + 1,
                    "lifecycleStatus": "active",
                    "priceCents": 4900,
                    "createdAt": "2026-04-21T10:00:00Z",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = get_products(
            State(state),
            Query(SearchProductsQuery {
                q: None,
                category: None,
                category_id: Some(category_id.to_string()),
                min_price: None,
                max_price: None,
                sort: None,
                limit: 10,
                offset: 0,
            }),
        )
        .await
        .unwrap();

        let products = resp.as_array().unwrap();
        assert_eq!(products.len(), 1);
        assert_eq!(products[0]["productId"], electronics_active);
    }

    #[tokio::test]
    async fn test_get_product_recommendations_category_fallback() {
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
        };

        // Create target product
        state
            .db
            .upsert_document(
                "products",
                "target_prod",
                json!({
                    "productId": "target_prod",
                    "name": "Target",
                    "categoryId": 42,
                    "lifecycleStatus": "active",
                    "priceCents": 1000,
                }),
            )
            .await
            .unwrap();

        let Json(resp) = get_product_recommendations(State(state), Path("target_prod".into()))
            .await
            .unwrap();

        assert_eq!(resp["source"], "category");
    }

    #[tokio::test]
    async fn test_get_product_recommendations_co_purchase() {
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
        };

        // Create product
        state
            .db
            .upsert_document(
                "products",
                "prod1",
                json!({
                    "productId": "prod1",
                    "name": "Product 1",
                    "categoryId": 1,
                    "lifecycleStatus": "active",
                }),
            )
            .await
            .unwrap();

        // Create precomputed recommendations
        state
            .db
            .upsert_document(
                "product_recommendations",
                "prod1",
                json!({
                    "productId": "prod1",
                    "recommendations": [{"productId": "prod2", "score": 5, "type": "co_purchase"}],
                    "computedAt": "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = get_product_recommendations(State(state), Path("prod1".into()))
            .await
            .unwrap();

        assert_eq!(resp["source"], "co_purchase");
        assert!(!resp["recommendations"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_product_recommendations_seller_curated() {
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
        };

        // Create product with bundledProductIds
        state
            .db
            .upsert_document(
                "products",
                "prod_bundle",
                json!({
                    "productId": "prod_bundle",
                    "name": "Bundled Product",
                    "categoryId": 1,
                    "lifecycleStatus": "active",
                    "bundledProductIds": ["prod_a", "prod_b"],
                }),
            )
            .await
            .unwrap();

        let Json(resp) = get_product_recommendations(State(state), Path("prod_bundle".into()))
            .await
            .unwrap();

        assert_eq!(resp["source"], "seller_curated");
    }
}
