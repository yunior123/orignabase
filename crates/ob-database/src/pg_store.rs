//! PostgreSQL adapter implementing `DatabaseStore`.
//!
//! Uses sqlx with connection pooling. Stores documents as JSONB rows
//! with a standard schema: `id UUID, data JSONB, created_at, updated_at`.

use crate::fields;
use ob_core::ports::db_store::{AppResult, DatabaseStore};
use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgPoolOptions};

/// PostgreSQL adapter for the DatabaseStore trait.
///
/// Documents are stored as JSONB `data` column rows, allowing the same
/// document-oriented API while gaining ACID
/// transactions, mature tooling, and PostgreSQL ecosystem access.
#[derive(Clone)]
pub struct PgDatabaseStore {
    pool: PgPool,
}

impl PgDatabaseStore {
    /// Connect to PostgreSQL with the given connection string.
    pub async fn connect(database_url: &str) -> AppResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .min_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect(database_url)
            .await
            .map_err(|e| ob_core::Error::Database(format!("PostgreSQL connection failed: {e}")))?;

        tracing::info!("Connected to PostgreSQL");
        Ok(Self { pool })
    }

    /// Connect to PostgreSQL and set an isolated schema search path on every
    /// pooled connection. Used by integration tests that need real Postgres
    /// semantics without sharing tables across parallel tests.
    pub async fn connect_to_schema(database_url: &str, schema: &str) -> AppResult<Self> {
        let search_path_sql = format!("SET search_path TO \"{schema}\", public");
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .min_connections(0)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .after_connect(move |conn, _meta| {
                let search_path_sql = search_path_sql.clone();
                Box::pin(async move {
                    sqlx::query(&search_path_sql).execute(conn).await?;
                    Ok(())
                })
            })
            .connect(database_url)
            .await
            .map_err(|e| ob_core::Error::Database(format!("PostgreSQL connection failed: {e}")))?;

        tracing::info!("Connected to PostgreSQL test schema {}", schema);
        Ok(Self { pool })
    }

    /// Create from an existing pool (useful for testing).
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Get a reference to the underlying pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Ensure a collection table exists. Creates it on first access.
    async fn ensure_table(&self, collection: &str) -> AppResult<()> {
        let table = sanitize_table_name(collection)?;
        sqlx::query(&format!(
            r#"
            CREATE TABLE IF NOT EXISTS {table} (
                id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
                data JSONB NOT NULL DEFAULT '{{}}'::jsonb,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )
            "#
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to create table {table}: {e}")))?;

        // Migrate legacy tables (from SQL migrations) to the PgDatabaseStore schema.
        // Legacy tables have explicit columns (user_id, email, etc.) and UUID id columns.
        // PgDatabaseStore stores everything in a JSONB `data` column, so we need to:
        // 1. Drop all foreign key constraints on this table
        // 2. Convert id from UUID to TEXT (for string-based IDs like "user_1")
        // 3. Drop extra columns that are NOT NULL (they'd block inserts into just id+data)
        // 4. Add the data column if missing
        sqlx::query(&format!(
            r#"
            DO $$ DECLARE
                r RECORD;
            BEGIN
                -- Drop all foreign key constraints ON this table
                FOR r IN
                    SELECT conname FROM pg_constraint
                    JOIN pg_class ON pg_constraint.conrelid = pg_class.oid
                    WHERE pg_class.relname = '{table}' AND contype = 'f'
                LOOP
                    BEGIN
                        EXECUTE 'ALTER TABLE {table} DROP CONSTRAINT ' || quote_ident(r.conname);
                    EXCEPTION WHEN OTHERS THEN NULL;
                    END;
                END LOOP;

                -- Drop all foreign key constraints REFERENCING this table (from other tables)
                FOR r IN
                    SELECT pg_class.relname AS src_table, pg_constraint.conname
                    FROM pg_constraint
                    JOIN pg_class ON pg_constraint.conrelid = pg_class.oid
                    JOIN pg_class AS ref ON pg_constraint.confrelid = ref.oid
                    WHERE ref.relname = '{table}' AND pg_constraint.contype = 'f'
                LOOP
                    BEGIN
                        EXECUTE 'ALTER TABLE ' || quote_ident(r.src_table) || ' DROP CONSTRAINT ' || quote_ident(r.conname);
                    EXCEPTION WHEN OTHERS THEN NULL;
                    END;
                END LOOP;

                -- Convert id column from UUID to TEXT if needed
                BEGIN
                    IF EXISTS (
                        SELECT 1 FROM information_schema.columns
                        WHERE table_name = '{table}' AND column_name = 'id' AND data_type = 'uuid'
                    ) THEN
                        ALTER TABLE {table} ALTER COLUMN id TYPE TEXT USING id::TEXT;
                    END IF;
                EXCEPTION WHEN OTHERS THEN NULL;
                END;

                -- Add data column if missing
                BEGIN
                    IF NOT EXISTS (
                        SELECT 1 FROM information_schema.columns
                        WHERE table_name = '{table}' AND column_name = 'data'
                    ) THEN
                        ALTER TABLE {table} ADD COLUMN data JSONB NOT NULL DEFAULT '{{}}'::jsonb;
                    END IF;
                EXCEPTION WHEN OTHERS THEN NULL;
                END;

                -- Drop extra columns that have NOT NULL constraints (would block id+data inserts)
                FOR r IN
                    SELECT column_name FROM information_schema.columns
                    WHERE table_name = '{table}'
                      AND column_name NOT IN ('id', 'data', 'created_at', 'updated_at')
                      AND is_nullable = 'NO'
                LOOP
                    BEGIN
                        EXECUTE 'ALTER TABLE {table} DROP COLUMN IF EXISTS ' || quote_ident(r.column_name);
                    EXCEPTION WHEN OTHERS THEN NULL;
                    END;
                END LOOP;
            END $$;
            "#
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| {
            ob_core::Error::Database(format!("Failed to migrate table {table}: {e}"))
        })?;

        // Ensure the updated_at trigger function exists
        match sqlx::query(
            r#"
            CREATE OR REPLACE FUNCTION set_updated_at()
            RETURNS TRIGGER AS $$
            BEGIN
                NEW.updated_at = now();
                RETURN NEW;
            END;
            $$ LANGUAGE plpgsql;
            "#,
        )
        .execute(&self.pool)
        .await
        {
            Ok(_) => {}
            Err(e) => {
                let err_str = e.to_string();
                if !err_str.contains("tuple concurrently updated") {
                    return Err(ob_core::Error::Database(format!(
                        "Failed to create set_updated_at function: {e}"
                    )));
                }
            }
        }

        // Ensure the updated_at trigger exists
        sqlx::query(&format!(
            r#"
            DO $$ BEGIN
                IF NOT EXISTS (
                    SELECT 1 FROM pg_trigger WHERE tgname = '{table}_set_updated_at'
                ) THEN
                    CREATE TRIGGER {table}_set_updated_at
                        BEFORE UPDATE ON {table}
                        FOR EACH ROW EXECUTE FUNCTION set_updated_at();
                END IF;
            END $$;
            "#
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| {
            ob_core::Error::Database(format!("Failed to create trigger for {table}: {e}"))
        })?;

        Ok(())
    }
}

/// Sanitize a collection name for use as a PostgreSQL table name.
/// Must be alphanumeric + underscores only.
fn sanitize_table_name(name: &str) -> AppResult<String> {
    ob_core::validate_identifier(name)?;
    Ok(name.to_string())
}

/// Serialize a JSONB Value to a string for sqlx binding (preserves JSON encoding).
fn json_to_string(val: &Value) -> String {
    serde_json::to_string(val).unwrap_or_else(|_| "{}".to_string())
}

/// Extract raw text from a Value for `data->>` comparisons (no JSON quotes).
/// `data->>` returns unquoted text, so binding `"\"hello\""` would fail to match `"hello"`.
fn json_to_text(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

// Translation shim removed — handlers must now write native PostgreSQL.

/// Simple named→positional parameter conversion for PostgreSQL.
/// Converts `$param_name` to `$1, $2, ...` and returns ordered values.
pub(crate) fn named_to_positional(query: &str, binds: Value) -> AppResult<(String, Vec<Value>)> {
    let obj = binds
        .as_object()
        .ok_or_else(|| ob_core::Error::Database("Binds must be a JSON object".into()))?;
    let mut pg_query = query.to_string();
    let mut values: Vec<Value> = Vec::new();
    for (i, (name, val)) in obj.iter().enumerate() {
        let named = format!("${name}");
        let positional = format!("${}", i + 1);
        pg_query = pg_query.replace(&named, &positional);
        values.push(val.clone());
    }
    Ok((pg_query, values))
}

/// Only matches keywords at word boundaries to avoid false matches in string literals.
fn extract_table_name(query: &str) -> Option<String> {
    let lower = query.to_lowercase();

    // Try each keyword pattern, ensuring word boundaries
    let patterns: &[(&str, bool)] = &[
        ("delete from", true), // multi-word, look for FROM after DELETE
        ("insert into", true), // multi-word, look for INTO after INSERT
        ("update", false),     // single keyword at statement start
        ("from", true),        // FROM in SELECT queries
    ];

    for &(keyword, is_multiword) in patterns {
        let mut search_start = 0;
        while let Some(pos) = lower[search_start..].find(keyword) {
            let abs_pos = search_start + pos;
            let after_keyword = &lower[abs_pos + keyword.len()..];

            // Check word boundary: keyword must be preceded by whitespace or start of string
            let before_ok = abs_pos == 0
                || query
                    .as_bytes()
                    .get(abs_pos - 1)
                    .is_none_or(|b| b.is_ascii_whitespace());

            // Check word boundary: keyword must be followed by whitespace
            let after_ok = after_keyword.starts_with(|c: char| c.is_ascii_whitespace());

            if before_ok && after_ok {
                // For "update" keyword, ensure it's at the start of the statement
                // (not inside a SET clause like "SET updated_at = ...")
                if !is_multiword {
                    let before_text = &lower[..abs_pos].trim();
                    if !before_text.is_empty() && !before_text.ends_with(';') {
                        // UPDATE not at statement start — skip
                        search_start = abs_pos + keyword.len();
                        continue;
                    }
                }

                let after = &query[abs_pos + keyword.len()..].trim();
                let table = after
                    .split(|c: char| c.is_whitespace() || c == '(' || c == ';')
                    .next()
                    .unwrap_or("");
                if !table.is_empty() && table.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    return Some(table.to_string());
                }
            }

            search_start = abs_pos + keyword.len();
        }
    }
    None
}

/// Bind a JSON Value to a sqlx query dynamically.
/// Numbers are bound as strings since JSONB `->>` returns text.
pub(crate) fn bind_json_value<'q>(
    q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    val: &'q Value,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match val {
        Value::Null => q.bind::<Option<String>>(None),
        Value::Bool(b) => q.bind(*b),
        Value::Number(n) => {
            // Bind as string since JSONB ->> returns text and comparisons
            // like "text >= bigint" fail in PostgreSQL
            q.bind(n.to_string())
        }
        Value::String(s) => q.bind(s.as_str()),
        Value::Array(_) | Value::Object(_) => q.bind(serde_json::to_string(val).unwrap()),
    }
}

/// Convert sqlx rows to Vec<Value> (best-effort extraction).
/// Standard columns that are always present — extra columns are SQL aliases.
const STANDARD_COLUMNS: &[&str] = &["data", "id", "created_at", "updated_at"];

fn inject_document_metadata(
    obj: &mut serde_json::Map<String, Value>,
    id: Option<String>,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
) {
    if let Some(id) = id {
        obj.insert(fields::ID.to_string(), Value::String(id));
    }
    if let Some(ts) = created_at {
        obj.insert(
            fields::CREATED_AT.to_string(),
            Value::String(ts.to_rfc3339()),
        );
    }
    if let Some(ts) = updated_at {
        obj.insert(
            fields::UPDATED_AT.to_string(),
            Value::String(ts.to_rfc3339()),
        );
    }
}

fn rows_to_values(rows: Vec<sqlx::postgres::PgRow>) -> AppResult<Vec<Value>> {
    use sqlx::{Column, Row};

    let mut results = Vec::with_capacity(rows.len());
    for row in &rows {
        // Try to get 'data' column — handle both JSONB (as serde_json::Value) and TEXT
        if let Ok(val) = row.try_get::<Value, _>("data") {
            let mut result = val;
            if let Some(obj) = result.as_object_mut() {
                inject_document_metadata(
                    obj,
                    row.try_get::<String, _>("id").ok(),
                    row.try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                        .ok(),
                    row.try_get::<chrono::DateTime<chrono::Utc>, _>("updated_at")
                        .ok(),
                );
                // Extract SQL aliases (e.g. COALESCE(...) AS roles) — these are
                // computed columns from raw queries that don't live in the JSONB data.
                for col in row.columns() {
                    let name = col.name();
                    if STANDARD_COLUMNS.contains(&name) {
                        continue;
                    }
                    if let Ok(v) = row.try_get::<Value, _>(name) {
                        obj.insert(name.to_string(), v);
                    } else if let Ok(v) = row.try_get::<bool, _>(name) {
                        obj.insert(name.to_string(), Value::Bool(v));
                    } else if let Ok(v) = row.try_get::<i64, _>(name) {
                        obj.insert(name.to_string(), Value::Number(v.into()));
                    } else if let Ok(v) = row.try_get::<String, _>(name) {
                        obj.insert(name.to_string(), Value::String(v));
                    }
                }
            }
            results.push(result);
            continue;
        }

        // Fallback: extract all columns as a JSON object (handles aggregate queries)
        let mut obj = serde_json::Map::new();
        for col in row.columns() {
            let name = col.name();
            // Try common types in order of likelihood
            if let Ok(v) = row.try_get::<i64, _>(name) {
                obj.insert(name.to_string(), Value::Number(v.into()));
            } else if let Ok(v) = row.try_get::<String, _>(name) {
                obj.insert(name.to_string(), Value::String(v));
            } else if let Ok(v) = row.try_get::<f64, _>(name) {
                if let Some(n) = serde_json::Number::from_f64(v) {
                    obj.insert(name.to_string(), Value::Number(n));
                }
            } else if let Ok(v) = row.try_get::<bool, _>(name) {
                obj.insert(name.to_string(), Value::Bool(v));
            }
        }
        if !obj.is_empty() {
            results.push(Value::Object(obj));
            continue;
        }

        // Last resort: try first column as string
        let val: Value = row
            .try_get::<String, _>(0)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null);
        results.push(val);
    }
    Ok(results)
}

