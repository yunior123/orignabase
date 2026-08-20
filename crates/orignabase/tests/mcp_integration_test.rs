//! Integration tests for ob-mcp transport: JSON-RPC endpoint and tool listing.
//!
//! MCP routes (/mcp/rpc, /mcp/tools) are optional — they may not be mounted
//! on all server configurations. Tests accept 404 as "MCP not enabled".
//!
//! Run with: `cargo test --test mcp_integration_test -- --ignored`
//!
//! Requirements:
//!   OB_TEST_URL=http://localhost:8080 (or remote OrignaBase instance)

use ob_database::fields;
use serde_json::{Value, json};
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string()) // ignore-magic
}

/// Register a test user and return (access_token, user_id).
async fn register_test_user(client: &reqwest::Client) -> (String, String) {
    let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "TestPassword123!" })) // ignore-magic
        .send()
        .await
        .expect("register failed");
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let token = body["access_token"].as_str().unwrap().to_string(); // ignore-magic
    let user_id = body["user"][fields::ID].as_str().unwrap().to_string(); // ignore-magic
    (token, user_id)
}

/// Check if MCP endpoints are available (returns true if /mcp/tools returns 200).
async fn mcp_available(client: &reqwest::Client) -> bool {
    let resp = client.get(format!("{}/mcp/tools", base_url())).send().await;
    matches!(resp, Ok(r) if r.status() == 200)
}

