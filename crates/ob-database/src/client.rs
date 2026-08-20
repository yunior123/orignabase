use crate::pg_store::PgDatabaseStore;
use ob_core::Error;
use ob_core::config::DatabaseConfig;
use ob_core::ports::db_store::DatabaseStore;

/// Database client wrapping the PostgreSQL adapter.
///
/// This is the primary database interface used by all handler state types.
/// It delegates all operations to `PgDatabaseStore` which implements the
/// `DatabaseStore` trait.
#[derive(Clone)]
pub struct DatabaseClient {
    pub(crate) inner: PgDatabaseStore,
}

impl DatabaseClient {
    /// Create a database client for testing.
    /// Uses the local PostgreSQL instance with an isolated schema per client
    /// so parallel tests can exercise the real DB layer without sharing state.
    pub async fn new_mem() -> Self {
        let url = std::env::var("OB_TEST_DATABASE_URL").unwrap_or_else(|_| {
            "postgres://orignabase:orignabase_dev@127.0.0.1:5432/orignabase".to_string()
        });
        let schema = format!("test_{}", uuid::Uuid::new_v4().simple());
        let admin_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .min_connections(0)
            .connect(&url)
            .await
            .unwrap();

        sqlx::query(&format!("CREATE SCHEMA \"{schema}\""))
            .execute(&admin_pool)
            .await
            .unwrap();
        admin_pool.close().await;

        let inner = PgDatabaseStore::connect_to_schema(&url, &schema)
            .await
            .unwrap();

        Self { inner }
    }

    /// Connect to PostgreSQL using the provided config.
    pub async fn connect(config: &DatabaseConfig) -> ob_core::Result<Self> {
        let inner = PgDatabaseStore::connect(&config.url)
            .await
            .map_err(|e| Error::Database(format!("Connection failed: {e}")))?;

        tracing::info!("Connected to PostgreSQL at {}", config.url);

        Ok(Self { inner })
    }

    /// Get a reference to the underlying PgDatabaseStore.
    pub fn inner(&self) -> &PgDatabaseStore {
        &self.inner
    }

    /// Execute a query with a timeout to prevent long-running queries from blocking.
    pub async fn query_with_timeout(&self, query: &str, timeout_secs: u64) -> ob_core::Result<()> {
        tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            self.inner.query_raw(query),
        )
        .await
        .map_err(|_| {
            Error::Database(format!(
                "Query timeout exceeded ({}s). Query may be too complex or resource-intensive.",
                timeout_secs
            ))
        })?
        .map(|_| ())?;
        Ok(())
    }
}
