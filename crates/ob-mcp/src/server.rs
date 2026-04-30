//! MCP server core — orchestrates tool routing and execution

use crate::McpState;
use crate::auth::McpContext;
use crate::errors::{JsonRpcError, McpError, McpResult};
use crate::safeguards::{IdempotencyTracker, SpendLimit};
use crate::tools;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{error, info};

/// MCP Server — routes JSON-RPC 2.0 requests to tool handlers
pub struct OrignaGtaMcp {
    pub state: McpState,
    pub idempotency: IdempotencyTracker,
    pub spend_limit: SpendLimit,
}

impl OrignaGtaMcp {
    pub fn new(state: McpState, idempotency: IdempotencyTracker, spend_limit: SpendLimit) -> Self {
        Self {
            state,
            idempotency,
            spend_limit,
        }
    }

    /// Process a JSON-RPC 2.0 request
    pub async fn handle_request(
        &self,
        request: JsonRpcRequest,
        ctx: McpContext,
    ) -> JsonRpcResponse {
        info!(method = %request.method, id = ?request.id, "Processing MCP request");

        let result = match request.method.as_str() {
            // Catalog tools (no auth required)
            "search_products" => self.search_products(&request.params, &ctx).await,
            "get_product" => self.get_product(&request.params, &ctx).await,
            "check_inventory" => self.check_inventory(&request.params, &ctx).await,

            // Shopping tools (auth required)
            "get_cart" => self.get_cart(&request.params, &ctx).await,
            "add_to_cart" => self.add_to_cart(&request.params, &ctx).await,
            "remove_from_cart" => self.remove_from_cart(&request.params, &ctx).await,
            "apply_coupon" => self.apply_coupon(&request.params, &ctx).await,

            // Order tools (auth required)
            "list_orders" => self.list_orders(&request.params, &ctx).await,
            "get_order" => self.get_order(&request.params, &ctx).await,
            "request_return" => self.request_return(&request.params, &ctx).await,
            "create_checkout" => self.create_checkout(&request.params, &ctx).await,

            // Admin tools (admin role required)
            "get_analytics" => self.get_analytics(&request.params, &ctx).await,
            "create_review" => self.create_review(&request.params, &ctx).await,

            // Info tools
            "tools/list" => Ok(self.list_tools()),

            _ => Err(McpError::MethodNotFound(request.method.clone())),
        };

        match result {
            Ok(content) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: Some(content),
                error: None,
                id: request.id,
            },
            Err(err) => {
                error!(method = %request.method, error = %err, "MCP request failed");
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: None,
                    error: Some(JsonRpcError::from(err)),
                    id: request.id,
                }
            }
        }
    }

    // Catalog tools
    async fn search_products(&self, params: &Value, _ctx: &McpContext) -> McpResult<Value> {
        tools::catalog::search_products(self.state.clone(), params).await
    }

    async fn get_product(&self, params: &Value, _ctx: &McpContext) -> McpResult<Value> {
        tools::catalog::get_product(self.state.clone(), params).await
    }

    async fn check_inventory(&self, params: &Value, _ctx: &McpContext) -> McpResult<Value> {
        tools::catalog::check_inventory(self.state.clone(), params).await
    }

    // Shopping tools
    async fn get_cart(&self, params: &Value, ctx: &McpContext) -> McpResult<Value> {
        let user_id = ctx.user_id()?;
        tools::shopping::get_cart(self.state.clone(), &user_id, params).await
    }

    async fn add_to_cart(&self, params: &Value, ctx: &McpContext) -> McpResult<Value> {
        let user_id = ctx.user_id()?;
        tools::shopping::add_to_cart(self.state.clone(), &user_id, params).await
    }

    async fn remove_from_cart(&self, params: &Value, ctx: &McpContext) -> McpResult<Value> {
        let user_id = ctx.user_id()?;
        tools::shopping::remove_from_cart(self.state.clone(), &user_id, params).await
    }

    async fn apply_coupon(&self, params: &Value, ctx: &McpContext) -> McpResult<Value> {
        let user_id = ctx.user_id()?;
        tools::shopping::apply_coupon(self.state.clone(), &user_id, params).await
    }

    // Order tools
    async fn list_orders(&self, params: &Value, ctx: &McpContext) -> McpResult<Value> {
        let user_id = ctx.user_id()?;
        tools::orders::list_orders(self.state.clone(), &user_id, params).await
    }

    async fn get_order(&self, params: &Value, ctx: &McpContext) -> McpResult<Value> {
        let user_id = ctx.user_id()?;
        tools::orders::get_order(self.state.clone(), &user_id, params).await
    }

    async fn request_return(&self, params: &Value, ctx: &McpContext) -> McpResult<Value> {
        let user_id = ctx.user_id()?;
        tools::orders::request_return(self.state.clone(), &user_id, params).await
    }

    async fn create_checkout(&self, params: &Value, ctx: &McpContext) -> McpResult<Value> {
        let user_id = ctx.user_id()?;
        tools::orders::create_checkout(self.state.clone(), &user_id, params).await
    }

    // Admin tools
    async fn get_analytics(&self, params: &Value, ctx: &McpContext) -> McpResult<Value> {
        ctx.require_admin()?;
        tools::admin::get_analytics(self.state.clone(), params).await
    }

    async fn create_review(&self, params: &Value, ctx: &McpContext) -> McpResult<Value> {
        let user_id = ctx.user_id()?;
        tools::admin::create_review(self.state.clone(), &user_id, params).await
    }

    /// List all available tools (for client discovery)
    fn list_tools(&self) -> Value {
        json!({
            "tools": [
                {
                    "name": "search_products",
                    "description": "Search products by query, category, price range",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string" },
                            "category": { "type": "string" },
                            "min_price": { "type": "integer" },
                            "max_price": { "type": "integer" },
                            "limit": { "type": "integer" },
                            "offset": { "type": "integer" }
                        },
                        "required": ["query"]
                    }
                },
                {
                    "name": "get_product",
                    "description": "Get product details by ID",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "product_id": { "type": "string" }
                        },
                        "required": ["product_id"]
                    }
                },
                {
                    "name": "check_inventory",
                    "description": "Check product stock availability",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "product_id": { "type": "string" }
                        },
                        "required": ["product_id"]
                    }
                },
                {
                    "name": "get_cart",
                    "description": "Get current user cart (requires authentication)",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "add_to_cart",
                    "description": "Add item to cart (requires authentication)",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "product_id": { "type": "string" },
                            "quantity": { "type": "integer" },
                            "idempotency_key": { "type": "string" }
                        },
                        "required": ["product_id", "quantity"]
                    }
                },
                {
                    "name": "list_orders",
                    "description": "List user orders (requires authentication)",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "status": { "type": "string" },
                            "limit": { "type": "integer" },
                            "offset": { "type": "integer" }
                        }
                    }
                },
                {
                    "name": "get_order",
                    "description": "Get order details (requires authentication, ownership verified)",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "order_id": { "type": "string" }
                        },
                        "required": ["order_id"]
                    }
                },
                {
                    "name": "get_analytics",
                    "description": "Get marketplace analytics (admin only)",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "period": { "type": "string", "enum": ["day", "week", "month"] }
                        }
                    }
                }
            ]
        })
    }
}

/// JSON-RPC 2.0 request
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
    pub id: Option<Value>,
}

/// JSON-RPC 2.0 response
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: Option<Value>,
}
