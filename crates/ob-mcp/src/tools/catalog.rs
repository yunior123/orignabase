//! Catalog tools — search, get product, check inventory

use crate::McpState;
use crate::errors::{McpError, McpResult};
use ob_core::constants::mcp_params as p;
use serde_json::{Value, json};

/// Search products by query, category, price range
pub async fn search_products(state: McpState, params: &Value) -> McpResult<Value> {
    let query = params
        .get(p::QUERY)
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'query' parameter".to_string()))?;

    let category = params.get(p::CATEGORY).and_then(|v| v.as_str());
    let min_price = params.get(p::MIN_PRICE).and_then(|v| v.as_u64());
    let max_price = params.get(p::MAX_PRICE).and_then(|v| v.as_u64());
    let raw_limit = params.get(p::LIMIT).and_then(|v| v.as_u64()).unwrap_or(20);
    let limit = raw_limit.clamp(1, 100) as usize;
    let offset = params.get(p::OFFSET).and_then(|v| v.as_u64()).unwrap_or(0) as usize;

    // If Meilisearch is available and enabled, use it
    if let Some(search) = &state.search
        && search.is_enabled()
    {
        // Build Meilisearch filter query
        let mut filters = Vec::new();
        if let Some(cat) = category {
            let safe_cat = cat.replace('\'', "\\'");
            filters.push(format!("categoryId = '{}'", safe_cat));
        }
        if let Some(min) = min_price {
            filters.push(format!("priceCents >= {}", min));
        }
        if let Some(max) = max_price {
            filters.push(format!("priceCents <= {}", max));
        }
        filters.push("lifecycleStatus = 'active'".to_string());

        let filter_str = if filters.is_empty() {
            None
        } else {
            Some(filters.join(" AND "))
        };

        match search
            .search(
                "products",
                query,
                Some(limit),
                Some(offset),
                filter_str.as_deref(),
            )
            .await
        {
            Ok(result) => {
                return Ok(json!({
                    "results": result.hits,
                    "total": result.estimated_total_hits.unwrap_or(0),
                    "limit": limit,
                    "offset": offset
                }));
            }
            Err(e) => {
                // Log but fall through to PostgreSQL fallback
                tracing::warn!("Meilisearch failed, falling back to PostgreSQL: {e}");
            }
        }
    }

    // Fallback: PostgreSQL query via query_bind
    let mut conditions = vec!["data->>'lifecycleStatus' = 'active'".to_string()];
    let mut binds = serde_json::Map::new();

    // Search by name/description using case-insensitive match (~~*)
    conditions.push(
        "(data->>'name' ~~* $search_query OR data->>'description' ~~* $search_query)".to_string(),
    );
    binds.insert("search_query".to_string(), json!(format!("%{}%", query)));

    if let Some(cat) = category {
        conditions.push("data->>'categoryId' = $category".to_string());
        binds.insert("category".to_string(), json!(cat));
    }
    if let Some(min) = min_price {
        conditions
            .push("(data->>'priceCents')::\"numeric\" >= ($min_price)::\"numeric\"".to_string());
        binds.insert("min_price".to_string(), json!(min));
    }
    if let Some(max) = max_price {
        conditions
            .push("(data->>'priceCents')::\"numeric\" <= ($max_price)::\"numeric\"".to_string());
        binds.insert("max_price".to_string(), json!(max));
    }

    let where_clause = conditions.join(" AND ");
    let sql = format!(
        "SELECT * FROM products WHERE {} ORDER BY data->>'createdAt' DESC LIMIT {} OFFSET {}",
        where_clause, limit, offset
    );

    let rows = state
        .db
        .query_bind(&sql, Value::Object(binds))
        .await
        .map_err(|e| McpError::Internal(format!("PostgreSQL search failed: {e}")))?;

    Ok(json!({
        "results": rows,
        "total": rows.len(),
        "limit": limit,
        "offset": offset
    }))
}

/// Get product by ID
pub async fn get_product(state: McpState, params: &Value) -> McpResult<Value> {
    let product_id = params
        .get(p::PRODUCT_ID)
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'product_id'".to_string()))?;

    // Validate ID format
    if !product_id.contains(':') {
        return Err(McpError::ValidationError(
            "Invalid product ID format".to_string(),
        ));
    }

    // Fetch from PostgreSQL
    let product = state
        .db
        .get_document("products", product_id)
        .await
        .map_err(|e| McpError::NotFound(format!("Product not found: {e}")))?;

    Ok(product)
}

