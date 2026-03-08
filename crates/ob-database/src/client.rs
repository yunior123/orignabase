use ob_core::Error;
use ob_core::config::DatabaseConfig;
use surrealdb::Surreal;
use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::opt::auth::Root;

/// Wrapper around the SurrealDB client with connection management.
#[derive(Clone)]
pub struct DatabaseClient {
    db: Surreal<Client>,
}

impl DatabaseClient {
    /// Connect to SurrealDB and configure namespace/database.
    pub async fn connect(config: &DatabaseConfig) -> ob_core::Result<Self> {
        let db = Surreal::new::<Ws>(&config.endpoint)
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

        Ok(Self { db })
    }

    /// Get a reference to the underlying SurrealDB client.
    pub fn inner(&self) -> &Surreal<Client> {
        &self.db
    }
}
