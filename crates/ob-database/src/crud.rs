use crate::DatabaseClient;
use ob_core::{Error, Result};
use serde_json::Value;

impl DatabaseClient {
    /// Create a document in a collection. Returns the created document.
    pub async fn create_document(&self, collection: &str, data: Value) -> Result<Value> {
        let result: Option<Value> = self
            .inner()
            .create(collection)
            .content(data)
            .await
            .map_err(|e| Error::Database(format!("Create failed: {e}")))?;

        result.ok_or_else(|| Error::Database("Create returned no result".into()))
    }

    /// Get a document by its record ID (e.g., "products:abc123").
    pub async fn get_document(&self, collection: &str, id: &str) -> Result<Value> {
        let record_id = format!("{collection}:{id}");
        let result: Option<Value> = self
            .inner()
            .select((collection, id))
            .await
            .map_err(|e| Error::Database(format!("Get failed: {e}")))?;

        result.ok_or_else(|| Error::NotFound(format!("Document {record_id} not found")))
    }

    /// Update a document by ID. Merges fields with existing document.
    pub async fn update_document(&self, collection: &str, id: &str, data: Value) -> Result<Value> {
        let result: Option<Value> = self
            .inner()
            .update((collection, id))
            .merge(data)
            .await
            .map_err(|e| Error::Database(format!("Update failed: {e}")))?;

        result.ok_or_else(|| Error::NotFound(format!("Document {collection}:{id} not found")))
    }

    /// Delete a document by ID.
    pub async fn delete_document(&self, collection: &str, id: &str) -> Result<Value> {
        let result: Option<Value> = self
            .inner()
            .delete((collection, id))
            .await
            .map_err(|e| Error::Database(format!("Delete failed: {e}")))?;

        result.ok_or_else(|| Error::NotFound(format!("Document {collection}:{id} not found")))
    }

    /// List documents in a collection with optional limit.
    pub async fn list_documents(
        &self,
        collection: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Value>> {
        let query = match limit {
            Some(n) => format!("SELECT * FROM {collection} LIMIT {n}"),
            None => format!("SELECT * FROM {collection}"),
        };

        let mut response = self
            .inner()
            .query(&query)
            .await
            .map_err(|e| Error::Database(format!("List failed: {e}")))?;

        let results: Vec<Value> = response
            .take(0)
            .map_err(|e| Error::Database(format!("Result extraction failed: {e}")))?;

        Ok(results)
    }

    /// Execute a raw SurrealQL query.
    pub async fn query_raw(&self, query: &str) -> Result<Vec<Value>> {
        let mut response = self
            .inner()
            .query(query)
            .await
            .map_err(|e| Error::Database(format!("Query failed: {e}")))?;

        let results: Vec<Value> = response
            .take(0)
            .map_err(|e| Error::Database(format!("Result extraction failed: {e}")))?;

        Ok(results)
    }

    /// Execute a parameterized SurrealQL query (safe from injection).
    pub async fn query_bind(
        &self,
        query: &str,
        binds: impl serde::Serialize + 'static,
    ) -> Result<Vec<Value>> {
        let mut response = self
            .inner()
            .query(query)
            .bind(binds)
            .await
            .map_err(|e| Error::Database(format!("Query failed: {e}")))?;

        let results: Vec<Value> = response
            .take(0)
            .map_err(|e| Error::Database(format!("Result extraction failed: {e}")))?;

        Ok(results)
    }
}
