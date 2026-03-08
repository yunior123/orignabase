use anyhow::Result;
use async_graphql::http::GraphiQLSource;
use axum::Router;
use clap::{Parser, Subcommand};
use ob_admin::admin_router;
use ob_admin::routes::AdminState;
use ob_auth::routes::{AuthState, auth_router};
use ob_core::Config;
use ob_database::DatabaseClient;
use ob_graphql::build_schema;
use ob_realtime::ChangeDispatcher;
use ob_realtime::registry::SubscriptionRegistry;
use ob_realtime::websocket::realtime_router;
use ob_security::{RuleEngine, parse_rules};
use ob_storage::routes::{StorageState, storage_router};
use ob_storage::{LocalStorage, SignedUrlGenerator};
use std::collections::HashMap;
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
    tracing::info!("  Admin:     http://{addr}/_admin/health");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
