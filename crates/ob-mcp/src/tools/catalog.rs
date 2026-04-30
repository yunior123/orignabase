//! Catalog tools — search, get product, check inventory

use crate::McpState;
use crate::errors::{McpError, McpResult};
use serde_json::{Value, json};

/// Search products by query, category, price range
pub async fn search_products(state: McpState, params: &Value) -> McpResult<Value> {
    let _query = params
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'query' parameter".to_string()))?;

    let category = params.get("category").and_then(|v| v.as_str());
    let min_price = params.get("min_price").and_then(|v| v.as_u64());
    let max_price = params.get("max_price").and_then(|v| v.as_u64());
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(20);
    let offset = params.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);

    // Validate limits
    if limit > 100 {
        return Err(McpError::ValidationError(
            "Limit must be <= 100".to_string(),
        ));
    }

    // If Meilisearch is available, use it; otherwise fall back to SurrealDB
    if let Some(_search) = &state.search {
        // Build Meilisearch filter query
        let mut filters = Vec::new();
        if let Some(cat) = category {
            filters.push(format!("categoryId = '{}'", cat));
        }
        if let Some(min) = min_price {
            filters.push(format!("priceCents >= {}", min));
        }
        if let Some(max) = max_price {
            filters.push(format!("priceCents <= {}", max));
        }
        filters.push("lifecycleStatus = 'active'".to_string());

        // Call Meilisearch
        let _filter_str = if filters.is_empty() {
            None
        } else {
            Some(filters.join(" AND "))
        };

        // NOTE: This calls search.search() method which would be implemented in ob-search
        // For now, stub the response
        return Ok(json!({
            "results": [],
            "total": 0,
            "limit": limit,
            "offset": offset
        }));
    }

    // Fallback: SurrealDB query
    // NOTE: In production, construct SurrealDB query via state.db
    Ok(json!({
        "results": [],
        "total": 0,
        "limit": limit,
        "offset": offset
    }))
}

/// Get product by ID
pub async fn get_product(_state: McpState, params: &Value) -> McpResult<Value> {
    let product_id = params
        .get("product_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'product_id'".to_string()))?;

    // Validate SurrealDB ID format
    if !product_id.contains(':') {
        return Err(McpError::ValidationError(
            "Invalid product ID format".to_string(),
        ));
    }

    // Fetch from SurrealDB
    // NOTE: state.db.get_document("products", product_id)
    // For now, stub
    Ok(json!({
        "id": product_id,
        "name": "Example Product",
        "description": "Product description",
        "priceCents": 10000,
        "stockQuantity": 5,
        "lifecycleStatus": "active"
    }))
}

/// Check inventory for a product
pub async fn check_inventory(_state: McpState, params: &Value) -> McpResult<Value> {
    let product_id = params
        .get("product_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'product_id'".to_string()))?;

    if !product_id.contains(':') {
        return Err(McpError::ValidationError(
            "Invalid product ID format".to_string(),
        ));
    }

    // Fetch stock from SurrealDB
    // NOTE: state.db.get_document("products", product_id)
    // and extract stockQuantity field
    Ok(json!({
        "product_id": product_id,
        "stock_quantity": 5,
        "available": true
    }))
}
