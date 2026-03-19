//! REST API endpoints for MCP server integration
//! Provides GET-based endpoints that wrap existing business logic handlers

use axum::{
    extract::{Path, Query, State, Extension},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, delete},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use ob_auth::middleware::AuthContext;
use crate::HandlersState;
use crate::shared::schema::collections;

pub fn router(state: HandlersState) -> Router {
    Router::new()
        // Products
        .route("/products", get(get_products))
        .route("/products/:id", get(get_product))
        // Cart
        .route("/cart", get(get_cart))
        .route("/cart/add", post(add_to_cart))
        .route("/cart/remove/:product_id", delete(remove_from_cart))
        .route("/cart/coupon", post(apply_coupon))
        // Orders
        .route("/orders", get(list_orders))
        .route("/orders/:id", get(get_order))
        .route("/orders/:id/returns", post(create_return_request))
        // Reviews
        .route("/products/:id/reviews", post(create_review))
        // Analytics
        .route("/analytics", get(get_analytics))
        .with_state(state)
}

// ───────────────────────────────────────────────────────────────────────────
// PRODUCTS
// ───────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SearchProductsQuery {
    q: Option<String>,
    category: Option<String>,
    min_price: Option<i64>,
    max_price: Option<i64>,
    sort: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 { 20 }

async fn get_products(
    State(state): State<HandlersState>,
    Query(qs): Query<SearchProductsQuery>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    // Query products from SurrealDB with filters
    let mut query = format!(
        "SELECT * FROM {} WHERE lifecycleStatus = 'active'",
        collections::PRODUCTS
    );

    if let Some(cat) = &qs.category {
        query.push_str(&format!(" AND category = '{}'", escape_surreal_string(cat)));
    }
    if let Some(min) = qs.min_price {
        query.push_str(&format!(" AND priceCents >= {}", min));
    }
    if let Some(max) = qs.max_price {
        query.push_str(&format!(" AND priceCents <= {}", max));
    }

    query.push_str(&format!(" LIMIT {} OFFSET {}", qs.limit, qs.offset));

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

// ───────────────────────────────────────────────────────────────────────────
// CART
// ───────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AddToCartRequest {
    #[serde(rename = "productId")]
    product_id: String,
    quantity: i64,
}

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
        .get("cart")
        .cloned()
        .unwrap_or_else(|| json!([]));

    Ok(Json(cart))
}

async fn add_to_cart(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<AddToCartRequest>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    let user_id = require_authenticated(&auth)?;

    // Get product to verify it exists and get price
    let _product = state
        .db
        .get_document(collections::PRODUCTS, &req.product_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Product not found".into()))?;

    // TODO: Implement actual cart update to SurrealDB
    // For now, return empty response
    Ok(Json(json!({"added": true})))
}

async fn remove_from_cart(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Path(_product_id): Path<String>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    let _user_id = require_authenticated(&auth)?;
    // TODO: Implement cart item removal
    Ok(Json(json!({"removed": true})))
}

#[derive(Deserialize)]
pub struct ApplyCouponRequest {
    code: String,
}

async fn apply_coupon(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(_req): Json<ApplyCouponRequest>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    let _user_id = require_authenticated(&auth)?;
    // TODO: Implement coupon logic
    Ok(Json(json!({"applied": true})))
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

    let mut query = format!(
        "SELECT * FROM {} WHERE buyerId = '{}'",
        collections::ORDERS,
        escape_surreal_string(&user_id)
    );

    if let Some(status) = &qs.status {
        query.push_str(&format!(" AND status = '{}'", escape_surreal_string(status)));
    }

    query.push_str(&format!(" ORDER BY createdAt DESC LIMIT {} OFFSET {}", qs.limit, qs.offset));

    let results = state.db.query_raw(&query).await?;

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

    // Verify user owns this order
    let buyer_id = order
        .get("buyerId")
        .and_then(|b| b.as_str())
        .unwrap_or("");

    if buyer_id != user_id {
        return Err(ob_core::Error::Forbidden("You do not own this order".into()));
    }

    Ok(Json(order))
}

#[derive(Deserialize)]
pub struct CreateReturnRequest {
    items: Vec<serde_json::Value>,
    reason: String,
}

async fn create_return_request(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Path(order_id): Path<String>,
    Json(_req): Json<CreateReturnRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ob_core::Error> {
    let _user_id = require_authenticated(&auth)?;

    // Verify order exists
    let _order = state
        .db
        .get_document(collections::ORDERS, &order_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Order not found".into()))?;

    // TODO: Implement return request creation
    Ok((StatusCode::CREATED, Json(json!({"status": "pending"}))))
}

// ───────────────────────────────────────────────────────────────────────────
// REVIEWS
// ───────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateReviewRequest {
    rating: i32,
    #[serde(default)]
    review: String,
}

async fn create_review(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Path(_product_id): Path<String>,
    Json(req): Json<CreateReviewRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ob_core::Error> {
    let _user_id = require_authenticated(&auth)?;

    if !(1..=5).contains(&req.rating) {
        return Err(ob_core::Error::Validation("Rating must be 1-5".into()));
    }

    // TODO: Verify user purchased this product
    // TODO: Implement review creation

    Ok((StatusCode::CREATED, Json(json!({"status": "created"}))))
}

// ───────────────────────────────────────────────────────────────────────────
// ANALYTICS
// ───────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AnalyticsQuery {
    period: Option<String>, // "day", "week", "month"
}

async fn get_analytics(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Query(_qs): Query<AnalyticsQuery>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    let _user_id = require_authenticated(&auth)?;

    // TODO: Calculate analytics from orders
    Ok(Json(json!({
        "totalOrders": 0,
        "totalRevenueCents": 0,
        "averageOrderValueCents": 0,
    })))
}

// ───────────────────────────────────────────────────────────────────────────
// HELPERS
// ───────────────────────────────────────────────────────────────────────────

fn require_authenticated(auth: &AuthContext) -> Result<String, ob_core::Error> {
    if auth.authenticated { Ok(auth.user_id.clone()) } else { Err(ob_core::Error::Auth("Authentication required".into())) }
}

fn escape_surreal_string(s: &str) -> String {
    s.replace('\'', "''")
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
}
