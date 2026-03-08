use axum::{Json, extract::State};
use axum::response::Html;
use ob_core::{Error, Result};
use ob_database::DatabaseClient;
use serde_json::{Value, json};

use crate::schema::{self, CollectionSchema};

const DASHBOARD_HTML: &str = include_str!("dashboard.html");

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

/// GET /_admin/ — Serve the admin dashboard SPA.
async fn dashboard() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

/// Build the admin router. All routes require admin authentication.
pub fn admin_router(state: AdminState) -> axum::Router {
    axum::Router::new()
        .route("/_admin", axum::routing::get(dashboard))
        .route("/_admin/", axum::routing::get(dashboard))
        .route("/_admin/health", axum::routing::get(health))
        .route(
            "/_admin/collections",
            axum::routing::get(list_collections).post(create_collection),
        )
        .route(
            "/_admin/collections/{name}",
            axum::routing::delete(drop_collection),
        )
        .route("/_admin/users", axum::routing::get(list_users))
        .route("/_admin/users/{id}", axum::routing::delete(delete_user))
        .route(
            "/_admin/users/{id}/roles",
            axum::routing::patch(update_roles),
        )
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
        assert!(ts.contains('+') || ts.ends_with('Z'), "timestamp should have timezone: {ts}");
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
}