/// Check inventory for a product
pub async fn check_inventory(state: McpState, params: &Value) -> McpResult<Value> {
    let product_id = params
        .get(p::PRODUCT_ID)
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'product_id'".to_string()))?;

    if !product_id.contains(':') {
        return Err(McpError::ValidationError(
            "Invalid product ID format".to_string(),
        ));
    }

    // Fetch stock from PostgreSQL
    let product = state
        .db
        .get_document("products", product_id)
        .await
        .map_err(|e| McpError::NotFound(format!("Product not found: {e}")))?;

    let stock_quantity = product
        .get("stockQuantity")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    Ok(json!({
        "product_id": product_id,
        "stock_quantity": stock_quantity,
        "available": stock_quantity > 0
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::McpState;
    use ob_database::fields;
    use std::sync::Arc;

    async fn make_state() -> McpState {
        McpState {
            db: Arc::new(ob_database::DatabaseClient::new_mem().await),
            search: None,
            config: Arc::new(ob_core::Config::load(None).unwrap()),
            jwt_keys: Arc::new(ob_auth::JwtKeys::from_secret("test-secret")),
        }
    }

    // ── search_products ──

    #[tokio::test]
    async fn test_search_products_missing_query() {
        let state = make_state().await;
        let result = search_products(state, &json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn test_search_products_valid_query() {
        let state = make_state().await;
        let result = search_products(state, &json!({"query": "shirt"}))
            .await
            .unwrap();
        assert_eq!(result["total"], 0);
        assert_eq!(result["limit"], 20);
        assert_eq!(result["offset"], 0);
    }

    #[tokio::test]
    async fn test_search_products_with_all_params() {
        let state = make_state().await;
        let params = json!({
            "query": "shirt",
            "category": "clothing",
            "min_price": 1000,
            "max_price": 5000,
            "limit": 10,
            "offset": 5
        });
        let result = search_products(state, &params).await.unwrap();
        assert_eq!(result["limit"], 10);
        assert_eq!(result["offset"], 5);
    }

    #[tokio::test]
    async fn test_search_products_limit_clamped_to_100() {
        let state = make_state().await;
        // limit > 100 is clamped to 100
        let result = search_products(state, &json!({"query": "x", "limit": 101})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["limit"], 100);
    }

    #[tokio::test]
    async fn test_search_products_limit_boundary_100() {
        let state = make_state().await;
        let result = search_products(state, &json!({"query": "x", "limit": 100})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["limit"], 100);
    }

    #[tokio::test]
    async fn test_search_products_default_limit_offset() {
        let state = make_state().await;
        let result = search_products(state, &json!({"query": "x"}))
            .await
            .unwrap();
        assert_eq!(result["limit"], 20);
        assert_eq!(result["offset"], 0);
    }

    #[tokio::test]
    async fn test_search_products_zero_limit_clamped_to_1() {
        let state = make_state().await;
        // limit=0 is clamped to 1
        let result = search_products(state, &json!({"query": "x", "limit": 0})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["limit"], 1);
    }

    #[tokio::test]
    async fn test_search_products_non_string_query() {
        let state = make_state().await;
        let result = search_products(state, &json!({"query": 42})).await;
        assert!(result.is_err());
    }

    // ── get_product ──

    #[tokio::test]
    async fn test_get_product_missing_id() {
        let state = make_state().await;
        let result = get_product(state, &json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn test_get_product_invalid_format_no_colon() {
        let state = make_state().await;
        let result = get_product(state, &json!({"product_id": "p1"})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::ValidationError(_)));
    }

    #[tokio::test]
    async fn test_get_product_empty_id() {
        let state = make_state().await;
        let result = get_product(state, &json!({"product_id": ""})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_product_not_found() {
        let state = make_state().await;
        let result = get_product(state, &json!({"product_id": "products:nonexistent"})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_get_product_returns_real_data() {
        let state = make_state().await;
        // Insert a product first
        state
            .db
            .upsert_document(
                "products",
                "products:test1",
                json!({
                    "name": "Test Product",
                    "description": "A test product",
                    "priceCents": 1500,
                    "stockQuantity": 10,
                    "lifecycleStatus": "active"
                }),
            )
            .await
            .unwrap();

        let result = get_product(state, &json!({"product_id": "products:test1"}))
            .await
            .unwrap();
        assert_eq!(result[fields::NAME], "Test Product");
        assert_eq!(result[fields::PRICE_CENTS], 1500);
        assert_eq!(result["stockQuantity"], 10);
    }

    // ── check_inventory ──

    #[tokio::test]
    async fn test_check_inventory_missing_id() {
        let state = make_state().await;
        let result = check_inventory(state, &json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn test_check_inventory_invalid_format() {
        let state = make_state().await;
        let result = check_inventory(state, &json!({"product_id": "badformat"})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::ValidationError(_)));
    }

    #[tokio::test]
    async fn test_check_inventory_integer_id() {
        let state = make_state().await;
        let result = check_inventory(state, &json!({"product_id": 123})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_check_inventory_not_found() {
        let state = make_state().await;
        let result = check_inventory(state, &json!({"product_id": "products:nonexistent"})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_check_inventory_returns_real_stock() {
        let state = make_state().await;
        // Insert a product
        state
            .db
            .upsert_document(
                "products",
                "products:inv1",
                json!({
                    "name": "Stocked Item",
                    "stockQuantity": 42,
                    "lifecycleStatus": "active"
                }),
            )
            .await
            .unwrap();

        let result = check_inventory(state, &json!({"product_id": "products:inv1"}))
            .await
            .unwrap();
        assert_eq!(result["product_id"], "products:inv1");
        assert_eq!(result["stock_quantity"], 42);
        assert_eq!(result["available"], true);
    }

    #[tokio::test]
    async fn test_check_inventory_out_of_stock() {
        let state = make_state().await;
        state
            .db
            .upsert_document(
                "products",
                "products:oos1",
                json!({
                    "name": "Out of Stock",
                    "stockQuantity": 0,
                    "lifecycleStatus": "active"
                }),
            )
            .await
            .unwrap();

        let result = check_inventory(state, &json!({"product_id": "products:oos1"}))
            .await
            .unwrap();
        assert_eq!(result["stock_quantity"], 0);
        assert_eq!(result["available"], false);
    }
}
