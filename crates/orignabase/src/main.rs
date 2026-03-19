use anyhow::Result;
use async_graphql::http::GraphiQLSource;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::HeaderValue;
use axum::http::header::AUTHORIZATION;
use axum::response::IntoResponse;
use clap::{Parser, Subcommand};
use ob_admin::admin_router;
use ob_admin::routes::AdminState;
use ob_analytics::{AnalyticsState, analytics_router};
use ob_auth::routes::{AuthState, auth_router};
use ob_core::Config;
use ob_database::DatabaseClient;
use ob_functions::routes::FunctionsState;
use ob_functions::{
    CronScheduler, DbTriggerExecutor, FunctionLimits, FunctionRegistry, WasmRuntime,
    functions_router,
};
use ob_graphql::build_schema;
use ob_realtime::ChangeDispatcher;
use ob_realtime::registry::SubscriptionRegistry;
use ob_realtime::websocket::realtime_router;
use ob_search::sync::{SearchAction, SearchSyncEvent, SearchSyncer};
use ob_search::{SearchClient, SearchConfig};
use ob_security::{RuleEngine, parse_rules};
use ob_storage::routes::{StorageState, storage_router};
use ob_storage::{LocalStorage, ResumableUploadManager, SignedUrlGenerator};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::PeerIpKeyExtractor;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

// ── CLI Credential Storage ──

/// Path to the CLI credentials file (~/.orignabase/credentials.json)
fn credentials_path() -> std::path::PathBuf {
    dirs_next::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".orignabase")
        .join("credentials.json")
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CliCredentials {
    server_url: String,
    api_key: String,
    #[serde(default)]
    project_name: Option<String>,
    #[serde(default)]
    logged_in_at: Option<String>,
}

