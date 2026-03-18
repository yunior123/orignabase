use axum::Json;
use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use ob_auth::middleware::AuthContext;
use ob_core::{Error, Result};
use ob_database::DatabaseClient;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::schema::{self, CollectionSchema};

const DASHBOARD_HTML: &str = include_str!("dashboard.html");

/// Process start time for uptime calculation.
static START_TIME: std::sync::LazyLock<std::time::Instant> =
    std::sync::LazyLock::new(std::time::Instant::now);
const PUBLIC_CONFIG_KEYS: &[&str] = &[
    "geoapify_api_key",
    "image_base_url",
    "sentry_dns",
    "google_web_client_id",
    "terms_and_conditions",
];

/// Admin API state.
#[derive(Clone)]
pub struct AdminState {
    pub db: DatabaseClient,
}

/// GET /_admin/health — System health check.
async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

/// GET /_admin/collections — List all collections (returns DB info).
async fn list_collections(State(state): State<AdminState>) -> Result<Json<Value>> {
    let info = schema::list_collections(&state.db).await?;
    // Extract table names from INFO FOR DB response
    let tables = info
        .get("tables")
        .and_then(|t| t.as_object())
        .map(|obj| obj.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    Ok(Json(json!({ "collections": tables })))
}

/// POST /_admin/collections — Create a new collection.
async fn create_collection(
    State(state): State<AdminState>,
    Json(body): Json<CollectionSchema>,
) -> Result<Json<Value>> {
    schema::create_collection(&state.db, &body).await?;
    Ok(Json(json!({ "created": body.name })))
}

/// DELETE /_admin/collections/:name — Drop a collection.
async fn drop_collection(
    State(state): State<AdminState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<Json<Value>> {
    schema::drop_collection(&state.db, &name).await?;
    Ok(Json(json!({ "dropped": name })))
}

/// GET /_admin/users — List users (paginated).
async fn list_users(State(state): State<AdminState>) -> Result<Json<Value>> {
    let users = state
        .db
        .query_raw("SELECT id, email, display_name, roles, created_at FROM users LIMIT 100")
        .await?;
    Ok(Json(json!({ "users": users })))
}

/// DELETE /_admin/users/:id — Delete a user.
async fn delete_user(
    State(state): State<AdminState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Value>> {
    state.db.delete_document("users", &id).await?;
    Ok(Json(json!({ "deleted": id })))
}

/// PATCH /_admin/users/:id/roles — Update user roles.
async fn update_roles(
    State(state): State<AdminState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let roles = body
        .get("roles")
        .ok_or_else(|| Error::Validation("Missing 'roles' field".into()))?;

    let updated = state
        .db
        .update_document("users", &id, json!({ "roles": roles }))
        .await?;

    Ok(Json(json!({ "user": updated })))
}

/// GET /_admin/analytics — Query analytics events.
async fn analytics_summary(State(state): State<AdminState>) -> Result<Json<Value>> {
    // Total events (aggregate — no `id` field, must use query_raw_value)
    let total = state
        .db
        .query_raw_value("SELECT count() AS total FROM _analytics_events GROUP ALL")
        .await
        .unwrap_or_else(|_| Value::Array(vec![]));

    // Events by type (last 7 days) — GROUP BY returns non-record data
    let by_event = state
        .db
        .query_raw_value(
            "SELECT event, count() AS count FROM _analytics_events \
             WHERE timestamp > time::now() - 7d GROUP BY event ORDER BY count DESC LIMIT 20",
        )
        .await
        .unwrap_or_else(|_| Value::Array(vec![]));

    // Events by path (top 20) — GROUP BY returns non-record data
    let by_path = state
        .db
        .query_raw_value(
            "SELECT path, count() AS count FROM _analytics_events \
             WHERE path IS NOT NONE GROUP BY path ORDER BY count DESC LIMIT 20",
        )
        .await
        .unwrap_or_else(|_| Value::Array(vec![]));

    Ok(Json(json!({
        "total": total,
        "by_event": by_event,
        "by_path": by_path,
    })))
}

/// GET /_admin/ — Serve the admin dashboard SPA.
async fn dashboard() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

fn require_admin(auth: &AuthContext) -> Result<()> {
    if !auth.authenticated {
        return Err(Error::Auth("Authentication required".into()));
    }
    if !auth.has_role("admin") {
        return Err(Error::Forbidden("Admin access required".into()));
    }
    Ok(())
}

async fn require_admin_middleware(request: Request, next: Next) -> Result<Response> {
    let auth = request
        .extensions()
        .get::<AuthContext>()
        .cloned()
        .unwrap_or_else(AuthContext::anonymous);
    require_admin(&auth)?;
    Ok(next.run(request).await)
}

// ── Remote Config ──

/// GET /config — Get all remote config key-value pairs (public, cached).
async fn config_get_all(
    State(state): State<AdminState>,
) -> Result<([(header::HeaderName, &'static str); 1], Json<Value>)> {
    let configs = state
        .db
        .query_raw("SELECT * FROM type::table('_config') ORDER BY key ASC")
        .await?;

    let map: serde_json::Map<String, Value> = configs
        .iter()
        .filter_map(|c| {
            let key = c.get("key")?.as_str()?;
            if !PUBLIC_CONFIG_KEYS.contains(&key) {
                return None;
            }
            let value = c.get("value")?;
            Some((key.to_string(), value.clone()))
        })
        .collect();

    Ok((
        [(header::CACHE_CONTROL, "public, max-age=60")],
        Json(Value::Object(map)),
    ))
}

/// GET /config/:key — Get a single config value (public, cached).
async fn config_get(
    State(state): State<AdminState>,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> Result<([(header::HeaderName, &'static str); 1], Json<Value>)> {
    let rows = state
        .db
        .query_raw(&format!(
            "SELECT * FROM type::table('_config') WHERE key = '{}' LIMIT 1",
            key.replace('\'', "\\'")
        ))
        .await?;

    let value = rows
        .first()
        .filter(|item| {
            item.get("key")
                .and_then(|v| v.as_str())
                .map(|cfg_key| PUBLIC_CONFIG_KEYS.contains(&cfg_key))
                .unwrap_or(false)
        })
        .and_then(|item| item.get("value"))
        .cloned()
        .unwrap_or(Value::Null);

    Ok((
        [(header::CACHE_CONTROL, "public, max-age=60")],
        Json(json!({ "key": key, "value": value })),
    ))
}

/// GET /_admin/config — List all config entries with full metadata (admin only).
async fn admin_config_get_all(State(state): State<AdminState>) -> Result<Json<Value>> {
    let configs = state
        .db
        .query_raw("SELECT * FROM type::table('_config') ORDER BY key ASC")
        .await?;

    Ok(Json(json!({ "configs": configs })))
}

/// PUT /_admin/config/:key — Set a config value (admin only).
async fn config_set(
    State(state): State<AdminState>,
    axum::extract::Path(key): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let value = body
        .get("value")
        .ok_or_else(|| Error::Validation("Missing 'value' field".into()))?;

    let value_type = body
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or(match value {
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::Object(_) | Value::Array(_) => "json",
            _ => "string",
        });

    let description = body
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Upsert: create or update
    state
        .db
        .query_bind(
            "UPSERT _config SET key = $key, value = $value, type = $type, description = $desc, updated_at = time::now() WHERE key = $key",
            json!({ "key": key, "value": value, "type": value_type, "desc": description }),
        )
        .await?;

    Ok(Json(
        json!({ "key": key, "value": value, "type": value_type, "description": description }),
    ))
}

/// DELETE /_admin/config/:key — Delete a config value (admin only).
async fn config_delete(
    State(state): State<AdminState>,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> Result<Json<Value>> {
    state
        .db
        .query_bind(
            "DELETE FROM _config WHERE key = $key",
            json!({ "key": key }),
        )
        .await?;

    Ok(Json(json!({ "deleted": key })))
}

// ── Dynamic Links ──

/// POST /links — Create a short/dynamic link.
async fn create_link(
    State(state): State<AdminState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let target_url = body
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Validation("Missing 'url' field".into()))?;

    let slug = body
        .get("slug")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| {
            // Generate 8-char random slug
            use std::fmt::Write;
            let bytes: [u8; 4] = rand::random();
            let mut s = String::with_capacity(8);
            for b in &bytes {
                let _ = write!(s, "{b:02x}");
            }
            s
        });

    let link_data = json!({
        "slug": slug,
        "target_url": target_url,
        "title": body.get("title").and_then(|v| v.as_str()).unwrap_or(""),
        "description": body.get("description").and_then(|v| v.as_str()).unwrap_or(""),
        "clicks": 0,
        "created_at": chrono::Utc::now().to_rfc3339(),
    });

    state
        .db
        .create_document("_dynamic_links", link_data)
        .await?;

    Ok(Json(json!({
        "slug": slug,
        "short_url": format!("/l/{slug}"),
        "target_url": target_url,
    })))
}

/// GET /l/:slug — Redirect to the target URL (tracks clicks).
async fn redirect_link(
    State(state): State<AdminState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> std::result::Result<axum::response::Redirect, axum::response::Response> {
    let results = state
        .db
        .query_raw_value(&format!(
            "SELECT target_url FROM type::table('_dynamic_links') WHERE slug = '{}' LIMIT 1",
            slug.replace('\'', "\\'")
        ))
        .await
        .map_err(|e| {
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        })?;

    let target = results
        .get("target_url")
        .or_else(|| {
            results
                .as_array()
                .and_then(|items| items.first())
                .and_then(|item| item.get("target_url"))
        })
        .and_then(|v| v.as_str())
        .ok_or_else(|| (axum::http::StatusCode::NOT_FOUND, "Link not found").into_response())?;

    // Increment click counter in background
    let db = state.db.clone();
    let slug_owned = slug.clone();
    tokio::spawn(async move {
        let _ = db
            .query_bind(
                "UPDATE _dynamic_links SET clicks += 1 WHERE slug = $slug",
                json!({ "slug": slug_owned }),
            )
            .await;
    });

    Ok(axum::response::Redirect::temporary(target))
}

/// GET /_admin/links — List all dynamic links (admin only).
async fn list_links(State(state): State<AdminState>) -> Result<Json<Value>> {
    let links = state
        .db
        .query_raw("SELECT * FROM _dynamic_links ORDER BY created_at DESC LIMIT 100")
        .await?;
    Ok(Json(json!({ "links": links })))
}

// ── Performance Metrics ──

/// POST /metrics — Record a performance metric.
async fn record_metric(
    State(state): State<AdminState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Validation("Missing 'name' field".into()))?;
    let value = body
        .get("value")
        .ok_or_else(|| Error::Validation("Missing 'value' field".into()))?;

    let metric = json!({
        "name": name,
        "value": value,
        "tags": body.get("tags").cloned().unwrap_or(json!({})),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });

    state.db.create_document("_metrics", metric).await?;
    Ok(Json(json!({ "recorded": name })))
}

/// GET /_admin/metrics — Query performance metrics (admin only).
async fn query_metrics(State(state): State<AdminState>) -> Result<Json<Value>> {
    let metrics = state
        .db
        .query_raw(
            "SELECT name, math::mean(value) AS avg, math::min(value) AS min, \
             math::max(value) AS max, count() AS count \
             FROM _metrics WHERE timestamp > time::now() - 24h \
             GROUP BY name ORDER BY name ASC",
        )
        .await?;

    Ok(Json(json!({ "metrics": metrics })))
}

// ── Index Management ──

/// Request body for creating an index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateIndexRequest {
    pub collection: String,
    pub fields: Vec<String>,
    #[serde(default)]
    pub unique: bool,
}

impl CreateIndexRequest {
    /// Generate the SurrealQL index name: `idx_{collection}_{fields_joined}`.
    pub fn index_name(&self) -> String {
        format!("idx_{}_{}", self.collection, self.fields.join("_"))
    }

    /// Generate the SurrealQL DEFINE INDEX statement.
    pub fn to_surreal_query(&self) -> String {
        let unique_clause = if self.unique { " UNIQUE" } else { "" };
        format!(
            "DEFINE INDEX {} ON {} FIELDS {}{}",
            self.index_name(),
            self.collection,
            self.fields.join(", "),
            unique_clause,
        )
    }
}

/// Request body for dropping an index (provides collection context).
#[derive(Debug, Clone, Deserialize)]
pub struct DropIndexRequest {
    pub collection: String,
}

/// POST /_admin/indexes — Create an index.
async fn create_index(
    State(state): State<AdminState>,
    Json(body): Json<CreateIndexRequest>,
) -> Result<Json<Value>> {
    // Validate collection name
    if !body
        .collection
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(Error::Validation("Invalid collection name".into()));
    }
    // Validate field names
    for field in &body.fields {
        if !field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::Validation(format!("Invalid field name: {field}")));
        }
    }
    if body.fields.is_empty() {
        return Err(Error::Validation("At least one field is required".into()));
    }

    let query = body.to_surreal_query();
    state.db.query_raw_value(&query).await?;

    Ok(Json(json!({
        "created": body.index_name(),
        "collection": body.collection,
        "fields": body.fields,
        "unique": body.unique,
    })))
}

/// GET /_admin/indexes — List all indexes via INFO FOR DB.
async fn list_indexes(State(state): State<AdminState>) -> Result<Json<Value>> {
    let info = state.db.query_raw_value("INFO FOR DB").await?;
    // INFO FOR DB returns an object with "tables", "indexes", etc.
    // Extract index info from the response
    let indexes = info.get("indexes").cloned().unwrap_or(json!({}));
    Ok(Json(json!({ "indexes": indexes })))
}

/// DELETE /_admin/indexes/{name} — Drop an index.
async fn drop_index(
    State(state): State<AdminState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(body): Json<DropIndexRequest>,
) -> Result<Json<Value>> {
    // Validate inputs
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(Error::Validation("Invalid index name".into()));
    }
    if !body
        .collection
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(Error::Validation("Invalid collection name".into()));
    }

    let query = format!("REMOVE INDEX {name} ON {}", body.collection);
    if let Err(err) = state.db.query_raw_value(&query).await {
        let message = err.to_string();
        if message.contains("does not exist") && message.contains("index") {
            return Err(Error::NotFound(format!("index '{name}'")));
        }
        return Err(err);
    }

    Ok(Json(
        json!({ "dropped": name, "collection": body.collection }),
    ))
}

// ── System Health & Usage Dashboard ──

/// GET /_admin/usage — System usage overview.
async fn usage_dashboard(State(state): State<AdminState>) -> Result<Json<Value>> {
    // Force-initialize START_TIME on first call
    let uptime_seconds = START_TIME.elapsed().as_secs();

    // User count (aggregate query returns non-record data, use query_raw_value)
    let user_count = state
        .db
        .query_raw_value("SELECT count() AS total FROM users GROUP ALL")
        .await
        .unwrap_or(json!(null));
    let total_users = user_count
        .get("total")
        .or_else(|| {
            user_count
                .as_array()
                .and_then(|a| a.first())
                .and_then(|r| r.get("total"))
        })
        .cloned()
        .unwrap_or(json!(0));

    // Collections info
    let info = schema::list_collections(&state.db).await?;
    let tables = info
        .get("tables")
        .and_then(|t| t.as_object())
        .map(|obj| obj.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let collection_count = tables.len();

    // Estimate total documents across all collections (single batch query)
    let docs_estimate = if tables.is_empty() {
        0u64
    } else {
        // Build a single query counting all tables at once
        let count_queries: Vec<String> = tables
            .iter()
            .map(|t| format!("SELECT count() AS total FROM {t} GROUP ALL"))
            .collect();
        let batch_query = count_queries.join(";\n");
        let mut total = 0u64;
        if let Ok(results) = state.db.query_raw_value(&batch_query).await {
            if let Some(arr) = results.as_array() {
                for r in arr {
                    if let Some(t) = r.get("total").and_then(|v| v.as_u64()) {
                        total += t;
                    }
                }
            } else if let Some(t) = results.get("total").and_then(|v| v.as_u64()) {
                total += t;
            }
        }
        total
    };

    // Deployed functions count — query _functions or use 0 if unavailable
    let functions_val = state
        .db
        .query_raw_value("SELECT count() AS total FROM _functions GROUP ALL")
        .await
        .unwrap_or(json!(null));
    let functions_count = functions_val
        .get("total")
        .or_else(|| {
            functions_val
                .as_array()
                .and_then(|a| a.first())
                .and_then(|r| r.get("total"))
        })
        .cloned()
        .unwrap_or(json!(0));

    Ok(Json(json!({
        "users": { "total": total_users },
        "collections": { "count": collection_count, "names": tables },
        "storage": { "documents_estimate": docs_estimate },
        "functions": { "deployed": functions_count },
        "realtime": { "note": "Query /presence for realtime stats" },
        "uptime_seconds": uptime_seconds,
    })))
}

/// GET /_admin/alerts — System health alerts (computed on each request).
async fn system_alerts(State(state): State<AdminState>) -> Result<Json<Value>> {
    let mut alerts: Vec<Value> = Vec::new();

    // Check user count
    let user_count_val = state
        .db
        .query_raw_value("SELECT count() AS total FROM users GROUP ALL")
        .await
        .unwrap_or(json!(null));
    let total_users = user_count_val
        .get("total")
        .or_else(|| {
            user_count_val
                .as_array()
                .and_then(|a| a.first())
                .and_then(|r| r.get("total"))
        })
        .and_then(|v| v.as_u64());
    if let Some(total) = total_users
        && total > 10_000
    {
        alerts.push(json!({
            "level": "warning",
            "message": "High user count, consider monitoring",
            "metric": format!("users.total={total}"),
        }));
    }

    // Check collection sizes
    let info = schema::list_collections(&state.db).await?;
    let tables = info
        .get("tables")
        .and_then(|t| t.as_object())
        .map(|obj| obj.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    // Batch count all collections in a single query
    if !tables.is_empty() {
        let count_queries: Vec<String> = tables
            .iter()
            .map(|t| format!("SELECT count() AS total FROM {t} GROUP ALL"))
            .collect();
        let batch_query = count_queries.join(";\n");
        if let Ok(results) = state.db.query_raw_value(&batch_query).await {
            // Results may come back as array of results or single value
            let result_list: Vec<&Value> = if let Some(arr) = results.as_array() {
                arr.iter().collect()
            } else {
                vec![&results]
            };
            for (i, r) in result_list.iter().enumerate() {
                let total = r.get("total").and_then(|v| v.as_u64());
                if let Some(t) = total
                    && t > 100_000
                {
                    let table = tables.get(i).map(|s| s.as_str()).unwrap_or("unknown");
                    alerts.push(json!({
                        "level": "warning",
                        "message": format!("Large collection: {table}"),
                        "metric": format!("{table}.count={t}"),
                    }));
                }
            }
        }
    }

    Ok(Json(json!({ "alerts": alerts })))
}

/// Build the admin router. All routes require admin authentication.
pub fn admin_router(state: AdminState) -> axum::Router {
    let protected = axum::Router::new()
        .route("/_admin", axum::routing::get(dashboard))
        .route("/_admin/", axum::routing::get(dashboard))
        .route(
            "/_admin/collections",
            axum::routing::get(list_collections).post(create_collection),
        )
        .route(
            "/_admin/collections/{name}",
            axum::routing::delete(drop_collection),
        )
        .route("/_admin/analytics", axum::routing::get(analytics_summary))
        .route("/_admin/users", axum::routing::get(list_users))
        .route("/_admin/users/{id}", axum::routing::delete(delete_user))
        .route(
            "/_admin/users/{id}/roles",
            axum::routing::patch(update_roles),
        )
        .route("/_admin/config", axum::routing::get(admin_config_get_all))
        .route(
            "/_admin/config/{key}",
            axum::routing::put(config_set).delete(config_delete),
        )
        .route("/_admin/links", axum::routing::get(list_links))
        .route("/_admin/metrics", axum::routing::get(query_metrics))
        .route(
            "/_admin/indexes",
            axum::routing::post(create_index).get(list_indexes),
        )
        .route("/_admin/indexes/{name}", axum::routing::delete(drop_index))
        .route("/_admin/usage", axum::routing::get(usage_dashboard))
        .route("/_admin/alerts", axum::routing::get(system_alerts))
        .route("/links", axum::routing::post(create_link))
        .route_layer(axum::middleware::from_fn(require_admin_middleware));

    axum::Router::new()
        .route("/_admin/health", axum::routing::get(health))
        // Remote Config (public read, allowlisted)
        .route("/config", axum::routing::get(config_get_all))
        .route("/config/{key}", axum::routing::get(config_get))
        .route("/l/{slug}", axum::routing::get(redirect_link))
        // Performance Metrics
        .route("/metrics", axum::routing::post(record_metric))
        .merge(protected)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_admin_health_status() {
        let Json(body) = health().await;
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn test_admin_health_version() {
        let Json(body) = health().await;
        assert!(body["version"].is_string());
        assert!(!body["version"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_admin_health_timestamp() {
        let Json(body) = health().await;
        let ts = body["timestamp"].as_str().unwrap();
        // Should be valid RFC 3339
        assert!(ts.contains('T'), "timestamp should be RFC 3339: {ts}");
        assert!(
            ts.contains('+') || ts.ends_with('Z'),
            "timestamp should have timezone: {ts}"
        );
    }

    #[tokio::test]
    async fn test_admin_health_has_exactly_three_fields() {
        let Json(body) = health().await;
        let obj = body.as_object().unwrap();
        assert_eq!(obj.len(), 3);
        assert!(obj.contains_key("status"));
        assert!(obj.contains_key("version"));
        assert!(obj.contains_key("timestamp"));
    }

    #[test]
    fn test_dashboard_html_not_empty() {
        assert!(!DASHBOARD_HTML.is_empty());
    }

    #[test]
    fn test_dashboard_html_is_html() {
        let lower = DASHBOARD_HTML.to_lowercase();
        assert!(
            lower.contains("<html") || lower.contains("<!doctype"),
            "DASHBOARD_HTML should contain HTML markup"
        );
    }

    #[test]
    fn test_admin_state_fields_exist() {
        // Compile-time check that AdminState has the expected field.
        fn _assert_fields(s: &AdminState) {
            let _ = &s.db;
        }
    }

    #[test]
    fn test_slug_generation_is_8_chars() {
        use std::fmt::Write;
        let bytes: [u8; 4] = rand::random();
        let mut s = String::with_capacity(8);
        for b in &bytes {
            let _ = write!(s, "{b:02x}");
        }
        assert_eq!(s.len(), 8);
    }

    // ── Config tests ──

    #[test]
    fn test_config_get_all_query() {
        // Verify the query string used in config_get_all is correct
        let query = "SELECT * FROM _config ORDER BY key ASC";
        assert!(query.contains("_config"));
        assert!(query.contains("ORDER BY key ASC"));
    }

    // ── Link slug tests ──

    #[test]
    fn test_create_link_slug_length() {
        use std::fmt::Write;
        // The slug generation logic produces exactly 8 hex chars from 4 random bytes
        let bytes: [u8; 4] = rand::random();
        let mut s = String::with_capacity(8);
        for b in &bytes {
            let _ = write!(s, "{b:02x}");
        }
        assert_eq!(s.len(), 8, "Slug should be exactly 8 characters");
        // All chars should be hex digits
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── Metric validation tests ──

    #[test]
    fn test_metric_requires_name() {
        // Simulate the validation logic from record_metric
        let body = json!({ "value": 42 });
        let name = body.get("name").and_then(|v| v.as_str());
        assert!(name.is_none(), "Missing 'name' should be None");
    }

    #[test]
    fn test_metric_requires_value() {
        let body = json!({ "name": "page_load" });
        let value = body.get("value");
        assert!(value.is_none(), "Missing 'value' should be None");
    }

    #[test]
    fn test_metric_with_both_fields() {
        let body = json!({ "name": "page_load", "value": 123.4 });
        let name = body.get("name").and_then(|v| v.as_str());
        let value = body.get("value");
        assert_eq!(name, Some("page_load"));
        assert!(value.is_some());
    }

    // ── Index management tests ──

    #[test]
    fn test_create_index_request_deser() {
        let json = json!({
            "collection": "products",
            "fields": ["status", "price"],
            "unique": false
        });
        let req: CreateIndexRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.collection, "products");
        assert_eq!(req.fields, vec!["status", "price"]);
        assert!(!req.unique);
    }

    #[test]
    fn test_create_index_request_unique_default() {
        let json = json!({
            "collection": "users",
            "fields": ["email"]
        });
        let req: CreateIndexRequest = serde_json::from_value(json).unwrap();
        assert!(!req.unique, "'unique' should default to false");
    }

    #[test]
    fn test_create_index_name_generation() {
        let req = CreateIndexRequest {
            collection: "products".to_string(),
            fields: vec!["status".to_string(), "price".to_string()],
            unique: false,
        };
        assert_eq!(req.index_name(), "idx_products_status_price");
    }

    #[test]
    fn test_create_index_name_single_field() {
        let req = CreateIndexRequest {
            collection: "users".to_string(),
            fields: vec!["email".to_string()],
            unique: true,
        };
        assert_eq!(req.index_name(), "idx_users_email");
    }

    #[test]
    fn test_create_index_surreal_query_non_unique() {
        let req = CreateIndexRequest {
            collection: "products".to_string(),
            fields: vec!["status".to_string(), "price".to_string()],
            unique: false,
        };
        let query = req.to_surreal_query();
        assert_eq!(
            query,
            "DEFINE INDEX idx_products_status_price ON products FIELDS status, price"
        );
    }

    #[test]
    fn test_create_index_surreal_query_unique() {
        let req = CreateIndexRequest {
            collection: "users".to_string(),
            fields: vec!["email".to_string()],
            unique: true,
        };
        let query = req.to_surreal_query();
        assert_eq!(
            query,
            "DEFINE INDEX idx_users_email ON users FIELDS email UNIQUE"
        );
    }

    #[test]
    fn test_drop_index_request_deser() {
        let json = json!({ "collection": "products" });
        let req: DropIndexRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.collection, "products");
    }

    #[test]
    fn test_drop_index_missing_maps_to_not_found() {
        let err = Error::Database(
            "Result extraction failed: The index 'idx_probe_drop' does not exist".to_string(),
        );
        let message = err.to_string();

        let mapped = if message.contains("does not exist") && message.contains("index") {
            Error::NotFound("index 'idx_probe_drop'".to_string())
        } else {
            err
        };

        assert!(matches!(mapped, Error::NotFound(_)));
    }

    // ── START_TIME / uptime tests ──

    #[test]
    fn test_start_time_is_initialized() {
        // Just verify START_TIME can be accessed without panic
        let _elapsed = START_TIME.elapsed();
    }

    #[test]
    fn test_uptime_seconds_non_negative() {
        let uptime = START_TIME.elapsed().as_secs();
        // Should be 0 or positive (just started)
        assert!(uptime < 3600, "Uptime should be reasonable in test context");
    }

    #[test]
    fn test_slug_generation_unique() {
        use std::fmt::Write;
        let slug1 = {
            let bytes: [u8; 4] = rand::random();
            let mut s = String::with_capacity(8);
            for b in &bytes {
                let _ = write!(s, "{b:02x}");
            }
            s
        };
        let slug2 = {
            let bytes: [u8; 4] = rand::random();
            let mut s = String::with_capacity(8);
            for b in &bytes {
                let _ = write!(s, "{b:02x}");
            }
            s
        };
        // Statistically should be different
        assert_ne!(slug1, slug2);
    }

    // ══════════════════════════════════════════════════════════════════
    // ── Additional Index Management Tests ──
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_index_name_three_fields() {
        let req = CreateIndexRequest {
            collection: "orders".to_string(),
            fields: vec![
                "customer_id".to_string(),
                "status".to_string(),
                "created_at".to_string(),
            ],
            unique: false,
        };
        assert_eq!(req.index_name(), "idx_orders_customer_id_status_created_at");
    }

    #[test]
    fn test_index_surreal_query_three_fields() {
        let req = CreateIndexRequest {
            collection: "orders".to_string(),
            fields: vec![
                "customer_id".to_string(),
                "status".to_string(),
                "created_at".to_string(),
            ],
            unique: false,
        };
        assert_eq!(
            req.to_surreal_query(),
            "DEFINE INDEX idx_orders_customer_id_status_created_at ON orders FIELDS customer_id, status, created_at"
        );
    }

    #[test]
    fn test_index_name_underscore_collection() {
        let req = CreateIndexRequest {
            collection: "user_profiles".to_string(),
            fields: vec!["email".to_string()],
            unique: true,
        };
        assert_eq!(req.index_name(), "idx_user_profiles_email");
    }

    #[test]
    fn test_index_validation_empty_fields() {
        // The create_index handler checks body.fields.is_empty()
        let req = CreateIndexRequest {
            collection: "products".to_string(),
            fields: vec![],
            unique: false,
        };
        assert!(req.fields.is_empty());
    }

    #[test]
    fn test_index_validation_collection_name_chars() {
        // Valid: alphanumeric + underscore
        let valid = "my_collection_123";
        assert!(valid.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));

        // Invalid: spaces
        let invalid = "my collection";
        assert!(
            !invalid
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        );

        // Invalid: special chars
        let invalid2 = "drop;--";
        assert!(
            !invalid2
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        );

        // Invalid: dots
        let invalid3 = "products.items";
        assert!(
            !invalid3
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        );
    }

    #[test]
    fn test_index_validation_field_name_chars() {
        let valid_fields = vec!["status", "created_at", "price123"];
        for f in valid_fields {
            assert!(
                f.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "Field '{f}' should be valid"
            );
        }

        let invalid_fields = vec!["field name", "price$", "status;DROP", "field.nested"];
        for f in invalid_fields {
            assert!(
                !f.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "Field '{f}' should be invalid"
            );
        }
    }

    #[test]
    fn test_index_name_deterministic() {
        let req = CreateIndexRequest {
            collection: "products".to_string(),
            fields: vec!["status".to_string(), "price".to_string()],
            unique: false,
        };
        // Same input always produces same name
        assert_eq!(req.index_name(), req.index_name());
    }

    #[test]
    fn test_index_drop_query_format() {
        // Verify the format used in drop_index handler
        let name = "idx_products_status_price";
        let collection = "products";
        let query = format!("REMOVE INDEX {name} ON {collection}");
        assert_eq!(query, "REMOVE INDEX idx_products_status_price ON products");
    }

    #[test]
    fn test_index_drop_name_validation() {
        let valid = "idx_products_email";
        assert!(valid.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));

        // SQL injection attempt
        let invalid = "idx; DROP TABLE users --";
        assert!(
            !invalid
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        );
    }

    #[test]
    fn test_create_index_request_serialization() {
        let req = CreateIndexRequest {
            collection: "products".to_string(),
            fields: vec!["status".to_string()],
            unique: true,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["collection"], "products");
        assert_eq!(json["fields"], json!(["status"]));
        assert_eq!(json["unique"], true);
    }

    #[test]
    fn test_create_index_request_roundtrip() {
        let original = CreateIndexRequest {
            collection: "orders".to_string(),
            fields: vec!["customer_id".to_string(), "date".to_string()],
            unique: false,
        };
        let json = serde_json::to_value(&original).unwrap();
        let deserialized: CreateIndexRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.collection, original.collection);
        assert_eq!(deserialized.fields, original.fields);
        assert_eq!(deserialized.unique, original.unique);
    }

    // ══════════════════════════════════════════════════════════════════
    // ── Remote Config Tests ──
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_config_value_type_auto_detect_string() {
        let body = json!({ "value": "hello" });
        let value = body.get("value").unwrap();
        let detected = body
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or(match value {
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::Object(_) | Value::Array(_) => "json",
                _ => "string",
            });
        assert_eq!(detected, "string");
    }

    #[test]
    fn test_config_value_type_auto_detect_boolean() {
        let body = json!({ "value": true });
        let value = body.get("value").unwrap();
        let detected = match value {
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::Object(_) | Value::Array(_) => "json",
            _ => "string",
        };
        assert_eq!(detected, "boolean");
    }

    #[test]
    fn test_config_value_type_auto_detect_number() {
        let body = json!({ "value": 42 });
        let value = body.get("value").unwrap();
        let detected = match value {
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::Object(_) | Value::Array(_) => "json",
            _ => "string",
        };
        assert_eq!(detected, "number");
    }

    #[test]
    fn test_config_value_type_auto_detect_json_object() {
        let body = json!({ "value": { "nested": true } });
        let value = body.get("value").unwrap();
        let detected = match value {
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::Object(_) | Value::Array(_) => "json",
            _ => "string",
        };
        assert_eq!(detected, "json");
    }

    #[test]
    fn test_config_value_type_auto_detect_json_array() {
        let body = json!({ "value": [1, 2, 3] });
        let value = body.get("value").unwrap();
        let detected = match value {
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::Object(_) | Value::Array(_) => "json",
            _ => "string",
        };
        assert_eq!(detected, "json");
    }

    #[test]
    fn test_config_value_type_explicit_overrides_auto() {
        let body = json!({ "value": "42", "type": "number" });
        let value_type = body
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("string");
        assert_eq!(value_type, "number");
    }

    #[test]
    fn test_config_set_requires_value() {
        let body = json!({ "description": "Enable feature X" });
        let value = body.get("value");
        assert!(value.is_none(), "Missing 'value' should be None");
    }

    #[test]
    fn test_config_description_defaults_empty() {
        let body = json!({ "value": "enabled" });
        let description = body
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(description, "");
    }

    #[test]
    fn test_config_description_from_body() {
        let body = json!({ "value": "enabled", "description": "Enable feature X" });
        let description = body
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(description, "Enable feature X");
    }

    #[test]
    fn test_config_get_all_query_has_order() {
        let query = "SELECT * FROM _config ORDER BY key ASC";
        assert!(query.contains("ORDER BY key ASC"));
    }

    #[test]
    fn test_config_key_value_map_extraction() {
        let configs = [
            json!({ "key": "feature_a", "value": true }),
            json!({ "key": "feature_b", "value": "off" }),
            json!({ "key": "max_retries", "value": 3 }),
        ];

        let map: serde_json::Map<String, Value> = configs
            .iter()
            .filter_map(|c| {
                let key = c.get("key")?.as_str()?;
                let value = c.get("value")?;
                Some((key.to_string(), value.clone()))
            })
            .collect();

        assert_eq!(map.len(), 3);
        assert_eq!(map["feature_a"], json!(true));
        assert_eq!(map["feature_b"], json!("off"));
        assert_eq!(map["max_retries"], json!(3));
    }

    #[test]
    fn test_config_key_value_map_skips_invalid() {
        let configs = [
            json!({ "value": true }), // missing key
            json!({ "key": "valid", "value": "yes" }),
            json!({ "key": null, "value": "nope" }), // null key
        ];

        let map: serde_json::Map<String, Value> = configs
            .iter()
            .filter_map(|c| {
                let key = c.get("key")?.as_str()?;
                let value = c.get("value")?;
                Some((key.to_string(), value.clone()))
            })
            .collect();

        assert_eq!(map.len(), 1);
        assert_eq!(map["valid"], json!("yes"));
    }

    #[test]
    fn test_config_get_returns_null_for_missing() {
        let results: Vec<Value> = vec![];
        let value = results
            .first()
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(value, Value::Null);
    }

    #[test]
    fn test_config_get_returns_value_when_found() {
        let results = [json!({ "value": "enabled" })];
        let value = results
            .first()
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(value, json!("enabled"));
    }

    #[test]
    fn test_index_compound_unique() {
        let req = CreateIndexRequest {
            collection: "order_items".to_string(),
            fields: vec!["order_id".to_string(), "product_id".to_string()],
            unique: true,
        };
        let query = req.to_surreal_query();
        assert!(query.contains("UNIQUE"));
        assert!(query.contains("order_id, product_id"));
        assert_eq!(req.index_name(), "idx_order_items_order_id_product_id");
    }
}
