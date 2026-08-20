//! DatabaseStore trait implementation for DatabaseClient.
//!
//! Delegates all operations to the underlying PgDatabaseStore adapter.

use crate::DatabaseClient;
use ob_core::ports::db_store::{AppResult, DatabaseStore};
use serde_json::Value;

impl DatabaseStore for DatabaseClient {
    // ── CRUD ────────────────────────────────────────────────────────────

    async fn create_document(&self, collection: &str, data: Value) -> AppResult<Value> {
        self.create_document(collection, data).await
    }

    async fn get_document(&self, collection: &str, id: &str) -> AppResult<Value> {
        self.get_document(collection, id).await
    }

    async fn update_document(&self, collection: &str, id: &str, data: Value) -> AppResult<Value> {
        self.update_document(collection, id, data).await
    }

    async fn upsert_document(&self, collection: &str, id: &str, data: Value) -> AppResult<Value> {
        self.upsert_document(collection, id, data).await
    }

    async fn delete_document(&self, collection: &str, id: &str) -> AppResult<Value> {
        self.delete_document(collection, id).await
    }

    async fn list_documents(
        &self,
        collection: &str,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> AppResult<Vec<Value>> {
        self.list_documents(collection, limit, offset).await
    }

    // ── Batch ───────────────────────────────────────────────────────────

    async fn batch_create(&self, collection: &str, docs: Vec<Value>) -> AppResult<Vec<Value>> {
        self.batch_create(collection, docs).await
    }

    async fn batch_update(
        &self,
        collection: &str,
        updates: Vec<(String, Value)>,
    ) -> AppResult<Vec<Value>> {
        self.batch_update(collection, updates).await
    }

    async fn batch_delete(&self, collection: &str, ids: Vec<String>) -> AppResult<Vec<Value>> {
        self.batch_delete(collection, ids).await
    }

    // ── Raw queries ─────────────────────────────────────────────────────

    async fn query_raw(&self, query: &str) -> AppResult<Vec<Value>> {
        self.query_raw(query).await
    }

    async fn query_raw_value(&self, query: &str) -> AppResult<Value> {
        self.query_raw_value(query).await
    }

    async fn query_bind(&self, query: &str, binds: Value) -> AppResult<Vec<Value>> {
        self.query_bind(query, binds).await
    }

    async fn query_bind_value(&self, query: &str, binds: Value) -> AppResult<Vec<Value>> {
        self.query_bind_value(query, binds).await
    }

    // ── FieldValue operations ───────────────────────────────────────────

    async fn update_with_field_values(
        &self,
        collection: &str,
        id: &str,
        data: Value,
    ) -> AppResult<Value> {
        self.update_with_field_values(collection, id, data).await
    }

    async fn update_document_cas(
        &self,
        collection: &str,
        id: &str,
        data: Value,
        check_field: &str,
        check_value: &Value,
    ) -> AppResult<Option<Value>> {
        self.update_document_cas(collection, id, data, check_field, check_value)
            .await
    }

    // ── Filter-based queries ───────────────────────────────────────────

    async fn find_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        value: &Value,
        limit: Option<usize>,
    ) -> AppResult<Vec<Value>> {
        self.find_where(collection, field, operator, value, limit)
            .await
    }

    async fn find_where_multi(
        &self,
        collection: &str,
        filters: &[(String, String, Value)],
        order_by: Option<&str>,
        order_dir: Option<&str>,
        limit: Option<usize>,
    ) -> AppResult<Vec<Value>> {
        self.find_where_multi(collection, filters, order_by, order_dir, limit)
            .await
    }

    async fn count_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        value: &Value,
    ) -> AppResult<usize> {
        self.count_where(collection, field, operator, value).await
    }

    async fn exists_where(&self, collection: &str, field: &str, value: &Value) -> AppResult<bool> {
        self.exists_where(collection, field, value).await
    }

    async fn update_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        field_value: &Value,
        data: Value,
    ) -> AppResult<Vec<Value>> {
        self.update_where(collection, field, operator, field_value, data)
            .await
    }

    async fn delete_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        value: &Value,
    ) -> AppResult<usize> {
        self.delete_where(collection, field, operator, value).await
    }
}
