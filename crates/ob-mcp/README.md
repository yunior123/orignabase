# ob-mcp — Model Context Protocol Server for OrignaBase

## Overview

`ob-mcp` is a Rust crate that exposes OrignaBase marketplace operations as Model Context Protocol (MCP) tools. It integrates directly into the OrignaBase Axum application (no separate process) and provides:

- **HTTP/SSE transport** — for remote Claude and agent clients
- **Stdio transport** — for local development with `claude config add`
- **JSON-RPC 2.0 protocol** — standard MCP message format
- **JWT authentication** — reuses ob-auth validation
- **Built-in safeguards** — idempotency, spend limits, error sanitization

## Architecture

```
ob-mcp crate
├── lib.rs           — Public API: McpState, OrignaGtaMcp
├── errors.rs        — JSON-RPC 2.0 error responses
├── auth.rs          — JWT extraction + context (reuses ob-auth)
├── safeguards.rs    — Idempotency tracking, spend limits, confirmation tokens
├── server.rs        — Main handler routing + tool dispatcher
├── tools/           — Tool implementations
│   ├── catalog.rs   — search_products, get_product, check_inventory
│   ├── shopping.rs  — cart management
│   ├── orders.rs    — list orders, get order, returns, checkout
│   └── admin.rs     — analytics, reviews
└── transport.rs     — HTTP routes + stdio setup
```

## Tools

### Public (no auth)
- `search_products(query, category?, min_price?, max_price?, limit?, offset?)` — full-text search
- `get_product(product_id)` — fetch product details
- `check_inventory(product_id)` — check stock availability

### Private (requires auth)
- `get_cart()` — read shopping cart
- `add_to_cart(product_id, quantity, idempotency_key?)` — add item
- `remove_from_cart(product_id)` — remove item
- `apply_coupon(code)` — apply coupon code
- `list_orders(status?, limit?, offset?)` — user orders
- `get_order(order_id)` — order details (ownership verified)
- `request_return(order_id, reason)` — file return request
- `create_checkout(items[], shipping_address, idempotency_key?)` — Stripe checkout

### Admin (admin role required)
- `get_analytics(period?)` — marketplace analytics
- `create_review(product_id, rating, review?)` — product review

## Integration

See `INTEGRATION.md` for:
1. Adding to `orignabase/Cargo.toml`
2. Mounting routes in `main.rs`
3. Testing endpoints with curl
4. Enabling Claude MCP access locally

## Safeguards

1. **Idempotency keys** — Track duplicate operations for cart/checkout/returns
2. **Spend limits** — Max $1M per request, $10M per 24h per user
3. **Confirmation tokens** — Time-limited tokens for sensitive operations
4. **Error sanitization** — No stack traces, no DB details in responses

## Schema Integration

All tools preserve existing OrignaBase schemas:
- Money: **integer cents** (no float conversion)
- PostgreSQL IDs: `collection:record_id` format preserved
- Timestamps: `createdAt` (orders), `dateCreated` (products), `timestamp` (webhooks)

## Development

### Local Testing

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

### With Claude

```bash
# In ~/.config/claude/claude.json
{
  "mcp_servers": {
    "orignabase": {
      "command": "cd ~/orignabase && cargo run -p orignabase -- --mcp-stdio"
    }
  }
}
```

Then in Claude:
```
claude: search for shoes under $100
```

## Future Enhancements

- WebSocket transport for real-time updates
- Resource subscriptions (product/order change notifications)
- Batch operations (atomic multi-tool calls)
- Tool versioning for API evolution
- Per-client rate limiting and usage tracking

## Status

✓ Crate structure created
✓ All modules stubbed with signatures
✓ Tool definitions documented
✓ Integration guide written
⚠️ Awaiting ob-handlers compilation fix (pre-existing issue)

Once ob-handlers compiles, run:
```bash
cd orignabase && cargo check -p ob-mcp
```

---

**Path:** `/Users/yuniorrodriguezosorio/Documents/GitHub/orignabase/crates/ob-mcp/`
