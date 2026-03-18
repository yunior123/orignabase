use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use ob_core::{Error, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::registry::{FunctionMeta, FunctionRegistry, TriggerType};

/// Shared state for function routes.
#[derive(Clone)]
pub struct FunctionsState {
    pub registry: Arc<FunctionRegistry>,
    /// Optional database client for storing execution logs.
    pub db: Option<ob_database::DatabaseClient>,
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
///
/// Records execution logs in `_function_logs` when a database client is available.
pub async fn invoke_function(
    State(state): State<FunctionsState>,
    Path(name): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse> {
    let module = state.registry.get_module(&name)?;
    let input_size = body.len();
    let input = String::from_utf8(body.to_vec())
        .map_err(|_| Error::Validation("Invalid UTF-8 input".into()))?;

    let start = std::time::Instant::now();

    let exec_result = state
        .registry
        .runtime()
        .execute(&module, "handle", &input)
        .await;

    let duration_ms = start.elapsed().as_millis() as u64;

    // Store execution log if DB is available
    if let Some(ref db) = state.db {
        let (status, error_message, output_size) = match &exec_result {
            Ok(output) => ("success", Value::Null, output.len()),
            Err(e) => ("error", json!(e.to_string()), 0),
        };

        let log_entry = json!({
            "function_name": name,
            "input_size": input_size,
            "output_size": output_size,
            "duration_ms": duration_ms,
            "status": status,
            "error_message": error_message,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        // Store log in background to not block the response
        let db = db.clone();
        tokio::spawn(async move {
            let _ = db.create_document("_function_logs", log_entry).await;
        });
    }

    let result = exec_result?;
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

/// GET /functions/:name/logs — Return last 50 execution logs for a function.
pub async fn get_function_logs(
    State(state): State<FunctionsState>,
    Path(name): Path<String>,
) -> Result<Json<Value>> {
    let db = state
        .db
        .as_ref()
        .ok_or_else(|| Error::Internal("Database not configured for function logs".into()))?;

    let logs = db
        .query_bind(
            "SELECT * FROM _function_logs WHERE function_name = $name \
             ORDER BY timestamp DESC LIMIT 50",
            json!({ "name": name }),
        )
        .await?;

    Ok(Json(json!({ "function": name, "logs": logs })))
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
        .route("/functions/deploy", axum::routing::post(deploy_function))
        .route(
            "/functions/invoke/{name}",
            axum::routing::post(invoke_function),
        )
        .route(
            "/functions/{name}",
            axum::routing::get(get_function).delete(delete_function),
        )
        .route(
            "/functions/{name}/logs",
            axum::routing::get(get_function_logs),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deploy_body_deser() {
        let json = json!({
            "name": "my_func",
            "wasm_base64": "AGFzbQEAAAA=",
            "triggers": [
                { "http": { "method": "GET", "path": "/hello" } }
            ],
            "description": "A test function"
        });
        let body: DeployBody = serde_json::from_value(json).unwrap();
        assert_eq!(body.name, "my_func");
        assert_eq!(body.wasm_base64, "AGFzbQEAAAA=");
        assert_eq!(body.triggers.len(), 1);
        assert_eq!(body.description, Some("A test function".to_string()));
    }

    #[test]
    fn test_deploy_body_deser_no_description() {
        let json = json!({
            "name": "minimal",
            "wasm_base64": "AGFzbQEAAAA=",
            "triggers": []
        });
        let body: DeployBody = serde_json::from_value(json).unwrap();
        assert_eq!(body.name, "minimal");
        assert!(body.description.is_none());
        assert!(body.triggers.is_empty());
    }

    #[test]
    fn test_deploy_body_deser_multiple_triggers() {
        let json = json!({
            "name": "multi",
            "wasm_base64": "AGFzbQEAAAA=",
            "triggers": [
                { "http": { "method": "POST", "path": "/webhook" } },
                { "cron": { "schedule": "0 * * * *" } },
                { "database": { "collection": "orders", "event": "create" } }
            ]
        });
        let body: DeployBody = serde_json::from_value(json).unwrap();
        assert_eq!(body.triggers.len(), 3);
    }

    #[test]
    fn test_functions_state_fields() {
        // Compile-time check that FunctionsState has the expected fields.
        fn _assert_fields(s: &FunctionsState) {
            let _ = &s.registry;
            let _ = &s.db;
        }
    }

    #[test]
    fn test_functions_state_db_is_optional() {
        // Verify that FunctionsState can be constructed with db = None
        // (compile-time check via type assertion)
        fn _assert_optional(db: Option<ob_database::DatabaseClient>) {
            let _: Option<ob_database::DatabaseClient> = db;
        }
    }

    #[test]
    fn test_base64_decode_valid() {
        let result = base64_decode("SGVsbG8=");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), b"Hello");
    }

    #[test]
    fn test_base64_decode_invalid() {
        let result = base64_decode("not valid base64!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_base64_decode_empty() {
        let result = base64_decode("");
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
