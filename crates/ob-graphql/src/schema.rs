use async_graphql::{EmptySubscription, Schema};

use crate::resolvers::{MutationRoot, QueryRoot};
use ob_database::DatabaseClient;
use ob_realtime::registry::ChangeEvent;
use ob_security::RuleEngine;
use std::sync::Arc;
use tokio::sync::mpsc;

/// GraphQL context available in all resolvers.
pub struct GqlContext {
    pub db: DatabaseClient,
    pub rules: Arc<RuleEngine>,
    pub user_id: Option<String>,
    pub roles: Vec<String>,
    pub authenticated: bool,
}

pub type AppSchema = Schema<QueryRoot, MutationRoot, EmptySubscription>;

/// Build the GraphQL schema with shared context.
pub fn build_schema(
    db: DatabaseClient,
    rules: Arc<RuleEngine>,
    change_tx: mpsc::Sender<ChangeEvent>,
) -> AppSchema {
    Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        .data(db)
        .data(rules)
        .data(change_tx)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ob_core::config::DatabaseConfig;

    /// Verify that Schema type alias resolves correctly at compile time.
    /// This is a compile-time check — if AppSchema type is malformed, this won't build.
    #[test]
    fn test_app_schema_type_is_valid() {
        fn _assert_schema_type(_s: &AppSchema) {}
        // If this compiles, the type alias is correctly defined.
    }

    /// Verify build_schema constructs a working schema with introspection.
    /// Requires a running SurrealDB: `surreal start --user root --pass root memory`
    #[tokio::test]
    #[ignore = "requires running SurrealDB instance"]
    async fn test_build_schema_and_introspect() {
        let config = DatabaseConfig {
            endpoint: "localhost:8000".to_string(),
            username: Some("root".to_string()),
            password: Some("root".to_string()),
            namespace: "test".to_string(),
            name: "test_graphql".to_string(),
        };
        let db = DatabaseClient::connect(&config).await.unwrap();
        let rules = Arc::new(RuleEngine::new(std::collections::HashMap::new()));
        let (change_tx, _change_rx) = tokio::sync::mpsc::channel(16);

        let schema = build_schema(db, rules, change_tx);

        // Introspect — should return type names without error
        let result = schema.execute("{ __schema { types { name } } }").await;
        assert!(result.errors.is_empty(), "Introspection errors: {:?}", result.errors);
        assert!(!result.data.to_string().is_empty());
    }
}