impl DatabaseStore for PgDatabaseStore {
    async fn create_document(&self, collection: &str, data: Value) -> AppResult<Value> {
        self.ensure_table(collection).await?;
        let table = sanitize_table_name(collection)?;

        let id = data
            .get(fields::ID)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let data_str = json_to_string(&data);

        let row = sqlx::query(&format!(
            r#"INSERT INTO {table} (id, data) VALUES ($1, $2::jsonb)
               ON CONFLICT (id) DO NOTHING
               RETURNING id, data::TEXT, created_at, updated_at"#
        ))
        .bind(&id)
        .bind(&data_str)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Create failed: {e}")))?
        .ok_or_else(|| {
            ob_core::Error::Validation(format!(
                "Document already exists in collection {collection}: {id}"
            ))
        })?;

        let mut result: Value = serde_json::from_str(row.get::<String, _>("data").as_str())
            .unwrap_or(Value::Object(Default::default()));

        // Inject the id and timestamps into the result
        if let Some(obj) = result.as_object_mut() {
            inject_document_metadata(
                obj,
                Some(row.get::<String, _>("id")),
                Some(row.get::<chrono::DateTime<chrono::Utc>, _>("created_at")),
                Some(row.get::<chrono::DateTime<chrono::Utc>, _>("updated_at")),
            );
        }

        Ok(result)
    }

