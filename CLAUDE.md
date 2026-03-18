@~/CLAUDE.md
@REPO_MAP.md

# OrignaBase — Project Rules

OrignaBase is a Rust BaaS (Backend as a Service) running on VPS at api.orignagta.ca.
Tech stack: Rust (axum), SurrealDB, async-graphql, tower, JWT auth, Stripe, FCM.

## Key Rules

1. ALL data access via `/graphql` POST — NO REST `/api/collections` endpoints exist
2. NEVER manually edit `.pb.go` — always `make protos` (requires Colima/Docker)
3. Auth: `/auth/*` endpoints — refresh via `{"refresh_token": "..."}` body (NOT Bearer header)
4. GraphQL filters: OBJECT `{field: {_op: val}}` NOT array — server calls `filters.as_object()`
5. WebSocket `/realtime` requires `?token=<jwt>` query param
6. Storage: 2-step presigned URL — POST `/storage/presign/upload` → PUT to signed URL
7. SurrealDB IDs contain `:` (e.g. `products:abc123`) — sanitize to `_` for Meilisearch

## Testing

- Unit: `cargo test`
- Integration: `cargo test -- --ignored` (needs `OB_TEST_URL=https://api.orignagta.ca`)
- Test files: `crates/orignabase/tests/` (auth, user, product, cart, order)
- ALL tests: 2491 Rust + 168 integration + 98 Flutter SDK — must ALL PASS before deploy

## VPS

- IP: 204.168.137.16
- SSH: `ssh -i ~/.ssh/id_ed25519 root@204.168.137.16`
- Deploy: push to main → CI builds → Docker → VPS

## Dev Commands

```bash
cargo test                          # unit tests
cargo test -- --ignored             # integration (needs running server)
cargo clippy -- -D warnings         # lint
cargo build --release               # release build (needs extra swap on 8GB)
```

## Critical Rust Patterns (from LEARNED.md)

- SurrealDB v2 Ws: `host:port` format only (no `ws://` prefix) — silent hang otherwise
- SurrealDB RecordId: use `Record` wrapper struct with `#[serde(flatten)]`, not plain `Value`
- tower_governor behind Docker/Caddy: use `PeerIpKeyExtractor` not `SmartIpKeyExtractor`
- Full LTO + codegen-units=1 OOMs on 8GB → add 4GB swap before release build