impl CliCredentials {
    fn save(&self) -> Result<()> {
        let path = credentials_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, &json)?;
        // Restrict permissions to owner only (Unix)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    fn load() -> Result<Self> {
        let path = credentials_path();
        if !path.exists() {
            anyhow::bail!(
                "Not logged in. Run `orignabase login --server <url> --api-key <key>` first."
            );
        }
        let content = std::fs::read_to_string(&path)?;
        let creds: Self = serde_json::from_str(&content)?;
        Ok(creds)
    }

    fn delete() -> Result<()> {
        let path = credentials_path();
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

#[derive(Parser)]
#[command(name = "orignabase")]
#[command(about = "OrignaBase — A lightweight, blazingly fast, self-hosted Backend-as-a-Service")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to configuration file
    #[arg(short, long, global = true)]
    config: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the OrignaBase server
    Serve,
    /// Print the current configuration
    Config,

    /// Authenticate with a remote OrignaBase server
    Login {
        /// OrignaBase server URL (e.g., https://api.example.com)
        #[arg(long)]
        server: String,
        /// Admin API key for authentication
        #[arg(long)]
        api_key: String,
        /// Optional project name for display
        #[arg(long)]
        project: Option<String>,
    },
    /// Remove stored credentials
    Logout,
    /// Show current login status and server info
    Whoami,
    /// Check the health of a remote OrignaBase server
    Status {
        /// Server URL (uses saved credentials if omitted)
        #[arg(long)]
        server: Option<String>,
    },

    /// Migrate data from another backend
    Migrate {
        #[command(subcommand)]
        source: MigrateSource,
    },
    /// Generate typed client code from the GraphQL schema
    Codegen {
        #[command(subcommand)]
        target: CodegenTarget,
    },
    /// Initialize a new OrignaBase project
    Init {
        /// Project name (used for directory and config)
        #[arg(default_value = ".")]
        path: String,
    },
    /// Manage database schema migrations
    Schema {
        #[command(subcommand)]
        action: SchemaAction,
    },
    /// Manage users (list, get, create)
    Users {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Export database to a JSON backup file
    Backup {
        /// Output file path
        #[arg(long, default_value = "./backup.json")]
        output: String,
    },
    /// Restore database from a JSON backup file
    Restore {
        /// Input backup file path
        #[arg(long)]
        input: String,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum CodegenTarget {
    /// Generate Dart/Flutter models from GraphQL introspection
    Dart {
        /// OrignaBase server URL to introspect
        #[arg(long, default_value = "http://localhost:8080")]
        url: String,
        /// Output directory for generated Dart files
        #[arg(long, default_value = "./lib/generated")]
        output: String,
    },
}

#[derive(Subcommand)]
enum SchemaAction {
    /// Inspect current database schema
    Inspect,
    /// Create a new migration file
    Create {
        /// Migration name (e.g., "add_users_table")
        name: String,
    },
    /// Run pending migrations
    Up,
    /// Rollback the last migration
    Down,
    /// Apply index definitions from indexes.toml
    Indexes,
}

#[derive(Subcommand)]
enum AuthAction {
    /// List all users
    List {
        /// Maximum number of users to show
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Show user details by ID
    Get {
        /// User ID
        id: String,
    },
}

#[derive(Subcommand)]
enum MigrateSource {
    /// Migrate from Firebase/Firestore JSON export
    FromFirebase {
        /// Path to the Firestore JSON export directory
        #[arg(long)]
        export_path: String,
        /// OrignaBase target URL (e.g., http://localhost:8080)
        #[arg(long, default_value = "http://localhost:8080")]
        target_url: String,
        /// Specific collections to migrate (comma-separated). If empty, migrates all.
        #[arg(long)]
        collections: Option<String>,
        /// Dry-run mode — show what would be migrated without writing
        #[arg(long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .json()
        .init();

    let cli = Cli::parse();
    let config_path = cli.config.as_deref().map(std::path::Path::new);
    let config = Config::load(config_path)?;

    match cli.command {
        Commands::Config => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "host": config.host,
                    "port": config.port,
                    "database": {
                        "endpoint": config.database.endpoint,
                        "namespace": config.database.namespace,
                        "name": config.database.name,
                    },
                    "auth": {
                        "access_token_ttl_secs": config.auth.access_token_ttl_secs,
                        "refresh_token_ttl_secs": config.auth.refresh_token_ttl_secs,
                    },
                    "security": {
                        "rules_path": config.security.rules_path,
                    }
                }))?
            );
            Ok(())
        }

        // ── Login / Logout / Whoami / Status ──
        Commands::Login {
            server,
            api_key,
            project,
        } => {
            // Validate the server URL by checking health endpoint
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()?;
            let health_url = format!("{server}/_admin/health");
            match client.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    let version = body["version"].as_str().unwrap_or("unknown");
                    let creds = CliCredentials {
                        server_url: server.clone(),
                        api_key,
                        project_name: project,
                        logged_in_at: Some(chrono::Utc::now().to_rfc3339()),
                    };
                    creds.save()?;
                    eprintln!("  Logged in to {server} (OrignaBase v{version})");
                    eprintln!("  Credentials saved to {}", credentials_path().display());
                }
                Ok(resp) => {
                    anyhow::bail!(
                        "Server at {server} returned HTTP {}. Is it an OrignaBase server?",
                        resp.status()
                    );
                }
                Err(e) => {
                    anyhow::bail!(
                        "Cannot reach {server}: {e}\n  Check the URL and ensure the server is running."
                    );
                }
            }
            Ok(())
        }
        Commands::Logout => {
            CliCredentials::delete()?;
            eprintln!("  Logged out. Credentials removed.");
            Ok(())
        }
        Commands::Whoami => {
            match CliCredentials::load() {
                Ok(creds) => {
                    println!("Server:     {}", creds.server_url);
                    println!(
                        "Project:    {}",
                        creds.project_name.as_deref().unwrap_or("(none)")
                    );
                    println!(
                        "API Key:    {}...",
                        &creds.api_key[..creds.api_key.len().min(8)]
                    );
                    println!(
                        "Logged in:  {}",
                        creds.logged_in_at.as_deref().unwrap_or("unknown")
                    );
                }
                Err(_) => {
                    eprintln!(
                        "Not logged in. Run `orignabase login --server <url> --api-key <key>`"
                    );
                }
            }
            Ok(())
        }
        Commands::Status { server } => {
            let url = if let Some(s) = server {
                s
            } else if let Ok(creds) = CliCredentials::load() {
                creds.server_url
            } else {
                anyhow::bail!(
                    "No server specified. Use --server <url> or login first with `orignabase login`."
                );
            };
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()?;
            match client.get(format!("{url}/_admin/health")).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    println!("Server:    {url}");
                    println!("Status:    healthy");
                    println!(
                        "Version:   {}",
                        body["version"].as_str().unwrap_or("unknown")
                    );
                    println!(
                        "Timestamp: {}",
                        body["timestamp"].as_str().unwrap_or("unknown")
                    );
                }
                Ok(resp) => {
                    eprintln!("Server:  {url}");
                    eprintln!("Status:  unhealthy (HTTP {})", resp.status());
                }
                Err(e) => {
                    eprintln!("Server:  {url}");
                    eprintln!("Status:  unreachable ({e})");
                }
            }
            Ok(())
        }

        Commands::Serve => {
            validate_config_warnings(&config)?;
            serve(config).await
        }
        Commands::Codegen { target } => match target {
            CodegenTarget::Dart { url, output } => codegen_dart(&url, &output).await,
        },
        Commands::Backup { output } => backup_database(&config, &output).await,
        Commands::Restore { input, yes } => restore_database(&config, &input, yes).await,
        Commands::Migrate { source } => match source {
            MigrateSource::FromFirebase {
                export_path,
                target_url,
                collections,
                dry_run,
            } => {
                migrate_from_firebase(&export_path, &target_url, collections.as_deref(), dry_run)
                    .await
            }
        },
        Commands::Init { path } => {
            let dir = std::path::Path::new(&path);
            std::fs::create_dir_all(dir)?;
            std::fs::write(
                dir.join("orignabase.toml"),
                include_str!("../../ob-core/src/config.rs")
                    .lines()
                    .take(0)
                    .collect::<String>()
                    + &format!(
                        r#"# OrignaBase Configuration
host = "127.0.0.1"
port = 8080

[database]
endpoint = "localhost:8000"
namespace = "orignabase"
name = "main"

[auth]
jwt_secret = "{}"

[security]
rules_path = "rules.ob"
"#,
                        uuid::Uuid::new_v4()
                    ),
            )?;
            std::fs::write(
                dir.join("rules.ob"),
                r#"// OrignaBase Security Rules
// See: https://orignabase.dev/docs/security-rules

match /users/{userId} {
  allow read: auth.uid != null;
  allow write: auth.uid == userId;
}
"#,
            )?;
            std::fs::write(
                dir.join("indexes.toml"),
                r#"# OrignaBase Index Definitions
# Apply with: orignabase schema indexes
#
# Indexes improve query performance on frequently filtered/sorted fields.
# Applied idempotently — safe to re-run anytime.

# [[indexes]]
# collection = "users"
# fields = ["email"]
# unique = true

# [[indexes]]
# collection = "orders"
# fields = ["user_id", "created_at"]
"#,
            )?;
            println!("Initialized OrignaBase project at {}", dir.display());
            println!("  Created: orignabase.toml, rules.ob, indexes.toml");
            println!("  Next: start SurrealDB, then run `orignabase serve`");
            Ok(())
        }
        Commands::Schema { action } => {
            let db = DatabaseClient::connect(&config.database).await?;
            match action {

                SchemaAction::Inspect => {
                    let tables = db.query_raw("INFO FOR DB").await?;
                    println!("{}", serde_json::to_string_pretty(&tables)?);
                    Ok(())
                }
                SchemaAction::Create { name } => {
                    let migrations_dir = std::path::Path::new("migrations");
                    std::fs::create_dir_all(migrations_dir)?;
                    let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
                    let filename = format!("{timestamp}_{name}.surql");
                    let path = migrations_dir.join(&filename);
                    std::fs::write(
                        &path,
                        format!("-- Migration: {name}\n-- Created: {timestamp}\n\n"),
                    )?;
                    println!("Created migration: {}", path.display());
                    Ok(())
                }
                SchemaAction::Up => {
                    let migrations_dir = std::path::Path::new("migrations");
                    if !migrations_dir.exists() {
                        println!("No migrations directory found.");
                        return Ok(());
                    }
                    let mut entries: Vec<_> = std::fs::read_dir(migrations_dir)?
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().extension().map(|x| x == "surql").unwrap_or(false))
                        .collect();
                    entries.sort_by_key(|e| e.file_name());
                    for entry in entries {
                        let sql = std::fs::read_to_string(entry.path())?;
                        println!("Running: {}", entry.file_name().to_string_lossy());
                        db.query_raw(&sql).await?;
                    }
                    println!("All migrations applied.");
                    Ok(())
                }
                SchemaAction::Down => {
                    anyhow::bail!(
                        "Down migrations not yet implemented.\n  \
                         Create a rollback file:  migrations/<timestamp>_<name>.down.surql\n  \
                         Then run it manually:    orignabase schema up  (with the .down.surql file)"
                    );
                }
                SchemaAction::Indexes => apply_indexes(&db).await,
            }
        }
        Commands::Users { action } => {
            let db = DatabaseClient::connect(&config.database).await?;
            match action {
                AuthAction::List { limit } => {
                    let query = format!(
                        "SELECT id, email, display_name, roles, email_verified, mfa_enabled, created_at FROM users LIMIT {limit}"
                    );
                    let users = db.query_raw(&query).await?;
                    println!("{}", serde_json::to_string_pretty(&users)?);
                    Ok(())
                }
                AuthAction::Get { id } => {
                    let users = db
                        .query_bind(
                            "SELECT id, email, display_name, roles, email_verified, mfa_enabled, created_at, custom_claims FROM type::thing($uid)",
                            serde_json::json!({ "uid": id }),
                        )
                        .await?;
                    if let Some(user) = users.first() {
                        println!("{}", serde_json::to_string_pretty(user)?);
                    } else {
                        println!("User not found: {id}");
                    }
                    Ok(())
                }
            }
        }
    }
}

