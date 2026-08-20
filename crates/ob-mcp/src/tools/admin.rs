//! Admin tools — analytics, reviews

use crate::McpState;
use crate::errors::{McpError, McpResult};
use ob_core::constants::{collections, fields};
use serde_json::{Value, json};

/// Get marketplace analytics (admin only)
pub async fn get_analytics(state: McpState, params: &Value) -> McpResult<Value> {
    let period = params
        .get("period")
        .and_then(|v| v.as_str())
        .unwrap_or("month");

    match period {
        "day" | "week" | "month" => {}
        _ => {
            return Err(McpError::ValidationError(
                "Period must be 'day', 'week', or 'month'".to_string(),
            ));
        }
    }

    let days = match period {
        "day" => 1,
        "week" => 7,
        "month" => 30,
        _ => 30,
    };

    let orders = state
        .db
        .find_where(
            collections::ORDERS,
            fields::ORDER_STATUS,
            "=",
            &json!("delivered"),
            None,
        )
        .await
        .unwrap_or_default();

    let total_orders = orders.len();
    let total_revenue: i64 = orders
        .iter()
        .filter_map(|o| o.get(fields::TOTAL_AMOUNT_CENTS).and_then(|v| v.as_i64()))
        .sum();
    let avg_order = if total_orders > 0 {
        total_revenue / total_orders as i64
    } else {
        0
    };
    let platform_fee: i64 = orders
        .iter()
        .filter_map(|o| o.get(fields::PLATFORM_FEE_CENTS).and_then(|v| v.as_i64()))
        .sum();

    let products = state
        .db
        .find_where(
            collections::PRODUCTS,
            fields::LIFECYCLE_STATUS,
            "=",
            &json!("active"),
            None,
        )
        .await
        .unwrap_or_default();

    let mut sorted_products: Vec<&Value> = products.iter().collect();
    sorted_products.sort_by(|a, b| {
        let a_count = a.get("purchaseCount").and_then(|v| v.as_u64()).unwrap_or(0);
        let b_count = b.get("purchaseCount").and_then(|v| v.as_u64()).unwrap_or(0);
        b_count.cmp(&a_count)
    });

    let top_products: Vec<Value> = sorted_products
        .into_iter()
        .take(5)
        .map(|p| {
            json!({
                "product_id": p.get(fields::ID).and_then(|v| v.as_str()).unwrap_or(""),
                "name": p.get(fields::NAME).and_then(|v| v.as_str()).unwrap_or(""),
                "purchase_count": p.get("purchaseCount").and_then(|v| v.as_u64()).unwrap_or(0),
            })
        })
        .collect();

    Ok(json!({
        "period": period,
        "days": days,
        "total_orders": total_orders,
        "total_revenue_cents": total_revenue,
        "average_order_cents": avg_order,
        "total_platform_fee_cents": platform_fee,
        "top_products": top_products,
        "top_sellers": []
    }))
}

/// Create product review (any authenticated user)
pub async fn create_review(state: McpState, user_id: &str, params: &Value) -> McpResult<Value> {
    let product_id = params
        .get("product_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing 'product_id'".to_string()))?;

    let rating = params
        .get("rating")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| McpError::InvalidParams("Missing 'rating'".to_string()))?;

    if !(1..=5).contains(&rating) {
        return Err(McpError::ValidationError("Rating must be 1-5".to_string()));
    }

    let review_text = params.get("review").and_then(|v| v.as_str());

    let record_id = product_id
        .split_once(':')
        .map(|(_, id)| id)
        .unwrap_or(product_id);

    let product = state
        .db
        .get_document(collections::PRODUCTS, record_id)
        .await;

    let _product = match product {
        Ok(doc) if !doc.is_null() => doc,
        Ok(_) | Err(ob_core::Error::NotFound(_)) => {
            return Err(McpError::NotFound("Product not found".to_string()));
        }
        Err(e) => {
            return Err(McpError::Internal(format!("Failed to fetch product: {e}")));
        }
    };

    let review_id = format!("review_{}", uuid::Uuid::new_v4());
    let review_data = json!({
        fields::ID: format!("reviews:{}", review_id),
        "productId": product_id,
        "userId": user_id,
        "rating": rating,
        "review": review_text.unwrap_or(""),
        fields::CREATED_AT: chrono::Utc::now().to_rfc3339(),
    });

    state
        .db
        .create_document(collections::REVIEWS, review_data)
        .await
        .map_err(|e| McpError::Internal(format!("Failed to create review: {e}")))?;

    Ok(json!({
        "review_id": review_id,
        "product_id": product_id,
        "user_id": user_id,
        "rating": rating,
        "review": review_text,
        "created": true
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::McpState;
    use std::sync::Arc;

    async fn make_state() -> McpState {
        McpState {
            db: Arc::new(ob_database::DatabaseClient::new_mem().await),
            search: None,
            config: Arc::new(ob_core::Config::load(None).unwrap()),
            jwt_keys: Arc::new(ob_auth::JwtKeys::from_secret("test-secret")),
        }
    }

    #[tokio::test]
    async fn test_get_analytics_default_period() {
        let state = make_state().await;
        let result = get_analytics(state, &json!({})).await.unwrap();
        assert_eq!(result["period"], "month");
        assert_eq!(result["days"], 30);
        assert!(result["total_orders"].is_u64());
    }

    #[tokio::test]
    async fn test_get_analytics_day_period() {
        let state = make_state().await;
        let result = get_analytics(state, &json!({"period": "day"}))
            .await
            .unwrap();
        assert_eq!(result["period"], "day");
        assert_eq!(result["days"], 1);
    }

    #[tokio::test]
    async fn test_get_analytics_invalid_period() {
        let state = make_state().await;
        let result = get_analytics(state, &json!({"period": "year"})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::ValidationError(_)));
    }

    #[tokio::test]
    async fn test_create_review_missing_product_id() {
        let state = make_state().await;
        let params = json!({"rating": 5});
        let result = create_review(state, "users:u1", &params).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn test_create_review_missing_rating() {
        let state = make_state().await;
        let params = json!({"product_id": "products:p1"});
        let result = create_review(state, "users:u1", &params).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn test_create_review_rating_out_of_range() {
        let state = make_state().await;
        for rating in [0u64, 6u64] {
            let params = json!({"product_id": "products:p1", "rating": rating});
            let result = create_review(state.clone(), "users:u1", &params).await;
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), McpError::ValidationError(_)));
        }
    }

    #[tokio::test]
    async fn test_create_review_product_not_found() {
        let state = make_state().await;
        let params = json!({
            "product_id": "products:nonexistent",
            "rating": 5,
            "review": "Great!"
        });
        let result = create_review(state, "users:u1", &params).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::NotFound(_)));
    }
}
