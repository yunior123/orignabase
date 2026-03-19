# ob-mcp Integration Guide

## Overview

The `ob-mcp` crate provides Model Context Protocol (MCP) server functionality for OrignaBase. It exposes marketplace operations (search, cart, orders, admin analytics) as MCP tools that can be called by Claude and other agents.

The server integrates directly into the OrignaBase Axum application — no separate process. It provides:
1. **HTTP/SSE transport** — mounted on `/mcp/` routes for remote clients
2. **Stdio transport** — for local development with Claude (via `claude config add`)

## Integration Steps

### 1. Add ob-mcp dependency to orignabase/Cargo.toml

In `crates/orignabase/Cargo.toml`, add to `[dependencies]`:

```toml
ob-mcp = { workspace = true }
```

### 2. Update crates/orignabase/src/main.rs

Import the MCP router creation function:

```rust
use ob_mcp::transport::create_mcp_router;
use ob_mcp::McpState;
```

After creating the Axum router (where you add other routes), add the MCP routes:

```rust
// Inside the main() function, after creating the base router:

// Create MCP state from existing dependencies
let mcp_state = McpState::new(
    Arc::clone(&db),           // DatabaseClient from ob-database
    search.clone(),             // Optional SearchClient from ob-search
    Arc::clone(&config),        // Config from ob-core
);

// Create MCP router
let mcp_router = create_mcp_router(mcp_state);

// Merge into main router
let app = app
    .merge(mcp_router)
    // ... rest of routes
```

### 3. MCP HTTP API Endpoints

Once integrated, the MCP server exposes:

- **POST `/mcp/rpc`** — JSON-RPC 2.0 endpoint
  - Request: `{ "jsonrpc": "2.0", "method": "search_products", "params": {...}, "id": 1 }`
  - Response: `{ "jsonrpc": "2.0", "result": {...}, "id": 1 }`

- **GET `/mcp/tools`** — List available tools (for client discovery)
  - Response: JSON array of tool definitions

### 4. Authentication

The MCP server expects JWT authentication via the `Authorization: Bearer <token>` header.

In production, add middleware to extract and verify JWT before routing to MCP handlers:

```rust
use ob_auth::middleware::JwtMiddleware;

let app = app
    .layer(JwtMiddleware)
    .merge(mcp_router)
```

For local development (stdout), authentication is optional — tests can send requests without auth header.

### 5. Local Development with Claude

To enable Claude to call OrignaBase MCP tools locally:

1. Start the OrignaBase dev server on port 8081:
   ```bash
   cd crates/orignabase && ENVIRONMENT=dev cargo run
   ```

2. Configure Claude MCP via stdin:
   ```bash
   # In your Claude config (~/.config/claude/claude.json or similar)
   {
     "mcp_servers": {
       "orignabase": {
         "command": "cd ~/orignabase && cargo run -p orignabase -- --mcp-stdio"
       }
     }
   }
   ```

3. Claude can now call tools like:
   ```
   claude: search for shoes under $100
   ```

### Tools Overview

#### Public (no auth required)
- `search_products(query, category?, min_price?, max_price?, limit?, offset?)` — search by text/filters
- `get_product(product_id)` — fetch product details
- `check_inventory(product_id)` — check stock

#### Private (requires authentication)

**Shopping:**
- `get_cart()` — read user cart
- `add_to_cart(product_id, quantity, idempotency_key?)` — add to cart
- `remove_from_cart(product_id)` — remove from cart
- `apply_coupon(code)` — apply coupon code

**Orders:**
- `list_orders(status?, limit?, offset?)` — list user orders
- `get_order(order_id)` — get order details (with ownership check)
- `request_return(order_id, reason)` — file return request
- `create_checkout(items[], shipping_address, idempotency_key?)` — create Stripe checkout

**Admin (admin role required):**
- `get_analytics(period?)` — marketplace analytics
- `create_review(product_id, rating, review?)` — create product review

### Safeguards

The MCP server includes built-in protections:

1. **Idempotency keys** — `add_to_cart`, `create_checkout`, `request_return` support idempotency keys to prevent duplicate operations
2. **Spend limits** — `create_checkout` enforces max $1,000,000 per request, $10,000,000 per 24h per user
3. **Confirmation tokens** — For sensitive operations, MCP can generate time-limited confirmation tokens
4. **Error sanitization** — All errors exclude stack traces and internal DB details

### Schema Integration

ob-mcp reuses all existing OrignaBase schemas:

| Collection | Timestamp | Notes |
|------------|-----------|-------|
| orders | `createdAt` | |
| products | `dateCreated` | |
| webhook_events | `timestamp` | NOT createdAt |

All monetary values remain **integer cents** — no conversion layer.

SurrealDB IDs preserved in format `collection:record_id`.

### Error Handling

All errors return JSON-RPC 2.0 error responses with sanitized messages:

```json
{
  "jsonrpc": "2.0",
  "error": {
    "code": 401,
    "message": "Unauthorized"
  },
  "id": 1
}
```

Error codes:
- `-32600` — Invalid request
- `-32601` — Method not found
- `-32602` — Invalid params
- `-32603` — Internal error
- `401` — Unauthorized
- `403` — Forbidden
- `404` — Not found
- `422` — Validation error

### Testing

Test the MCP server with curl:

```bash
# Search products
curl -X POST http://localhost:8081/mcp/rpc \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "search_products",
    "params": {"query": "shoes", "limit": 10},
    "id": 1
  }'

# List tools
curl http://localhost:8081/mcp/tools
```

### Future Enhancements

1. **WebSocket transport** — Real-time tool availability updates
2. **Resource subscription** — Tools that notify on product/order changes
3. **Batch operations** — Single request to perform multiple tool calls atomically
4. **Tool versioning** — Support for breaking API changes
5. **Rate limiting per client** — Track usage per connected Claude session

### Troubleshooting

**MCP server not responding:**
- Check that `/mcp/rpc` route is mounted on the app router
- Verify `ob-mcp` is added to workspace members in Cargo.toml
- Check logs for JWT parsing errors (if auth is enabled)

**Tools returning empty results:**
- Verify database connection is working (`/health` endpoint)
- Check Meilisearch availability (if search is enabled)
- See tracing logs for query errors

**Authentication failing:**
- Ensure `Authorization: Bearer <jwt>` header is included
- Verify JWT is signed with the correct public key from ob-auth
- Check JWT expiration (use `exp` claim)

---

**File:** `/Users/yuniorrodriguezosorio/Documents/GitHub/orignabase/crates/ob-mcp/INTEGRATION.md`