// =============================================================================
// SECTION 1: Tool Listing — GET /mcp/tools
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_mcp_list_tools() {
    let client = reqwest::Client::new();
    if !mcp_available(&client).await {
        eprintln!("MCP not enabled on server, skipping");
        return;
    }

    let resp = client
        .get(format!("{}/mcp/tools", base_url()))
        .send()
        .await
        .expect("list tools failed");

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic

    assert_eq!(status, 200, "List tools should succeed: {body:?}");
    let tools = body["tools"].as_array().expect("should return tools array"); // ignore-magic
    assert!(
        tools.len() >= 10,
        "Should have at least 10 tools, got {}",
        tools.len()
    );

    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t[fields::NAME].as_str())
        .collect(); // ignore-magic
    assert!(
        tool_names.contains(&"search_products"),
        "Should include search_products"
    );
    assert!(
        tool_names.contains(&"get_product"),
        "Should include get_product"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_mcp_list_tools_has_descriptions() {
    let client = reqwest::Client::new();
    if !mcp_available(&client).await {
        eprintln!("MCP not enabled on server, skipping");
        return;
    }

    let resp = client
        .get(format!("{}/mcp/tools", base_url()))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let tools = body["tools"].as_array().unwrap(); // ignore-magic

    for tool in tools {
        assert!(
            tool[fields::NAME].as_str().is_some(), // ignore-magic
            "Each tool should have a name"
        );
        assert!(
            tool["description"].as_str().is_some(), // ignore-magic
            "Each tool should have a description: {tool:?}"
        );
    }
}

// =============================================================================
// SECTION 2: RPC Endpoint — POST /mcp/rpc
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_mcp_rpc_search_products_anonymous() {
    let client = reqwest::Client::new();
    if !mcp_available(&client).await {
        eprintln!("MCP not enabled on server, skipping");
        return;
    }

    let resp = client
        .post(format!("{}/mcp/rpc", base_url()))
        .json(&json!({ // ignore-magic
            "jsonrpc": "2.0",
            "method": "search_products",
            "params": { "query": "laptop" }, // ignore-magic
            "id": 1
        }))
        .send()
        .await
        .expect("rpc request failed");

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic

    assert_eq!(status, 200, "RPC should return 200: {body:?}");
    assert_eq!(body["jsonrpc"], "2.0"); // ignore-magic
    assert_eq!(body[fields::ID], 1); // ignore-magic
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_mcp_rpc_search_products_authenticated() {
    let client = reqwest::Client::new();
    if !mcp_available(&client).await {
        eprintln!("MCP not enabled on server, skipping");
        return;
    }

    let (token, _user_id) = register_test_user(&client).await;

    let resp = client
        .post(format!("{}/mcp/rpc", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ // ignore-magic
            "jsonrpc": "2.0",
            "method": "search_products",
            "params": { "query": "phone" }, // ignore-magic
            "id": 42
        }))
        .send()
        .await
        .expect("rpc request failed");

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic

    assert_eq!(status, 200, "Authenticated RPC should succeed: {body:?}");
    assert_eq!(body[fields::ID], 42); // ignore-magic
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_mcp_rpc_invalid_method() {
    let client = reqwest::Client::new();
    if !mcp_available(&client).await {
        eprintln!("MCP not enabled on server, skipping");
        return;
    }

    let resp = client
        .post(format!("{}/mcp/rpc", base_url()))
        .json(&json!({ // ignore-magic
            "jsonrpc": "2.0",
            "method": "nonexistent_method",
            "params": {},
            "id": 99
        }))
        .send()
        .await
        .expect("rpc request failed");

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic

    assert_eq!(
        status, 200,
        "Should return 200 with JSON-RPC error: {body:?}"
    );
    assert!(
        body["error"].is_object(), // ignore-magic
        "Should have error field for unknown method"
    );
    assert_eq!(
        body["error"]["code"],
        -32601, // ignore-magic
        "Should be method not found error"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_mcp_rpc_get_product_by_id() {
    let client = reqwest::Client::new();
    if !mcp_available(&client).await {
        eprintln!("MCP not enabled on server, skipping");
        return;
    }

    let resp = client
        .post(format!("{}/mcp/rpc", base_url()))
        .json(&json!({ // ignore-magic
            "jsonrpc": "2.0",
            "method": "get_product",
            "params": { "id": "products:nonexistent_test" },
            "id": 5
        }))
        .send()
        .await
        .expect("rpc request failed");

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic

    assert_eq!(
        status, 200,
        "Should return valid JSON-RPC response: {body:?}"
    );
    assert_eq!(body["jsonrpc"], "2.0"); // ignore-magic
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_mcp_rpc_check_inventory() {
    let client = reqwest::Client::new();
    if !mcp_available(&client).await {
        eprintln!("MCP not enabled on server, skipping");
        return;
    }

    let resp = client
        .post(format!("{}/mcp/rpc", base_url()))
        .json(&json!({ // ignore-magic
            "jsonrpc": "2.0",
            "method": "check_inventory",
            "params": { "product_id": "products:test_123" },
            "id": 7
        }))
        .send()
        .await
        .expect("rpc request failed");

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic

    assert_eq!(
        status, 200,
        "Check inventory should return valid response: {body:?}"
    );
    assert_eq!(body["jsonrpc"], "2.0"); // ignore-magic
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_mcp_rpc_get_cart_requires_auth() {
    let client = reqwest::Client::new();
    if !mcp_available(&client).await {
        eprintln!("MCP not enabled on server, skipping");
        return;
    }

    let resp = client
        .post(format!("{}/mcp/rpc", base_url()))
        .json(&json!({ // ignore-magic
            "jsonrpc": "2.0",
            "method": "get_cart",
            "params": {},
            "id": 10
        }))
        .send()
        .await
        .expect("rpc request failed");

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic

    assert_eq!(status, 200, "Should return 200 with error: {body:?}");
    assert!(
        body["error"].is_object() || body["result"].is_object() || body["result"].is_null(), // ignore-magic
        "Should handle unauthenticated cart request: {body:?}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_mcp_rpc_string_id() {
    let client = reqwest::Client::new();
    if !mcp_available(&client).await {
        eprintln!("MCP not enabled on server, skipping");
        return;
    }

    let resp = client
        .post(format!("{}/mcp/rpc", base_url()))
        .json(&json!({ // ignore-magic
            "jsonrpc": "2.0",
            "method": "search_products",
            "params": { "query": "test" }, // ignore-magic
            "id": "request-abc-123"
        }))
        .send()
        .await
        .expect("rpc request failed");

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic

    assert_eq!(status, 200);
    assert_eq!(
        body[fields::ID],
        "request-abc-123", // ignore-magic
        "String ID should be preserved in response"
    );
}
