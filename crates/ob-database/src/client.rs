use ob_core::Error;
use ob_core::config::DatabaseConfig;
use std::time::Duration;
use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect as connect_any};
use surrealdb::opt::auth::Root;

/// Wrapper around the SurrealDB client with connection management and resilience.
#[derive(Clone)]
pub struct DatabaseClient {
    db: Surreal<Any>,
}

impl DatabaseClient {
    /// Create an in-memory database client for testing.
    pub async fn new_mem() -> Self {
        let db = connect_any("mem://").await.unwrap();
        db.use_ns("test").use_db("test").await.unwrap();
        Self { db }
    }

    /// Connect to SurrealDB and configure namespace/database.
    pub async fn connect(config: &DatabaseConfig) -> ob_core::Result<Self> {
        let db = connect_any(&config.endpoint)
            .await
            .map_err(|e| Error::Database(format!("Connection failed: {e}")))?;

        // Authenticate if credentials provided
        if let (Some(user), Some(pass)) = (&config.username, &config.password) {
            db.signin(Root {
                username: user,
                password: pass,
            })
            .await
            .map_err(|e| Error::Database(format!("Auth failed: {e}")))?;
        }

        db.use_ns(&config.namespace)
            .use_db(&config.name)
            .await
            .map_err(|e| Error::Database(format!("Namespace/DB select failed: {e}")))?;

        tracing::info!(
            "Connected to SurrealDB at {} (ns={}, db={})",
            config.endpoint,
            config.namespace,
            config.name
        );

        // Spawn health check task
        let db_clone = db.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                if let Err(e) = db_clone.query("SELECT 1 AS test").await {
                    tracing::error!("Database health check failed: {e}");
                }
            }
        });

        Ok(Self { db })
    }

    /// Get a reference to the underlying SurrealDB client.
    pub fn inner(&self) -> &Surreal<Any> {
        &self.db
    }

    /// Execute a query with a timeout to prevent long-running queries from blocking.
    pub async fn query_with_timeout(&self, query: &str, timeout_secs: u64) -> ob_core::Result<()> {
        tokio::time::timeout(Duration::from_secs(timeout_secs), self.db.query(query))
            .await
            .map_err(|_| {
                Error::Database(format!(
                    "Query timeout exceeded ({}s). Query may be too complex or resource-intensive.",
                    timeout_secs
                ))
            })?
            .map_err(|e| Error::Database(format!("Query execution failed: {e}")))?;
        Ok(())
    }
}