/// Build CORS layer with explicit origin whitelist.
/// CRITICAL FIX: Replace .allow_origin(Any) with specific production domains.
fn build_cors_layer(is_test_mode: bool) -> tower_http::cors::CorsLayer {
    let mut allowed_origins = vec![
        "https://orignagta.ca".parse::<HeaderValue>().unwrap(),
        "https://www.orignagta.ca".parse::<HeaderValue>().unwrap(),
        "https://dev.orignagta.ca".parse::<HeaderValue>().unwrap(),
        "https://staging.orignagta.ca".parse::<HeaderValue>().unwrap(),
    ];

    // Allow localhost ONLY in test mode (for local development)
    if is_test_mode {
        allowed_origins.push("http://localhost:3000".parse::<HeaderValue>().unwrap());
        allowed_origins.push("http://localhost:5173".parse::<HeaderValue>().unwrap());
    }

    let mut cors = tower_http::cors::CorsLayer::new()
        .allow_credentials(true);

    for origin in allowed_origins {
        cors = cors.allow_origin(origin);
    }

    cors
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
}

/// Validate configuration and panic on critical security issues.
/// CRITICAL FIX: In production (no OB_TEST_MODE), panic if JWT secret is default.
fn validate_config_warnings(config: &Config) -> Result<()> {
    let mut warnings = Vec::new();
    let mut fatal = Vec::new();

    let is_test_mode = std::env::var("OB_TEST_MODE").unwrap_or_default() == "1";

    if config.auth.jwt_secret == "CHANGE_ME_IN_PRODUCTION" {
        let msg = "JWT secret is the default value (INSECURE). Set OB_AUTH__JWT_SECRET or auth.jwt_secret in orignabase.toml.";
        warnings.push((
            "critical",
            msg,
        ));

        // CRITICAL FIX: Panic in production, warn in test mode
        if !is_test_mode {
            eprintln!();
            eprintln!("  [CRITICAL] {}", msg);
            eprintln!();
            eprintln!("  REFUSING TO START: Production cannot run with default JWT secret.");
            eprintln!("  Set OB_AUTH__JWT_SECRET to a cryptographically secure random value.");
            eprintln!();
            panic!("JWT secret is the default value — cannot start in production");
        }
        fatal.push("JWT secret is the default value");
    }

    if config.auth.jwt_secret.len() < 32 {
        warnings.push((
            "critical",
            "JWT secret is shorter than 32 characters. Use a strong random secret (≥ 64 chars).",
        ));
        fatal.push("JWT secret is shorter than 32 characters");
    }

    if !Path::new(&config.security.rules_path).exists() {
        warnings.push((
            "warn",
            "No security rules file found. All access will be denied by default.",
        ));
    }

    if config.host == "0.0.0.0" {
        // Not a warning for production, but worth noting
        warnings.push((
            "info",
            "Server listening on all interfaces (0.0.0.0). Use 127.0.0.1 for local-only access.",
        ));
    }

    for (level, msg) in &warnings {
        match *level {
            "critical" => eprintln!("  [CRITICAL] {msg}"),
            "warn" => eprintln!("  [WARNING]  {msg}"),
            "info" => eprintln!("  [INFO]     {msg}"),
            _ => eprintln!("  {msg}"),
        }
    }

    // Block startup on critical issues
    if warnings.iter().any(|(l, _)| *l == "critical") {
        eprintln!();
        eprintln!("  Fix critical issues above before running in production.");
        eprintln!("  To proceed anyway, set OB_AUTH__JWT_SECRET to a secure value.");
    }

    if !fatal.is_empty() {
        anyhow::bail!(fatal.join("; "));
    }

    Ok(())
}

