use async_graphql::{EmptySubscription, Schema};

use crate::resolvers::{MutationRoot, QueryRoot};
use ob_database::DatabaseClient;
use ob_security::RuleEngine;
use std::sync::Arc;

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
pub fn build_schema(db: DatabaseClient, rules: Arc<RuleEngine>) -> AppSchema {
    Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        .data(db)
        .data(rules)
        .finish()
}
