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

/// GET /_admin/collections — List all collections.
async fn list_collections(State(state): State<AdminState>) -> Result<Json<Value>> {
    let info = schema::list_collections(&state.db).await?;
    Ok(Json(json!({ "collections": info })))
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
