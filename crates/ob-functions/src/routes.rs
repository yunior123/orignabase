use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use ob_core::{Error, Result};
use serde::Deserialize;
use std::sync::Arc;

use crate::registry::{FunctionMeta, FunctionRegistry, TriggerType};

/// Shared state for function routes.
#[derive(Clone)]
pub struct FunctionsState {
    pub registry: Arc<FunctionRegistry>,
}

#[derive(Deserialize)]
pub struct DeployBody {
    pub name: String,
    pub wasm_base64: String,
    pub triggers: Vec<TriggerType>,
    pub description: Option<String>,
}

/// POST /functions/deploy — Deploy a new WASM function.
pub async fn deploy_function(
    State(state): State<FunctionsState>,
    Json(request): Json<DeployBody>,
) -> Result<Json<FunctionMeta>> {
    let wasm_bytes = base64_decode(&request.wasm_base64)?;
    let meta = state.registry.register(
        &request.name,
        &wasm_bytes,
        request.triggers,
        request.description,
    )?;
    Ok(Json(meta))
}

/// POST /functions/invoke/:name — Invoke a function by name.
pub async fn invoke_function(
    State(state): State<FunctionsState>,
    Path(name): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse> {
    let module = state.registry.get_module(&name)?;
    let input = String::from_utf8(body.to_vec())
        .map_err(|_| Error::Validation("Invalid UTF-8 input".into()))?;

    let result = state
        .registry
        .runtime()
        .execute(&module, "handle", &input)
        .await?;

    Ok((StatusCode::OK, result))
}

/// GET /functions — List all deployed functions.
pub async fn list_functions(
    State(state): State<FunctionsState>,
) -> Result<Json<Vec<FunctionMeta>>> {
    Ok(Json(state.registry.list()))
}

/// GET /functions/:name — Get function metadata.
pub async fn get_function(
    State(state): State<FunctionsState>,
    Path(name): Path<String>,
) -> Result<Json<FunctionMeta>> {
    let meta = state.registry.get_meta(&name)?;
    Ok(Json(meta))
}

/// DELETE /functions/:name — Unregister a function.
pub async fn delete_function(
    State(state): State<FunctionsState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>> {
    state.registry.unregister(&name)?;
    Ok(Json(serde_json::json!({ "deleted": name })))
}

fn base64_decode(input: &str) -> Result<Vec<u8>> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    STANDARD
        .decode(input)
        .map_err(|e| Error::Validation(format!("Invalid base64: {e}")))
}

/// Build the functions router.
pub fn functions_router(state: FunctionsState) -> axum::Router {
    axum::Router::new()
        .route("/functions", axum::routing::get(list_functions))
        .route(
            "/functions/deploy",
            axum::routing::post(deploy_function),
        )
        .route(
            "/functions/invoke/{name}",
            axum::routing::post(invoke_function),
        )
        .route(
            "/functions/{name}",
            axum::routing::get(get_function).delete(delete_function),
        )
        .with_state(state)
}