    async fn get_document(&self, collection: &str, id: &str) -> AppResult<Value> {
        self.ensure_table(collection).await?;
        let table = sanitize_table_name(collection)?;

        // Strip collection prefix if present (e.g., "products:abc" → "abc")
        let bare_id = id.strip_prefix(&format!("{collection}:")).unwrap_or(id);

        let row = sqlx::query(&format!(
            r#"SELECT id, data::TEXT, created_at, updated_at FROM {table} WHERE id = $1"#
        ))
        .bind(bare_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Get failed: {e}")))?;

        let row = row
            .ok_or_else(|| ob_core::Error::NotFound(format!("{collection}:{bare_id} not found")))?;

        let mut result: Value = serde_json::from_str(row.get::<String, _>("data").as_str())
            .unwrap_or(Value::Object(Default::default()));

        if let Some(obj) = result.as_object_mut() {
            inject_document_metadata(
                obj,
                Some(row.get(fields::ID)),
                Some(row.get::<chrono::DateTime<chrono::Utc>, _>("created_at")),
                Some(row.get::<chrono::DateTime<chrono::Utc>, _>("updated_at")),
            );
        }

        Ok(result)
    }

    async fn update_document(&self, collection: &str, id: &str, data: Value) -> AppResult<Value> {
        self.ensure_table(collection).await?;
        let table = sanitize_table_name(collection)?;
        let bare_id = id.strip_prefix(&format!("{collection}:")).unwrap_or(id);
        let data_str = json_to_string(&data);

        let row = sqlx::query(&format!(
            r#"
            UPDATE {table}
            SET data = data || $2::jsonb
            WHERE id = $1
            RETURNING id, data::TEXT, created_at, updated_at
            "#
        ))
        .bind(bare_id)
        .bind(&data_str)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Update failed: {e}")))?;

        let row = row
            .ok_or_else(|| ob_core::Error::NotFound(format!("{collection}:{bare_id} not found")))?;

        let mut result: Value = serde_json::from_str(row.get::<String, _>("data").as_str())
            .unwrap_or(Value::Object(Default::default()));

        if let Some(obj) = result.as_object_mut() {
            inject_document_metadata(
                obj,
                Some(row.get(fields::ID)),
                Some(row.get::<chrono::DateTime<chrono::Utc>, _>("created_at")),
                Some(row.get::<chrono::DateTime<chrono::Utc>, _>("updated_at")),
            );
        }

        Ok(result)
    }

    async fn upsert_document(&self, collection: &str, id: &str, data: Value) -> AppResult<Value> {
        self.ensure_table(collection).await?;
        let table = sanitize_table_name(collection)?;
        let bare_id = id.strip_prefix(&format!("{collection}:")).unwrap_or(id);
        let data_str = json_to_string(&data);

        let row = sqlx::query(&format!(
            r#"
            INSERT INTO {table} (id, data) VALUES ($1, $2::jsonb)
            ON CONFLICT (id) DO UPDATE SET data = {table}.data || EXCLUDED.data
            RETURNING id, data::TEXT, created_at, updated_at
            "#
        ))
        .bind(bare_id)
        .bind(&data_str)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Upsert failed: {e}")))?;

        let mut result: Value = serde_json::from_str(row.get::<String, _>("data").as_str())
            .unwrap_or(Value::Object(Default::default()));

        if let Some(obj) = result.as_object_mut() {
            inject_document_metadata(
                obj,
                Some(row.get(fields::ID)),
                Some(row.get::<chrono::DateTime<chrono::Utc>, _>("created_at")),
                Some(row.get::<chrono::DateTime<chrono::Utc>, _>("updated_at")),
            );
        }

        Ok(result)
    }

    async fn delete_document(&self, collection: &str, id: &str) -> AppResult<Value> {
        self.ensure_table(collection).await?;
        let table = sanitize_table_name(collection)?;
        let bare_id = id.strip_prefix(&format!("{collection}:")).unwrap_or(id);

        let row = sqlx::query(&format!(
            r#"DELETE FROM {table} WHERE id = $1 RETURNING id, data::TEXT"#
        ))
        .bind(bare_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Delete failed: {e}")))?;

        let row = row
            .ok_or_else(|| ob_core::Error::NotFound(format!("{collection}:{bare_id} not found")))?;

        let mut result: Value = serde_json::from_str(row.get::<String, _>("data").as_str())
            .unwrap_or(Value::Object(Default::default()));

        if let Some(obj) = result.as_object_mut() {
            obj.insert(fields::ID.to_string(), Value::String(row.get(fields::ID)));
        }

        Ok(result)
    }

    async fn list_documents(
        &self,
        collection: &str,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> AppResult<Vec<Value>> {
        self.ensure_table(collection).await?;
        let table = sanitize_table_name(collection)?;
        let n = limit.unwrap_or(1000).min(10_000) as i64;
        let o = offset.unwrap_or(0) as i64;

        let rows = sqlx::query(&format!(
            r#"SELECT id, data::TEXT, created_at, updated_at FROM {table} ORDER BY created_at DESC LIMIT $1 OFFSET $2"#
        ))
        .bind(n)
        .bind(o)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ob_core::Error::Database(format!("List failed: {e}")))?;

        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            let mut val: Value = serde_json::from_str(row.get::<String, _>("data").as_str())
                .unwrap_or(Value::Object(Default::default()));

            if let Some(obj) = val.as_object_mut() {
                inject_document_metadata(
                    obj,
                    Some(row.get(fields::ID)),
                    Some(row.get::<chrono::DateTime<chrono::Utc>, _>("created_at")),
                    Some(row.get::<chrono::DateTime<chrono::Utc>, _>("updated_at")),
                );
            }
            results.push(val);
        }

        Ok(results)
    }

    async fn batch_create(&self, collection: &str, docs: Vec<Value>) -> AppResult<Vec<Value>> {
        if docs.is_empty() {
            return Ok(vec![]);
        }
        let mut results = Vec::with_capacity(docs.len());
        for doc in docs {
            results.push(self.create_document(collection, doc).await?);
        }
        Ok(results)
    }

    async fn batch_update(
        &self,
        collection: &str,
        updates: Vec<(String, Value)>,
    ) -> AppResult<Vec<Value>> {
        if updates.is_empty() {
            return Ok(vec![]);
        }
        let mut results = Vec::with_capacity(updates.len());
        for (id, data) in updates {
            results.push(self.update_document(collection, &id, data).await?);
        }
        Ok(results)
    }

    async fn batch_delete(&self, collection: &str, ids: Vec<String>) -> AppResult<Vec<Value>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            results.push(self.delete_document(collection, &id).await?);
        }
        Ok(results)
    }

    async fn query_raw(&self, query: &str) -> AppResult<Vec<Value>> {
        if let Some(table) = extract_table_name(query)
            && let Err(e) = self.ensure_table(&table).await
        {
            tracing::warn!("Failed to ensure table {table}: {e}");
        }
        let rows = sqlx::query(query)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| ob_core::Error::Database(format!("Query failed: {e}")))?;

        rows_to_values(rows)
    }

    async fn query_raw_value(&self, query: &str) -> AppResult<Value> {
        if let Some(table) = extract_table_name(query)
            && let Err(e) = self.ensure_table(&table).await
        {
            tracing::warn!("Failed to ensure table {table}: {e}");
        }
        let row = sqlx::query(query)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| ob_core::Error::Database(format!("Query failed: {e}")))?;

        let val: Value = row
            .try_get::<String, _>(0)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null);
        Ok(val)
    }

    async fn query_bind(&self, query: &str, binds: Value) -> AppResult<Vec<Value>> {
        // If binds are empty, pass query directly
        let is_empty_binds = binds.as_object().is_some_and(|o| o.is_empty());
        if is_empty_binds {
            if let Some(table) = extract_table_name(query)
                && let Err(e) = self.ensure_table(&table).await
            {
                tracing::warn!("Failed to ensure table {table}: {e}");
            }
            let rows = sqlx::query(query)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| ob_core::Error::Database(format!("Query failed: {e}")))?;
            return rows_to_values(rows);
        }

        // Convert named params ($param_name) to positional ($1, $2, ...)
        let (pg_query, bind_values) = named_to_positional(query, binds)?;
        if let Some(table) = extract_table_name(&pg_query)
            && let Err(e) = self.ensure_table(&table).await
        {
            tracing::warn!("Failed to ensure table {table}: {e}");
        }

        let mut q = sqlx::query(&pg_query);
        for val in &bind_values {
            q = bind_json_value(q, val);
        }

        let rows = q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| ob_core::Error::Database(format!("Query failed: {e}")))?;

        rows_to_values(rows)
    }

    async fn query_bind_value(&self, query: &str, binds: Value) -> AppResult<Vec<Value>> {
        self.query_bind(query, binds).await
    }

    async fn update_with_field_values(
        &self,
        collection: &str,
        id: &str,
        data: Value,
    ) -> AppResult<Value> {
        // For now, treat as a regular merge update.
        // TODO: Translate FieldValue markers (_increment, _arrayUnion, etc.)
        // to PostgreSQL JSONB operations.
        self.update_document(collection, id, data).await
    }

    async fn update_document_cas(
        &self,
        collection: &str,
        id: &str,
        data: Value,
        check_field: &str,
        check_value: &Value,
    ) -> AppResult<Option<Value>> {
        self.ensure_table(collection).await?;
        let table = sanitize_table_name(collection)?;
        let bare_id = id.strip_prefix(&format!("{collection}:")).unwrap_or(id);
        let data_str = json_to_string(&data);
        let check_str = json_to_string(check_value);

        let row = sqlx::query(&format!(
            r#"
            UPDATE {table}
            SET data = data || $3::jsonb
            WHERE id = $1 AND data->>'{check_field}' = $2::jsonb #>> '{{}}'
            RETURNING id, data::TEXT, created_at, updated_at
            "#
        ))
        .bind(bare_id)
        .bind(&check_str)
        .bind(&data_str)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ob_core::Error::Database(format!("CAS update failed: {e}")))?;

        match row {
            None => Ok(None),
            Some(row) => {
                let mut result: Value = serde_json::from_str(row.get::<String, _>("data").as_str())
                    .unwrap_or(Value::Object(Default::default()));

                if let Some(obj) = result.as_object_mut() {
                    obj.insert(fields::ID.to_string(), Value::String(row.get(fields::ID)));
                    obj.insert(
                        fields::UPDATED_AT.to_string(),
                        Value::String(
                            row.get::<chrono::DateTime<chrono::Utc>, _>("updated_at")
                                .to_rfc3339(),
                        ),
                    );
                }

                Ok(Some(result))
            }
        }
    }

    // ── Filter-based query methods ─────────────────────────────────────

    /// Find documents in `collection` where `field` matches `value`
    /// using the given SQL `operator` (`=`, `!=`, `<`, `>`, `<=`, `>=`).
    ///
    /// Field comparison uses JSONB text extraction (`data->>'field'`),
    /// so all values are compared as text. Returns up to `limit`
    /// documents (unlimited when `None`).
    async fn find_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        value: &Value,
        limit: Option<usize>,
    ) -> AppResult<Vec<Value>> {
        self.ensure_table(collection).await?;
        let table = sanitize_table_name(collection)?;
        let limit_clause = limit.map_or(String::new(), |l| format!(" LIMIT {l}"));
        let val_str = json_to_text(value);

        let rows = sqlx::query(&format!(
            "SELECT id, data::TEXT, created_at, updated_at FROM {table} WHERE data->>'{field}' {operator} $1{limit_clause}"
        ))
        .bind(&val_str)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ob_core::Error::Database(format!("find_where failed: {e}")))?;

        rows_to_values(rows)
    }

    /// Find documents matching multiple field conditions combined with AND.
    ///
    /// Each filter is a `(field, operator, value)` tuple. Results can be
    /// sorted via `order_by` / `order_dir` and capped with `limit`.
    /// All comparisons use JSONB text extraction (`data->>'field'`).
    async fn find_where_multi(
        &self,
        collection: &str,
        filters: &[(String, String, Value)],
        order_by: Option<&str>,
        order_dir: Option<&str>,
        limit: Option<usize>,
    ) -> AppResult<Vec<Value>> {
        self.ensure_table(collection).await?;
        let table = sanitize_table_name(collection)?;

        let mut conditions = Vec::with_capacity(filters.len());
        let mut bind_values = Vec::with_capacity(filters.len());

        for (i, (field, operator, value)) in filters.iter().enumerate() {
            conditions.push(format!("data->>'{field}' {operator} ${}", i + 1));
            bind_values.push(json_to_text(value));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };

        let order_clause = order_by.map_or(String::new(), |ob| {
            let dir = order_dir.unwrap_or("ASC");
            format!(" ORDER BY data->>'{ob}' {dir}")
        });

        let limit_clause = limit.map_or(String::new(), |l| format!(" LIMIT {l}"));

        let query = format!(
            "SELECT id, data::TEXT, created_at, updated_at FROM {table}{where_clause}{order_clause}{limit_clause}"
        );

        let mut q = sqlx::query(&query);
        for val in &bind_values {
            q = q.bind(val);
        }

        let rows = q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| ob_core::Error::Database(format!("find_where_multi failed: {e}")))?;

        rows_to_values(rows)
    }

    async fn count_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        value: &Value,
    ) -> AppResult<usize> {
        self.ensure_table(collection).await?;
        let table = sanitize_table_name(collection)?;
        let val_str = json_to_text(value);

        let row = sqlx::query(&format!(
            "SELECT COUNT(*) as cnt FROM {table} WHERE data->>'{field}' {operator} $1"
        ))
        .bind(&val_str)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| ob_core::Error::Database(format!("count_where failed: {e}")))?;

        let count: i64 = row.get("cnt");
        Ok(count as usize)
    }

    async fn exists_where(&self, collection: &str, field: &str, value: &Value) -> AppResult<bool> {
        self.ensure_table(collection).await?;
        let table = sanitize_table_name(collection)?;
        let val_str = json_to_text(value);

        let row = sqlx::query(&format!(
            "SELECT EXISTS(SELECT 1 FROM {table} WHERE data->>'{field}' = $1) as exists_flag"
        ))
        .bind(&val_str)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| ob_core::Error::Database(format!("exists_where failed: {e}")))?;

        let exists: bool = row.get("exists_flag");
        Ok(exists)
    }

    async fn update_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        field_value: &Value,
        data: Value,
    ) -> AppResult<Vec<Value>> {
        self.ensure_table(collection).await?;
        let table = sanitize_table_name(collection)?;
        let filter_str = json_to_text(field_value);
        let data_str = json_to_string(&data);

        let rows = sqlx::query(&format!(
            "UPDATE {table} SET data = data || $1::jsonb, updated_at = now() \
             WHERE data->>'{field}' {operator} $2 \
             RETURNING id, data::TEXT, created_at, updated_at"
        ))
        .bind(&data_str)
        .bind(&filter_str)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ob_core::Error::Database(format!("update_where failed: {e}")))?;

        rows_to_values(rows)
    }

    async fn delete_where(
        &self,
        collection: &str,
        field: &str,
        operator: &str,
        value: &Value,
    ) -> AppResult<usize> {
        self.ensure_table(collection).await?;
        let table = sanitize_table_name(collection)?;
        let val_str = json_to_text(value);

        let result = sqlx::query(&format!(
            "DELETE FROM {table} WHERE data->>'{field}' {operator} $1"
        ))
        .bind(&val_str)
        .execute(&self.pool)
        .await
        .map_err(|e| ob_core::Error::Database(format!("delete_where failed: {e}")))?;

        Ok(result.rows_affected() as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These tests require a running PostgreSQL instance.
    /// Run: docker exec -i orignabase-pg psql -U orignabase -d orignabase < migrations/001_full_schema.sql
    async fn test_store() -> PgDatabaseStore {
        let url = std::env::var("OB_TEST_DATABASE_URL").unwrap_or_else(|_| {
            "postgres://orignabase:orignabase_dev@127.0.0.1:5432/orignabase".to_string()
        });
        PgDatabaseStore::connect(&url).await.unwrap()
    }

    #[tokio::test]
    async fn test_pg_create_and_get() {
        let store = test_store().await;
        let data = serde_json::json!({"name": "Test User", "email": "test@example.com"});
        let created = store.create_document("test_users", data).await.unwrap();
        assert!(created.get(fields::ID).is_some());

        let id = created[fields::ID].as_str().unwrap().to_string();
        let fetched = store.get_document("test_users", &id).await.unwrap();
        assert_eq!(fetched[fields::NAME], "Test User");
        assert_eq!(fetched[fields::EMAIL], "test@example.com");
    }

    #[tokio::test]
    async fn test_pg_create_rejects_duplicate_id_without_overwriting() {
        let store = test_store().await;
        let collection = format!("test_create_duplicate_{}", uuid::Uuid::new_v4().simple());
        let id = "fixed-id";
        let original = serde_json::json!({
            "id": id,
            "sellerId": "seller-a",
            "name": "Original"
        });
        let replacement = serde_json::json!({
            "id": id,
            "sellerId": "seller-b",
            "name": "Replacement"
        });

        let created = store.create_document(&collection, original).await.unwrap();
        assert_eq!(created[fields::SELLER_ID], "seller-a");

        let duplicate = store.create_document(&collection, replacement).await;
        assert!(
            matches!(duplicate, Err(ob_core::Error::Validation(_))),
            "duplicate create should fail validation, got {duplicate:?}"
        );

        let fetched = store.get_document(&collection, id).await.unwrap();
        assert_eq!(fetched[fields::SELLER_ID], "seller-a");
        assert_eq!(fetched[fields::NAME], "Original");
    }

    #[tokio::test]
    async fn test_pg_metadata_timestamps_override_stale_payload_values() {
        let store = test_store().await;
        let stale_created = "2000-01-01T00:00:00Z";
        let stale_updated = "2000-01-02T00:00:00Z";
        let created = store
            .create_document(
                "test_metadata_override",
                serde_json::json!({
                    "name": "Timestamp Canonical",
                    "createdAt": stale_created,
                    "updatedAt": stale_updated,
                }),
            )
            .await
            .unwrap();

        let created_at = created[fields::CREATED_AT].as_str().unwrap();
        let updated_at = created[fields::UPDATED_AT].as_str().unwrap();

        assert_ne!(created_at, stale_created);
        assert_ne!(updated_at, stale_updated);

        let fetched = store
            .get_document(
                "test_metadata_override",
                created[fields::ID].as_str().unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(fetched[fields::CREATED_AT], created[fields::CREATED_AT]);
        assert_eq!(fetched[fields::UPDATED_AT], created[fields::UPDATED_AT]);
    }

    #[tokio::test]
    async fn test_pg_update() {
        let store = test_store().await;
        let data = serde_json::json!({"name": "Alice", "age": 30});
        let created = store.create_document("test_update", data).await.unwrap();
        let id = created[fields::ID].as_str().unwrap().to_string();

        let updated = store
            .update_document("test_update", &id, serde_json::json!({"age": 31}))
            .await
            .unwrap();
        assert_eq!(updated["age"], 31);
        assert_eq!(updated[fields::NAME], "Alice"); // preserved
    }

    #[tokio::test]
    async fn test_pg_update_overrides_stale_timestamp_payload_values() {
        let store = test_store().await;
        let collection = format!(
            "test_update_timestamp_override_{}",
            uuid::Uuid::new_v4().simple()
        );
        let created = store
            .create_document(&collection, serde_json::json!({"name": "Alice"}))
            .await
            .unwrap();
        let id = created[fields::ID].as_str().unwrap().to_string();
        let original_created_at = created[fields::CREATED_AT].clone();

        let updated = store
            .update_document(
                &collection,
                &id,
                serde_json::json!({
                    "createdAt": "2000-01-01T00:00:00Z",
                    "updatedAt": "2000-01-02T00:00:00Z",
                    "age": 31
                }),
            )
            .await
            .unwrap();

        assert_eq!(updated[fields::CREATED_AT], original_created_at);
        assert_ne!(
            updated[fields::UPDATED_AT],
            serde_json::json!("2000-01-02T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn test_pg_upsert() {
        let store = test_store().await;
        let data = serde_json::json!({"key": "theme", "value": "dark"});
        let result = store
            .upsert_document("test_config", "theme_key", data)
            .await
            .unwrap();
        assert_eq!(result["value"], "dark");

        // Upsert again with new data
        let updated = store
            .upsert_document(
                "test_config",
                "theme_key",
                serde_json::json!({"value": "light"}),
            )
            .await
            .unwrap();
        assert_eq!(updated["value"], "light");
        assert_eq!(updated["key"], "theme"); // merged from previous
    }

    #[tokio::test]
    async fn test_pg_upsert_overrides_stale_timestamp_payload_values() {
        let store = test_store().await;
        let created = store
            .upsert_document(
                "test_upsert_timestamp_override",
                "theme_key",
                serde_json::json!({"key": "theme", "value": "dark"}),
            )
            .await
            .unwrap();

        let updated = store
            .upsert_document(
                "test_upsert_timestamp_override",
                "theme_key",
                serde_json::json!({
                    "value": "light",
                    "createdAt": "2000-01-01T00:00:00Z",
                    "updatedAt": "2000-01-02T00:00:00Z"
                }),
            )
            .await
            .unwrap();

        assert_eq!(updated[fields::CREATED_AT], created[fields::CREATED_AT]);
        assert_ne!(
            updated[fields::UPDATED_AT],
            serde_json::json!("2000-01-02T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn test_pg_delete() {
        let store = test_store().await;
        let data = serde_json::json!({"temp": true});
        let created = store.create_document("test_del", data).await.unwrap();
        let id = created[fields::ID].as_str().unwrap().to_string();

        let deleted = store.delete_document("test_del", &id).await.unwrap();
        assert_eq!(deleted["temp"], true);

        let result = store.get_document("test_del", &id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_pg_list() {
        let store = test_store().await;
        for i in 0..3 {
            store
                .create_document("test_list", serde_json::json!({"i": i}))
                .await
                .unwrap();
        }
        let docs = store
            .list_documents("test_list", Some(2), None)
            .await
            .unwrap();
        assert_eq!(docs.len(), 2);
    }

    #[tokio::test]
    async fn test_pg_list_with_offset() {
        let store = test_store().await;
        let docs = store
            .list_documents("test_list", Some(10), Some(5))
            .await
            .unwrap();
        // Just verify it doesn't error — we can't predict exact count
        assert!(docs.len() <= 10);
    }

    #[tokio::test]
    async fn test_pg_batch_create() {
        let store = test_store().await;
        let docs = vec![
            serde_json::json!({"name": "Batch1"}),
            serde_json::json!({"name": "Batch2"}),
        ];
        let results = store.batch_create("test_batch", docs).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_pg_get_not_found() {
        let store = test_store().await;
        let result = store.get_document("test_404", "nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}
