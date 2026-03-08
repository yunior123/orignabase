use crate::DatabaseClient;
use ob_core::{Error, Result};
use serde_json::Value;
use surrealdb::RecordId;

/// Generic record wrapper that handles SurrealDB's RecordId type.
#[derive(Debug, serde::Deserialize)]
struct Record {
    id: RecordId,
    #[serde(flatten)]
    rest: std::collections::HashMap<String, Value>,
}

impl Record {
    fn into_value(self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("id".to_string(), Value::String(self.id.to_string()));
        for (k, v) in self.rest {
            map.insert(k, v);
        }
        Value::Object(map)
    }
}

/// Extract records from a SurrealDB response, converting RecordIds to strings.
fn take_records(response: &mut surrealdb::Response, index: usize) -> Result<Vec<Value>> {
    let records: Vec<Record> = response
        .take(index)
        .map_err(|e| Error::Database(format!("Result extraction failed: {e}")))?;
    Ok(records.into_iter().map(Record::into_value).collect())
}

impl DatabaseClient {
    /// Create a document in a collection. Returns the created document.
    pub async fn create_document(&self, collection: &str, data: Value) -> Result<Value> {
        let query = format!("CREATE {collection} CONTENT $data RETURN AFTER");
        let mut response = self
            .inner()
            .query(&query)
            .bind(("data", data))
            .await
            .map_err(|e| Error::Database(format!("Create failed: {e}")))?;

        let results = take_records(&mut response, 0)?;

        results
            .into_iter()
            .next()
            .ok_or_else(|| Error::Database("Create returned no result".into()))
    }

    /// Get a document by its record ID (e.g., "products:abc123").
    pub async fn get_document(&self, collection: &str, id: &str) -> Result<Value> {
        let record_id = format!("{collection}:{id}");
        let query = format!("SELECT * FROM {collection}:{id}");
        let mut response = self
            .inner()
            .query(&query)
            .await
            .map_err(|e| Error::Database(format!("Get failed: {e}")))?;

        let results = take_records(&mut response, 0)?;

        results
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("Document {record_id} not found")))
    }

    /// Update a document by ID. Merges fields with existing document.
    pub async fn update_document(&self, collection: &str, id: &str, data: Value) -> Result<Value> {
        let record_id = format!("{collection}:{id}");
        let query = format!("UPDATE {collection}:{id} MERGE $data RETURN AFTER");
        let mut response = self
            .inner()
            .query(&query)
            .bind(("data", data))
            .await
            .map_err(|e| Error::Database(format!("Update failed: {e}")))?;

        let results = take_records(&mut response, 0)?;

        results
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("Document {record_id} not found")))
    }

    /// Delete a document by ID.
    pub async fn delete_document(&self, collection: &str, id: &str) -> Result<Value> {
        let record_id = format!("{collection}:{id}");
        let query = format!("DELETE {collection}:{id} RETURN BEFORE");
        let mut response = self
            .inner()
            .query(&query)
            .await
            .map_err(|e| Error::Database(format!("Delete failed: {e}")))?;

        let results = take_records(&mut response, 0)?;

        results
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("Document {record_id} not found")))
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

        take_records(&mut response, 0)
    }

    /// Execute a raw SurrealQL query that returns records.
    pub async fn query_raw(&self, query: &str) -> Result<Vec<Value>> {
        let mut response = self
            .inner()
            .query(query)
            .await
            .map_err(|e| Error::Database(format!("Query failed: {e}")))?;

        take_records(&mut response, 0)
    }

    /// Execute a raw SurrealQL query that returns non-record data (e.g., INFO FOR DB).
    pub async fn query_raw_value(&self, query: &str) -> Result<Value> {
        let mut response = self
            .inner()
            .query(query)
            .await
            .map_err(|e| Error::Database(format!("Query failed: {e}")))?;

        let result: Option<Value> = response
            .take(0)
            .map_err(|e| Error::Database(format!("Result extraction failed: {e}")))?;

        Ok(result.unwrap_or(Value::Null))
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

        take_records(&mut response, 0)
    }
}