async fn serve(config: Config) -> Result<()> {
    // --- Database ---
    let db = DatabaseClient::connect(&config.database).await?;

    // --- Shared HTTP Client ---
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("failed to build HTTP client");

    // --- Security Rules ---
    let is_test_mode = std::env::var("OB_TEST_MODE").unwrap_or_default() == "1";
    let rules_path = if is_test_mode && config.security.rules_path == "rules.ob" {
        let test_rules_path = "rules.test.ob";
        if std::path::Path::new(test_rules_path).exists() {
            tracing::info!(
                "OB_TEST_MODE=1 detected; loading test security rules from '{}'",
                test_rules_path
            );
            test_rules_path.to_string()
        } else {
            config.security.rules_path.clone()
        }
    } else {
        config.security.rules_path.clone()
    };

    let rules = if std::path::Path::new(&rules_path).exists() {
        let content = std::fs::read_to_string(&rules_path)?;
        parse_rules(&content)?
    } else {
        tracing::warn!(
            "No rules file at '{}', using empty rules (all access denied)",
            rules_path
        );
        HashMap::new()
    };
    let rule_engine = Arc::new(RuleEngine::new(rules));

    // --- Realtime ---
    let registry = SubscriptionRegistry::new();
    let (dispatcher, dispatcher_tx) = ChangeDispatcher::new(registry.clone());
    tokio::spawn(dispatcher.run());

    // --- DB Trigger Executor ---
    let (trigger_tx, trigger_rx) = tokio::sync::mpsc::channel(1024);
    let (native_trigger_tx, native_trigger_rx) = tokio::sync::mpsc::channel(1024);

    // --- Search Sync ---
    let search_config = config
        .search
        .as_ref()
        .map(|search| SearchConfig {
            enabled: true,
            url: search.url.clone(),
            api_key: search.api_key.clone(),
            indexes: HashMap::new(),
        })
        .unwrap_or_default();
    let search_client = SearchClient::new(search_config, http_client.clone());
    let (search_syncer, search_sync_tx) = SearchSyncer::new(search_client.clone());
    tokio::spawn(search_syncer.run());

    // --- Fan-out channel: producers send here, bridge distributes to all consumers ---
    let (change_tx, mut change_rx) =
        tokio::sync::mpsc::channel::<ob_realtime::registry::ChangeEvent>(1024);

    #[cfg(feature = "cluster")]
    let mut cluster_tx: Option<tokio::sync::mpsc::Sender<ob_realtime::registry::ChangeEvent>> =
        None;

    // --- Cluster bridge (optional) ---
    #[cfg(feature = "cluster")]
    {
        if config.cluster.enabled {
            use ob_realtime::cluster::{ClusterBridge, ClusterConfig as NatsClusterConfig};
            let nats_config = NatsClusterConfig {
                nats_url: config.cluster.nats_url.clone(),
                stream_name: "ORIGNABASE_CHANGES".to_string(),
                node_id: config
                    .cluster
                    .node_id
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            };
            let (bridge, cluster_local_tx) = ClusterBridge::new(nats_config, dispatcher_tx.clone());
            cluster_tx = Some(cluster_local_tx);
            tokio::spawn(async move {
                if let Err(e) = bridge.run().await {
                    tracing::error!("Cluster bridge error: {e}");
                }
            });
            tracing::info!("NATS cluster sync enabled");
        }
    }

    {
        let dispatcher_tx = dispatcher_tx.clone();
        let trigger_tx = trigger_tx.clone();
        let native_trigger_tx = native_trigger_tx.clone();
        let search_sync_tx = search_sync_tx.clone();
        #[cfg(feature = "cluster")]
        let cluster_tx = cluster_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = change_rx.recv().await {
                // Fan-out to realtime dispatcher
                let _ = dispatcher_tx.send(event.clone()).await;
                // Fan-out to DB trigger executor
                let _ = trigger_tx.send(event.clone()).await;
                // Fan-out to native Rust trigger executor
                let _ = native_trigger_tx.send(event.clone()).await;
                // Fan-out to cluster publisher for remote realtime sync
                #[cfg(feature = "cluster")]
                if let Some(cluster_tx) = &cluster_tx {
                    let _ = cluster_tx.send(event.clone()).await;
                }
                // Fan-out to search syncer
                let search_action = match event.action {
                    ob_realtime::registry::ChangeAction::Create
                    | ob_realtime::registry::ChangeAction::Update => SearchAction::Upsert,
                    ob_realtime::registry::ChangeAction::Delete => SearchAction::Delete,
                };
                let _ = search_sync_tx
                    .send(SearchSyncEvent {
                        action: search_action,
                        index: event.collection.clone(),
                        document_id: event.document_id.clone(),
                        data: event.data,
                    })
                    .await;
            }
        });
    }

    // --- GraphQL ---
    let schema = build_schema(
        db.clone(),
        rule_engine.clone(),
        change_tx.clone(),
        search_client.clone(),
    );

    // --- JWT Keys ---
    // Try RS256 (auto-generate if keys don't exist), fall back to HS256
    let jwt_keys = {
        let keys_dir = std::path::Path::new("./data/keys");
        let private_path = keys_dir.join("jwt_private.pem");
        let public_path = keys_dir.join("jwt_public.pem");

        if private_path.exists() && public_path.exists() {
            let private_pem = std::fs::read(&private_path)?;
            let public_pem = std::fs::read(&public_path)?;
            match ob_auth::JwtKeys::from_rsa_pem(&private_pem, &public_pem) {
                Ok(keys) => {
                    tracing::info!(
                        "Using RS256 JWT signing (RSA keys from {})",
                        keys_dir.display()
                    );
                    keys
                }
                Err(e) => {
                    tracing::warn!("Invalid RSA keys, falling back to HS256: {e}");
                    ob_auth::JwtKeys::from_secret(&config.auth.jwt_secret)
                }
            }
        } else {
            // Try to auto-generate RSA keys
            match ob_auth::generate_rsa_keys(keys_dir) {
                Ok((private_pem, public_pem)) => {
                    match ob_auth::JwtKeys::from_rsa_pem(&private_pem, &public_pem) {
                        Ok(keys) => {
                            tracing::info!(
                                "Auto-generated RS256 JWT keys at {}",
                                keys_dir.display()
                            );
                            keys
                        }
                        Err(e) => {
                            tracing::warn!("Failed to load generated RSA keys: {e}");
                            ob_auth::JwtKeys::from_secret(&config.auth.jwt_secret)
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Could not generate RSA keys (openssl not found?), using HS256: {e}"
                    );
                    ob_auth::JwtKeys::from_secret(&config.auth.jwt_secret)
                }
            }
        }
    };

    // --- Auth ---
    // Read Apple private key from file if configured
    let apple_private_key = config
        .auth
        .apple
        .as_ref()
        .and_then(|a| std::fs::read_to_string(&a.private_key_path).ok());

    // --- Email Service ---
    let email_service = ob_auth::EmailConfig::from_env()
        .map(|config| ob_auth::EmailService::with_db(config, db.clone()));
    if email_service.is_some() {
        tracing::info!("Email service configured via OB_EMAIL__* env vars");
    }

    // --- TOTP Encryption Key ---
    let totp_encryption_key =
        std::env::var("OB_AUTH__TOTP_ENCRYPTION_KEY")
            .ok()
            .and_then(|hex_key| {
                let bytes = hex::decode(&hex_key).ok()?;
                if bytes.len() == 32 {
                    let mut key = [0u8; 32];
                    key.copy_from_slice(&bytes);
                    Some(key)
                } else {
                    tracing::warn!("OB_AUTH__TOTP_ENCRYPTION_KEY must be 64 hex chars (32 bytes)");
                    None
                }
            });

    let base_url = std::env::var("OB_BASE_URL")
        .unwrap_or_else(|_| format!("http://{}:{}", config.host, config.port));

    let require_email_verification = std::env::var("OB_AUTH__REQUIRE_EMAIL_VERIFICATION")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    // --- HTTP Client (shared across auth handlers) ---
    let auth_http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("failed to build auth HTTP client");

    let auth_state = AuthState {
        db: db.clone(),
        jwt_keys: jwt_keys.clone(),
        access_ttl: config.auth.access_token_ttl_secs,
        refresh_ttl: config.auth.refresh_token_ttl_secs,
        google_client_id: config.auth.google.as_ref().map(|g| g.client_id.clone()),
        google_client_secret: config
            .auth
            .google
            .as_ref()
            .and_then(|g| g.client_secret.clone()),
        apple_team_id: config.auth.apple.as_ref().map(|a| a.team_id.clone()),
        apple_key_id: config.auth.apple.as_ref().map(|a| a.key_id.clone()),
        apple_service_id: config.auth.apple.as_ref().map(|a| a.service_id.clone()),
        apple_private_key,
        oidc_issuer_url: config.auth.oidc.as_ref().map(|o| o.issuer_url.clone()),
        oidc_client_id: config.auth.oidc.as_ref().map(|o| o.client_id.clone()),
        email_service,
        require_email_verification,
        totp_encryption_key,
        base_url: base_url.clone(),
        oauth_state_nonces: Arc::new(dashmap::DashMap::new()),
        turnstile_secret_key: config.secret("turnstile_secret_key").map(|s| s.to_string()),
        http_client: auth_http_client,
    };

    // --- Storage ---
    let storage = LocalStorage::new("./data/storage")?;
    let url_gen = SignedUrlGenerator::new(&config.auth.jwt_secret, &base_url);
    let resumable = ResumableUploadManager::new("./data/storage/.uploads")?;
    let storage_state = StorageState {
        storage,
        url_generator: url_gen,
        resumable,
    };

    // --- Functions (WASM) ---
    let wasm_runtime = Arc::new(WasmRuntime::new(FunctionLimits::default())?);
    let function_registry = Arc::new(FunctionRegistry::new(wasm_runtime.clone()));
    let functions_state = FunctionsState {
        registry: function_registry.clone(),
        db: Some(db.clone()),
    };

    // --- Cron Scheduler ---
    let cron_scheduler = CronScheduler::new(function_registry.clone(), wasm_runtime.clone());
    tokio::spawn(cron_scheduler.run());

    // --- DB Trigger Executor ---
    let db_trigger_executor =
        DbTriggerExecutor::new(function_registry.clone(), wasm_runtime.clone(), trigger_rx);
    tokio::spawn(db_trigger_executor.run());

    // --- Analytics ---
    let analytics_state = AnalyticsState {
        ip_salt: config.auth.jwt_secret.clone(), // Reuse secret as salt
        db: db.clone(),
    };

    // --- Analytics Retention Policy ---
    let retention_policy = ob_analytics::RetentionPolicy::new(db.clone(), 90);
    tokio::spawn(retention_policy.run());

    // --- Task Queue ---
    let task_queue = Arc::new(ob_database::TaskQueue::new(db.clone()));
    {
        let queue = task_queue.clone();
        tokio::spawn(async move {
            ob_database::run_worker(queue, "default", |task| async move {
                tracing::info!(
                    task_type = %task.task_type,
                    "Processing task"
                );
                // Default worker logs tasks — custom handlers are registered via the API
                Ok(())
            })
            .await;
        });
        tracing::info!("Task queue worker started (queue: default)");
    }

    // --- Notifications (FCM Push) ---
    let notifications_state = ob_notifications::NotificationsState::new(
        db.clone(),
        std::env::var("OB_FCM_PROJECT_ID").ok(),
        std::env::var("OB_FCM_SERVICE_ACCOUNT").ok(),
        http_client.clone(),
    );

    // --- Admin ---
    let admin_state = AdminState { db: db.clone() };

    // --- Business Logic Handlers (origna_gta marketplace) ---
    let handlers_state =
        ob_handlers::HandlersState::new(std::sync::Arc::new(config.clone()), db.clone());
    let native_trigger_executor = ob_handlers::native_triggers::NativeTriggerExecutor::new(
        handlers_state.clone(),
        native_trigger_rx,
    );
    tokio::spawn(native_trigger_executor.run());

    // --- Rate Limiting (governor) ---
    // In test mode (OB_TEST_MODE=1), use much higher limits to avoid 429s in integration tests
    let (auth_replenish_ms, auth_burst) = if is_test_mode {
        (1, 10000u32) // effectively unlimited for tests
    } else {
        (6000, 10u32) // 10 requests per 60 seconds per IP
    };
    let (api_replenish_ms, api_burst) = if is_test_mode {
        (1, 100000u32) // effectively unlimited for tests
    } else {
        (600, 100u32) // 100 requests per 60 seconds per IP
    };

    // Auth routes
    let auth_governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .key_extractor(PeerIpKeyExtractor)
            .per_millisecond(auth_replenish_ms)
            .burst_size(auth_burst)
            .finish()
            .expect("valid governor config for auth"),
    );
    let auth_governor_limiter = auth_governor_conf.limiter().clone();
    // Spawn periodic cleanup for auth limiter
    {
        let limiter = auth_governor_limiter.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                limiter.retain_recent();
            }
        });
    }

    // API routes
    let api_governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .key_extractor(PeerIpKeyExtractor)
            .per_millisecond(api_replenish_ms)
            .burst_size(api_burst)
            .finish()
            .expect("valid governor config for api"),
    );
    let api_governor_limiter = api_governor_conf.limiter().clone();
    // Spawn periodic cleanup for API limiter
    {
        let limiter = api_governor_limiter.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                limiter.retain_recent();
            }
        });
    }

    // --- Router Assembly ---
    let addr = format!("{}:{}", config.host, config.port);

    // --- HTTP Function Triggers (catch-all under /fn/*) ---
    let fn_registry_for_triggers = function_registry.clone();
    let fn_runtime_for_triggers = wasm_runtime.clone();

    // Auth router with strict rate limiting (10 req/min per IP)
    let auth = auth_router(auth_state).layer(GovernorLayer::new(auth_governor_conf));

    // Tenant config for namespace resolution
    let tenant_config = config.tenant.clone();

    // --- Static File Hosting (optional) ---
    // If ./public/ exists, serve static files from it (Firebase Hosting replacement)
    let public_dir = std::path::Path::new("./public");

    // Health check is outside the governor layer to avoid IP extraction issues
    // behind reverse proxies (Docker/Caddy).
    let health_route = Router::new().route("/health", axum::routing::get(|| async { "ok" }));

    let app = Router::new()
        .route(
            "/graphql",
            axum::routing::get(|| async {
                axum::response::Html(GraphiQLSource::build().endpoint("/graphql").finish())
            })
            .post({
                let schema = schema.clone();
                let jwt_keys_for_gql = Arc::new(jwt_keys.clone());
                move |req: axum::extract::Request| {
                    let schema = schema.clone();
                    let keys = jwt_keys_for_gql.clone();
                    async move {
                        // Extract AuthContext from Bearer token
                        let auth_ctx = if let Some(auth_header) = req.headers().get(AUTHORIZATION) {
                            if let Ok(header_str) = auth_header.to_str() {
                                if let Some(token) = header_str.strip_prefix("Bearer ") {
                                    match ob_auth::jwt::verify_token(token, &keys) {
                                        Ok(claims) if claims.typ == "access" => {
                                            ob_auth::AuthContext::from_claims(claims)
                                        }
                                        _ => ob_auth::AuthContext::anonymous(),
                                    }
                                } else {
                                    ob_auth::AuthContext::anonymous()
                                }
                            } else {
                                ob_auth::AuthContext::anonymous()
                            }
                        } else {
                            ob_auth::AuthContext::anonymous()
                        };

                        // Parse GraphQL request from body
                        let body = axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024)
                            .await
                            .unwrap_or_default();
                        let gql_req: async_graphql::Request = serde_json::from_slice(&body)
                            .unwrap_or_else(|_| async_graphql::Request::new(""));

                        // Inject auth context into GraphQL data
                        let gql_req = gql_req.data(auth_ctx);
                        let resp = schema.execute(gql_req).await;
                        axum::Json(resp)
                    }
                }
            }),
        )
        .merge(auth)
        .merge(realtime_router(registry, jwt_keys.clone()))
        .merge(storage_router(storage_state))
        .merge(functions_router(functions_state))
        .merge(analytics_router(analytics_state))
        .merge(ob_notifications::notifications_router(notifications_state))
        .merge(admin_router(admin_state))
        .merge(ob_handlers::handlers_router(handlers_state))
        // HTTP function triggers: /fn/{*path} catches any method
        .route(
            "/fn/{*path}",
            axum::routing::any({
                let registry = fn_registry_for_triggers;
                let runtime = fn_runtime_for_triggers;
                move |method: axum::http::Method,
                      path: axum::extract::Path<String>,
                      body: axum::body::Bytes| {
                    let registry = registry.clone();
                    let runtime = runtime.clone();
                    async move {
                        let fn_path = format!("/{}", path.0);
                        let method_str = method.as_str();
                        match registry.find_http_trigger(method_str, &fn_path) {
                            Some(fn_name) => match registry.get_module(&fn_name) {
                                Ok(module) => {
                                    let input =
                                        String::from_utf8(body.to_vec()).unwrap_or_default();
                                    match runtime.execute(&module, "handle", &input).await {
                                        Ok(result) => {
                                            (axum::http::StatusCode::OK, result).into_response()
                                        }
                                        Err(e) => (
                                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                                            format!("Function error: {e}"),
                                        )
                                            .into_response(),
                                    }
                                }
                                Err(e) => (
                                    axum::http::StatusCode::NOT_FOUND,
                                    format!("Function not found: {e}"),
                                )
                                    .into_response(),
                            },
                            None => (
                                axum::http::StatusCode::NOT_FOUND,
                                "No function registered for this route".to_string(),
                            )
                                .into_response(),
                        }
                    }
                }
            }),
        )
        // Static file hosting (if ./public/ directory exists)
        .nest_service("/static", tower_http::services::ServeDir::new(public_dir))
        // API-wide rate limiting (100 req/min per IP) — applied after auth's own stricter limit
        .layer(GovernorLayer::new(api_governor_conf))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024)) // 2MB default body limit
        .layer(CatchPanicLayer::new())
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(
            build_cors_layer(is_test_mode),
        )
        .layer(axum::middleware::from_fn(
            ob_auth::middleware::auth_extractor,
        ))
        .layer(axum::Extension(Arc::new(jwt_keys.clone())))
        .layer(axum::Extension(tenant_config))
        .layer(axum::middleware::from_fn(
            ob_core::tenant::tenant_middleware,
        ))
        .merge(health_route);

    tracing::info!("OrignaBase listening on {addr}");
    tracing::info!("  GraphiQL:  http://{addr}/graphql");
    tracing::info!("  Realtime:  ws://{addr}/realtime");
    tracing::info!("  Functions: http://{addr}/functions");
    tracing::info!("  Analytics: http://{addr}/analytics/event");
    tracing::info!("  Admin:     http://{addr}/_admin/health");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;

    Ok(())
}

