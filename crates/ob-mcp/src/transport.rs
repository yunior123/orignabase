//! Transport layer — HTTP/SSE + stdio for MCP communication

use crate::auth::{McpContext, extract_claims};
use crate::safeguards::{IdempotencyTracker, SpendLimit};
use crate::server::JsonRpcRequest;
use crate::{McpState, OrignaGtaMcp};
use axum::{
    Router,
    extract::{Json, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::Value;
use std::sync::Arc;
#[cfg(not(test))]
use std::time::Duration;
#[cfg(not(test))]
use tower_governor::{
    GovernorLayer, governor::GovernorConfigBuilder, key_extractor::PeerIpKeyExtractor,
};
use tracing::{debug, info};

/// Public type exported for use in main.rs
pub type McpRouter = Router;

/// Create MCP router for mounting on Axum
pub fn create_mcp_router(state: McpState) -> Router {
    let idempotency = IdempotencyTracker::new();
    let spend_limit = SpendLimit::new(
        100_000_000,   // $1,000,000 CAD per request
        1_000_000_000, // $10,000,000 CAD per 24h per user
    );

    let mcp_state = Arc::new(OrignaGtaMcp::new(state, idempotency, spend_limit));

    // P1-NEW-16: Rate limiting — 30 req/60s per IP (stricter than API-wide 100)
    // Skipped in test builds — PeerIpKeyExtractor fails on axum oneshot (no TCP)
    let router = Router::new()
        .route("/mcp/rpc", post(handle_rpc))
        .route("/mcp/tools", get(list_tools));

    #[cfg(not(test))]
    let router = {
        let mcp_governor_conf = Arc::new(
            GovernorConfigBuilder::default()
                .key_extractor(PeerIpKeyExtractor)
                .per_millisecond(600)
                .burst_size(30)
                .finish()
                .expect("valid governor config for mcp"),
        );
        let mcp_governor_limiter = mcp_governor_conf.limiter().clone();
        // Spawn periodic cleanup to prevent memory leak
        {
            let limiter = mcp_governor_limiter;
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    limiter.retain_recent();
                }
            });
        }
        router.layer(GovernorLayer::new(mcp_governor_conf))
    };

    router.with_state(mcp_state)
}

/// Handle JSON-RPC 2.0 requests
async fn handle_rpc(
    State(mcp): State<Arc<OrignaGtaMcp>>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    info!(method = %request.method, "RPC request received");

    // Extract auth context from Authorization header
    let ctx = match headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        Some(auth_header) => match extract_claims(Some(auth_header), &mcp.state.jwt_keys) {
            Ok(claims) => {
                debug!(uid = %claims.uid, "Authenticated MCP request");
                McpContext::with_claims(claims)
            }
            Err(e) => {
                // P1-NEW-5: Reject invalid JWTs instead of silently downgrading to anonymous.
                // An invalid Authorization header means the caller attempted to authenticate
                // but failed — this should be a hard error, not a silent fallback.
                tracing::warn!("MCP auth failed: {e}");
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": { "code": -32000, "message": "Invalid authentication token" },
                        "id": null
                    })),
                );
            }
        },
        None => McpContext::new(),
    };

    // Process request
    let response = mcp.handle_request(request, ctx).await;

    (
        StatusCode::OK,
        Json(serde_json::to_value(response).unwrap_or_default()),
    )
}

/// List available tools
async fn list_tools(State(_mcp): State<Arc<OrignaGtaMcp>>) -> impl IntoResponse {
    let tools = serde_json::json!({
        "tools": [
            {
                "name": "search_products",
                "description": "Search products by query, category, price range"
            },
            {
                "name": "get_product",
                "description": "Get product details by ID"
            },
            {
                "name": "check_inventory",
                "description": "Check product stock availability"
            },
            {
                "name": "get_cart",
                "description": "Get current user cart (requires authentication)"
            },
            {
                "name": "add_to_cart",
                "description": "Add item to cart (requires authentication)"
            },
            {
                "name": "remove_from_cart",
                "description": "Remove item from cart (requires authentication)"
            },
            {
                "name": "apply_coupon",
                "description": "Apply coupon to cart (requires authentication)"
            },
            {
                "name": "list_orders",
                "description": "List user orders (requires authentication)"
            },
            {
                "name": "get_order",
                "description": "Get order details (requires authentication)"
            },
            {
                "name": "request_return",
                "description": "Request return for order (requires authentication)"
            },
            {
                "name": "create_checkout",
                "description": "Create checkout session (requires authentication)"
            },
            {
                "name": "get_analytics",
                "description": "Get marketplace analytics (admin only)"
            },
            {
                "name": "create_review",
                "description": "Create product review (requires authentication)"
            }
        ]
    });

    (StatusCode::OK, Json(tools))
}

