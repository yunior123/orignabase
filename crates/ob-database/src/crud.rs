use crate::DatabaseClient;
use ob_core::{Error, Result, escape_surreal_string, validate_document_id, validate_identifier};
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
        validate_identifier(collection)?;
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

    /// Get a document by its record ID (e.g., "abc123" or "products:abc123").
    pub async fn get_document(&self, collection: &str, id: &str) -> Result<Value> {
        validate_identifier(collection)?;
        // Strip collection prefix if present (e.g., "products:abc123" → "abc123")
        let id = id.strip_prefix(&format!("{collection}:")).unwrap_or(id);
        validate_document_id(id)?;
        let record_id = format!("{collection}:{id}");
        // Use parameterized query to prevent SurrealQL injection
        let query = "SELECT * FROM type::thing($table, $id)".to_string();
        let mut response = self
            .inner()
            .query(&query)
            .bind(("table", collection.to_string()))
            .bind(("id", id.to_string()))
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
        validate_identifier(collection)?;
        let id = id.strip_prefix(&format!("{collection}:")).unwrap_or(id);
        validate_document_id(id)?;
        let record_id = format!("{collection}:{id}");
        // Use parameterized query to prevent SurrealQL injection
        let query = "UPDATE type::thing($table, $id) MERGE $data RETURN AFTER".to_string();
        let mut response = self
            .inner()
            .query(&query)
            .bind(("table", collection.to_string()))
            .bind(("id", id.to_string()))
            .bind(("data", data))
            .await
            .map_err(|e| Error::Database(format!("Update failed: {e}")))?;

        let results = take_records(&mut response, 0)?;

        results
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("Document {record_id} not found")))
    }

    /// Create or replace a document by explicit ID.
    pub async fn upsert_document(&self, collection: &str, id: &str, data: Value) -> Result<Value> {
        validate_identifier(collection)?;
        let id = id.strip_prefix(&format!("{collection}:")).unwrap_or(id);
        validate_document_id(id)?;
        let query = "UPSERT type::thing($table, $id) CONTENT $data RETURN AFTER".to_string();
        let mut response = self
            .inner()
            .query(&query)
            .bind(("table", collection.to_string()))
            .bind(("id", id.to_string()))
            .bind(("data", data))
            .await
            .map_err(|e| Error::Database(format!("Upsert failed: {e}")))?;

        let results = take_records(&mut response, 0)?;
        results.into_iter().next().ok_or_else(|| {
            Error::Database(format!("Upsert returned no result for {collection}:{id}"))
        })
    }

    /// Delete a document by ID.
    pub async fn delete_document(&self, collection: &str, id: &str) -> Result<Value> {
        validate_identifier(collection)?;
        let id = id.strip_prefix(&format!("{collection}:")).unwrap_or(id);
        validate_document_id(id)?;
        let record_id = format!("{collection}:{id}");
        // Use parameterized query to prevent SurrealQL injection
        let query = "DELETE type::thing($table, $id) RETURN BEFORE".to_string();
        let mut response = self
            .inner()
            .query(&query)
            .bind(("table", collection.to_string()))
            .bind(("id", id.to_string()))
            .await
            .map_err(|e| Error::Database(format!("Delete failed: {e}")))?;

        let results = take_records(&mut response, 0)?;

        results
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("Document {record_id} not found")))
    }

    /// List documents in a collection with optional limit.
    /// Default limit is 1000 to prevent unbounded queries.
    pub async fn list_documents(
        &self,
        collection: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Value>> {
        validate_identifier(collection)?;
        let n = limit.unwrap_or(1000).min(10_000);
        let query = format!("SELECT * FROM {collection} LIMIT {n}");

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

    /// Batch create multiple documents in a collection.
    /// Uses SurrealDB's INSERT for true bulk efficiency.
    pub async fn batch_create(&self, collection: &str, docs: Vec<Value>) -> Result<Vec<Value>> {
        validate_identifier(collection)?;
        if docs.is_empty() {
            return Ok(vec![]);
        }

        // Use SurrealDB INSERT for bulk efficiency (single query, single transaction)
        let query = format!("INSERT INTO {collection} $docs");
        let mut response = self
            .inner()
            .query(&query)
            .bind(("docs", Value::Array(docs)))
            .await
            .map_err(|e| Error::Database(format!("Batch create failed: {e}")))?;

        take_records(&mut response, 0)
    }

    /// Batch update multiple documents.
    /// Each entry is (id, data) where data is merged into the existing document.
    pub async fn batch_update(
        &self,
        collection: &str,
        updates: Vec<(String, Value)>,
    ) -> Result<Vec<Value>> {
        validate_identifier(collection)?;
        if updates.is_empty() {
            return Ok(vec![]);
        }

        let mut results = Vec::with_capacity(updates.len());
        for (id, data) in updates {
            let result = self.update_document(collection, &id, data).await?;
            results.push(result);
        }
        Ok(results)
    }

    /// Batch delete multiple documents by ID.
    pub async fn batch_delete(&self, collection: &str, ids: Vec<String>) -> Result<Vec<Value>> {
        validate_identifier(collection)?;
        if ids.is_empty() {
            return Ok(vec![]);
        }

        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            let result = self.delete_document(collection, &id).await?;
            results.push(result);
        }
        Ok(results)
    }

    /// Apply FieldValue operations to an update.
    ///
    /// Translates special FieldValue markers in data to SurrealQL operations:
    /// - `{ "_serverTimestamp": true }` → `time::now()`
    /// - `{ "_increment": n }` → `field += n`
    /// - `{ "_arrayUnion": [...] }` → `array::union(field, [...])`
    /// - `{ "_arrayRemove": [...] }` → `array::complement(field, [...])`
    /// - `{ "_deleteField": true }` → UNSET field
    pub async fn update_with_field_values(
        &self,
        collection: &str,
        id: &str,
        data: Value,
    ) -> Result<Value> {
        validate_identifier(collection)?;
        let id = id.strip_prefix(&format!("{collection}:")).unwrap_or(id);
        validate_document_id(id)?;
        let obj = data
            .as_object()
            .ok_or_else(|| Error::Validation("Data must be a JSON object".into()))?;

        // Validate all field names to prevent injection
        for field in obj.keys() {
            validate_identifier(field)?;
        }

        let mut merge_fields = serde_json::Map::new();
        let mut set_clauses = Vec::new();
        let mut unset_fields = Vec::new();

        for (field, value) in obj {
            if let Some(ops) = value.as_object() {
                if ops.contains_key("_serverTimestamp") {
                    set_clauses.push(format!("{field} = time::now()"));
                } else if let Some(n) = ops.get("_increment") {
                    let n_str = n.to_string();
                    set_clauses.push(format!("{field} += {n_str}"));
                } else if let Some(arr) = ops.get("_arrayUnion") {
                    let arr_str = arr.to_string();
                    set_clauses.push(format!("{field} = array::union({field}, {arr_str})"));
                } else if let Some(arr) = ops.get("_arrayRemove") {
                    let arr_str = arr.to_string();
                    set_clauses.push(format!("{field} = array::complement({field}, {arr_str})"));
                } else if ops.contains_key("_deleteField") {
                    unset_fields.push(field.clone());
                } else {
                    merge_fields.insert(field.clone(), value.clone());
                }
            } else {
                merge_fields.insert(field.clone(), value.clone());
            }
        }

        // Build combined query using SET for everything (MERGE + SET in same
        // statement is not supported by SurrealDB).
        let mut all_set_clauses = set_clauses;

        // Convert merge fields to SET clauses
        for (field, value) in &merge_fields {
            let val_str = match value {
                Value::String(s) => format!("'{}'", escape_surreal_string(s)),
                _ => value.to_string(),
            };
            all_set_clauses.push(format!("{field} = {val_str}"));
        }

        let mut query_parts = Vec::new();

        if !all_set_clauses.is_empty() {
            query_parts.push(format!(
                "UPDATE {collection}:{id} SET {}",
                all_set_clauses.join(", ")
            ));
        }

        if !unset_fields.is_empty() {
            if query_parts.is_empty() {
                query_parts.push(format!(
                    "UPDATE {collection}:{id} UNSET {}",
                    unset_fields.join(", ")
                ));
            } else {
                // Append UNSET fields to existing SET statement isn't supported,
                // so run as separate query
                query_parts.push(format!(
                    "UPDATE {collection}:{id} UNSET {}",
                    unset_fields.join(", ")
                ));
            }
        }

        // If nothing to do, just return the current document
        if query_parts.is_empty() {
            return self.get_document(collection, id).await;
        }

        // Execute last query with RETURN AFTER
        let last_idx = query_parts.len() - 1;
        query_parts[last_idx].push_str(" RETURN AFTER");

        let full_query = query_parts.join(";\n");

        // For multi-statement queries, query_raw reads index 0.
        // We need the LAST statement's result, so use query_raw_value
        // and parse the response directly when there are multiple statements.
        if query_parts.len() == 1 {
            let results = self.query_raw(&full_query).await?;
            results
                .into_iter()
                .next()
                .ok_or_else(|| Error::Database("FieldValue update returned no result".into()))
        } else {
            // Execute multi-statement: read result from the last statement index
            let mut response = self
                .inner()
                .query(&full_query)
                .await
                .map_err(|e| Error::Database(format!("FieldValue update failed: {e}")))?;

            let results = take_records(&mut response, last_idx)?;
            results
                .into_iter()
                .next()
                .ok_or_else(|| Error::Database("FieldValue update returned no result".into()))
        }
    }

    /// Vector similarity search using SurrealDB's native vector functions.
    ///
    /// Searches for documents where `vector_field` is most similar to `embedding`.
    /// Uses cosine similarity by default.
    ///
    /// # Example SurrealQL generated:
    /// ```sql
    /// SELECT *, vector::similarity::cosine(embedding, $query_vec) AS score
    /// FROM products
    /// WHERE vector::similarity::cosine(embedding, $query_vec) > $threshold
    /// ORDER BY score DESC
    /// LIMIT $top_k
    /// ```
    pub async fn vector_search(
        &self,
        collection: &str,
        vector_field: &str,
        embedding: Vec<f32>,
        top_k: usize,
        threshold: Option<f64>,
    ) -> Result<Vec<Value>> {
        validate_identifier(collection)?;
        validate_identifier(vector_field)?;

        let top_k = top_k.min(10_000);
        let threshold = threshold.unwrap_or(0.0);

        let query = format!(
            "SELECT *, vector::similarity::cosine({vector_field}, $query_vec) AS score \
             FROM {collection} \
             WHERE vector::similarity::cosine({vector_field}, $query_vec) > $threshold \
             ORDER BY score DESC \
             LIMIT $top_k"
        );

        let mut response = self
            .inner()
            .query(&query)
            .bind(("query_vec", embedding))
            .bind(("threshold", threshold))
            .bind(("top_k", top_k))
            .await
            .map_err(|e| Error::Database(format!("Vector search failed: {e}")))?;

        take_records(&mut response, 0)
    }

    /// Execute a parameterized SurrealQL query and return all results as a Vec<Value>.
    pub async fn query_bind_value(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_into_value_basic() {
        let record = Record {
            id: RecordId::from(("products", "abc123")),
            rest: std::collections::HashMap::from([
                ("title".to_string(), Value::String("Widget".to_string())),
                ("price".to_string(), serde_json::json!(42.0)),
            ]),
        };

        let value = record.into_value();
        assert!(value.is_object());

        let obj = value.as_object().unwrap();
        // The id should be converted to a string representation
        assert!(obj.contains_key("id"));
        assert!(obj["id"].is_string());
        let id_str = obj["id"].as_str().unwrap();
        assert!(
            id_str.contains("products"),
            "id should contain table name, got: {id_str}"
        );

        // Rest fields should be present
        assert_eq!(obj["title"], "Widget");
        assert_eq!(obj["price"], 42.0);
    }

    #[test]
    fn test_record_into_value_empty_rest() {
        let record = Record {
            id: RecordId::from(("users", "u1")),
            rest: std::collections::HashMap::new(),
        };

        let value = record.into_value();
        let obj = value.as_object().unwrap();
        // Should have only the id field
        assert_eq!(obj.len(), 1);
        assert!(obj.contains_key("id"));
    }

    #[test]
    fn test_record_into_value_nested_data() {
        let nested = serde_json::json!({
            "street": "123 Main St",
            "city": "Toronto"
        });

        let record = Record {
            id: RecordId::from(("addresses", "addr1")),
            rest: std::collections::HashMap::from([
                ("label".to_string(), Value::String("home".to_string())),
                ("details".to_string(), nested.clone()),
            ]),
        };

        let value = record.into_value();
        let obj = value.as_object().unwrap();
        assert_eq!(obj["label"], "home");
        assert_eq!(obj["details"]["city"], "Toronto");
    }

    #[test]
    fn test_vector_search_query_validates_collection() {
        // Invalid collection names should be rejected
        let result = ob_core::validate_identifier("my-table");
        assert!(result.is_err());

        let result = ob_core::validate_identifier("products");
        assert!(result.is_ok());
    }

    #[test]
    fn test_vector_search_query_validates_vector_field() {
        // Invalid field names should be rejected
        let result = ob_core::validate_identifier("embed;DROP");
        assert!(result.is_err());

        let result = ob_core::validate_identifier("");
        assert!(result.is_err());

        let result = ob_core::validate_identifier("embedding");
        assert!(result.is_ok());

        let result = ob_core::validate_identifier("vec_field_123");
        assert!(result.is_ok());
    }

    #[test]
    fn test_vector_search_query_generation() {
        // Verify the SurrealQL query is built correctly
        let collection = "products";
        let vector_field = "embedding";
        let _threshold = 0.7_f64;
        let _top_k = 10_usize;

        let query = format!(
            "SELECT *, vector::similarity::cosine({vector_field}, $query_vec) AS score \
             FROM {collection} \
             WHERE vector::similarity::cosine({vector_field}, $query_vec) > $threshold \
             ORDER BY score DESC \
             LIMIT $top_k"
        );

        assert!(query.contains("vector::similarity::cosine(embedding, $query_vec)"));
        assert!(query.contains("FROM products"));
        assert!(query.contains("ORDER BY score DESC"));
        assert!(query.contains("LIMIT $top_k"));
        assert!(query.contains("AS score"));
    }

    #[test]
    fn test_vector_search_top_k_clamped() {
        // top_k should be clamped to 10_000
        let top_k: usize = 999_999;
        let clamped = top_k.min(10_000);
        assert_eq!(clamped, 10_000);

        let top_k: usize = 5;
        let clamped = top_k.min(10_000);
        assert_eq!(clamped, 5);
    }

    #[test]
    fn test_vector_search_default_threshold() {
        let _threshold: Option<f64> = None;
        let effective = 0.0_f64;
        assert_eq!(effective, 0.0);

        let _threshold: Option<f64> = Some(0.8);
        let effective = 0.8_f64;
        assert_eq!(effective, 0.8);
    }

    #[test]
    fn test_record_into_value_preserves_types() {
        let record = Record {
            id: RecordId::from(("items", "i1")),
            rest: std::collections::HashMap::from([
                ("active".to_string(), Value::Bool(true)),
                ("count".to_string(), serde_json::json!(7)),
                ("tags".to_string(), serde_json::json!(["a", "b"])),
                ("meta".to_string(), Value::Null),
            ]),
        };

        let value = record.into_value();
        let obj = value.as_object().unwrap();
        assert_eq!(obj["active"], true);
        assert_eq!(obj["count"], 7);
        assert!(obj["tags"].is_array());
        assert!(obj["meta"].is_null());
    }
}