// ── Backup / Restore ──

async fn backup_database(config: &Config, output: &str) -> Result<()> {
    let db = ob_database::DatabaseClient::connect(&config.database).await?;

    // List all tables in the database
    let tables_result = db.query_raw_value("INFO FOR DB").await?;

    let table_names: Vec<String> = tables_result
        .get("tables")
        .or_else(|| tables_result.get("tb"))
        .and_then(|v| v.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();

    if table_names.is_empty() {
        println!("No tables found in database. Nothing to back up.");
        return Ok(());
    }

    println!("Backing up {} tables...", table_names.len());

    let mut backup = serde_json::Map::new();
    let mut total_docs: u64 = 0;

    for table in &table_names {
        let docs = db.query_raw(&format!("SELECT * FROM {table}")).await?;

        let count = docs.len() as u64;
        total_docs += count;
        println!("  {table}: {count} documents");
        backup.insert(table.clone(), serde_json::Value::Array(docs));
    }

    let backup_json = serde_json::json!({ "tables": backup });
    std::fs::write(output, serde_json::to_string_pretty(&backup_json)?)?;

    println!("Backup complete: {total_docs} documents written to {output}");
    Ok(())
}

async fn restore_database(config: &Config, input: &str, skip_confirm: bool) -> Result<()> {
    let content = std::fs::read_to_string(input)
        .map_err(|e| anyhow::anyhow!("Failed to read backup file '{input}': {e}"))?;

    let backup: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Invalid JSON in backup file: {e}"))?;

    let tables = backup
        .get("tables")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("Backup file missing 'tables' object"))?;

    let total_docs: usize = tables
        .values()
        .filter_map(|v| v.as_array())
        .map(|a| a.len())
        .sum();
    println!(
        "Restore plan: {} tables, {} documents from {input}",
        tables.len(),
        total_docs
    );

    if !skip_confirm {
        use std::io::Write;
        print!("Proceed? [y/N] ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Restore cancelled.");
            return Ok(());
        }
    }

    let db = ob_database::DatabaseClient::connect(&config.database).await?;

    let mut restored: u64 = 0;
    let mut errors: u64 = 0;

    for (table, docs_value) in tables {
        let docs = match docs_value.as_array() {
            Some(arr) => arr,
            None => {
                eprintln!("  Skipping '{table}': value is not an array");
                continue;
            }
        };

        for doc in docs {
            let doc_json = serde_json::to_string(doc)?;
            let query = format!("CREATE {table} CONTENT {doc_json}");
            match db.query_raw(&query).await {
                Ok(_) => restored += 1,
                Err(e) => {
                    eprintln!("  Error restoring doc to '{table}': {e}");
                    errors += 1;
                }
            }
        }

        println!("  {table}: {} documents", docs.len());
    }

    println!("Restore complete: {restored} succeeded, {errors} errors");
    Ok(())
}

