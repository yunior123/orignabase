//! Transaction abstraction for PostgreSQL.
//!
//! Wraps multiple queries in a real ACID transaction using PostgreSQL's BEGIN/COMMIT.

use crate::DatabaseClient;
use crate::pg_store::{bind_json_value, named_to_positional};
use ob_core::{Error, Result};
use serde_json::Value;

/// A transactional batch of operations.
///
/// PostgreSQL transactions use real BEGIN/COMMIT for full ACID guarantees.
pub struct Transaction {
    queries: Vec<(String, Option<Value>)>,
}

impl Transaction {
    pub fn new() -> Self {
        Self {
            queries: Vec::new(),
        }
    }

    /// Add a query with optional bind parameters.
    pub fn add(&mut self, query: &str, binds: Option<Value>) -> &mut Self {
        self.queries.push((query.to_string(), binds));
        self
    }

    /// Add a query without parameters.
    pub fn add_raw(&mut self, query: &str) -> &mut Self {
        self.queries.push((query.to_string(), None));
        self
    }

    /// Number of operations in this transaction.
    pub fn len(&self) -> usize {
        self.queries.len()
    }

    /// Whether this transaction is empty.
    pub fn is_empty(&self) -> bool {
        self.queries.is_empty()
    }

    /// Execute all operations in a single PostgreSQL transaction.
    pub async fn commit(self, db: &DatabaseClient) -> Result<Vec<Value>> {
        if self.queries.is_empty() {
            return Ok(vec![]);
        }

        let mut pg_tx = db
            .inner()
            .pool()
            .begin()
            .await
            .map_err(|e| Error::Database(format!("Failed to begin transaction: {e}")))?;

        let mut results = Vec::with_capacity(self.queries.len());

        for (query, binds) in &self.queries {
            if let Some(binds) = binds {
                // Convert named params ($param_name) to positional ($1, $2, ...)
                let (pg_query, bind_values) =
                    named_to_positional(query, binds.clone()).map_err(|e| {
                        Error::Database(format!("Transaction query translation failed: {e}"))
                    })?;

                let mut q = sqlx::query(&pg_query);
                for val in &bind_values {
                    q = bind_json_value(q, val);
                }

                let result = q
                    .execute(&mut *pg_tx)
                    .await
                    .map_err(|e| Error::Database(format!("Transaction query failed: {e}")))?;

                let rows_affected = result.rows_affected();
                results.push(serde_json::json!({"rows_affected": rows_affected}));
            } else {
                let result = sqlx::query(query)
                    .execute(&mut *pg_tx)
                    .await
                    .map_err(|e| Error::Database(format!("Transaction query failed: {e}")))?;

                let rows_affected = result.rows_affected();
                results.push(serde_json::json!({"rows_affected": rows_affected}));
            }
        }

        pg_tx
            .commit()
            .await
            .map_err(|e| Error::Database(format!("Transaction commit failed: {e}")))?;

        Ok(results)
    }
}

impl Default for Transaction {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_new_empty() {
        let tx = Transaction::new();
        assert!(tx.is_empty());
        assert_eq!(tx.len(), 0);
    }

    #[test]
    fn test_transaction_add_operations() {
        let mut tx = Transaction::new();
        tx.add("SELECT 1", None);
        tx.add_raw("SELECT 2");
        assert_eq!(tx.len(), 2);
        assert!(!tx.is_empty());
    }

    #[test]
    fn test_transaction_chaining() {
        let mut tx = Transaction::new();
        tx.add_raw("SELECT 1")
            .add_raw("SELECT 2")
            .add_raw("SELECT 3");
        assert_eq!(tx.len(), 3);
    }

    #[test]
    fn test_transaction_default() {
        let tx = Transaction::default();
        assert!(tx.is_empty());
    }
}
