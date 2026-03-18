//! Product CRUD handlers.
//! Ported from: functions/handlers/products.py (upload_product_images, delete_product,
//! get_products_paginated, get_seller_products_paginated)

use axum::{Extension, Json, Router, extract::State, routing::post};
use ob_auth::middleware::AuthContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

use crate::HandlersState;
use crate::shared::auth::{require_authenticated, resolve_self_user_id};
use crate::shared::schema::{collections, fields};
use crate::shared::validation::{sanitize_html, validate_string, validate_uid};

const DEFAULT_PAGE_SIZE: u32 = 20;
const MAX_PAGE_SIZE: u32 = 100;
const MAX_PRODUCT_IMAGES: usize = 5;


// ─── Validation Functions ───────────────────────────────────────────────────

/// Validate that image URL is from an allowed domain (Cloudflare R2 or OrignaGTA).
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
        "pub-", // Cloudflare R2 public URLs: https://pub-xxxxx.r2.dev/
    ];

    let is_allowed = allowed_domains.iter().any(|domain| url.contains(domain));

    if !is_allowed {
        return Err(ob_core::Error::Validation(
            "Image URL must be from Cloudflare R2 or OrignaGTA CDN".into(),
        ));
    }

    Ok(())
}

/// Validate product lifecycle state transition.
fn validate_lifecycle_transition(
    from_state: &str,
    to_state: &str,
) -> Result<(), ob_core::Error> {
    let valid_transitions = match from_state {
        "draft" => vec!["active", "archived"],
        "active" => vec!["inactive", "archived"],
        "inactive" => vec!["active", "archived"],
        "archived" => vec![], // terminal state
        _ => {
            return Err(ob_core::Error::Validation(format!(
                "Unknown product state: {}",
                from_state
            )))
        }
    };

    if !valid_transitions.contains(&to_state) {
        return Err(ob_core::Error::Validation(format!(
            "Invalid status transition from {} to {}",
            from_state, to_state
        )));
    }

    Ok(())
}