// ── Firebase Migration ──

async fn migrate_from_firebase(
    export_path: &str,
    target_url: &str,
    collections: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let export_dir = Path::new(export_path);
    if !export_dir.exists() {
        anyhow::bail!("Export path does not exist: {export_path}");
    }

    let collection_filter: Option<Vec<&str>> =
        collections.map(|c| c.split(',').map(str::trim).collect());

    println!("OrignaBase Firebase Migration Tool");
    println!("  Source:      {export_path}");
    println!("  Target:      {target_url}");
    println!("  Collections: {}", collections.unwrap_or("all"));
    println!("  Dry run:     {dry_run}");
    println!();

    // Discover collection JSON files in the export directory.
    // Supports two formats:
    //   1. Flat: export_path/<collection>.json (array of documents)
    //   2. Nested: export_path/<collection>/<doc_id>.json (one doc per file)
    let mut total_docs = 0u64;
    let mut total_errors = 0u64;
    let client = reqwest::Client::new();

    let mut entries: Vec<_> = std::fs::read_dir(export_dir)?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in &entries {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Determine collection name
        let collection_name = if path.is_file() && name_str.ends_with(".json") {
            name_str.trim_end_matches(".json").to_string()
        } else if path.is_dir() {
            name_str.to_string()
        } else {
            continue;
        };

        // Apply collection filter
        if let Some(ref filter) = collection_filter
            && !filter.contains(&collection_name.as_str())
        {
            continue;
        }

        // Load documents
        let docs: Vec<serde_json::Value> = if path.is_file() {
            let content = std::fs::read_to_string(&path)?;
            let parsed: serde_json::Value = serde_json::from_str(&content)?;
            match parsed {
                serde_json::Value::Array(arr) => arr,
                serde_json::Value::Object(_) => vec![parsed],
                _ => {
                    println!("  ⚠ Skipping {name_str}: not an array or object");
                    continue;
                }
            }
        } else {
            // Directory: each file is a document
            let mut docs = Vec::new();
            let mut sub_entries: Vec<_> =
                std::fs::read_dir(&path)?.filter_map(|e| e.ok()).collect();
            sub_entries.sort_by_key(|e| e.file_name());
            for sub in &sub_entries {
                if sub.path().extension().is_some_and(|ext| ext == "json") {
                    let content = std::fs::read_to_string(sub.path())?;
                    let doc: serde_json::Value = serde_json::from_str(&content)?;
                    docs.push(doc);
                }
            }
            docs
        };

        println!("  {collection_name}: {} documents", docs.len());

        if dry_run {
            // Show first 3 doc IDs as preview
            for (i, doc) in docs.iter().take(3).enumerate() {
                let id = doc
                    .get("id")
                    .or_else(|| doc.get("_id"))
                    .or_else(|| doc.get("__name__"))
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| format!("doc_{i}"));
                println!("   - {id}");
            }
            if docs.len() > 3 {
                println!("   ... and {} more", docs.len() - 3);
            }
            total_docs += docs.len() as u64;
            continue;
        }

        // Translate Firestore types → plain JSON, then POST to OrignaBase
        // Use parameterized GraphQL variables to prevent injection
        let url = format!("{target_url}/graphql");
        let collection = &collection_name;
        let mutation_query = format!(
            "mutation CreateDoc($input: JSON!) {{ create_{collection}(input: $input) {{ id }} }}"
        );

        let mut batch_success = 0u64;
        for doc in &docs {
            let translated = translate_firestore_doc(doc);
            let mutation = serde_json::json!({
                "query": mutation_query,
                "variables": { "input": translated },
            });

            match client.post(&url).json(&mutation).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    if body
                        .get("errors")
                        .is_some_and(|e| e.is_array() && !e.as_array().unwrap().is_empty())
                    {
                        let err_msg = body["errors"][0]["message"]
                            .as_str()
                            .unwrap_or("unknown error");
                        eprintln!("  x GraphQL error: {err_msg}");
                        total_errors += 1;
                    } else {
                        batch_success += 1;
                        total_docs += 1;
                    }
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    let display_len = body.len().min(200);
                    eprintln!("  x Failed ({status}): {}", &body[..display_len]);
                    total_errors += 1;
                }
                Err(e) => {
                    eprintln!("  x Network error: {e}");
                    total_errors += 1;
                }
            }

            // Rate limit: 50ms between docs to avoid overwhelming the server
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        println!("    Migrated {batch_success}/{} documents", docs.len());
    }

    println!();
    if dry_run {
        println!("Dry run complete: {total_docs} documents would be migrated");
    } else {
        println!("Migration complete: {total_docs} succeeded, {total_errors} errors");
        if total_errors > 0 {
            eprintln!("  {total_errors} documents failed to migrate. Check errors above.");
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Translate Firestore-export typed values to plain JSON.
/// Firestore exports use `{"stringValue": "x"}`, `{"integerValue": "42"}`, etc.
fn translate_firestore_doc(doc: &serde_json::Value) -> serde_json::Value {
    match doc {
        serde_json::Value::Object(map) => {
            // Check for Firestore typed value wrappers
            if map.len() == 1 {
                if let Some(v) = map.get("stringValue") {
                    return v.clone();
                }
                if let Some(v) = map.get("integerValue") {
                    // Firestore stores integers as strings
                    if let Some(s) = v.as_str()
                        && let Ok(n) = s.parse::<i64>()
                    {
                        return serde_json::Value::Number(n.into());
                    }
                    return v.clone();
                }
                if let Some(v) = map.get("doubleValue") {
                    return v.clone();
                }
                if let Some(v) = map.get("booleanValue") {
                    return v.clone();
                }
                if let Some(v) = map.get("timestampValue") {
                    return v.clone();
                }
                if let Some(v) = map.get("nullValue") {
                    let _ = v;
                    return serde_json::Value::Null;
                }
                if let Some(v) = map.get("arrayValue") {
                    if let Some(values) = v.get("values")
                        && let Some(arr) = values.as_array()
                    {
                        return serde_json::Value::Array(
                            arr.iter().map(translate_firestore_doc).collect(),
                        );
                    }
                    return serde_json::Value::Array(vec![]);
                }
                if let Some(v) = map.get("mapValue") {
                    if let Some(fields) = v.get("fields") {
                        return translate_firestore_doc(fields);
                    }
                    return serde_json::json!({});
                }
                if let Some(v) = map.get("geoPointValue") {
                    return v.clone();
                }
                if let Some(v) = map.get("referenceValue") {
                    return v.clone();
                }
            }

            // Check for Firestore "fields" wrapper (top-level document format)
            if let Some(fields) = map.get("fields") {
                let mut result = serde_json::Map::new();
                if let Some(fields_map) = fields.as_object() {
                    for (k, v) in fields_map {
                        result.insert(k.clone(), translate_firestore_doc(v));
                    }
                }
                // Preserve document name/ID if present
                if let Some(name) = map.get("name")
                    && let Some(name_str) = name.as_str()
                {
                    // Extract doc ID from path like "projects/x/databases/y/documents/collection/DOC_ID"
                    if let Some(id) = name_str.rsplit('/').next() {
                        result.insert("id".to_string(), serde_json::Value::String(id.to_string()));
                    }
                }
                return serde_json::Value::Object(result);
            }

            // Regular object — recurse
            let mut result = serde_json::Map::new();
            for (k, v) in map {
                // Skip Firestore metadata fields
                if k == "__name__" || k == "createTime" || k == "updateTime" {
                    continue;
                }
                result.insert(k.clone(), translate_firestore_doc(v));
            }
            serde_json::Value::Object(result)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(translate_firestore_doc).collect())
        }
        other => other.clone(),
    }
}

// ── Dart Codegen ──

async fn codegen_dart(url: &str, output_dir: &str) -> Result<()> {
    println!("Introspecting GraphQL schema at {url}/graphql ...");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    let introspection_query = serde_json::json!({
        "query": r#"{
            __schema {
                types {
                    name
                    kind
                    fields {
                        name
                        type {
                            name
                            kind
                            ofType { name kind ofType { name kind } }
                        }
                    }
                }
                queryType { name }
                mutationType { name }
            }
        }"#
    });

    let resp = client
        .post(format!("{url}/graphql"))
        .json(&introspection_query)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        anyhow::bail!(
            "Introspection failed: HTTP {status}\n  \
             Check: Is the server running? Is --url correct?\n  \
             Try:   curl {url}/_admin/health"
        );
    }

    let body: serde_json::Value = resp.json().await?;
    let types = body["data"]["__schema"]["types"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("No types in introspection result"))?;

    // Create output directory
    std::fs::create_dir_all(output_dir)?;

    let mut models = String::new();
    models.push_str("// AUTO-GENERATED by `orignabase codegen dart`\n");
    models.push_str("// Do not edit manually.\n\n");
    models.push_str("import 'package:freezed_annotation/freezed_annotation.dart';\n");
    models.push_str("import 'package:json_annotation/json_annotation.dart';\n\n");
    models.push_str("part 'models.freezed.dart';\n");
    models.push_str("part 'models.g.dart';\n\n");

    let mut count = 0;
    for typ in types {
        let name = typ["name"].as_str().unwrap_or("");
        let kind = typ["kind"].as_str().unwrap_or("");

        // Skip built-in GraphQL types
        if name.starts_with("__")
            || name == "Boolean"
            || name == "String"
            || name == "Int"
            || name == "Float"
            || name == "ID"
            || name == "QueryRoot"
            || name == "MutationRoot"
        {
            continue;
        }

        if kind == "OBJECT" {
            if let Some(fields) = typ["fields"].as_array() {
                if fields.is_empty() {
                    continue;
                }

                models.push_str(&format!("@freezed\nclass {name} with _${name} {{\n"));
                models.push_str(&format!("  const factory {name}({{\n"));

                for field in fields {
                    let field_name = field["name"].as_str().unwrap_or("unknown");
                    let dart_type = graphql_type_to_dart(&field["type"]);
                    models.push_str(&format!("    {dart_type}? {field_name},\n"));
                }

                models.push_str(&format!("  }}) = _{name};\n\n"));
                models.push_str(&format!(
                    "  factory {name}.fromJson(Map<String, dynamic> json) => _${name}FromJson(json);\n"
                ));
                models.push_str("}\n\n");
                count += 1;
            }
        }
    }

    let output_path = format!("{output_dir}/models.dart");
    std::fs::write(&output_path, &models)?;

    println!("Generated {count} Dart models -> {output_path}");
    println!("   Run `dart run build_runner build` to generate Freezed/JSON serialization code.");

    Ok(())
}

