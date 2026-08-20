//! CRUD operations delegating to PgDatabaseStore.
//!
//! All methods mirror the original API so handler code
//! requires zero changes.

use crate::DatabaseClient;
use ob_core::ports::db_store::DatabaseStore;
use serde_json::Value;

impl DatabaseClient {
    /// Create a document in a collection. Returns the created document.
    pub async fn create_document(&self, collection: &str, data: Value) -> ob_core::Result<Value> {
        self.inner.create_document(collection, data).await
    }

    /// Get a document by its record ID.
    pub async fn get_document(&self, collection: &str, id: &str) -> ob_core::Result<Value> {
        self.inner.get_document(collection, id).await
    }

    /// Update a document by ID. Merges fields with existing document.
    pub async fn update_document(
        &self,
        collection: &str,
        id: &str,
        data: Value,
    ) -> ob_core::Result<Value> {
        self.inner.update_document(collection, id, data).await
    }

    /// Create or replace a document at an explicit ID.
    pub async fn upsert_document(
        &self,
        collection: &str,
        id: &str,
        data: Value,
    ) -> ob_core::Result<Value> {
        self.inner.upsert_document(collection, id, data).await
    }

    /// Delete a document by ID.
    pub async fn delete_document(&self, collection: &str, id: &str) -> ob_core::Result<Value> {
        self.inner.delete_document(collection, id).await
    }

    /// List documents in a collection with optional limit and offset.
    pub async fn list_documents(
        &self,
        collection: &str,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> ob_core::Result<Vec<Value>> {
        self.inner.list_documents(collection, limit, offset).await
    }

    /// Execute a raw query that returns records.
    pub async fn query_raw(&self, query: &str) -> ob_core::Result<Vec<Value>> {
        self.inner.query_raw(query).await
    }

    /// Execute a raw query that returns a single value.
    pub async fn query_raw_value(&self, query: &str) -> ob_core::Result<Value> {
        self.inner.query_raw_value(query).await
    }

    /// Execute a parameterized query returning rows.
    pub async fn query_bind(
        &self,
        query: &str,
        binds: impl serde::Serialize + 'static,
    ) -> ob_core::Result<Vec<Value>> {
        let binds_value = serde_json::to_value(&binds)
            .map_err(|e| ob_core::Error::Database(format!("Bind serialization failed: {e}")))?;
        self.inner.query_bind(query, binds_value).await
    }

    /// Execute a parameterized query returning rows (alternative API).
    pub async fn query_bind_value(
        &self,
        query: &str,
        binds: impl serde::Serialize + 'static,
    ) -> ob_core::Result<Vec<Value>> {
        let binds_value = serde_json::to_value(&binds)
            .map_err(|e| ob_core::Error::Database(format!("Bind serialization failed: {e}")))?;
        self.inner.query_bind_value(query, binds_value).await
    }

    /// Batch create multiple documents.
    pub async fn batch_create(
        &self,
        collection: &str,
        docs: Vec<Value>,
    ) -> ob_core::Result<Vec<Value>> {
        self.inner.batch_create(collection, docs).await
    }

    /// Batch update multiple documents.
    pub async fn batch_update(
        &self,
        collection: &str,
        updates: Vec<(String, Value)>,
    ) -> ob_core::Result<Vec<Value>> {
        self.inner.batch_update(collection, updates).await
    }

    /// Batch delete multiple documents by ID.
    pub async fn batch_delete(
        &self,
        collection: &str,
        ids: Vec<String>,
    ) -> ob_core::Result<Vec<Value>> {
        self.inner.batch_delete(collection, ids).await
    }

    /// Update with FieldValue markers (increment, arrayUnion, serverTimestamp, etc.).
    pub async fn update_with_field_values(
        &self,
        collection: &str,
        id: &str,
        data: Value,
    ) -> ob_core::Result<Value> {
        self.inner
            .update_with_field_values(collection, id, data)
            .await
    }

    /// Compare-and-swap update: only applies the update if the specified field matches
    /// the expected value. Returns `Some(doc)` on success, `None` if precondition failed.
    pub async fn update_document_cas(
        &self,
        collection: &str,
        id: &str,
        data: Value,
        check_field: &str,
        check_value: &Value,
    ) -> ob_core::Result<Option<Value>> {
        self.inner
            .update_document_cas(collection, id, data, check_field, check_value)
            .await
    }

    // ── Filter-based queries (hexagonal — no SQL in handlers) ───────

    /// Find documents where field matches value with operator.
    pub async fn find_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        value: &Value,
        limit: Option<usize>,
    ) -> ob_core::Result<Vec<Value>> {
        self.inner
            .find_where(collection, field, operator, value, limit)
            .await
    }

    /// Find documents matching multiple field conditions (AND).
    pub async fn find_where_multi(
        &self,
        collection: &str,
        filters: &[(String, String, Value)],
        order_by: Option<&str>,
        order_dir: Option<&str>,
        limit: Option<usize>,
    ) -> ob_core::Result<Vec<Value>> {
        self.inner
            .find_where_multi(collection, filters, order_by, order_dir, limit)
            .await
    }

    /// Count documents matching a field condition.
    pub async fn count_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        value: &Value,
    ) -> ob_core::Result<usize> {
        self.inner
            .count_where(collection, field, operator, value)
            .await
    }

    /// Check if any document exists matching a field condition.
    pub async fn exists_where(
        &self,
        collection: &str,
        field: &str,
        value: &Value,
    ) -> ob_core::Result<bool> {
        self.inner.exists_where(collection, field, value).await
    }

    /// Update all documents matching a field condition.
    pub async fn update_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        field_value: &Value,
        data: Value,
    ) -> ob_core::Result<Vec<Value>> {
        self.inner
            .update_where(collection, field, operator, field_value, data)
            .await
    }

    /// Delete all documents matching a field condition.
    pub async fn delete_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        value: &Value,
    ) -> ob_core::Result<usize> {
        self.inner
            .delete_where(collection, field, operator, value)
            .await
    }

    /// Vector similarity search.
    pub async fn vector_search(
        &self,
        collection: &str,
        vector_field: &str,
        _embedding: Vec<f32>,
        top_k: usize,
        _threshold: Option<f64>,
    ) -> ob_core::Result<Vec<Value>> {
        // Requires pgvector extension and vector column setup
        let _ = (collection, vector_field, top_k);
        Err(ob_core::Error::Database(
            "vector_search not yet implemented for PostgreSQL adapter".into(),
        ))
    }
}
