use async_graphql::{EmptySubscription, Schema};

use crate::resolvers::{MutationRoot, QueryRoot};
use ob_database::DatabaseClient;
use ob_realtime::registry::ChangeEvent;
use ob_search::SearchClient;
use ob_security::RuleEngine;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub struct GraphQlLimits {
    pub enable_introspection: bool,
    pub max_depth: usize,
    pub max_complexity: usize,
}

impl Default for GraphQlLimits {
    fn default() -> Self {
        Self {
            enable_introspection: std::env::var("OB_ENABLE_INTROSPECTION").as_deref() == Ok("true"),
            max_depth: std::env::var("OB_GRAPHQL_MAX_DEPTH")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(12),
            max_complexity: std::env::var("OB_GRAPHQL_MAX_COMPLEXITY")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100),
        }
    }
}

/// GraphQL context available in all resolvers.
pub struct GqlContext {
    pub db: DatabaseClient,
    pub rules: Arc<RuleEngine>,
    pub user_id: Option<String>,
    pub roles: Vec<String>,
    pub authenticated: bool,
}

pub type AppSchema = Schema<QueryRoot, MutationRoot, EmptySubscription>;

/// Build the GraphQL schema with shared context and security limits.
pub fn build_schema(
    db: DatabaseClient,
    rules: Arc<RuleEngine>,
    change_tx: mpsc::Sender<ChangeEvent>,
    search: SearchClient,
) -> AppSchema {
    let limits = GraphQlLimits::default();
    build_schema_with_limits(db, rules, change_tx, search, limits)
}

/// Build the GraphQL schema with explicit security limits.
/// Call this variant when you need to override the defaults derived from env vars.
pub fn build_schema_with_limits(
    db: DatabaseClient,
    rules: Arc<RuleEngine>,
    change_tx: mpsc::Sender<ChangeEvent>,
    search: SearchClient,
    limits: GraphQlLimits,
) -> AppSchema {
    let mut builder = Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        .data(db)
        .data(rules)
        .data(change_tx)
        .data(search)
        .limit_depth(limits.max_depth)
        .limit_complexity(limits.max_complexity);

    if !limits.enable_introspection {
        builder = builder.disable_introspection();
    }

    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    // Serialize env-var tests to avoid races when cargo runs tests in parallel.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_app_schema_type_is_valid() {
        fn _assert_schema_type(_s: &AppSchema) {}
    }

    #[test]
    fn test_build_schema_disables_introspection_by_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("OB_ENABLE_INTROSPECTION") };
        let limits = GraphQlLimits::default();
        assert!(!limits.enable_introspection);
    }

    #[test]
    fn test_build_schema_enables_introspection_when_env_set() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("OB_ENABLE_INTROSPECTION", "true") };
        let limits = GraphQlLimits::default();
        assert!(limits.enable_introspection);
        unsafe { std::env::remove_var("OB_ENABLE_INTROSPECTION") };
    }

    #[test]
    fn test_build_schema_default_limits() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("OB_ENABLE_INTROSPECTION") };
        unsafe { std::env::remove_var("OB_GRAPHQL_MAX_DEPTH") };
        unsafe { std::env::remove_var("OB_GRAPHQL_MAX_COMPLEXITY") };
        let limits = GraphQlLimits::default();
        assert!(!limits.enable_introspection);
        assert_eq!(limits.max_depth, 12);
        assert_eq!(limits.max_complexity, 100);
    }

    #[test]
    fn test_build_schema_custom_limits_from_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("OB_GRAPHQL_MAX_DEPTH", "8") };
        unsafe { std::env::set_var("OB_GRAPHQL_MAX_COMPLEXITY", "50") };
        let limits = GraphQlLimits::default();
        assert_eq!(limits.max_depth, 8);
        assert_eq!(limits.max_complexity, 50);
        unsafe { std::env::remove_var("OB_GRAPHQL_MAX_DEPTH") };
        unsafe { std::env::remove_var("OB_GRAPHQL_MAX_COMPLEXITY") };
    }
}