fn graphql_type_to_dart(typ: &serde_json::Value) -> String {
    let kind = typ["kind"].as_str().unwrap_or("");
    let name = typ["name"].as_str().unwrap_or("");

    match kind {
        "SCALAR" => match name {
            "String" | "ID" => "String".to_string(),
            "Int" => "int".to_string(),
            "Float" => "double".to_string(),
            "Boolean" => "bool".to_string(),
            "DateTime" => "DateTime".to_string(),
            _ => "dynamic".to_string(),
        },
        "OBJECT" => name.to_string(),
        "LIST" => {
            let inner = graphql_type_to_dart(&typ["ofType"]);
            format!("List<{inner}>")
        }
        "NON_NULL" => graphql_type_to_dart(&typ["ofType"]),
        _ => "dynamic".to_string(),
    }
}

// --- Index definitions ---

#[derive(serde::Deserialize)]
struct IndexFile {
    indexes: Vec<IndexDef>,
}

#[derive(serde::Deserialize)]
struct IndexDef {
    collection: String,
    fields: Vec<String>,
    unique: Option<bool>,
}

async fn apply_indexes(db: &DatabaseClient) -> Result<()> {
    let index_file = std::path::Path::new("indexes.toml");
    if !index_file.exists() {
        println!("No indexes.toml found. Create one to define your indexes.");
        println!("Example:");
        println!(
            r#"[[indexes]]
collection = "products"
fields = ["status", "price"]
unique = false"#
        );
        return Ok(());
    }

    let content = std::fs::read_to_string(index_file)?;
    let parsed: IndexFile = toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Failed to parse indexes.toml: {e}"))?;

    println!("Applying {} indexes...", parsed.indexes.len());

    for idx in &parsed.indexes {
        ob_core::validate_identifier(&idx.collection)
            .map_err(|e| anyhow::anyhow!("Invalid collection name '{}': {e}", idx.collection))?;
        for field in &idx.fields {
            ob_core::validate_identifier(field)
                .map_err(|e| anyhow::anyhow!("Invalid field name '{field}': {e}"))?;
        }

        let name = format!("idx_{}_{}", idx.collection, idx.fields.join("_"));
        let unique = if idx.unique.unwrap_or(false) {
            " UNIQUE"
        } else {
            ""
        };
        let query = format!(
            "DEFINE INDEX {name} ON {} FIELDS {}{unique}",
            idx.collection,
            idx.fields.join(", ")
        );

        match db.query_raw_value(&query).await {
            Ok(_) => println!("  ✓ {name}"),
            Err(e) => println!("  ✗ {name}: {e}"),
        }
    }

    println!("Done.");
    Ok(())
}