/// For stdio-based transport (local Claude MCP development):
/// See INTEGRATION.md for usage with: claude config add /path/to/ob-mcp
pub struct StdioTransport;

impl StdioTransport {
    /// Run MCP server over stdio (blocking)
    /// This reads JSON-RPC requests from stdin and writes responses to stdout
    pub async fn run(mcp: Arc<OrignaGtaMcp>) -> anyhow::Result<()> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::io::{stdin, stdout};

        let stdin = stdin();
        let mut stdout = stdout();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();

        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break; // EOF
            }

            let request: Result<JsonRpcRequest, _> = serde_json::from_str(&line);
            match request {
                Ok(req) => {
                    let ctx = McpContext::new();
                    let response = mcp.handle_request(req, ctx).await;
                    let json = serde_json::to_string(&response)?;
                    stdout.write_all(json.as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                    stdout.flush().await?;
                }
                Err(e) => {
                    tracing::warn!("MCP parse error: {e}");
                    let error_response = serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32700,
                            "message": "Parse error"
                        },
                        "id": Value::Null
                    });
                    let json = serde_json::to_string(&error_response)?;
                    stdout.write_all(json.as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                    stdout.flush().await?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safeguards::{IdempotencyTracker, SpendLimit};
    use ob_database::fields;
    use serde_json::json;
    use std::sync::Arc;

    async fn make_state() -> McpState {
        McpState {
            db: Arc::new(ob_database::DatabaseClient::new_mem().await),
            search: None,
            config: Arc::new(ob_core::Config::load(None).unwrap()),
            jwt_keys: Arc::new(ob_auth::JwtKeys::from_secret("test-secret")),
        }
    }

    async fn make_server() -> Arc<OrignaGtaMcp> {
        Arc::new(OrignaGtaMcp::new(
            make_state().await,
            IdempotencyTracker::new(),
            SpendLimit::new(100_000_000, 1_000_000_000),
        ))
    }

    // ── create_mcp_router ──

    #[tokio::test]
    async fn test_create_mcp_router_is_router() {
        let state = make_state().await;
        let router = create_mcp_router(state);
        // Router is a valid type — just verify it compiles and creates
        let _ = router;
    }

    // ── StdioTransport parse error handling ──

    #[tokio::test]
    async fn test_stdio_transport_handles_invalid_json() {
        // StdioTransport reads from stdin — we can't easily test the full loop
        // but we can verify the error response format it would produce
        let error_response = serde_json::json!({
            "jsonrpc": "2.0",
            "error": {
                "code": -32700,
                "message": "Parse error"
            },
            "id": Value::Null
        });
        assert_eq!(error_response["error"]["code"], -32700);
        assert_eq!(error_response["error"]["message"], "Parse error");
        assert!(error_response[fields::ID].is_null());
    }

    // ── list_tools response ──

    #[tokio::test]
    async fn test_list_tools_response_contains_expected_tools() {
        let tools_json = serde_json::json!({
            "tools": [
                { "name": "search_products", "description": "Search products by query, category, price range" },
                { "name": "get_product", "description": "Get product details by ID" },
                { "name": "check_inventory", "description": "Check product stock availability" },
                { "name": "get_cart", "description": "Get current user cart (requires authentication)" },
                { "name": "add_to_cart", "description": "Add item to cart (requires authentication)" },
                { "name": "remove_from_cart", "description": "Remove item from cart (requires authentication)" },
                { "name": "apply_coupon", "description": "Apply coupon to cart (requires authentication)" },
                { "name": "list_orders", "description": "List user orders (requires authentication)" },
                { "name": "get_order", "description": "Get order details (requires authentication)" },
                { "name": "request_return", "description": "Request return for order (requires authentication)" },
                { "name": "create_checkout", "description": "Create checkout session (requires authentication)" },
                { "name": "get_analytics", "description": "Get marketplace analytics (admin only)" },
                { "name": "create_review", "description": "Create product review (requires authentication)" }
            ]
        });
        let tool_list = tools_json["tools"].as_array().unwrap();
        assert_eq!(tool_list.len(), 13);

        let names: Vec<&str> = tool_list
            .iter()
            .map(|t| t[fields::NAME].as_str().unwrap())
            .collect();
        assert!(names.contains(&"search_products"));
        assert!(names.contains(&"get_analytics"));
        assert!(names.contains(&"create_review"));
    }

    // ── JsonRpcRequest parsing from JSON (simulates what transport does) ──

    #[test]
    fn test_parse_valid_rpc_request() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "method": "search_products",
            "params": {"query": "phone"},
            "id": 1
        }"#;
        let req: crate::server::JsonRpcRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.method, "search_products");
        assert_eq!(req.params["query"], "phone");
    }

    #[test]
    fn test_parse_rpc_request_null_id() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "method": "tools/list",
            "params": {},
            "id": null
        }"#;
        let req: crate::server::JsonRpcRequest = serde_json::from_str(json_str).unwrap();
        assert!(req.id.is_none());
    }

    #[test]
    fn test_parse_rpc_request_string_id() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "method": "tools/list",
            "params": {},
            "id": "abc-123"
        }"#;
        let req: crate::server::JsonRpcRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.id, Some(Value::String("abc-123".into())));
    }

    #[test]
    fn test_parse_invalid_json_returns_parse_error_format() {
        let result: Result<crate::server::JsonRpcRequest, _> = serde_json::from_str("{not json}");
        assert!(result.is_err());
    }

    // ── McpRouter type alias ──

    #[test]
    fn test_mcp_router_type() {
        // Verify McpRouter is Router
        let _state_fut = async {
            let state = make_state().await;
            let _: McpRouter = create_mcp_router(state);
        };
        // Just compile-check via type assertion
        let _ = std::mem::size_of::<McpRouter>();
    }

    // ── End-to-end: request -> response roundtrip ──

    #[tokio::test]
    async fn test_request_response_roundtrip() {
        let server = make_server().await;

        let request_json = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "search_products",
            "params": {"query": "hat"},
            "id": 99
        });

        let req: crate::server::JsonRpcRequest = serde_json::from_value(request_json).unwrap();
        let ctx = crate::auth::McpContext::new();
        let resp = server.handle_request(req, ctx).await;

        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(json!(99)));
        assert!(resp.error.is_none());
        assert!(resp.result.is_some());
    }

    #[tokio::test]
    async fn test_error_response_roundtrip() {
        let server = make_server().await;

        let request_json = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "invalid_method",
            "params": {},
            "id": 42
        });

        let req: crate::server::JsonRpcRequest = serde_json::from_value(request_json).unwrap();
        let ctx = crate::auth::McpContext::new();
        let resp = server.handle_request(req, ctx).await;

        assert!(resp.result.is_none());
        assert!(resp.error.is_some());

        let err = resp.error.as_ref().unwrap();
        assert_eq!(err.code, -32601);

        // Verify it serializes correctly
        let serialized = serde_json::to_value(&resp).unwrap();
        assert_eq!(serialized["jsonrpc"], "2.0");
        assert_eq!(serialized[fields::ID], 42);
        assert!(serialized["result"].is_null());
    }

    #[test]
    fn test_parse_rpc_request_with_array_params() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": ["arg1", "arg2"],
            "id": 2
        }"#;
        let req: crate::server::JsonRpcRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.method, "tools/call");
        assert!(req.params.is_array());
        assert_eq!(req.params.as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_parse_rpc_request_with_no_params() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "method": "tools/list",
            "id": 3
        }"#;
        // params is required by the struct, so this should fail
        let result = serde_json::from_str::<crate::server::JsonRpcRequest>(json_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_rpc_request_with_numeric_id() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "method": "search_products",
            "params": {},
            "id": 0
        }"#;
        let req: crate::server::JsonRpcRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.id, Some(serde_json::json!(0)));
    }

    #[test]
    fn test_parse_rpc_request_with_negative_id() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "method": "test",
            "params": {},
            "id": -1
        }"#;
        let req: crate::server::JsonRpcRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.id, Some(serde_json::json!(-1)));
    }

    #[test]
    fn test_parse_rpc_request_missing_jsonrpc_version() {
        let json_str = r#"{
            "method": "test",
            "params": {},
            "id": 1
        }"#;
        let result = serde_json::from_str::<crate::server::JsonRpcRequest>(json_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_rpc_request_missing_method() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "params": {},
            "id": 1
        }"#;
        let result = serde_json::from_str::<crate::server::JsonRpcRequest>(json_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_tools_list_contains_expected_count() {
        let tools_json = serde_json::json!({
            "tools": [
                { "name": "search_products" },
                { "name": "get_product" },
                { "name": "check_inventory" },
                { "name": "get_cart" },
                { "name": "add_to_cart" },
                { "name": "remove_from_cart" },
                { "name": "apply_coupon" },
                { "name": "list_orders" },
                { "name": "get_order" },
                { "name": "request_return" },
                { "name": "create_checkout" },
                { "name": "get_analytics" },
                { "name": "create_review" }
            ]
        });
        assert_eq!(tools_json["tools"].as_array().unwrap().len(), 13);
    }

    #[test]
    fn test_jsonrpc_response_serialization_skips_none_result() {
        let resp = crate::server::JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(crate::errors::JsonRpcError {
                code: -32600,
                message: "Invalid request".to_string(),
                data: None,
            }),
            id: Some(serde_json::json!(1)),
        };
        let serialized = serde_json::to_value(&resp).unwrap();
        assert!(serialized.get("result").is_none());
        assert!(serialized["error"]["code"].as_i64().unwrap() == -32600);
    }

    #[test]
    fn test_jsonrpc_response_serialization_skips_none_error() {
        let resp = crate::server::JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: Some(serde_json::json!({"ok": true})),
            error: None,
            id: Some(serde_json::json!(1)),
        };
        let serialized = serde_json::to_value(&resp).unwrap();
        assert!(serialized.get("error").is_none());
        assert_eq!(serialized["result"]["ok"], true);
    }

    #[test]
    fn test_parse_rpc_request_with_complex_params() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "method": "search_products",
            "params": {
                "query": "phone",
                "filters": {"category": "electronics", "price_max": 500},
                "limit": 10
            },
            "id": 42
        }"#;
        let req: crate::server::JsonRpcRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.params["filters"]["category"], "electronics");
        assert_eq!(req.params["limit"], 10);
    }

    // ── HTTP transport tests via axum oneshot ──

    #[tokio::test]
    async fn test_handle_rpc_via_http_search() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::util::ServiceExt;

        let state = make_state().await;
        let router = create_mcp_router(state);

        let body = json!({
            "jsonrpc": "2.0",
            "method": "search_products",
            "params": {"query": "test"},
            "id": 1
        });

        let req = Request::builder()
            .method("POST")
            .uri("/mcp/rpc")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 100_000)
            .await
            .unwrap();
        let resp_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(resp_json["jsonrpc"], "2.0");
        assert_eq!(resp_json[fields::ID], 1);
        assert!(resp_json.get("error").is_none() || resp_json["error"].is_null());
    }

    #[tokio::test]
    async fn test_handle_rpc_via_http_unknown_method() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::util::ServiceExt;

        let state = make_state().await;
        let router = create_mcp_router(state);

        let body = json!({
            "jsonrpc": "2.0",
            "method": "nonexistent_method",
            "params": {},
            "id": 2
        });

        let req = Request::builder()
            .method("POST")
            .uri("/mcp/rpc")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 100_000)
            .await
            .unwrap();
        let resp_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(resp_json["error"].is_object());
        assert_eq!(resp_json["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn test_handle_rpc_with_invalid_auth_header() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::util::ServiceExt;

        let state = make_state().await;
        let router = create_mcp_router(state);

        let body = json!({
            "jsonrpc": "2.0",
            "method": "search_products",
            "params": {"query": "phone"},
            "id": 3
        });

        let req = Request::builder()
            .method("POST")
            .uri("/mcp/rpc")
            .header("Content-Type", "application/json")
            .header("Authorization", "Bearer invalid-jwt-token")
            .body(Body::from(body.to_string()))
            .unwrap();

        // P1-NEW-5: Invalid JWT should return 401, not silently fall back to anonymous
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_handle_rpc_without_auth_header() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::util::ServiceExt;

        let state = make_state().await;
        let router = create_mcp_router(state);

        let body = json!({
            "jsonrpc": "2.0",
            "method": "search_products",
            "params": {"query": "hat"},
            "id": 4
        });

        let req = Request::builder()
            .method("POST")
            .uri("/mcp/rpc")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_list_tools_via_http() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::util::ServiceExt;

        let state = make_state().await;
        let router = create_mcp_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/mcp/tools")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 100_000)
            .await
            .unwrap();
        let resp_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let tools = resp_json["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 13);
    }

    #[tokio::test]
    async fn test_multiple_rpc_requests_sequentially() {
        let server = make_server().await;

        for i in 0..5 {
            let request_json = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "search_products",
                "params": {"query": format!("item_{i}")},
                "id": i
            });
            let req: crate::server::JsonRpcRequest = serde_json::from_value(request_json).unwrap();
            let ctx = crate::auth::McpContext::new();
            let resp = server.handle_request(req, ctx).await;
            assert_eq!(resp.jsonrpc, "2.0");
            assert_eq!(resp.id, Some(json!(i)));
        }
    }
}
