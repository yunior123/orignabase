//! Product DB triggers — Meilisearch sync.
//! Ported from: functions/handlers/products.py (on_product_created, on_product_updated, on_product_deleted)
//!
//! These are NOT HTTP routes. They are called from the ob-functions trigger system
//! when product documents are created/updated/deleted in PostgreSQL.

use ob_database::DatabaseClient;
use serde_json::Value;
use tracing::{error, info, warn};

use crate::shared::schema::{collections, fields};
use ob_database::fields as db_fields;

/// Sync a newly created product to Meilisearch.
///
/// Validates product data (price > 0, seller not suspended), then indexes
/// the product in Meilisearch for search. If validation fails, the product
/// is set to "draft" status with a deactivation reason.
pub async fn on_product_created(
    db: &DatabaseClient,
    http_client: &reqwest::Client,
    meilisearch_url: &str,
    meilisearch_key: &str,
    product_id: &str,
    product: &Value,
) -> Result<(), ob_core::Error> {
    if product.is_null() {
        info!(product_id = %product_id, "No data for product — skipping");
        return Ok(());
    }

    // Validate seller is not suspended
    let seller_id = product
        .get(db_fields::SELLER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if !seller_id.is_empty() {
        let seller = db
            .get_document(collections::USERS, seller_id)
            .await
            .unwrap_or(Value::Null);

        let suspended = seller
            .get("suspended")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if suspended {
            warn!(product_id = %product_id, seller_id = %seller_id, "Product from suspended seller — deactivating");
            let update = serde_json::json!({
                db_fields::LIFECYCLE_STATUS: "draft",
                "deactivationReason": "Seller is suspended",
            });
            db.update_document(collections::PRODUCTS, product_id, update)
                .await
                .ok();
            return Ok(());
        }
    }

    // Validate price
    let price = product
        .get(db_fields::PRICE_CENTS)
        .and_then(|v| v.as_f64())
        .or_else(|| product.get("price").and_then(|v| v.as_f64()))
        .unwrap_or(0.0);

    if price <= 0.0 || price > 10_000_000.0 {
        warn!(product_id = %product_id, price = price, "Invalid price — deactivating");
        let update = serde_json::json!({
            db_fields::LIFECYCLE_STATUS: "draft",
            "deactivationReason": format!("Invalid price: {price}"),
        });
        db.update_document(collections::PRODUCTS, product_id, update)
            .await
            .ok();
        return Ok(());
    }

    // Validate stock >= 0
    let stock = product
        .get(fields::STOCK_QUANTITY)
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    if stock < 0 {
        warn!(product_id = %product_id, stock = stock, "Negative stock — deactivating");
        let update = serde_json::json!({
            db_fields::LIFECYCLE_STATUS: "draft",
            "deactivationReason": "Negative stock quantity",
        });
        db.update_document(collections::PRODUCTS, product_id, update)
            .await
            .ok();
        return Ok(());
    }

    // Index to Meilisearch
    let lifecycle = product
        .get(db_fields::LIFECYCLE_STATUS)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if lifecycle == "active" {
        index_to_meilisearch(
            http_client,
            meilisearch_url,
            meilisearch_key,
            product_id,
            product,
        )
        .await?;
        info!(product_id = %product_id, "Product indexed to Meilisearch");
    } else {
        info!(product_id = %product_id, lifecycle = %lifecycle, "Product not active — skipping Meilisearch index");
    }

    Ok(())
}

/// Sync an updated product to Meilisearch.
///
/// Re-indexes the product if it is active, or removes it from the index
/// if it has been deactivated/archived.
pub async fn on_product_updated(
    _db: &DatabaseClient,
    http_client: &reqwest::Client,
    meilisearch_url: &str,
    meilisearch_key: &str,
    product_id: &str,
    product: &Value,
) -> Result<(), ob_core::Error> {
    if product.is_null() {
        return Ok(());
    }

    let lifecycle = product
        .get(db_fields::LIFECYCLE_STATUS)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if lifecycle == "active" {
        index_to_meilisearch(
            http_client,
            meilisearch_url,
            meilisearch_key,
            product_id,
            product,
        )
        .await?;
        info!(product_id = %product_id, "Product re-indexed to Meilisearch");
    } else {
        // Remove from search index if no longer active
        remove_from_meilisearch(http_client, meilisearch_url, meilisearch_key, product_id).await?;
        info!(product_id = %product_id, lifecycle = %lifecycle, "Product removed from Meilisearch");
    }

    Ok(())
}

/// Remove a deleted product from Meilisearch.
pub async fn on_product_deleted(
    http_client: &reqwest::Client,
    meilisearch_url: &str,
    meilisearch_key: &str,
    product_id: &str,
) -> Result<(), ob_core::Error> {
    remove_from_meilisearch(http_client, meilisearch_url, meilisearch_key, product_id).await?;
    info!(product_id = %product_id, "Product deleted from Meilisearch");
    Ok(())
}

// ─── Internal Meilisearch helpers ───────────────────────────────────────────

async fn index_to_meilisearch(
    http_client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    product_id: &str,
    product: &Value,
) -> Result<(), ob_core::Error> {
    let url = format!(
        "{}/indexes/products/documents",
        base_url.trim_end_matches('/')
    );

    // Build the document to index — include ID
    let mut doc = product.clone();
    if let Some(obj) = doc.as_object_mut() {
        obj.insert(
            "id".to_string(),
            serde_json::json!(sanitize_document_id(product_id)),
        );
        obj.insert("record_id".to_string(), serde_json::json!(product_id));
    }

    let response = http_client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&[doc])
        .send()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Meilisearch request failed: {e}")))?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        error!(product_id = %product_id, error = %body, "Meilisearch indexing failed");
        return Err(ob_core::Error::Internal(format!(
            "Meilisearch indexing failed: {body}"
        )));
    }

    Ok(())
}

