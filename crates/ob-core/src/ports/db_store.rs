//! Database abstraction layer — the "port" in hexagonal architecture.
//!
//! All handlers interact with the database exclusively through this trait.
//! Concrete adapters (PostgreSQL) implement it in `ob-database`.
//! Changing the backing database = writing a new adapter, zero handler changes.

use serde_json::Value;

pub type AppResult<T> = crate::Result<T>;

// ─── DatabaseStore ─────────────────────────────────────────────────────────

/// Database-agnostic CRUD + query interface.
///
/// Uses RPITIT (return-position impl trait in traits) for native async support
/// on edition 2024. The concrete adapter is resolved at compile time via
/// generics, avoiding the overhead of `async-trait` boxing.
pub trait DatabaseStore: Send + Sync {
    // ── CRUD ────────────────────────────────────────────────────────────

    fn create_document(
        &self,
        collection: &str,
        data: Value,
    ) -> impl std::future::Future<Output = AppResult<Value>> + Send;

    fn get_document(
        &self,
        collection: &str,
        id: &str,
    ) -> impl std::future::Future<Output = AppResult<Value>> + Send;

    fn update_document(
        &self,
        collection: &str,
        id: &str,
        data: Value,
    ) -> impl std::future::Future<Output = AppResult<Value>> + Send;

    fn upsert_document(
        &self,
        collection: &str,
        id: &str,
        data: Value,
    ) -> impl std::future::Future<Output = AppResult<Value>> + Send;

    fn delete_document(
        &self,
        collection: &str,
        id: &str,
    ) -> impl std::future::Future<Output = AppResult<Value>> + Send;

    fn list_documents(
        &self,
        collection: &str,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> impl std::future::Future<Output = AppResult<Vec<Value>>> + Send;

    // ── Batch ───────────────────────────────────────────────────────────

    fn batch_create(
        &self,
        collection: &str,
        docs: Vec<Value>,
    ) -> impl std::future::Future<Output = AppResult<Vec<Value>>> + Send;

    fn batch_update(
        &self,
        collection: &str,
        updates: Vec<(String, Value)>,
    ) -> impl std::future::Future<Output = AppResult<Vec<Value>>> + Send;

    fn batch_delete(
        &self,
        collection: &str,
        ids: Vec<String>,
    ) -> impl std::future::Future<Output = AppResult<Vec<Value>>> + Send;

    // ── Raw queries ─────────────────────────────────────────────────────

    fn query_raw(
        &self,
        query: &str,
    ) -> impl std::future::Future<Output = AppResult<Vec<Value>>> + Send;

    fn query_raw_value(
        &self,
        query: &str,
    ) -> impl std::future::Future<Output = AppResult<Value>> + Send;

    fn query_bind(
        &self,
        query: &str,
        binds: Value,
    ) -> impl std::future::Future<Output = AppResult<Vec<Value>>> + Send;

    fn query_bind_value(
        &self,
        query: &str,
        binds: Value,
    ) -> impl std::future::Future<Output = AppResult<Vec<Value>>> + Send;

    // ── FieldValue operations ───────────────────────────────────────────

    fn update_with_field_values(
        &self,
        collection: &str,
        id: &str,
        data: Value,
    ) -> impl std::future::Future<Output = AppResult<Value>> + Send;

    // ── Compare-and-swap (CAS) ─────────────────────────────────────────

    /// Update a document only if a JSONB field matches the expected value.
    /// Returns `Some(updated_doc)` on success, `None` if the precondition failed.
    /// This prevents TOCTOU race conditions without requiring transactions.
    fn update_document_cas(
        &self,
        collection: &str,
        id: &str,
        data: Value,
        check_field: &str,
        check_value: &Value,
    ) -> impl std::future::Future<Output = AppResult<Option<Value>>> + Send;

    // ── Filter-based queries (eliminates raw SQL from handlers) ────────

    /// Find documents where a single field matches a value.
    /// Operators: "=", "!=", "<", ">", "<=", ">=".
    fn find_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        value: &Value,
        limit: Option<usize>,
    ) -> impl std::future::Future<Output = AppResult<Vec<Value>>> + Send;

    /// Find documents matching multiple field conditions (AND).
    /// Each filter: (field, operator, value).
    fn find_where_multi(
        &self,
        collection: &str,
        filters: &[(String, String, Value)],
        order_by: Option<&str>,
        order_dir: Option<&str>,
        limit: Option<usize>,
    ) -> impl std::future::Future<Output = AppResult<Vec<Value>>> + Send;

    /// Count documents matching a field condition.
    fn count_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        value: &Value,
    ) -> impl std::future::Future<Output = AppResult<usize>> + Send;

    /// Check if any document exists matching a field condition.
    fn exists_where(
        &self,
        collection: &str,
        field: &str,
        value: &Value,
    ) -> impl std::future::Future<Output = AppResult<bool>> + Send;

    /// Update all documents matching a field condition.
    fn update_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        field_value: &Value,
        data: Value,
    ) -> impl std::future::Future<Output = AppResult<Vec<Value>>> + Send;

    /// Delete all documents matching a field condition.
    fn delete_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        value: &Value,
    ) -> impl std::future::Future<Output = AppResult<usize>> + Send;
}
