//! Transport layer — HTTP/SSE + stdio for MCP communication

use crate::auth::McpContext;
use crate::safeguards::{IdempotencyTracker, SpendLimit};
use crate::server::JsonRpcRequest;
use crate::{McpState, OrignaGtaMcp};
use axum::{
    Router,
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::Value;
use std::sync::Arc;
use tracing::info;

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

    Router::new()
        .route("/mcp/rpc", post(handle_rpc))
        .route("/mcp/tools", get(list_tools))
        .with_state(mcp_state)
}

/// Handle JSON-RPC 2.0 requests
async fn handle_rpc(
    State(mcp): State<Arc<OrignaGtaMcp>>,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    info!(method = %request.method, "RPC request received");

    // Extract auth context (in production, from Authorization header via middleware)
    let ctx = McpContext::new();

    // Process request
    let response = mcp.handle_request(request, ctx).await;

    (StatusCode::OK, Json(response))
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
                Err(_e) => {
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
