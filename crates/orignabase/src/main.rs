use anyhow::Result;
use async_graphql::http::GraphiQLSource;
use axum::Router;
use clap::{Parser, Subcommand};
use ob_admin::admin_router;
use ob_admin::routes::AdminState;
use ob_auth::routes::{AuthState, auth_router};
use ob_analytics::{AnalyticsState, analytics_router};
use ob_core::Config;
use ob_functions::{FunctionLimits, FunctionRegistry, WasmRuntime, functions_router};
use ob_functions::routes::FunctionsState;
use ob_database::DatabaseClient;
use ob_graphql::build_schema;
use ob_realtime::ChangeDispatcher;
use ob_realtime::registry::SubscriptionRegistry;
use ob_realtime::websocket::realtime_router;
use ob_security::{RuleEngine, parse_rules};
use ob_storage::routes::{StorageState, storage_router};
use ob_storage::{LocalStorage, SignedUrlGenerator};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

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
    /// Migrate data from another backend
    Migrate {
        #[command(subcommand)]
        source: MigrateSource,
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
        Commands::Serve => serve(config).await,
        Commands::Migrate { source } => match source {
            MigrateSource::FromFirebase {
                export_path,
                target_url,
                collections,
                dry_run,
            } => migrate_from_firebase(&export_path, &target_url, collections.as_deref(), dry_run).await,
        },
    }
}

async fn serve(config: Config) -> Result<()> {
    // --- Database ---
    let db = DatabaseClient::connect(&config.database).await?;

    // --- Security Rules ---
    let rules = if std::path::Path::new(&config.security.rules_path).exists() {
        let content = std::fs::read_to_string(&config.security.rules_path)?;
        parse_rules(&content)?
    } else {
        tracing::warn!(
            "No rules file at '{}', using empty rules (all access denied)",
            config.security.rules_path
        );
        HashMap::new()
    };
    let rule_engine = Arc::new(RuleEngine::new(rules));

    // --- GraphQL ---
    let schema = build_schema(db.clone(), rule_engine.clone());

    // --- Auth ---
    let auth_state = AuthState {
        db: db.clone(),
        jwt_secret: config.auth.jwt_secret.clone(),
        access_ttl: config.auth.access_token_ttl_secs,
        refresh_ttl: config.auth.refresh_token_ttl_secs,
    };

    // --- Realtime ---
    let registry = SubscriptionRegistry::new();
    let (dispatcher, _change_tx) = ChangeDispatcher::new(registry.clone());
    tokio::spawn(dispatcher.run());

    // --- Storage ---
    let storage = LocalStorage::new("./data/storage")?;
    let url_gen = SignedUrlGenerator::new(
        &config.auth.jwt_secret,
        &format!("http://{}:{}", config.host, config.port),
    );
    let storage_state = StorageState {
        storage,
        url_generator: url_gen,
    };

    // --- Functions (WASM) ---
    let wasm_runtime = Arc::new(WasmRuntime::new(FunctionLimits::default())?);
    let function_registry = Arc::new(FunctionRegistry::new(wasm_runtime));
    let functions_state = FunctionsState {
        registry: function_registry,
    };

    // --- Analytics ---
    let analytics_state = AnalyticsState {
        ip_salt: config.auth.jwt_secret.clone(), // Reuse secret as salt
    };

    // --- Admin ---
    let admin_state = AdminState { db: db.clone() };

    // --- Router Assembly ---
    let addr = format!("{}:{}", config.host, config.port);

    let app = Router::new()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .route(
            "/graphql",
            axum::routing::get(|| async {
                axum::response::Html(GraphiQLSource::build().endpoint("/graphql").finish())
            })
            .post_service(async_graphql_axum::GraphQL::new(schema.clone())),
        )
        .merge(auth_router(auth_state))
        .merge(realtime_router(registry))
        .merge(storage_router(storage_state))
        .merge(functions_router(functions_state))
        .merge(analytics_router(analytics_state))
        .merge(admin_router(admin_state))
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        );

    tracing::info!("OrignaBase listening on {addr}");
    tracing::info!("  GraphiQL:  http://{addr}/graphql");
    tracing::info!("  Realtime:  ws://{addr}/realtime");
    tracing::info!("  Functions: http://{addr}/functions");
    tracing::info!("  Analytics: http://{addr}/analytics/event");
    tracing::info!("  Admin:     http://{addr}/_admin/health");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

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

    let collection_filter: Option<Vec<&str>> = collections.map(|c| c.split(',').map(str::trim).collect());

    println!("🔥 OrignaBase Firebase Migration Tool");
    println!("   Source:      {export_path}");
    println!("   Target:      {target_url}");
    println!("   Collections: {}", collections.unwrap_or("all"));
    println!("   Dry run:     {dry_run}");
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
            && !filter.contains(&collection_name.as_str()) {
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
            let mut sub_entries: Vec<_> = std::fs::read_dir(&path)?
                .filter_map(|e| e.ok())
                .collect();
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

        println!("📦 {collection_name}: {} documents", docs.len());

        if dry_run {
            // Show first 3 doc IDs as preview
            for (i, doc) in docs.iter().take(3).enumerate() {
                let id = doc.get("id")
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
        for doc in &docs {
            let translated = translate_firestore_doc(doc);
            let url = format!("{target_url}/graphql");
            let collection = &collection_name;

            // Build a simple GraphQL mutation
            let fields_json = serde_json::to_string(&translated)?;
            let mutation = serde_json::json!({
                "query": format!(
                    "mutation {{ create_{collection}(input: {fields_json}) {{ id }} }}"
                ),
            });

            match client.post(&url).json(&mutation).send().await {
                Ok(resp) if resp.status().is_success() => {
                    total_docs += 1;
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    eprintln!("  ✗ Failed ({status}): {}", &body[..body.len().min(200)]);
                    total_errors += 1;
                }
                Err(e) => {
                    eprintln!("  ✗ Network error: {e}");
                    total_errors += 1;
                }
            }
        }

        println!("   ✓ Migrated {} documents", docs.len());
    }

    println!();
    if dry_run {
        println!("🔍 Dry run complete: {total_docs} documents would be migrated");
    } else {
        println!("✅ Migration complete: {total_docs} succeeded, {total_errors} errors");
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
                        && let Ok(n) = s.parse::<i64>() {
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
                        && let Some(arr) = values.as_array() {
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
                    && let Some(name_str) = name.as_str() {
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
