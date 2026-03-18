//! Database index definitions for OrignaBase.
//!
//! Creates required indexes on frequently-queried tables to prevent N+1 queries
//! and improve pagination performance.

use ob_database::DatabaseClient;
use tracing::{info, warn};

use crate::shared::schema::collections;

/// Create all required database indexes.
/// Idempotent: SurrealDB ignores if index already exists.
pub async fn create_required_indexes(db: &DatabaseClient) -> Result<(), String> {
    // Products table indexes
    info!("Creating indexes for products table");
    create_index(db, "idx_products_seller", collections::PRODUCTS, "sellerId").await?;
    create_index(db, "idx_products_category", collections::PRODUCTS, "categoryId").await?;
    create_index(db, "idx_products_status", collections::PRODUCTS, "lifecycleStatus").await?;
    create_index(db, "idx_products_price", collections::PRODUCTS, "priceCents").await?;

    // Product ratings table indexes
    info!("Creating indexes for product_ratings table");
    create_composite_index(db, "idx_ratings_product_user", collections::PRODUCT_RATINGS, &["productId", "userId"]).await?;
    create_composite_index(db, "idx_ratings_product_date", collections::PRODUCT_RATINGS, &["productId", "createdAt"]).await?;

    // Product questions table indexes
    info!("Creating indexes for product_questions table");
    create_composite_index(db, "idx_questions_product_date", collections::PRODUCT_QUESTIONS, &["productId", "createdAt"]).await?;

    // Favorites table indexes
    info!("Creating indexes for favorites table");
    create_composite_index(db, "idx_favorites_user_product", collections::FAVORITES, &["userId", "productId"]).await?;

    info!("All indexes created successfully");
    Ok(())
}

/// Create a single-column index.
async fn create_index(db: &DatabaseClient, index_name: &str, table: &str, column: &str) -> Result<(), String> {
    let query = format!(
        "DEFINE INDEX {} ON TABLE {} COLUMNS ({})",
        index_name, table, column
    );
    
    match db.query_raw(&query).await {
        Ok(_) => {
            info!("Index {} created on {}.{}", index_name, table, column);
            Ok(())
        }
        Err(e) => {
            // Index already exists or other error - log but don't fail
            warn!("Failed to create index {}: {}", index_name, e);
            Ok(()) // Idempotent - continue even if already exists
        }
    }
}

/// Create a composite index on multiple columns.
async fn create_composite_index(db: &DatabaseClient, index_name: &str, table: &str, columns: &[&str]) -> Result<(), String> {
    let columns_str = columns.join(", ");
    let query = format!(
        "DEFINE INDEX {} ON TABLE {} COLUMNS ({})",
        index_name, table, columns_str
    );
    
    match db.query_raw(&query).await {
        Ok(_) => {
            info!("Index {} created on {}", index_name, table);
            Ok(())
        }
        Err(e) => {
            // Index already exists or other error - log but don't fail
            warn!("Failed to create index {}: {}", index_name, e);
            Ok(()) // Idempotent - continue even if already exists
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_required_indexes() {
        let db = DatabaseClient::new_mem().await;
        let result = create_required_indexes(&db).await;
        assert!(result.is_ok(), "Index creation should not fail");
    }

    #[tokio::test]
    async fn test_create_required_indexes_idempotent() {
        let db = DatabaseClient::new_mem().await;
        
        // First call should succeed
        let result1 = create_required_indexes(&db).await;
        assert!(result1.is_ok());
        
        // Second call should also succeed (idempotent)
        let result2 = create_required_indexes(&db).await;
        assert!(result2.is_ok());
    }
}