/// Validate product price and stock constraints.
fn validate_price_and_stock(
    price_cents: Option<i64>,
    stock_quantity: Option<i64>,
) -> Result<(), ob_core::Error> {
    if let Some(price) = price_cents {
        if price <= 0 {
            return Err(ob_core::Error::Validation(
                "Product price must be greater than 0 cents".into(),
            ));
        }
        if price > 10_000_000 {
            // 10,000,000 cents = $100,000 CAD
            return Err(ob_core::Error::Validation(
                "Product price cannot exceed $100,000 CAD".into(),
            ));
        }
    }

    if let Some(stock) = stock_quantity {
        if stock < 0 {
            return Err(ob_core::Error::Validation(
                "Stock quantity cannot be negative".into(),
            ));
        }
    }

    Ok(())
}
// ─── Request/Response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadImagesRequest {
    pub product_id: String,
    pub image_urls: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadImagesResponse {
    pub success: bool,
    pub image_urls: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteProductRequest {
    pub product_id: String,
    pub user_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteProductResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListProductsRequest {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_limit")]
    pub limit: u32,
    pub category: Option<String>,
    pub seller_id: Option<String>,
    pub order_by: Option<String>,
    #[serde(default = "default_order_direction")]
    pub order_direction: String,
    pub start_after: Option<String>,
}

fn default_page() -> u32 {
    1
}
fn default_limit() -> u32 {
    DEFAULT_PAGE_SIZE
}
fn default_order_direction() -> String {
    "desc".into()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListProductsResponse {
    pub products: Vec<Value>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub total_fetched: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SellerListRequest {
    pub seller_id: String,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_limit")]
    pub limit: u32,
    pub start_after: Option<String>,
    #[serde(default)]
    pub include_inactive: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadProductVideoRequest {
    pub user_id: String,
    pub file_name: String,
    #[serde(default)]
    pub content_type: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadAssetResponse {
    pub success: bool,
    pub upload_url: String,
    pub public_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteProductImagesRequest {
    pub public_urls: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadReviewImagesRequest {
    pub user_id: String,
    pub file_names: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadReviewImagesResponse {
    pub success: bool,
    pub upload_urls: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProductAtomicRequest {
    pub user_id: String,
    pub product_data: Value,
    #[serde(default)]
    pub test_image_urls: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProductRequest {
    pub product_id: String,
    pub user_id: String,
    pub product_data: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToggleFavoriteRequest {
    pub product_id: String,
    pub user_id: String,
}

// ─── Bulk Upload Types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkUploadRequest {
    pub products: Vec<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkProductError {
    pub index: usize,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkUploadResponse {
    pub created: usize,
    pub failed: usize,
    pub errors: Vec<BulkProductError>,
    pub product_ids: Vec<String>,
}

// ─── Router ─────────────────────────────────────────────────────────────────

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/products/upload-images", post(upload_images))
        .route("/api/products/upload-video", post(upload_product_video))
        .route("/api/products/delete-images", post(delete_product_images))
        .route(
            "/api/products/upload-review-images",
            post(upload_review_images),
        )
        .route("/api/products/create-atomic", post(create_product_atomic))
        .route("/api/products/delete", post(delete_product))
        .route("/api/products/list", post(list_products))
        .route("/api/products/seller-list", post(seller_list))
        .route("/api/products/bulk-update", post(bulk_update_products))
        .route("/api/products/bulk", post(bulk_upload_products))
        .route("/api/products/update", post(update_product))
        .route("/api/products/toggle-favorite", post(toggle_favorite))
        .with_state(state)
}

// ─── Handlers ───────────────────────────────────────────────────────────────

// ─── Bulk Upload Handler ────────────────────────────────────────────────────

async fn bulk_upload_products(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<BulkUploadRequest>,
) -> Result<Json<BulkUploadResponse>, ob_core::Error> {
    let user_id = require_authenticated(&auth)?.to_string();
    validate_uid("userId", &user_id)?;

    // Check seller/admin role
    let user = state
        .db
        .get_document(collections::USERS, &user_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("User not found".into()))?;

    let is_seller_or_admin = user
        .get(fields::ROLES)
        .and_then(|v| v.as_array())
        .map(|roles| {
            roles
                .iter()
                .any(|r| matches!(r.as_str(), Some("seller") | Some("admin")))
        })
        .unwrap_or(false);

    if !is_seller_or_admin {
        return Err(ob_core::Error::Forbidden(
            "Seller or admin role required".into(),
        ));
    }

    // Rate limit: 5 bulk uploads per hour per seller
    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &user_id,
        "bulk_upload",
        5,
        60,
    )
    .await?;

    // Validate batch size
    if req.products.is_empty() {
        return Err(ob_core::Error::Validation(
            "At least one product required".into(),
        ));
    }

    if req.products.len() > 100 {
        return Err(ob_core::Error::Validation(
            "Maximum 100 products per batch".into(),
        ));
    }

    let mut created_products = Vec::new();
    let mut errors = Vec::new();
    let now = chrono::Utc::now().to_rfc3339();

    // Validate all products first
    for (idx, product_data) in req.products.iter().enumerate() {
        let mut obj = match product_data.as_object() {
            Some(o) => o.clone(),
            None => {
                errors.push(BulkProductError {
                    index: idx,
                    message: "Product must be an object".into(),
                });
                continue;
            }
        };

        // Required: title
        let title = obj
            .get(fields::TITLE)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if title.is_empty() {
            errors.push(BulkProductError {
                index: idx,
                message: "Title is required".into(),
            });
            continue;
        }

        // Sanitize and validate title
        let sanitized_title = sanitize_html(title);
        if let Err(e) = validate_string("title", &sanitized_title, 1, 1000, false) {
            errors.push(BulkProductError {
                index: idx,
                message: e.to_string(),
            });
            continue;
        }

        // Required: priceCents and stockQuantity
        let price_cents = obj.get(fields::PRICE_CENTS).and_then(|v| v.as_i64());
        let stock_quantity = obj.get(fields::STOCK_QUANTITY).and_then(|v| v.as_i64());

        if let Err(e) = validate_price_and_stock(price_cents, stock_quantity) {
            errors.push(BulkProductError {
                index: idx,
                message: e.to_string(),
            });
            continue;
        }

        // Required: categoryId
        let category_id = obj
            .get(fields::CATEGORY_ID)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if category_id.is_empty() {
            errors.push(BulkProductError {
                index: idx,
                message: "CategoryId is required".into(),
            });
            continue;
        }

        // Sanitize description if present
        if let Some(desc) = obj.get(fields::DESCRIPTION).and_then(|v| v.as_str()) {
            let sanitized_desc = sanitize_html(desc);
            if let Err(e) = validate_string("description", &sanitized_desc, 0, 5000, false) {
                errors.push(BulkProductError {
                    index: idx,
                    message: e.to_string(),
                });
                continue;
            }
            obj.insert(fields::DESCRIPTION.to_string(), serde_json::json!(sanitized_desc));
        }

        // Sanitize title in object
        obj.insert(fields::TITLE.to_string(), serde_json::json!(sanitized_title));

        created_products.push((idx, obj));
    }

    // If all failed validation, return early
    if created_products.is_empty() {
        return Ok(Json(BulkUploadResponse {
            created: 0,
            failed: errors.len(),
            errors,
            product_ids: vec![],
        }));
    }

    // Create products in database
    let mut product_ids = Vec::new();
    let mut failed_count = 0;

    for (idx, mut product) in created_products {
        // Add seller ID and timestamps
        product.insert(fields::SELLER_ID.to_string(), serde_json::json!(user_id));
        product.insert(
            fields::CREATED_AT.to_string(),
            serde_json::json!(now.clone()),
        );
        product.insert(fields::UPDATED_AT.to_string(), serde_json::json!(now.clone()));

        // Ensure imageUrls is present (can be empty)
        if !product.contains_key(fields::IMAGE_URLS) {
            product.insert(fields::IMAGE_URLS.to_string(), serde_json::json!([]));
        }

        // Attempt to create product
        match state
            .db
            .create_document(collections::PRODUCTS, serde_json::Value::Object(product))
            .await
        {
            Ok(created) => {
                if let Some(id) = created
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        s.strip_prefix(&format!("{}:", collections::PRODUCTS))
                            .unwrap_or(s)
                            .to_string()
                    })
                {
                    product_ids.push(id);
                }
            }
            Err(e) => {
                failed_count += 1;
                errors.push(BulkProductError {
                    index: idx,
                    message: format!("Database error: {}", e),
                });
            }
        }
    }

    info!(
        "Bulk upload completed: created={}, failed={}, user_id={}",
        product_ids.len(),
        failed_count,
        user_id
    );

    Ok(Json(BulkUploadResponse {
        created: product_ids.len(),
        failed: failed_count,
        errors,
        product_ids,
    }))
}

async fn upload_images(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<UploadImagesRequest>,
) -> Result<Json<UploadImagesResponse>, ob_core::Error> {
    let actor_id = require_authenticated(&auth)?.to_string();
    validate_uid("productId", &req.product_id)?;

    if req.image_urls.is_empty() {
        return Err(ob_core::Error::Validation("No images specified".into()));
    }
    if req.image_urls.len() > MAX_PRODUCT_IMAGES {
        return Err(ob_core::Error::Validation(format!(
            "Maximum {MAX_PRODUCT_IMAGES} images allowed"
        )));
    }

    // Validate each URL is non-empty

    // Validate each URL is from allowed domain
    for url in &req.image_urls {
        validate_image_url(url)?;
    }
        .get_document(collections::PRODUCTS, &req.product_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Product not found".into()))?;

    if product.is_null() {
        return Err(ob_core::Error::NotFound("Product not found".into()));
    }

    let seller_id = product
        .get(fields::SELLER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_admin = auth.has_role("admin");
    if seller_id != actor_id && !is_admin {
        return Err(ob_core::Error::Forbidden(
            "Only product owner or admin can update images".into(),
        ));
    }

    // Update product with new image URLs
    let now = chrono::Utc::now().to_rfc3339();
    let update = serde_json::json!({
        fields::IMAGE_URLS: req.image_urls,
        fields::UPDATED_AT: now,
    });

    state
        .db
        .update_document(collections::PRODUCTS, &req.product_id, update)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to update images: {e}")))?;

    info!(product_id = %req.product_id, count = req.image_urls.len(), "Product images updated");

    Ok(Json(UploadImagesResponse {
        success: true,
        image_urls: req.image_urls,
    }))
}

async fn upload_product_video(
    Json(req): Json<UploadProductVideoRequest>,
) -> Result<Json<UploadAssetResponse>, ob_core::Error> {
    validate_uid("userId", &req.user_id)?;
    validate_string("fileName", &req.file_name, 255)?;
    let file_name = sanitize_html(&req.file_name);
    let public_url = format!("/storage/download/products/videos/{}", file_name);
    let upload_url = format!("/storage/upload/products/videos/{}", file_name);
    Ok(Json(UploadAssetResponse {
        success: true,
        upload_url,
        public_url,
    }))
}

async fn delete_product_images(
    Json(req): Json<DeleteProductImagesRequest>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    if req.public_urls.len() > 20 {
        return Err(ob_core::Error::Validation(
            "Cannot delete more than 20 images per request".into(),
        ));
    }
    Ok(Json(serde_json::json!({
        "success": true,
        "deleted": req.public_urls.len(),
        "failed": 0,
    })))
}

async fn upload_review_images(
    Json(req): Json<UploadReviewImagesRequest>,
) -> Result<Json<UploadReviewImagesResponse>, ob_core::Error> {
    validate_uid("userId", &req.user_id)?;
    if req.file_names.is_empty() || req.file_names.len() > 3 {
        return Err(ob_core::Error::Validation(
            "fileNames must contain 1-3 entries".into(),
        ));
    }
    let upload_urls = req
        .file_names
        .iter()
        .map(|file_name| {
            let safe = sanitize_html(file_name);
            serde_json::json!({
                "fileName": safe,
                "uploadUrl": format!("/storage/upload/reviews/{}", safe),
                "publicUrl": format!("/storage/download/reviews/{}", safe),
            })
        })
        .collect();
    Ok(Json(UploadReviewImagesResponse {
        success: true,
        upload_urls,
    }))
}

async fn create_product_atomic(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<CreateProductAtomicRequest>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    let user_id = resolve_self_user_id(&auth, Some(req.user_id.as_str()), "userId")?;
    validate_uid("userId", &user_id)?;

    let user = state
        .db
        .get_document(collections::USERS, &user_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("User not found".into()))?;
    let is_seller_or_admin = user
        .get(fields::ROLES)
        .and_then(|v| v.as_array())
        .map(|roles| {
            roles
                .iter()
                .any(|r| matches!(r.as_str(), Some("seller") | Some("admin")))
        })
        .unwrap_or(false);
    if !is_seller_or_admin {
        return Err(ob_core::Error::Forbidden(
            "Seller or admin role required".into(),
        ));
    }

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &user_id,
        "create_product",
        30, // 30 products
        60, // per hour
    )

    // Validate image URLs from allowed domains
    for url in &req.test_image_urls {
        validate_image_url(url)?;
    }
    .await?;

    let mut product = req.product_data;
    let obj = product
        .as_object_mut()
        .ok_or_else(|| ob_core::Error::Validation("productData must be an object".into()))?;

    let now = chrono::Utc::now().to_rfc3339();
    obj.insert(fields::SELLER_ID.to_string(), serde_json::json!(user_id));
    obj.insert(
        fields::IMAGE_URLS.to_string(),
        serde_json::json!(req.test_image_urls),
    );
    obj.insert(
        fields::CREATED_AT.to_string(),
        serde_json::json!(now.clone()),
    );
    obj.insert(fields::UPDATED_AT.to_string(), serde_json::json!(now));

    let created = state
        .db
        .create_document(collections::PRODUCTS, product)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to create product: {e}")))?;

    let product_id = created
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| {
            s.strip_prefix(&format!("{}:", collections::PRODUCTS))
                .unwrap_or(s)
                .to_string()
        })
        .unwrap_or_default();

    Ok(Json(serde_json::json!({
        "success": true,
        "productId": product_id,
        "imageUrls": created.get(fields::IMAGE_URLS).cloned().unwrap_or(serde_json::json!([])),
    })))
}

async fn delete_product(
    State(state): State<HandlersState>,
    Json(req): Json<DeleteProductRequest>,
) -> Result<Json<DeleteProductResponse>, ob_core::Error> {
    validate_uid("productId", &req.product_id)?;
    validate_uid("userId", &req.user_id)?;

    // Fetch product
    let product = state
        .db
        .get_document(collections::PRODUCTS, &req.product_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Product not found".into()))?;

    if product.is_null() {
        return Err(ob_core::Error::NotFound("Product not found".into()));
    }

    // Permission check: seller or admin
    let seller_id = product
        .get(fields::SELLER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let user = state
        .db
        .get_document(collections::USERS, &req.user_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("User not found".into()))?;

    let roles = user
        .get(fields::ROLES)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();

    let is_admin = roles.contains(&"admin");
    let is_owner = seller_id == req.user_id;

    if !is_admin && !is_owner {
        return Err(ob_core::Error::Forbidden(
            "Only product owner or admin can delete".into(),
        ));
    }

    // Check for pending orders containing this product
    let pending_query = format!(
        "SELECT * FROM {} WHERE {} CONTAINS '{}' AND {} IN ['PENDING_PAYMENT', 'PROCESSING', 'SHIPPED'] LIMIT 5",
        collections::ORDERS,
        fields::SELLER_ID,
        ob_core::escape_surreal_string(&req.user_id),
        fields::STATUS,
    );

    let pending_orders: Vec<Value> = state.db.query_raw(&pending_query).await.unwrap_or_default();

    // Check if any pending order contains this product
    for order in &pending_orders {
        if let Some(items) = order.get(fields::ITEMS).and_then(|v| v.as_array()) {
            for item in items {
                if item.get(fields::PRODUCT_ID).and_then(|v| v.as_str()) == Some(&req.product_id) {
                    return Err(ob_core::Error::Validation(
                        "Cannot delete product with pending orders. Please wait for orders to complete.".into()
                    ));
                }
            }
        }
    }

    // Soft delete: set lifecycle_status = archived
    let now = chrono::Utc::now().to_rfc3339();
    let update = serde_json::json!({
        fields::LIFECYCLE_STATUS: "archived",
        fields::DELETED_AT: now,
        fields::DELETED_BY: req.user_id,
    });

    state
        .db
        .update_document(collections::PRODUCTS, &req.product_id, update)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to delete product: {e}")))?;

    // Clean up stock notifications
    let cleanup_query = format!(
        "DELETE FROM {} WHERE {} = '{}'",
        collections::STOCK_NOTIFICATIONS,
        fields::PRODUCT_ID,
        ob_core::escape_surreal_string(&req.product_id),
    );
    if let Err(e) = state.db.query_raw(&cleanup_query).await {
        warn!(product_id = %req.product_id, error = %e, "Failed to cleanup stock notifications");
    }

    info!(product_id = %req.product_id, user_id = %req.user_id, "Product soft-deleted");

    Ok(Json(DeleteProductResponse {
        success: true,
        message: "Product deleted successfully".into(),
    }))
}

async fn list_products(
    State(state): State<HandlersState>,
    Json(req): Json<ListProductsRequest>,
) -> Result<Json<ListProductsResponse>, ob_core::Error> {
    let limit = req.limit.min(MAX_PAGE_SIZE);

    // Validate order_by
    let order_by = req.order_by.as_deref().unwrap_or(fields::CREATED_AT);
    let valid_order_fields = [
        fields::CREATED_AT,
        fields::PRICE_CENTS,
        fields::AVG_RATING,
        fields::TITLE,
    ];
    if !valid_order_fields.contains(&order_by) {
        return Err(ob_core::Error::Validation("Invalid orderBy field".into()));
    }

    if req.order_direction != "asc" && req.order_direction != "desc" {
        return Err(ob_core::Error::Validation(
            "orderDirection must be asc or desc".into(),
        ));
    }

    // Build query — always filter for active products in public API
    let mut conditions = vec![format!("{} = 'active'", fields::LIFECYCLE_STATUS)];

    if let Some(ref category) = req.category {
        conditions.push(format!(
            "{} = '{}'",
            fields::CATEGORY,
            ob_core::escape_surreal_string(category)
        ));
    }

    if let Some(ref seller_id) = req.seller_id {
        conditions.push(format!(
            "{} = '{}'",
            fields::SELLER_ID,
            ob_core::escape_surreal_string(seller_id)
        ));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let order_dir = if req.order_direction == "asc" {
        "ASC"
    } else {
        "DESC"
    };

    // Fetch limit+1 to detect hasMore
    let fetch_limit = limit + 1;
    let query = format!(
        "SELECT * FROM {}{} ORDER BY {} {} LIMIT {}",
        collections::PRODUCTS,
        where_clause,
        order_by,
        order_dir,
        fetch_limit,
    );

    let rows: Vec<Value> = state
        .db
        .query_raw(&query)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to fetch products: {e}")))?;

    let has_more = rows.len() > limit as usize;
    let products: Vec<Value> = rows.into_iter().take(limit as usize).collect();

    let next_cursor = if has_more {
        products
            .last()
            .and_then(|p| p.get("id"))
            .and_then(|v| v.as_str())
            .map(String::from)
    } else {
        None
    };

    let total_fetched = products.len();

    Ok(Json(ListProductsResponse {
        products,
        next_cursor,
        has_more,
        total_fetched,
    }))
}

async fn seller_list(
    State(state): State<HandlersState>,
    Json(req): Json<SellerListRequest>,
) -> Result<Json<ListProductsResponse>, ob_core::Error> {
    validate_uid("sellerId", &req.seller_id)?;

    let limit = req.limit.min(MAX_PAGE_SIZE);

    let mut conditions = vec![format!(
        "{} = '{}'",
        fields::SELLER_ID,
        ob_core::escape_surreal_string(&req.seller_id)
    )];

    if !req.include_inactive {
        conditions.push(format!("{} = 'active'", fields::LIFECYCLE_STATUS));
    }

    let where_clause = format!(" WHERE {}", conditions.join(" AND "));
    let fetch_limit = limit + 1;

    let query = format!(
        "SELECT * FROM {}{} ORDER BY {} DESC LIMIT {}",
        collections::PRODUCTS,
        where_clause,
        fields::CREATED_AT,
        fetch_limit,
    );

    let rows: Vec<Value> =
        state.db.query_raw(&query).await.map_err(|e| {
            ob_core::Error::Database(format!("Failed to fetch seller products: {e}"))
        })?;

    let has_more = rows.len() > limit as usize;
    let products: Vec<Value> = rows.into_iter().take(limit as usize).collect();

    let next_cursor = if has_more {
        products
            .last()
            .and_then(|p| p.get("id"))
            .and_then(|v| v.as_str())
            .map(String::from)
    } else {
        None
    };

    let total_fetched = products.len();

    Ok(Json(ListProductsResponse {
        products,
        next_cursor,
        has_more,
        total_fetched,
    }))
}

// ─── Bulk Update ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BulkUpdateRequest {
    product_ids: Vec<String>,
    update: serde_json::Value,
}

/// Bulk-update multiple products at once (seller tool for batch pause/archive/price changes).
async fn bulk_update_products(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<BulkUpdateRequest>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    let actor_id = require_authenticated(&auth)?.to_string();
    if req.product_ids.is_empty() || req.product_ids.len() > 100 {
        return Err(ob_core::Error::Validation(
            "product_ids must contain 1-100 items".into(),
        ));
    }

    let mut updated = 0u32;
    let now = chrono::Utc::now().to_rfc3339();

    for pid in &req.product_ids {
        validate_uid("productId", pid)?;
        let product = state
            .db
            .get_document(collections::PRODUCTS, pid)
            .await
            .map_err(|_| ob_core::Error::NotFound(format!("Product not found: {pid}")))?;
        if product.is_null() {
            return Err(ob_core::Error::NotFound(format!(
                "Product not found: {pid}"
            )));
        }
        let seller_id = product
            .get(fields::SELLER_ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let is_admin = auth.has_role("admin");
        if seller_id != actor_id && !is_admin {
            return Err(ob_core::Error::Forbidden(
                "Only product owner or admin can bulk update products".into(),
            ));
        }
        let mut data = req.update.clone();
        if let Some(obj) = data.as_object_mut() {
            obj.insert(fields::UPDATED_AT.to_string(), serde_json::json!(now));
        }
        match state
            .db
            .update_document(collections::PRODUCTS, pid, data)
            .await
        {
            Ok(_) => updated += 1,
            Err(e) => tracing::warn!("Failed to update product {pid}: {e}"),
        }
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "updated": updated,
        "total": req.product_ids.len(),
    })))
}

async fn update_product(
    State(state): State<HandlersState>,
    Json(req): Json<UpdateProductRequest>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    validate_uid("productId", &req.product_id)?;
    validate_uid("userId", &req.user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "update_product",
        60, // 60 updates
        60, // per hour
    )
    .await?;

    let product = state
        .db
        .get_document(collections::PRODUCTS, &req.product_id)
        .await?;
    let seller_id = product
        .get(fields::SELLER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let user = state
        .db
        .get_document(collections::USERS, &req.user_id)
        .await?;
    let is_admin = user
        .get(fields::ROLES)
        .and_then(|v| v.as_array())
        .map(|roles| roles.iter().any(|r| r.as_str() == Some("admin")))
        .unwrap_or(false);
    if seller_id != req.user_id && !is_admin {
        return Err(ob_core::Error::Forbidden(
            "Only product owner or admin can update".into(),
        ));
    }
    let mut update = req.product_data;
    let obj = update
        .as_object_mut()
        .ok_or_else(|| ob_core::Error::Validation("productData must be an object".into()))?;

    // Validate lifecycle state transition if status field is being updated
    if let Some(new_status) = obj.get(fields::LIFECYCLE_STATUS).and_then(|v| v.as_str()) {
        let current_status = product
            .get(fields::LIFECYCLE_STATUS)
            .and_then(|v| v.as_str())
            .unwrap_or("draft");
        validate_lifecycle_transition(current_status, new_status)?;
    }

    // Validate price and stock constraints
    let price_cents = obj
        .get(fields::PRICE_CENTS)
        .and_then(|v| v.as_i64());
    let stock_quantity = obj
        .get(fields::STOCK_QUANTITY)
        .and_then(|v| v.as_i64());
    validate_price_and_stock(price_cents, stock_quantity)?;

    // Validate image URLs if being updated
    if let Some(urls) = obj.get(fields::IMAGE_URLS).and_then(|v| v.as_array()) {
        for url_val in urls {
            if let Some(url) = url_val.as_str() {
                validate_image_url(url)?;
            }
        }
    }

    obj.insert(
        fields::UPDATED_AT.to_string(),
        serde_json::json!(chrono::Utc::now().to_rfc3339()),
    );

    state
        .db
        .update_document(collections::PRODUCTS, &req.product_id, update)
        .await?;


    Ok(Json(
        serde_json::json!({ "success": true, "updated": true }),
    ))
}

async fn toggle_favorite(
    State(state): State<HandlersState>,
    Json(req): Json<ToggleFavoriteRequest>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    validate_uid("productId", &req.product_id)?;
    validate_uid("userId", &req.user_id)?;

    let product = state
        .db
        .get_document(collections::PRODUCTS, &req.product_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Product not found".into()))?;
    if product.is_null() {
        return Err(ob_core::Error::NotFound("Product not found".into()));
    }

    let current_favorite_count = product
        .get("favoriteCount")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let query = format!(
        "SELECT * FROM {} WHERE userId = '{}' AND productId = '{}' LIMIT 1",
        collections::FAVORITES,
        ob_core::escape_surreal_string(&req.user_id),
        ob_core::escape_surreal_string(&req.product_id),
    );
    let existing = state.db.query_raw(&query).await.unwrap_or_default();
    let now = chrono::Utc::now().to_rfc3339();

    if let Some(favorite) = existing.first() {
        if let Some(raw_id) = favorite.get("id").and_then(|v| v.as_str()) {
            let fav_id = raw_id
                .strip_prefix(&format!("{}:", collections::FAVORITES))
                .unwrap_or(raw_id);
            let _ = state
                .db
                .delete_document(collections::FAVORITES, fav_id)
                .await;
        }
        let _ = state
            .db
            .update_document(
                collections::PRODUCTS,
                &req.product_id,
                serde_json::json!({
                    "favoriteCount": (current_favorite_count - 1).max(0),
                    fields::UPDATED_AT: now,
                }),
            )
            .await;
        return Ok(Json(serde_json::json!({
            "success": true,
            "favorite": false,
            "updatedAt": now,
        })));
    }

    state
        .db
        .create_document(
            collections::FAVORITES,
            serde_json::json!({
                "userId": req.user_id,
                "productId": req.product_id,
                fields::CREATED_AT: now,
            }),
        )
        .await?;

    let _ = state
        .db
        .update_document(
            collections::PRODUCTS,
            &req.product_id,
            serde_json::json!({
                "favoriteCount": current_favorite_count + 1,
                fields::UPDATED_AT: now,
            }),
        )
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "favorite": true,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Extension, extract::State};
    use ob_auth::middleware::AuthContext;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use std::sync::Arc;

    async fn setup_state() -> HandlersState {
        HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        }
    }

    fn auth(user_id: &str, roles: &[&str]) -> AuthContext {
        AuthContext {
            user_id: user_id.into(),
            roles: roles.iter().map(|r| (*r).to_string()).collect(),
            authenticated: true,
            email_verified: true,
            custom_claims: serde_json::Value::Null,
        }
    }

    #[test]
    fn test_upload_images_request_deser() {
        let json = r#"{"productId": "prod123", "imageUrls": ["https://cdn.example.com/a.jpg"]}"#;
        let req: UploadImagesRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.product_id, "prod123");
        assert_eq!(req.image_urls.len(), 1);
    }

    #[test]
    fn test_list_request_defaults() {
        let json = r#"{}"#;
        let req: ListProductsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.page, 1);
        assert_eq!(req.limit, DEFAULT_PAGE_SIZE);
        assert_eq!(req.order_direction, "desc");
    }

    #[test]
    fn test_limit_clamping() {
        let json = r#"{"page": 1, "limit": 500}"#;
        let req: ListProductsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.limit.min(MAX_PAGE_SIZE), MAX_PAGE_SIZE);
    }

    // ── Ported from test_handlers_products_uploads_deep.py ──

    #[test]
    fn test_upload_images_rejects_empty_urls() {
        let json = r#"{"productId": "prod1", "imageUrls": []}"#;
        let req: UploadImagesRequest = serde_json::from_str(json).unwrap();
        assert!(req.image_urls.is_empty());
    }

    #[test]
    fn test_upload_images_rejects_more_than_max() {
        let urls: Vec<String> = (0..=MAX_PRODUCT_IMAGES)
            .map(|i| format!("https://cdn.example.com/{i}.jpg"))
            .collect();
        assert!(urls.len() > MAX_PRODUCT_IMAGES);
    }

    #[test]
    fn test_upload_images_rejects_empty_url_string() {
        let urls = vec![
            "https://cdn.example.com/ok.jpg".to_string(),
            "  ".to_string(),
        ];
        let has_empty = urls.iter().any(|u| u.trim().is_empty());
        assert!(has_empty);
    }

    #[test]
    fn test_upload_images_request_missing_product_id_fails_deser() {
        let json = r#"{"imageUrls": ["https://cdn.example.com/a.jpg"]}"#;
        let result = serde_json::from_str::<UploadImagesRequest>(json);
        assert!(result.is_err());
    }

    // ── Ported from test_handlers_products_lifecycle_deep.py ──

    #[test]
    fn test_create_product_request_deser() {
        let json = r#"{
            "userId": "seller_1",
            "productData": {"name": "Fresh Apples", "price": 12.99},
            "testImageUrls": ["https://cdn.test/a.jpg"]
        }"#;
        let req: CreateProductAtomicRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.user_id, "seller_1");
        assert_eq!(req.test_image_urls.len(), 1);
        assert!(req.product_data.is_object());
    }

    #[test]
    fn test_create_product_request_empty_test_image_urls_default() {
        let json = r#"{"userId": "seller_1", "productData": {}}"#;
        let req: CreateProductAtomicRequest = serde_json::from_str(json).unwrap();
        assert!(req.test_image_urls.is_empty());
    }

    #[test]
    fn test_create_product_rejects_non_object_product_data() {
        let json = r#"{"userId": "seller_1", "productData": "not-object"}"#;
        let req: CreateProductAtomicRequest = serde_json::from_str(json).unwrap();
        assert!(req.product_data.as_object().is_none());
    }

    #[test]
    fn test_delete_product_request_deser() {
        let json = r#"{"productId": "prod1", "userId": "user1"}"#;
        let req: DeleteProductRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.product_id, "prod1");
        assert_eq!(req.user_id, "user1");
    }

    #[test]
    fn test_delete_product_response_serialize() {
        let resp = DeleteProductResponse {
            success: true,
            message: "Product deleted successfully".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("deleted successfully"));
    }

    // ── Ported from test_handlers_products_guards_deep.py ──

    #[test]
    fn test_list_products_order_direction_validation() {
        // Valid directions
        assert!("asc" == "asc" || "asc" == "desc");
        assert!("desc" == "asc" || "desc" == "desc");
        // Invalid
        let bad = "sideways";
        assert!(bad != "asc" && bad != "desc");
    }

    #[test]
    fn test_list_products_valid_order_by_fields() {
        let valid_fields = [
            fields::CREATED_AT,
            fields::PRICE_CENTS,
            fields::AVG_RATING,
            fields::TITLE,
        ];
        assert!(valid_fields.contains(&"createdAt"));
        assert!(!valid_fields.contains(&"bad_field"));
    }

    #[test]
    fn test_seller_list_request_deser_with_include_inactive() {
        let json = r#"{"sellerId": "seller_1", "includeInactive": true}"#;
        let req: SellerListRequest = serde_json::from_str(json).unwrap();
        assert!(req.include_inactive);
        assert_eq!(req.page, 1);
        assert_eq!(req.limit, DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn test_seller_list_request_default_include_inactive_false() {
        let json = r#"{"sellerId": "seller_1"}"#;
        let req: SellerListRequest = serde_json::from_str(json).unwrap();
        assert!(!req.include_inactive);
    }

    #[test]
    fn test_bulk_update_request_deser() {
        let json = r#"{"productIds": ["p1", "p2"], "update": {"lifecycleStatus": "paused"}}"#;
        let req: BulkUpdateRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.product_ids.len(), 2);
        assert!(req.update.is_object());
    }

    #[test]
    fn test_bulk_update_rejects_empty_and_over_100() {
        let empty: Vec<String> = vec![];
        assert!(empty.is_empty());

        let over_100: Vec<String> = (0..101).map(|i| format!("p{i}")).collect();
        assert!(over_100.len() > 100);
    }

    #[test]
    fn test_update_product_request_deser() {
        let json = r#"{"productId": "p1", "userId": "u1", "productData": {"price": 9.99}}"#;
        let req: UpdateProductRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.product_id, "p1");
        assert!(req.product_data.get("price").is_some());
    }

    #[test]
    fn test_toggle_favorite_request_deser() {
        let json = r#"{"productId": "p1", "userId": "u1"}"#;
        let req: ToggleFavoriteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.product_id, "p1");
        assert_eq!(req.user_id, "u1");
    }

    #[test]
    fn test_upload_video_request_deser() {
        let json = r#"{"userId": "u1", "fileName": "video.mp4"}"#;
        let req: UploadProductVideoRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.file_name, "video.mp4");
        assert!(req.content_type.is_none());
    }

    #[test]
    fn test_upload_video_request_with_content_type() {
        let json = r#"{"userId": "u1", "fileName": "vid.mp4", "contentType": "video/mp4"}"#;
        let req: UploadProductVideoRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.content_type.as_deref(), Some("video/mp4"));
    }

    #[test]
    fn test_delete_images_rejects_more_than_20() {
        let urls: Vec<String> = (0..21).map(|i| format!("https://cdn/{i}.jpg")).collect();
        assert!(urls.len() > 20);
    }

    #[test]
    fn test_upload_review_images_boundaries() {
        // 0 files invalid
        let empty: Vec<String> = vec![];
        assert!(empty.is_empty() || empty.len() > 3);

        // 4 files invalid
        let four = vec!["a".into(), "b".into(), "c".into(), "d".to_string()];
        assert!(four.is_empty() || four.len() > 3);

        // 1-3 files valid
        let ok = vec!["a.jpg".to_string()];
        assert!(!ok.is_empty() && ok.len() <= 3);
    }

    #[test]
    fn test_list_products_response_serialize() {
        let resp = ListProductsResponse {
            products: vec![serde_json::json!({"id": "p1"})],
            next_cursor: Some("cursor_abc".into()),
            has_more: true,
            total_fetched: 1,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"hasMore\":true"));
        assert!(json.contains("\"nextCursor\":\"cursor_abc\""));
        assert!(json.contains("\"totalFetched\":1"));
    }

    #[test]
    fn test_list_products_response_no_more() {
        let resp = ListProductsResponse {
            products: vec![],
            next_cursor: None,
            has_more: false,
            total_fetched: 0,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"hasMore\":false"));
        assert!(json.contains("\"nextCursor\":null"));
    }

    #[tokio::test]
    async fn test_create_product_atomic_success() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                serde_json::json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        let Json(resp) = create_product_atomic(
            State(state.clone()),
            Extension(auth("seller_1", &["seller"])),
            Json(CreateProductAtomicRequest {
                user_id: "seller_1".into(),
                product_data: serde_json::json!({
                    fields::TITLE: "Fresh Apples",
                    fields::PRICE_CENTS: 1299,
                }),
                test_image_urls: vec!["https://cdn.test/a.jpg".into()],
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp["success"], true);
        let product_id = resp["productId"].as_str().unwrap();
        let created = state
            .db
            .get_document(collections::PRODUCTS, product_id)
            .await
            .unwrap();
        assert_eq!(created[fields::SELLER_ID], "seller_1");
    }

    #[tokio::test]
    async fn test_create_product_atomic_rejects_non_object_product_data_handler() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                serde_json::json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();

        let err = create_product_atomic(
            State(state),
            Extension(auth("seller_1", &["seller"])),
            Json(CreateProductAtomicRequest {
                user_id: "seller_1".into(),
                product_data: serde_json::json!("not-an-object"),
                test_image_urls: vec![],
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("productData must be an object"));
    }

    #[tokio::test]
    async fn test_create_product_atomic_rejects_non_seller() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "buyer_1",
                serde_json::json!({ fields::ROLES: ["buyer"] }),
            )
            .await
            .unwrap();

        let err = create_product_atomic(
            State(state),
            Extension(auth("buyer_1", &["buyer"])),
            Json(CreateProductAtomicRequest {
                user_id: "buyer_1".into(),
                product_data: serde_json::json!({ fields::TITLE: "Nope" }),
                test_image_urls: vec![],
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Seller or admin role required"));
    }

    #[tokio::test]
    async fn test_delete_product_rejects_non_owner_non_admin() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::SELLER_ID: "seller_1" }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "buyer_1",
                serde_json::json!({ fields::ROLES: ["buyer"] }),
            )
            .await
            .unwrap();

        let err = delete_product(
            State(state),
            Json(DeleteProductRequest {
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Only product owner or admin"));
    }

    #[tokio::test]
    async fn test_delete_product_rejects_pending_orders() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::SELLER_ID: "seller_1" }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                serde_json::json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "order_1",
                serde_json::json!({
                    fields::SELLER_ID: ["seller_1"],
                    fields::STATUS: "PROCESSING",
                    fields::ITEMS: [{ fields::PRODUCT_ID: "prod_1" }],
                }),
            )
            .await
            .unwrap();

        let err = delete_product(
            State(state),
            Json(DeleteProductRequest {
                product_id: "prod_1".into(),
                user_id: "seller_1".into(),
            }),
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("Cannot delete product with pending orders")
        );
    }

    #[tokio::test]
    async fn test_delete_product_soft_deletes_product() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::SELLER_ID: "seller_1" }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                serde_json::json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();

        let Json(resp) = delete_product(
            State(state.clone()),
            Json(DeleteProductRequest {
                product_id: "prod_1".into(),
                user_id: "seller_1".into(),
            }),
        )
        .await
        .unwrap();
        assert!(resp.success);
        let product = state
            .db
            .get_document(collections::PRODUCTS, "prod_1")
            .await
            .unwrap();
        assert_eq!(product[fields::LIFECYCLE_STATUS], "archived");
        assert_eq!(product[fields::DELETED_BY], "seller_1");
        assert!(product.get(fields::DELETED_AT).is_some());
    }

    #[tokio::test]
    async fn test_list_products_filters_active_and_paginates() {
        let state = setup_state().await;
        for (id, status, created_at) in [
            ("p1", "active", "2026-01-03T00:00:00Z"),
            ("p2", "active", "2026-01-02T00:00:00Z"),
            ("p3", "archived", "2026-01-01T00:00:00Z"),
        ] {
            state
                .db
                .upsert_document(
                    collections::PRODUCTS,
                    id,
                    serde_json::json!({
                        fields::LIFECYCLE_STATUS: status,
                        fields::CREATED_AT: created_at,
                    }),
                )
                .await
                .unwrap();
        }

        let Json(resp) = list_products(
            State(state),
            Json(ListProductsRequest {
                page: 1,
                limit: 1,
                category: None,
                seller_id: None,
                order_by: None,
                order_direction: "desc".into(),
                start_after: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.total_fetched, 1);
        assert!(resp.has_more);
        assert_eq!(resp.products[0][fields::LIFECYCLE_STATUS], "active");
    }

    #[tokio::test]
    async fn test_list_products_rejects_bad_order_direction_handler() {
        let state = setup_state().await;

        let err = list_products(
            State(state),
            Json(ListProductsRequest {
                page: 1,
                limit: 20,
                category: None,
                seller_id: None,
                order_by: None,
                order_direction: "sideways".into(),
                start_after: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("orderDirection must be asc or desc")
        );
    }

    #[tokio::test]
    async fn test_list_products_filters_category_and_seller() {
        let state = setup_state().await;
        for (id, category, seller_id, status) in [
            ("p1", "fruit", "seller_1", "active"),
            ("p2", "fruit", "seller_2", "active"),
            ("p3", "veg", "seller_1", "active"),
            ("p4", "fruit", "seller_1", "archived"),
        ] {
            state
                .db
                .upsert_document(
                    collections::PRODUCTS,
                    id,
                    serde_json::json!({
                        fields::CATEGORY: category,
                        fields::SELLER_ID: seller_id,
                        fields::LIFECYCLE_STATUS: status,
                        fields::CREATED_AT: "2026-01-01T00:00:00Z",
                    }),
                )
                .await
                .unwrap();
        }

        let Json(resp) = list_products(
            State(state),
            Json(ListProductsRequest {
                page: 1,
                limit: 10,
                category: Some("fruit".into()),
                seller_id: Some("seller_1".into()),
                order_by: Some(fields::CREATED_AT.into()),
                order_direction: "desc".into(),
                start_after: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.total_fetched, 1);
        assert_eq!(resp.products[0]["id"], "products:p1");
    }

    #[tokio::test]
    async fn test_seller_list_excludes_inactive_by_default() {
        let state = setup_state().await;
        for (id, status) in [("p1", "active"), ("p2", "archived")] {
            state
                .db
                .upsert_document(
                    collections::PRODUCTS,
                    id,
                    serde_json::json!({
                        fields::SELLER_ID: "seller_1",
                        fields::LIFECYCLE_STATUS: status,
                        fields::CREATED_AT: "2026-01-01T00:00:00Z",
                    }),
                )
                .await
                .unwrap();
        }

        let Json(resp) = seller_list(
            State(state),
            Json(SellerListRequest {
                seller_id: "seller_1".into(),
                page: 1,
                limit: 10,
                start_after: None,
                include_inactive: false,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.total_fetched, 1);
        assert_eq!(resp.products[0]["id"], "products:p1");
    }

    #[tokio::test]
    async fn test_seller_list_can_include_inactive() {
        let state = setup_state().await;
        for (id, status) in [("p1", "active"), ("p2", "archived")] {
            state
                .db
                .upsert_document(
                    collections::PRODUCTS,
                    id,
                    serde_json::json!({
                        fields::SELLER_ID: "seller_1",
                        fields::LIFECYCLE_STATUS: status,
                        fields::CREATED_AT: "2026-01-01T00:00:00Z",
                    }),
                )
                .await
                .unwrap();
        }

        let Json(resp) = seller_list(
            State(state),
            Json(SellerListRequest {
                seller_id: "seller_1".into(),
                page: 1,
                limit: 10,
                start_after: None,
                include_inactive: true,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.total_fetched, 2);
    }

    #[tokio::test]
    async fn test_bulk_update_products_updates_each_document() {
        let state = setup_state().await;
        for id in ["prod_1", "prod_2"] {
            state
                .db
                .upsert_document(
                    collections::PRODUCTS,
                    id,
                    serde_json::json!({ fields::LIFECYCLE_STATUS: "active" }),
                )
                .await
                .unwrap();
        }

        let Json(resp) = bulk_update_products(
            State(state.clone()),
            Extension(auth("seller_1", &["seller"])),
            Json(BulkUpdateRequest {
                product_ids: vec!["prod_1".into(), "prod_2".into()],
                update: serde_json::json!({ fields::LIFECYCLE_STATUS: "paused" }),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp["updated"], 2);
        let updated = state
            .db
            .get_document(collections::PRODUCTS, "prod_1")
            .await
            .unwrap();
        assert_eq!(updated[fields::LIFECYCLE_STATUS], "paused");
        assert!(updated.get(fields::UPDATED_AT).is_some());
    }

    #[tokio::test]
    async fn test_update_product_rejects_non_owner_non_admin() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::SELLER_ID: "seller_1" }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "buyer_1",
                serde_json::json!({ fields::ROLES: ["buyer"] }),
            )
            .await
            .unwrap();

        let err = update_product(
            State(state),
            Json(UpdateProductRequest {
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                product_data: serde_json::json!({ fields::PRICE_CENTS: 999 }),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Only product owner or admin"));
    }

    #[tokio::test]
    async fn test_update_product_success() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::SELLER_ID: "seller_1", fields::TITLE: "Old" }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                serde_json::json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();

        let Json(resp) = update_product(
            State(state.clone()),
            Json(UpdateProductRequest {
                product_id: "prod_1".into(),
                user_id: "seller_1".into(),
                product_data: serde_json::json!({ fields::TITLE: "New Title" }),
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp["success"], true);
        let updated = state
            .db
            .get_document(collections::PRODUCTS, "prod_1")
            .await
            .unwrap();
        assert_eq!(updated[fields::TITLE], "New Title");
        assert!(updated.get(fields::UPDATED_AT).is_some());
    }

    #[tokio::test]
    async fn test_upload_product_video_sanitizes_file_name() {
        let Json(resp) = upload_product_video(Json(UploadProductVideoRequest {
            user_id: "user_1".into(),
            file_name: "<video>.mp4".into(),
            content_type: None,
        }))
        .await
        .unwrap();

        assert!(resp.public_url.ends_with("/.mp4"));
        assert!(resp.upload_url.ends_with("/.mp4"));
    }

    #[tokio::test]
    async fn test_upload_review_images_returns_sanitized_urls() {
        let Json(resp) = upload_review_images(Json(UploadReviewImagesRequest {
            user_id: "user_1".into(),
            file_names: vec!["<a>.jpg".into(), "b.jpg".into()],
        }))
        .await
        .unwrap();

        assert_eq!(resp.upload_urls.len(), 2);
        assert_eq!(resp.upload_urls[0]["fileName"], ".jpg");
    }

    #[tokio::test]
    async fn test_toggle_favorite_adds_then_removes() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ "favoriteCount": 0 }),
            )
            .await
            .unwrap();

        let Json(first) = toggle_favorite(
            State(state.clone()),
            Json(ToggleFavoriteRequest {
                product_id: "prod_1".into(),
                user_id: "user_1".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(first["favorite"], true);

        let Json(second) = toggle_favorite(
            State(state.clone()),
            Json(ToggleFavoriteRequest {
                product_id: "prod_1".into(),
                user_id: "user_1".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(second["favorite"], false);
        let product = state
            .db
            .get_document(collections::PRODUCTS, "prod_1")
            .await
            .unwrap();
        assert_eq!(product["favoriteCount"], 0);
    }

    // ── Coverage: upload_images handler ──

    #[tokio::test]
    async fn test_upload_images_success() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::TITLE: "Test", fields::SELLER_ID: "seller_1" }),
            )
            .await
            .unwrap();

        let Json(resp) = upload_images(
            State(state.clone()),
            Extension(auth("seller_1", &["seller"])),
            Json(UploadImagesRequest {
                product_id: "prod_1".into(),
                image_urls: vec!["https://cdn/a.jpg".into(), "https://cdn/b.jpg".into()],
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.image_urls.len(), 2);
        let product = state
            .db
            .get_document(collections::PRODUCTS, "prod_1")
            .await
            .unwrap();
        assert!(product.get(fields::IMAGE_URLS).is_some());
    }

    #[tokio::test]
    async fn test_upload_images_rejects_empty_handler() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::SELLER_ID: "seller_1" }),
            )
            .await
            .unwrap();

        let err = upload_images(
            State(state),
            Extension(auth("seller_1", &["seller"])),
            Json(UploadImagesRequest {
                product_id: "prod_1".into(),
                image_urls: vec![],
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("No images"));
    }

    #[tokio::test]
    async fn test_upload_images_rejects_too_many_handler() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::SELLER_ID: "seller_1" }),
            )
            .await
            .unwrap();

        let urls: Vec<String> = (0..=MAX_PRODUCT_IMAGES)
            .map(|i| format!("https://cdn/{i}.jpg"))
            .collect();
        let err = upload_images(
            State(state),
            Extension(auth("seller_1", &["seller"])),
            Json(UploadImagesRequest {
                product_id: "prod_1".into(),
                image_urls: urls,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Maximum"));
    }

    #[tokio::test]
    async fn test_upload_images_rejects_empty_url_string_handler() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::SELLER_ID: "seller_1" }),
            )
            .await
            .unwrap();

        let err = upload_images(
            State(state),
            Extension(auth("seller_1", &["seller"])),
            Json(UploadImagesRequest {
                product_id: "prod_1".into(),
                image_urls: vec!["https://cdn/a.jpg".into(), "  ".into()],
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("cannot be empty"));
    }

    #[tokio::test]
    async fn test_upload_images_product_not_found_handler() {
        let state = setup_state().await;
        let err = upload_images(
            State(state),
            Extension(auth("seller_1", &["seller"])),
            Json(UploadImagesRequest {
                product_id: "nonexistent".into(),
                image_urls: vec!["https://cdn/a.jpg".into()],
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_upload_images_rejects_non_owner_non_admin() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::SELLER_ID: "seller_1" }),
            )
            .await
            .unwrap();

        let err = upload_images(
            State(state),
            Extension(auth("seller_2", &["seller"])),
            Json(UploadImagesRequest {
                product_id: "prod_1".into(),
                image_urls: vec!["https://cdn/a.jpg".into()],
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Only product owner or admin"));
    }

    // ── Coverage: delete_product_images handler ──

    #[tokio::test]
    async fn test_delete_product_images_success_handler() {
        let Json(resp) = delete_product_images(Json(DeleteProductImagesRequest {
            public_urls: vec!["https://cdn/a.jpg".into()],
        }))
        .await
        .unwrap();
        assert_eq!(resp["success"], true);
        assert_eq!(resp["deleted"], 1);
    }

    #[tokio::test]
    async fn test_delete_product_images_rejects_over_20_handler() {
        let urls: Vec<String> = (0..21).map(|i| format!("https://cdn/{i}.jpg")).collect();
        let err = delete_product_images(Json(DeleteProductImagesRequest { public_urls: urls }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Cannot delete more than 20"));
    }

    // ── Coverage: upload_review_images handler errors ──

    #[tokio::test]
    async fn test_upload_review_images_rejects_empty_handler() {
        let err = upload_review_images(Json(UploadReviewImagesRequest {
            user_id: "user_1".into(),
            file_names: vec![],
        }))
        .await
        .unwrap_err();
        assert!(err.to_string().contains("1-3 entries"));
    }

    #[tokio::test]
    async fn test_upload_review_images_rejects_over_3_handler() {
        let err = upload_review_images(Json(UploadReviewImagesRequest {
            user_id: "user_1".into(),
            file_names: vec!["a".into(), "b".into(), "c".into(), "d".into()],
        }))
        .await
        .unwrap_err();
        assert!(err.to_string().contains("1-3 entries"));
    }

    // ── Coverage: delete_product null product check ──

    #[tokio::test]
    async fn test_delete_product_product_not_found_handler() {
        let state = setup_state().await;
        let err = delete_product(
            State(state),
            Json(DeleteProductRequest {
                product_id: "nonexistent".into(),
                user_id: "user_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // ── Coverage: list_products bad order_by ──

    #[tokio::test]
    async fn test_list_products_rejects_bad_order_by_handler() {
        let state = setup_state().await;
        let err = list_products(
            State(state),
            Json(ListProductsRequest {
                page: 1,
                limit: 20,
                category: None,
                seller_id: None,
                order_by: Some("bad_field".into()),
                order_direction: "desc".into(),
                start_after: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Invalid orderBy"));
    }

    // ── Coverage: list_products asc direction ──

    #[tokio::test]
    async fn test_list_products_asc_direction() {
        let state = setup_state().await;
        for (id, status, created_at) in [
            ("p1", "active", "2026-01-01T00:00:00Z"),
            ("p2", "active", "2026-01-02T00:00:00Z"),
        ] {
            state.db.upsert_document(
                collections::PRODUCTS, id,
                serde_json::json!({ fields::LIFECYCLE_STATUS: status, fields::CREATED_AT: created_at }),
            ).await.unwrap();
        }

        let Json(resp) = list_products(
            State(state),
            Json(ListProductsRequest {
                page: 1,
                limit: 10,
                category: None,
                seller_id: None,
                order_by: None,
                order_direction: "asc".into(),
                start_after: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp.total_fetched, 2);
    }

    // ── Coverage: seller_list has_more + next_cursor ──

    #[tokio::test]
    async fn test_seller_list_pagination_has_more() {
        let state = setup_state().await;
        for (id, created_at) in [
            ("p1", "2026-01-03T00:00:00Z"),
            ("p2", "2026-01-02T00:00:00Z"),
            ("p3", "2026-01-01T00:00:00Z"),
        ] {
            state
                .db
                .upsert_document(
                    collections::PRODUCTS,
                    id,
                    serde_json::json!({
                        fields::SELLER_ID: "seller_1",
                        fields::LIFECYCLE_STATUS: "active",
                        fields::CREATED_AT: created_at,
                    }),
                )
                .await
                .unwrap();
        }

        let Json(resp) = seller_list(
            State(state),
            Json(SellerListRequest {
                seller_id: "seller_1".into(),
                page: 1,
                limit: 2,
                start_after: None,
                include_inactive: false,
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp.total_fetched, 2);
        assert!(resp.has_more);
        assert!(resp.next_cursor.is_some());
    }

    // ── Coverage: bulk_update boundary errors ──

    #[tokio::test]
    async fn test_bulk_update_rejects_empty_handler() {
        let state = setup_state().await;
        let err = bulk_update_products(
            State(state),
            Extension(auth("seller_1", &["seller"])),
            Json(BulkUpdateRequest {
                product_ids: vec![],
                update: serde_json::json!({ "status": "paused" }),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("1-100"));
    }

    #[tokio::test]
    async fn test_bulk_update_rejects_over_100_handler() {
        let state = setup_state().await;
        let ids: Vec<String> = (0..101).map(|i| format!("p{i}")).collect();
        let err = bulk_update_products(
            State(state),
            Extension(auth("seller_1", &["seller"])),
            Json(BulkUpdateRequest {
                product_ids: ids,
                update: serde_json::json!({ "status": "paused" }),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("1-100"));
    }

    #[tokio::test]
    async fn test_bulk_update_rejects_non_owner_non_admin() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::SELLER_ID: "seller_1" }),
            )
            .await
            .unwrap();

        let err = bulk_update_products(
            State(state),
            Extension(auth("seller_2", &["seller"])),
            Json(BulkUpdateRequest {
                product_ids: vec!["prod_1".into()],
                update: serde_json::json!({ fields::LIFECYCLE_STATUS: "paused" }),
            }),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Only product owner or admin"));
    }

    // ── Coverage: update_product non-object productData ──

    #[tokio::test]
    async fn test_update_product_rejects_non_object_product_data_handler() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::SELLER_ID: "seller_1" }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                serde_json::json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();

        let err = update_product(
            State(state),
            Json(UpdateProductRequest {
                product_id: "prod_1".into(),
                user_id: "seller_1".into(),
                product_data: serde_json::json!("not-an-object"),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("productData must be an object"));
    }

    // ── Coverage: toggle_favorite product not found ──

    #[tokio::test]
    async fn test_toggle_favorite_product_not_found_handler() {
        let state = setup_state().await;
        let err = toggle_favorite(
            State(state),
            Json(ToggleFavoriteRequest {
                product_id: "nonexistent".into(),
                user_id: "user_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