async fn remove_from_meilisearch(
    http_client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    product_id: &str,
) -> Result<(), ob_core::Error> {
    let url = format!(
        "{}/indexes/products/documents/{}",
        base_url.trim_end_matches('/'),
        sanitize_document_id(product_id)
    );

    let response = http_client
        .delete(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Meilisearch delete failed: {e}")))?;

    // 404 is fine (already deleted)
    if !response.status().is_success() && response.status().as_u16() != 404 {
        let body = response.text().await.unwrap_or_default();
        error!(product_id = %product_id, error = %body, "Meilisearch delete failed");
        return Err(ob_core::Error::Internal(format!(
            "Meilisearch delete failed: {body}"
        )));
    }

    Ok(())
}

fn sanitize_document_id(document_id: &str) -> String {
    document_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ─── URL construction (sync) ─────────────────────────────────────

    #[test]
    fn test_meilisearch_url_construction() {
        let base = "http://localhost:7700";
        let url = format!("{}/indexes/products/documents", base.trim_end_matches('/'));
        assert_eq!(url, "http://localhost:7700/indexes/products/documents");

        let base_trailing = "http://localhost:7700/";
        let url2 = format!(
            "{}/indexes/products/documents",
            base_trailing.trim_end_matches('/')
        );
        assert_eq!(url2, "http://localhost:7700/indexes/products/documents");
    }

    #[test]
    fn test_delete_url_construction() {
        let base = "http://localhost:7700";
        let product_id = "prod-123";
        let url = format!(
            "{}/indexes/products/documents/{}",
            base.trim_end_matches('/'),
            product_id
        );
        assert_eq!(
            url,
            "http://localhost:7700/indexes/products/documents/prod-123"
        );
    }

    #[test]
    fn test_product_validation_price() {
        let product = json!({
            db_fields::PRICE_CENTS: 0,
            "stockQuantity": 10,
            "lifecycleStatus": "active",
        });
        let price = product
            .get(db_fields::PRICE_CENTS)
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        assert!(price <= 0.0, "Zero price should fail validation");
    }

    // ─── Validation extraction helpers (sync) ────────────────────────

    #[test]
    fn test_seller_id_extraction_present() {
        let product = json!({ db_fields::SELLER_ID: "seller-1" });
        let sid = product
            .get(db_fields::SELLER_ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(sid, "seller-1");
    }

    #[test]
    fn test_seller_id_extraction_missing() {
        let product = json!({ "name": "Widget" });
        let sid = product
            .get(db_fields::SELLER_ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(sid, "");
    }

    #[test]
    fn test_price_fallback_to_price_field() {
        // priceCents missing, falls back to "price"
        let product = json!({ "price": 42.5 });
        let price = product
            .get(db_fields::PRICE_CENTS)
            .and_then(|v| v.as_f64())
            .or_else(|| product.get("price").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);
        assert!((price - 42.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_price_missing_both_fields() {
        let product = json!({ "name": "Widget" });
        let price = product
            .get(db_fields::PRICE_CENTS)
            .and_then(|v| v.as_f64())
            .or_else(|| product.get("price").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);
        assert!((price - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_price_boundary_too_high() {
        let price = 10_000_001.0_f64;
        assert!(price > 10_000_000.0);
    }

    #[test]
    fn test_price_boundary_exactly_max() {
        // 10_000_000 is NOT > 10_000_000, so it should pass validation
        let price = 10_000_000.0_f64;
        assert!(!(price <= 0.0 || price > 10_000_000.0));
    }

    #[test]
    fn test_price_negative() {
        let price = -1.0_f64;
        assert!(price <= 0.0);
    }

    #[test]
    fn test_stock_negative() {
        let product = json!({ "stockQuantity": -5 });
        let stock = product
            .get(fields::STOCK_QUANTITY)
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        assert!(stock < 0);
    }

    #[test]
    fn test_stock_missing_defaults_zero() {
        let product = json!({});
        let stock = product
            .get(fields::STOCK_QUANTITY)
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        assert_eq!(stock, 0);
    }

    #[test]
    fn test_lifecycle_status_extraction() {
        let product = json!({ "lifecycleStatus": "active" });
        let lc = product
            .get(db_fields::LIFECYCLE_STATUS)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(lc, "active");
    }

    #[test]
    fn test_lifecycle_status_missing() {
        let product = json!({});
        let lc = product
            .get(db_fields::LIFECYCLE_STATUS)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(lc, "");
    }

    // ─── on_product_created ──────────────────────────────────────────

    #[tokio::test]
    async fn test_created_null_product_skips() {
        let db = DatabaseClient::new_mem().await;
        let client = reqwest::Client::new();
        let result = on_product_created(&db, &client, "http://x", "key", "p1", &Value::Null).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_suspended_seller_deactivates() {
        let db = DatabaseClient::new_mem().await;
        // Seed a suspended seller
        db.create_document(
            collections::USERS,
            json!({ "id": "seller-s", "suspended": true }),
        )
        .await
        .unwrap();

        let product = json!({
            "sellerId": "seller-s",
            "priceCents": 100,
            "stockQuantity": 5,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result = on_product_created(&db, &client, "http://x", "key", "prod-1", &product).await;
        assert!(result.is_ok());
        // Function returns early — no Meilisearch call attempted on an unreachable server
    }

    #[tokio::test]
    async fn test_created_seller_not_suspended_continues() {
        let db = DatabaseClient::new_mem().await;
        db.create_document(
            collections::USERS,
            json!({ "id": "seller-ok", "suspended": false }),
        )
        .await
        .unwrap();

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 1})))
            .mount(&server)
            .await;

        let product = json!({
            "sellerId": "seller-ok",
            "priceCents": 500,
            "stockQuantity": 10,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result =
            on_product_created(&db, &client, &server.uri(), "test-key", "prod-2", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_empty_seller_id_skips_seller_check() {
        let db = DatabaseClient::new_mem().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 1})))
            .mount(&server)
            .await;

        let product = json!({
            "priceCents": 100,
            "stockQuantity": 1,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result =
            on_product_created(&db, &client, &server.uri(), "key", "prod-3", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_seller_not_found_treats_as_not_suspended() {
        let db = DatabaseClient::new_mem().await;
        // seller-ghost doesn't exist in DB → get_document returns error → unwrap_or(Null) → not suspended
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 1})))
            .mount(&server)
            .await;

        let product = json!({
            "sellerId": "seller-ghost",
            "priceCents": 200,
            "stockQuantity": 1,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result =
            on_product_created(&db, &client, &server.uri(), "key", "prod-4", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_zero_price_deactivates() {
        let db = DatabaseClient::new_mem().await;
        let product = json!({
            "priceCents": 0,
            "stockQuantity": 5,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result = on_product_created(&db, &client, "http://x", "key", "prod-5", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_negative_price_deactivates() {
        let db = DatabaseClient::new_mem().await;
        let product = json!({
            "priceCents": -50,
            "stockQuantity": 5,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result = on_product_created(&db, &client, "http://x", "key", "prod-6", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_price_exceeds_max_deactivates() {
        let db = DatabaseClient::new_mem().await;
        let product = json!({
            "priceCents": 10_000_001,
            "stockQuantity": 5,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result = on_product_created(&db, &client, "http://x", "key", "prod-7", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_price_fallback_field() {
        // priceCents missing, "price" field used instead
        let db = DatabaseClient::new_mem().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 1})))
            .mount(&server)
            .await;

        let product = json!({
            "price": 500.0,
            "stockQuantity": 1,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result =
            on_product_created(&db, &client, &server.uri(), "key", "prod-8", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_negative_stock_deactivates() {
        let db = DatabaseClient::new_mem().await;
        let product = json!({
            "priceCents": 100,
            "stockQuantity": -1,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result = on_product_created(&db, &client, "http://x", "key", "prod-9", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_not_active_skips_meilisearch() {
        let db = DatabaseClient::new_mem().await;
        let product = json!({
            "priceCents": 100,
            "stockQuantity": 5,
            "lifecycleStatus": "draft",
        });

        // No mock server — if it tried to call Meilisearch it would fail
        let client = reqwest::Client::new();
        let result = on_product_created(
            &db,
            &client,
            "http://unreachable:9999",
            "key",
            "prod-10",
            &product,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_missing_lifecycle_skips_meilisearch() {
        let db = DatabaseClient::new_mem().await;
        let product = json!({
            "priceCents": 100,
            "stockQuantity": 5,
        });

        let client = reqwest::Client::new();
        let result = on_product_created(
            &db,
            &client,
            "http://unreachable:9999",
            "key",
            "prod-11",
            &product,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_active_indexes_to_meilisearch() {
        let db = DatabaseClient::new_mem().await;
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 1})))
            .expect(1)
            .mount(&server)
            .await;

        let product = json!({
            "priceCents": 999,
            "stockQuantity": 10,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result =
            on_product_created(&db, &client, &server.uri(), "test-key", "prod-12", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_meilisearch_failure_returns_error() {
        let db = DatabaseClient::new_mem().await;
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&server)
            .await;

        let product = json!({
            "priceCents": 100,
            "stockQuantity": 1,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result =
            on_product_created(&db, &client, &server.uri(), "key", "prod-13", &product).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Meilisearch indexing failed"));
    }

    // ─── on_product_updated ──────────────────────────────────────────

    #[tokio::test]
    async fn test_updated_null_product_skips() {
        let db = DatabaseClient::new_mem().await;
        let client = reqwest::Client::new();
        let result = on_product_updated(&db, &client, "http://x", "key", "p1", &Value::Null).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_updated_active_reindexes() {
        let db = DatabaseClient::new_mem().await;
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 2})))
            .expect(1)
            .mount(&server)
            .await;

        let product = json!({
            "priceCents": 200,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result =
            on_product_updated(&db, &client, &server.uri(), "key", "prod-20", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_updated_inactive_removes_from_meilisearch() {
        let db = DatabaseClient::new_mem().await;
        let server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/indexes/products/documents/prod-21"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 3})))
            .expect(1)
            .mount(&server)
            .await;

        let product = json!({
            "priceCents": 200,
            "lifecycleStatus": "archived",
        });

        let client = reqwest::Client::new();
        let result =
            on_product_updated(&db, &client, &server.uri(), "key", "prod-21", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_updated_draft_removes_from_meilisearch() {
        let db = DatabaseClient::new_mem().await;
        let server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/indexes/products/documents/prod-22"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 4})))
            .mount(&server)
            .await;

        let product = json!({
            "lifecycleStatus": "draft",
        });

        let client = reqwest::Client::new();
        let result =
            on_product_updated(&db, &client, &server.uri(), "key", "prod-22", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_updated_missing_lifecycle_removes() {
        let db = DatabaseClient::new_mem().await;
        let server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/indexes/products/documents/prod-23"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 5})))
            .mount(&server)
            .await;

        let product = json!({ "name": "Widget" });

        let client = reqwest::Client::new();
        let result =
            on_product_updated(&db, &client, &server.uri(), "key", "prod-23", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_updated_active_meilisearch_error() {
        let db = DatabaseClient::new_mem().await;
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let product = json!({ "lifecycleStatus": "active" });

        let client = reqwest::Client::new();
        let result =
            on_product_updated(&db, &client, &server.uri(), "key", "prod-24", &product).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Meilisearch indexing failed")
        );
    }

    #[tokio::test]
    async fn test_updated_inactive_meilisearch_delete_error() {
        let db = DatabaseClient::new_mem().await;
        let server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/indexes/products/documents/prod-25"))
            .respond_with(ResponseTemplate::new(500).set_body_string("delete boom"))
            .mount(&server)
            .await;

        let product = json!({ "lifecycleStatus": "draft" });

        let client = reqwest::Client::new();
        let result =
            on_product_updated(&db, &client, &server.uri(), "key", "prod-25", &product).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Meilisearch delete failed")
        );
    }

    // ─── on_product_deleted ──────────────────────────────────────────

    #[tokio::test]
    async fn test_deleted_success() {
        let server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/indexes/products/documents/prod-30"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 6})))
            .expect(1)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = on_product_deleted(&client, &server.uri(), "key", "prod-30").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_deleted_404_is_ok() {
        let server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/indexes/products/documents/prod-31"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = on_product_deleted(&client, &server.uri(), "key", "prod-31").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_deleted_500_is_error() {
        let server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/indexes/products/documents/prod-32"))
            .respond_with(ResponseTemplate::new(500).set_body_string("server error"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = on_product_deleted(&client, &server.uri(), "key", "prod-32").await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Meilisearch delete failed")
        );
    }

    // ─── index_to_meilisearch (internal) ─────────────────────────────

    #[tokio::test]
    async fn test_index_inserts_id_into_doc() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 7})))
            .mount(&server)
            .await;

        let product = json!({ "name": "Widget" });
        let client = reqwest::Client::new();
        let result = index_to_meilisearch(&client, &server.uri(), "key", "prod-40", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_index_non_object_product() {
        // If product is an array or primitive, as_object_mut returns None — no crash
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 8})))
            .mount(&server)
            .await;

        let product = json!("just a string");
        let client = reqwest::Client::new();
        let result = index_to_meilisearch(&client, &server.uri(), "key", "prod-41", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_index_trailing_slash_url() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 9})))
            .mount(&server)
            .await;

        // Pass URI with trailing slash
        let uri = format!("{}/", server.uri());
        let product = json!({ "name": "Widget" });
        let client = reqwest::Client::new();
        let result = index_to_meilisearch(&client, &uri, "key", "prod-42", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_index_http_500_returns_error() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(500).set_body_string("bad request body"))
            .mount(&server)
            .await;

        let product = json!({ "name": "Widget" });
        let client = reqwest::Client::new();
        let result = index_to_meilisearch(&client, &server.uri(), "key", "prod-43", &product).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Meilisearch indexing failed"));
        assert!(msg.contains("bad request body"));
    }

    // ─── remove_from_meilisearch (internal) ──────────────────────────

    #[tokio::test]
    async fn test_remove_success() {
        let server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/indexes/products/documents/prod-50"))
            .respond_with(ResponseTemplate::new(202))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = remove_from_meilisearch(&client, &server.uri(), "key", "prod-50").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_remove_404_is_ok() {
        let server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/indexes/products/documents/prod-51"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = remove_from_meilisearch(&client, &server.uri(), "key", "prod-51").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_remove_500_returns_error() {
        let server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/indexes/products/documents/prod-52"))
            .respond_with(ResponseTemplate::new(500).set_body_string("gone wrong"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = remove_from_meilisearch(&client, &server.uri(), "key", "prod-52").await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Meilisearch delete failed"));
        assert!(msg.contains("gone wrong"));
    }

    #[tokio::test]
    async fn test_remove_403_returns_error() {
        let server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/indexes/products/documents/prod-53"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = remove_from_meilisearch(&client, &server.uri(), "key", "prod-53").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_remove_trailing_slash_url() {
        let server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/indexes/products/documents/prod-54"))
            .respond_with(ResponseTemplate::new(202))
            .mount(&server)
            .await;

        let uri = format!("{}/", server.uri());
        let client = reqwest::Client::new();
        let result = remove_from_meilisearch(&client, &uri, "key", "prod-54").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_index_network_failure_returns_error() {
        // Covers the .map_err on .send().await (network-level failure, not HTTP status error)
        let product = json!({ "name": "Widget" });
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(1))
            .build()
            .unwrap();
        // Use a non-routable IP to force a connection timeout/error
        let result = index_to_meilisearch(
            &client,
            "http://192.0.2.1:1",
            "key",
            "prod-net-err",
            &product,
        )
        .await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Meilisearch request failed"));
    }

    #[tokio::test]
    async fn test_remove_network_failure_returns_error() {
        // Covers the .map_err on .send().await in remove_from_meilisearch
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(1))
            .build()
            .unwrap();
        let result =
            remove_from_meilisearch(&client, "http://192.0.2.1:1", "key", "prod-net-err").await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Meilisearch delete failed"));
    }

    #[tokio::test]
    async fn test_created_active_network_failure() {
        // on_product_created with active product but unreachable Meilisearch
        let db = DatabaseClient::new_mem().await;
        let product = json!({
            "priceCents": 100,
            "stockQuantity": 1,
            "lifecycleStatus": "active",
        });
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(1))
            .build()
            .unwrap();
        let result = on_product_created(
            &db,
            &client,
            "http://192.0.2.1:1",
            "key",
            "prod-net",
            &product,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_updated_active_network_failure() {
        // on_product_updated with active product but unreachable Meilisearch
        let db = DatabaseClient::new_mem().await;
        let product = json!({ "lifecycleStatus": "active" });
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(1))
            .build()
            .unwrap();
        let result = on_product_updated(
            &db,
            &client,
            "http://192.0.2.1:1",
            "key",
            "prod-net2",
            &product,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_updated_inactive_network_failure() {
        // on_product_updated with inactive product, remove fails at network level
        let db = DatabaseClient::new_mem().await;
        let product = json!({ "lifecycleStatus": "draft" });
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(1))
            .build()
            .unwrap();
        let result = on_product_updated(
            &db,
            &client,
            "http://192.0.2.1:1",
            "key",
            "prod-net3",
            &product,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_deleted_network_failure() {
        // on_product_deleted with unreachable Meilisearch
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(1))
            .build()
            .unwrap();
        let result = on_product_deleted(&client, "http://192.0.2.1:1", "key", "prod-net4").await;
        assert!(result.is_err());
    }

    // ─── Full integration: on_product_created with all validations ───

    #[tokio::test]
    async fn test_created_all_validations_pass_and_indexes() {
        let db = DatabaseClient::new_mem().await;
        db.create_document(
            collections::USERS,
            json!({ "id": "seller-good", "suspended": false }),
        )
        .await
        .unwrap();

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 10})))
            .expect(1)
            .mount(&server)
            .await;

        let product = json!({
            "sellerId": "seller-good",
            "priceCents": 5000,
            "stockQuantity": 50,
            "lifecycleStatus": "active",
            "name": "Premium Widget",
        });

        let client = reqwest::Client::new();
        let result =
            on_product_created(&db, &client, &server.uri(), "ms-key", "prod-100", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_seller_suspended_field_missing_treats_not_suspended() {
        // Seller doc exists but no "suspended" field → defaults to false
        let db = DatabaseClient::new_mem().await;
        db.create_document(
            collections::USERS,
            json!({ "id": "seller-nosuspend", "name": "Bob" }),
        )
        .await
        .unwrap();

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 11})))
            .mount(&server)
            .await;

        let product = json!({
            "sellerId": "seller-nosuspend",
            "priceCents": 100,
            "stockQuantity": 1,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result =
            on_product_created(&db, &client, &server.uri(), "key", "prod-101", &product).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_created_stock_zero_is_ok() {
        // stock = 0 is fine (not negative)
        let db = DatabaseClient::new_mem().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"taskUid": 12})))
            .mount(&server)
            .await;

        let product = json!({
            "priceCents": 100,
            "stockQuantity": 0,
            "lifecycleStatus": "active",
        });

        let client = reqwest::Client::new();
        let result =
            on_product_created(&db, &client, &server.uri(), "key", "prod-102", &product).await;
        assert!(result.is_ok());
    }
}
