//! MCP Server for OrignaBase
//!
//! Exposes marketplace operations (search, cart, orders, admin) as MCP tools.
//! Uses JSON-RPC 2.0 over HTTP/SSE and stdio for local development.
//!
//! Architecture:
//! - Tools reuse existing ob-handlers, ob-database, ob-auth logic
//! - All monetary values remain integer cents (no conversion)
//! - SurrealDB IDs: collection:record_id format preserved
//! - Authentication via JWT middleware
//! - Safeguards: idempotency keys, confirmation tokens, spend limits

pub mod server;
pub mod tools;
pub mod auth;
pub mod safeguards;
pub mod transport;
pub mod errors;

pub use server::OrignaGtaMcp;
pub use transport::McpRouter;

use std::sync::Arc;
use ob_database::DatabaseClient;
use ob_search::SearchClient;
use ob_core::Config;

/// MCP server state — shared across all tool invocations
#[derive(Clone)]
pub struct McpState {
    pub db: Arc<DatabaseClient>,
    pub search: Option<Arc<SearchClient>>,
    pub config: Arc<Config>,
}

impl McpState {
    /// Create a new MCP server state from dependencies
    pub fn new(
        db: Arc<DatabaseClient>,
        search: Option<Arc<SearchClient>>,
        config: Arc<Config>,
    ) -> Self {
        Self { db, search, config }
    }
}
