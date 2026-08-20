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
        tools::shopping::add_to_cart(
            self.state.clone(),
            &user_id,
            params,
            Some(&self.idempotency),
        )
        .await
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
        tools::orders::create_checkout(
            self.state.clone(),
            &user_id,
            params,
            Some(&self.spend_limit),
            Some(&self.idempotency),
        )
        .await
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::McpClaims;
    use ob_database::fields;
    use std::sync::Arc;

    fn make_claims(role: Option<&str>) -> McpClaims {
        McpClaims {
            sub: "users:u1".into(),
            uid: "u1".into(),
            role: role.map(String::from),
            iat: 0,
            exp: i64::MAX,
        }
    }

    async fn make_state() -> McpState {
        McpState {
            db: Arc::new(ob_database::DatabaseClient::new_mem().await),
            search: None,
            config: Arc::new(ob_core::Config::load(None).unwrap()),
            jwt_keys: Arc::new(ob_auth::JwtKeys::from_secret("test-secret")),
        }
    }

    async fn make_server() -> OrignaGtaMcp {
        OrignaGtaMcp::new(
            make_state().await,
            IdempotencyTracker::new(),
            SpendLimit::new(100_000_000, 1_000_000_000),
        )
    }

    // ── list_tools ──

    #[tokio::test]
    #[serial_test::serial]
    async fn test_list_tools_contains_all_tools() {
        let server = make_server().await;
        let tools = server.list_tools();
        let tool_list = tools["tools"].as_array().unwrap();
        assert!(tool_list.len() >= 8);

        let names: Vec<&str> = tool_list
            .iter()
            .map(|t| t[fields::NAME].as_str().unwrap())
            .collect();
        assert!(names.contains(&"search_products"));
        assert!(names.contains(&"get_product"));
        assert!(names.contains(&"check_inventory"));
        assert!(names.contains(&"get_cart"));
        assert!(names.contains(&"add_to_cart"));
        assert!(names.contains(&"list_orders"));
        assert!(names.contains(&"get_order"));
        assert!(names.contains(&"get_analytics"));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_list_tools_has_required_fields() {
        let server = make_server().await;
        let tools = server.list_tools();
        for tool in tools["tools"].as_array().unwrap() {
            assert!(tool[fields::NAME].is_string(), "tool missing name");
            assert!(tool["description"].is_string(), "tool missing description");
            assert!(tool["inputSchema"].is_object(), "tool missing inputSchema");
        }
    }

    // ── handle_request routing ──

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_unknown_method() {
        let server = make_server().await;
        let ctx = McpContext::new();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "nonexistent_method".into(),
            params: json!({}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_tools_list() {
        let server = make_server().await;
        let ctx = McpContext::new();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "tools/list".into(),
            params: json!({}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_none());
        assert!(resp.result.is_some());
        assert!(resp.result.unwrap()["tools"].is_array());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_search_products_unauthenticated() {
        let server = make_server().await;
        let ctx = McpContext::new();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "search_products".into(),
            params: json!({"query": "shirt"}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_search_products_missing_query() {
        let server = make_server().await;
        let ctx = McpContext::new();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "search_products".into(),
            params: json!({}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_get_cart_unauthenticated() {
        let server = make_server().await;
        let ctx = McpContext::new();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "get_cart".into(),
            params: json!({}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, 401);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_get_cart_authenticated() {
        let server = make_server().await;
        let ctx = McpContext::with_claims(make_claims(Some("buyer")));
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "get_cart".into(),
            params: json!({}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_admin_unauthenticated() {
        let server = make_server().await;
        let ctx = McpContext::new();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "get_analytics".into(),
            params: json!({}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, 401);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_admin_non_admin() {
        let server = make_server().await;
        let ctx = McpContext::with_claims(make_claims(Some("buyer")));
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "get_analytics".into(),
            params: json!({}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, 403);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_admin_authorized() {
        let server = make_server().await;
        let ctx = McpContext::with_claims(make_claims(Some("admin")));
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "get_analytics".into(),
            params: json!({}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_response_preserves_id() {
        let server = make_server().await;
        let ctx = McpContext::new();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "tools/list".into(),
            params: json!({}),
            id: Some(json!(42)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert_eq!(resp.id, Some(json!(42)));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_response_jsonrpc_version() {
        let server = make_server().await;
        let ctx = McpContext::new();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "tools/list".into(),
            params: json!({}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert_eq!(resp.jsonrpc, "2.0");
    }

    // ── JsonRpcRequest / Response serialization ──

    #[test]
    fn test_jsonrpc_request_deserialize() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "method": "search_products",
            "params": {"query": "shirt"},
            "id": 1
        }"#;
        let req: JsonRpcRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "search_products");
        assert_eq!(req.params["query"], "shirt");
        assert_eq!(req.id, Some(json!(1)));
    }

    #[test]
    fn test_jsonrpc_response_serialize() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            result: Some(json!({"ok": true})),
            error: None,
            id: Some(json!(1)),
        };
        let serialized = serde_json::to_value(&resp).unwrap();
        assert_eq!(serialized["jsonrpc"], "2.0");
        assert!(serialized["error"].is_null());
        assert_eq!(serialized[fields::ID], 1);
    }

    #[test]
    fn test_jsonrpc_response_serialize_with_error() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "Method not found".into(),
                data: None,
            }),
            id: Some(json!(1)),
        };
        let serialized = serde_json::to_value(&resp).unwrap();
        assert!(serialized["result"].is_null());
        assert_eq!(serialized["error"]["code"], -32601);
    }

    // ── All MCP methods route correctly ──

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_get_product() {
        let server = make_server().await;
        server
            .state
            .db
            .upsert_document(
                "products",
                "products:p1",
                json!({
                    "name": "Test Item",
                    "stockQuantity": 10,
                    "lifecycleStatus": "active"
                }),
            )
            .await
            .unwrap();
        let ctx = McpContext::new();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "get_product".into(),
            params: json!({"product_id": "products:p1"}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_check_inventory() {
        let server = make_server().await;

        server
            .state
            .db
            .upsert_document(
                "products",
                "products:p1",
                json!({
                    "name": "Test Item",
                    "stockQuantity": 10,
                    "lifecycleStatus": "active"
                }),
            )
            .await
            .unwrap();

        let ctx = McpContext::new();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "check_inventory".into(),
            params: json!({"product_id": "products:p1"}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_list_orders_authenticated() {
        let server = make_server().await;
        let ctx = McpContext::with_claims(make_claims(Some("buyer")));
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "list_orders".into(),
            params: json!({}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_create_review_authenticated_no_purchase() {
        // Authenticated but product doesn't exist — should return NotFound (404)
        let server = make_server().await;
        let ctx = McpContext::with_claims(make_claims(Some("buyer")));
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "create_review".into(),
            params: json!({"product_id": "products:nonexistent_review_test", "rating": 5}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, 404);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_request_create_review_unauthenticated() {
        let server = make_server().await;
        let ctx = McpContext::new();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "create_review".into(),
            params: json!({"product_id": "products:p1", "rating": 5}),
            id: Some(json!(1)),
        };
        let resp = server.handle_request(req, ctx).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, 401);
    }
}
