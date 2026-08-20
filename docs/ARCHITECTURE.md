# OrignaBase Architecture

## Overview

OrignaBase is a self-hosted Backend-as-a-Service (BaaS) written in Rust, designed as a Firebase replacement for the Origna GTA marketplace. It provides a unified API surface via GraphQL, with REST fallbacks for domain-specific operations (payments, orders, chat, shipping).

**Tech Stack:**
- **Runtime:** Rust (edition 2024, rust-version 1.85) + Tokio async runtime
- **Web Framework:** axum 0.8 with tower middleware stack
- **GraphQL:** async-graphql 7 with depth/complexity limits
- **Database:** PostgreSQL v2 (embedded RocksDB for local dev, standalone for production)
- **Search:** Meilisearch (optional — warns but starts without)
- **Auth:** JWT (RS256 primary, HS256 fallback), Argon2 password hashing, TOTP MFA
- **Payments:** Stripe API (Connect, Checkout, Refunds)
- **Realtime:** WebSocket via axum + DashMap subscription registry
- **Functions:** wasmi WASM runtime for user-defined functions
- **Push:** Firebase Cloud Messaging (FCM HTTP v1 API)
- **Cluster:** NATS JetStream for multi-node realtime sync
- **Storage:** Local filesystem + S3/R2 compatible storage
- **Rate Limiting:** tower-governor (IP-based) + per-user DB-backed rate limiter

**Deployment:**
- VPS at 204.168.137.16 (api.orignagta.ca)
- Docker container behind Caddy reverse proxy
- CI: push to main → Docker build → deploy to VPS

## Crate Dependency Graph

```
orignabase (binary)
├── ob-core          (config, error, validation, tenant)
├── ob-database      (PostgreSQL client, CRUD, query translator)
│   └── ob-core
├── ob-auth          (JWT, routes, MFA, email, middleware)
│   ├── ob-core
│   └── ob-database
├── ob-graphql       (dynamic schema, resolvers)
│   ├── ob-core
│   ├── ob-database
│   ├── ob-security
│   ├── ob-realtime
│   └── ob-search
├── ob-security      (rules DSL parser + evaluator)
│   └── ob-core
├── ob-realtime      (WebSocket, dispatcher, registry)
│   ├── ob-core
│   ├── ob-database
│   └── ob-auth
├── ob-storage       (local + S3, signed URLs, transforms)
│   ├── ob-core
│   └── ob-auth
├── ob-search        (Meilisearch client + syncer)
│   └── ob-core
├── ob-functions     (WASM runtime, cron, triggers)
│   ├── ob-core
│   └── ob-database
├── ob-analytics     (privacy-first event tracking)
│   ├── ob-core
│   └── ob-database
├── ob-admin         (dashboard, schema mgmt, config API)
│   ├── ob-core
│   └── ob-database
├── ob-notifications (FCM push proxy)
│   ├── ob-core
│   └── ob-database
├── ob-mcp           (Model Context Protocol server)
│   ├── ob-core
│   ├── ob-database
│   ├── ob-auth
│   └── ob-search
└── ob-handlers      (business logic: payments, orders, chat, etc.)
    ├── ob-core
    ├── ob-database
    ├── ob-auth
    └── ob-storage
```

## Crate Documentation

### ob-core

**Purpose:** Foundational types shared by all other crates. Zero dependencies on other workspace crates.

**Modules:**
- `config.rs` — `Config` struct with TOML loading + `OB_*` env var overrides
- `error.rs` — `Error` enum and `Result<T>` type alias
- `validate.rs` — `validate_identifier()`, `validate_document_id()`, `escape_sql_string()`, `validate_known_collection()`
- `tenant.rs` — `TenantContext`, `tenant_middleware` (header/subdomain extraction)
- `state.rs` — Application state utilities
- `server.rs` — Server lifecycle helpers

**Key Types:**
```rust
pub enum Error {
    Config(String),     // → 500, hides details
    Database(String),   // → 500, hides details
    Auth(String),       // → 401
    Forbidden(String),  // → 403
    NotFound(String),   // → 404
    Validation(String), // → 400
    UnsupportedMediaType(String), // → 415
    Internal(String),   // → 500, hides details
}
```

`Error` implements `IntoResponse` for axum. `Database`/`Internal`/`Config` variants never leak details to clients.

**Config Loading:** `Config::load(path)` reads from `orignabase.toml`, then overlays `OB_*` env vars. Naming convention: `OB_AUTH__JWT_SECRET` → `auth.jwt_secret` (double underscore for nesting).

**Secrets:** `Config.secrets` is a `HashMap<String, String>` populated from `OB_SECRETS__*` env vars. Used for `stripe_secret_key`, `turnstile_secret_key`, etc.

**Dependencies:** serde, toml, dotenvy, chrono, axum, thiserror

---

### ob-database

**Purpose:** PostgreSQL abstraction layer. All database access goes through this crate.

**Modules:**
- `client.rs` — `DatabaseClient` (connect, query, health check)
- `crud.rs` — CRUD operations, FieldValue operations, batch ops, vector search
- `query.rs` — `QueryTranslator` (GraphQL filter → SQL WHERE clause)
- `transaction.rs` — `Transaction` wrapper for multi-statement atomicity
- `task_queue.rs` — `Task`, `TaskQueue`, `TaskStatus`, `run_worker`

**Key Types:**
```rust
pub struct DatabaseClient { db: PgPool }
pub struct Record { id: RecordId, #[serde(flatten)] rest: HashMap<String, Value> }
pub struct QueryTranslator;  // static methods
pub struct Transaction { queries: Vec<(String, Option<Value>)> }
pub struct Task { task_type, payload, status, queue, priority, ... }
pub enum TaskStatus { Pending, Running, Completed, Failed, DeadLetter }
```

**CRUD Methods on DatabaseClient:**
- `create_document(collection, data)` → `Value`
- `get_document(collection, id)` → `Value`
- `update_document(collection, id, data)` → `Value`
- `upsert_document(collection, id, data)` → `Value`
- `delete_document(collection, id)` → `Value`
- `list_documents(collection, limit)` → `Vec<Value>`
- `batch_create(collection, docs)` → `Vec<Value>` (uses `INSERT INTO`)
- `batch_update(collection, updates)` → `Vec<Value>`
- `batch_delete(collection, ids)` → `Vec<Value>`
- `update_with_field_values(collection, id, data)` — handles `_serverTimestamp`, `_increment`, `_arrayUnion`, `_arrayRemove`, `_deleteField`
- `vector_search(collection, field, embedding, top_k, threshold)`
- `query_raw(query)`, `query_raw_value(query)`, `query_bind(query, binds)`

**PostgreSQL Record ID Pattern:**
Records use standard UUID primary keys. The `collection:record_id` string format is used for API compatibility with the SDK client.

**Query Translator:**
Supports operators: `_eq`, `_neq`, `_gt`, `_gte`, `_lt`, `_lte`, `_in`, `_contains`, `_starts_with`.
Input format is OBJECT `{field: {_op: val}}` — NOT array. All field names validated via `validate_identifier()`.

**Task Queue:**
Self-hosted replacement for Google Cloud Tasks. Tasks stored in `_task_queue` collection. Workers poll and process by `queue` name. Supports priority, retry with max_retries, scheduled execution, dead-lettering.

**Dependencies:** ob-core, sqlx (postgres), tokio

---

### ob-auth

**Purpose:** Authentication, authorization, and user management.

**Modules:**
- `jwt.rs` — `Claims`, `JwtKeys`, token issuance/verification, RSA key generation + rotation
- `routes.rs` — All auth endpoints (register, login, logout, refresh, MFA, magic link, OAuth, admin)
- `middleware.rs` — `AuthContext`, `auth_extractor` (axum middleware)
- `totp.rs` — TOTP/MFA setup, verification, encryption at rest
- `email.rs` — `EmailService`, `EmailConfig`, SMTP via lettre
- `oauth.rs` — Google, Apple, OIDC OAuth flows
- `password.rs` — Argon2 hashing
- `rate_limit.rs` — `RateLimiter` (per-user DB-backed)
- `key_rotation.rs` — `KeyRotationManager`, RSA key fingerprinting
- `turnstile.rs` — Cloudflare Turnstile verification
- `login_tracking.rs` — Login attempt tracking for lockout

**JWT Strategy:**
- **Primary:** RS256 (asymmetric) — auto-generates RSA key pair on first start, stored in `./data/keys/`
- **Fallback:** HS256 (symmetric) — uses `auth.jwt_secret` from config
- **Key Rotation:** `JwtKeys::from_rsa_pem_with_rotation()` accepts previous public keys for fallback verification (tokens signed with old keys still valid until expiry)

**Token Types:**
| Token Type | TTL | Purpose |
|-----------|-----|---------|
| `access` | 15min (configurable) | API access |
| `refresh` | 7 days (configurable) | Token renewal |
| `email_verify` | 24h | Email verification |
| `password_reset` | 1h | Password reset |
| `magic_link` | 15min | Passwordless login |
| `mfa_challenge` | 5min | TOTP verification step |

**AuthState:** Shared across all auth routes. Contains `DatabaseClient`, `JwtKeys`, OAuth configs, `EmailService`, TOTP encryption key, Turnstile key.

**Dependencies:** ob-core, ob-database, jsonwebtoken, argon2, lettre, totp-rs, aes-gcm, dashmap

---

### ob-graphql

**Purpose:** Dynamic GraphQL schema builder and CRUD resolvers.

**Modules:**
- `schema.rs` — `build_schema()`, `build_schema_with_limits()`, `GraphQlLimits`
- `resolvers.rs` — `QueryRoot`, `MutationRoot` with generic CRUD operations

**Key Types:**
```rust
pub struct GraphQlLimits {
    pub enable_introspection: bool,  // disabled by default in prod
    pub max_depth: usize,            // default 12
    pub max_complexity: usize,       // default 100
}
pub struct GqlContext {
    pub db: DatabaseClient,
    pub rules: Arc<RuleEngine>,
    pub user_id: Option<String>,
    pub roles: Vec<String>,
    pub authenticated: bool,
}
```

**Schema Composition:**
`Schema::build(QueryRoot, MutationRoot, EmptySubscription)` with injected data: `DatabaseClient`, `RuleEngine`, `mpsc::Sender<ChangeEvent>`, `SearchClient`. Security limits applied via `limit_depth()` and `limit_complexity()`.

**Auth in Resolvers:** JWT token extracted from `Authorization: Bearer <token>` in main.rs, injected as `AuthContext` into async-graphql data. Resolvers access via `ctx.data::<AuthContext>()`.

**Dependencies:** ob-core, ob-database, ob-security, ob-realtime, ob-search, async-graphql

---

### ob-security

**Purpose:** Firestore-style security rules DSL parser and evaluator.

**Modules:**
- `parser.rs` — PEG grammar (pest), parses `.ob` rules files
- `evaluator.rs` — `RuleEngine`, `SecurityContext` — evaluates rules against auth context

**DSL Example:**
```
match /users/{userId} {
  allow read: auth.uid != null;
  allow write: auth.uid == userId;
}
```

**Key Types:**
```rust
pub struct RuleEngine { rules: HashMap<String, RuleSet> }
pub struct SecurityContext { pub uid: Option<String>, pub roles: Vec<String>, ... }
pub struct RuleSet { pub collection: String, pub rules: Vec<SecurityRule> }
pub enum Expression { Bool, Number, StringLit, Path, FunctionCall, Comparison, And, Or, Not }
```

**Dependencies:** ob-core, pest, pest_derive

---

### ob-realtime

**Purpose:** WebSocket-based realtime subscriptions with change dispatching.

**Modules:**
- `websocket.rs` — WebSocket handler, `ClientMessage`/`ServerMessage` protocol, `RealtimeState`
- `dispatcher.rs` — `ChangeDispatcher` (listens for events, pushes to subscribers)
- `registry.rs` — `SubscriptionRegistry`, `Subscription`, `ChangeEvent`, `PresenceInfo`
- `cluster.rs` — `ClusterBridge` (NATS JetStream for multi-node sync)

**Protocol:**
```
Client → Server: {"type": "subscribe", "id": "...", "collection": "products"}
Server → Client: {"type": "subscribed", "id": "..."}
Server → Client: {"type": "change", "subscription_id": "...", "event": {...}}
Client → Server: {"type": "ping"}
Server → Client: {"type": "pong"}
```

**Connection:** WebSocket upgrade at `/realtime?token=<jwt>`. Max 100 subscriptions per connection.

**Registry Architecture:** `DashMap`-based concurrent registry mapping `(collection, filter_hash)` → set of subscription IDs. `Arc<str>` for collection names to avoid expensive String cloning during broadcast.

**Cluster:** Optional NATS JetStream bridge (`#[cfg(feature = "cluster")]`) for syncing realtime events across multiple server nodes.

**Dependencies:** ob-core, ob-database, ob-auth, axum (ws), dashmap, futures-util, async-nats

---

### ob-storage

**Purpose:** File storage with local filesystem and S3/R2 backends.

**Modules:**
- `local.rs` — `LocalStorage` (filesystem-based `StorageBackend`)
- `s3.rs` — `S3Config`, `S3Storage` (AWS S3 / Cloudflare R2)
- `routes.rs` — Upload, download, presigned URL endpoints
- `signed_url.rs` — `SignedUrlGenerator` (HMAC-signed URLs)
- `resumable.rs` — `ResumableUploadManager` (chunked/tus-style uploads)
- `transform.rs` — Image processing (resize, crop, format conversion)

**StorageBackend Trait:**
```rust
pub trait StorageBackend: Send + Sync {
    fn upload(&self, path: &str, data: &[u8], content_type: &str) -> impl Future<Output = Result<ObjectMeta>>;
    fn download(&self, path: &str) -> impl Future<Output = Result<Vec<u8>>>;
    fn delete(&self, path: &str) -> impl Future<Output = Result<()>>;
    fn exists(&self, path: &str) -> impl Future<Output = Result<bool>>;
    fn metadata(&self, path: &str) -> impl Future<Output = Result<ObjectMeta>>;
    fn list(&self, prefix: &str) -> impl Future<Output = Result<Vec<ObjectMeta>>>;
}
```

**Upload Flow:** 2-step presigned URL pattern:
1. `POST /storage/presign/upload` → returns signed URL + metadata
2. Client `PUT`s to signed URL

**Security:** Magic byte validation via `infer` crate, MIME type whitelist (jpeg, png, gif, webp, pdf), path sanitization against directory traversal, max 500MB regular / 5GB resumable.

**Dependencies:** ob-core, ob-auth, image, infer, hmac, sha2, base64, aws-sdk-s3

---

### ob-search

**Purpose:** Meilisearch integration for full-text search.

**Modules:**
- `client.rs` — `SearchClient` (HTTP wrapper around Meilisearch REST API)
- `config.rs` — `SearchConfig`, `IndexConfig`
- `sync.rs` — `SearchSyncer` (background task syncing DB changes → Meilisearch)

**Key Types:**
```rust
pub struct SearchClient { config: SearchConfig, http: reqwest::Client }
pub struct SearchResult { hits: Vec<Value>, query: String, processing_time_ms: u64, ... }
pub struct SearchSyncer { client: SearchClient, receiver: mpsc::Receiver<SearchSyncEvent> }
```

**Sync Architecture:** Change events flow through the main fan-out channel in main.rs:
```
DB Change → change_tx → fan-out task → search_sync_tx → SearchSyncer → Meilisearch HTTP API
```

**Index Configuration:** Indexes defined in `orignabase.toml` under `[search.indexes.<name>]` with `searchable`, `filterable`, `sortable` field lists. Applied at startup via `ensure_indexes()`.

**ID Sanitization:** PostgreSQL IDs contain `:` (e.g., `products:abc123`) — sanitized to `_` for Meilisearch index keys.

**Optional:** If `search` config section is absent, `SearchClient` operates in disabled mode (returns empty results, logs warnings). Server starts normally without Meilisearch.

**Dependencies:** ob-core, reqwest, serde_json

---

### ob-functions

**Purpose:** User-defined WASM functions with triggers, HTTP endpoints, and cron scheduling.

**Modules:**
- `runtime.rs` — `WasmRuntime` (wasmi engine with fuel limits)
- `registry.rs` — `FunctionRegistry`, `TriggerType`
- `routes.rs` — `/functions/*` endpoints, `FunctionsState`
- `triggers.rs` — `DbTriggerExecutor` (executes functions on DB changes)
- `scheduler.rs` — `CronScheduler` (cron-based function execution)

**WasmRuntime:**
- Uses wasmi interpreter (lightweight, no JIT/cranelift overhead)
- Fuel limit: 1B instructions per invocation
- Wall-clock timeout: 30s
- Functions export `alloc(i32) -> i32` and `{name}(i32, i32) -> i64`
- Runs in `tokio::task::spawn_blocking` since wasmi is synchronous

**Triggers:**
- Database triggers: fire on Create/Update/Delete events per collection
- HTTP triggers: catch-all `/fn/{*path}` routes
- Cron triggers: scheduled via cron expressions

**Dependencies:** ob-core, ob-database, wasmi, cron, notify (file watcher)

---

### ob-analytics

**Purpose:** Privacy-first analytics event tracking with daily rollups.

**Modules:**
- `event.rs` — `AnalyticsEvent`, `DailyRollup`
- `retention.rs` — `RetentionPolicy` (auto-deletes events older than N days)
- `routes.rs` — `/analytics/event` ingestion endpoint, `AnalyticsState`

**Key Types:**
```rust
pub struct AnalyticsEvent {
    pub event: String,
    pub visitor_hash: String,  // SHA-256 hashed, no PII
    pub properties: Value,
    pub path: Option<String>,
    pub referrer: Option<String>,
    pub country: Option<String>,
    pub device: Option<String>,
    pub browser: Option<String>,
}
```

**Privacy:** Visitor identifiers are SHA-256 hashed with server-side salt (reuses `auth.jwt_secret`). No raw IPs or PII stored.

**Retention:** `RetentionPolicy` spawns a background task that deletes events older than 90 days (configurable).

**Dependencies:** ob-core, ob-database, sha2

---

### ob-admin

**Purpose:** Server administration — dashboard, collection management, config API, index management, usage metrics.

**Modules:**
- `routes.rs` — All `/_admin/*` endpoints, HTML dashboard
- `schema.rs` — Collection schema introspection and creation

**Key Endpoints:**
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/_admin/health` | Health check (version, timestamp) |
| GET | `/_admin/collections` | List all tables |
| POST | `/_admin/collections` | Create collection |
| GET | `/_admin/config` | Public config keys |
| POST | `/_admin/indexes` | Create index |
| GET | `/_admin/dashboard` | HTML dashboard |
| GET | `/_admin/metrics` | Usage metrics |

**Dashboard:** Embedded HTML (`include_str!("dashboard.html")`) served as single-page admin UI.

**Dependencies:** ob-core, ob-database

---

### ob-notifications

**Purpose:** Firebase Cloud Messaging (FCM) push notification proxy.

**Modules:**
- `routes.rs` — `/notifications/*` endpoints, `NotificationsState`, FCM OAuth2 token management

**Key Types:**
```rust
pub struct NotificationsState {
    pub db: DatabaseClient,
    pub fcm_project_id: Option<String>,
    pub fcm_service_account: Option<String>,
    pub http_client: reqwest::Client,
    fcm_token_cache: Arc<RwLock<CachedToken>>,
}
```

**FCM Flow:** Uses Google service account JWT → OAuth2 access token exchange. Token cached with 60s pre-expiry refresh. Supports device token registration, topic subscriptions, and targeted push sends.

**Dependencies:** ob-core, ob-database, reqwest

---

### ob-handlers

**Purpose:** Business logic handlers for the Origna GTA marketplace. Replaces all 114 Python Cloud Functions with native Rust handlers.

**Modules:**
- `payments/` — Stripe Checkout, PaymentIntent creation, webhook handling
- `orders/` — Order lifecycle (create, status transitions, shipping, refunds, returns)
- `products/` — Product CRUD, search, lifecycle management
- `chat/` — Realtime chat/messaging
- `coupons/` — Coupon validation, redemption
- `geocoding/` — Address geocoding
- `shipping_calc/` — Shipping rate calculation
- `users/` — User profile management
- `warehouses/` — Warehouse/inventory management
- `addresses/` — Address CRUD
- `digital/` — Digital product license management
- `pdf/` — Invoice/receipt PDF generation
- `push/` — Push notification dispatching
- `email/` — Transactional email sending
- `cron/` — Cron job definitions (auto-capture, cleanup, payout)
- `native_triggers/` — Native Rust trigger executor (replaces WASM triggers)
- `admin/` — Admin-specific operations
- `shared/` — Shared utilities (rate limiter, schema constants)

**Key Types:**
```rust
pub struct HandlersState {
    pub config: Arc<Config>,
    pub db: DatabaseClient,
    pub http_client: reqwest::Client,
    pub stripe_client: Option<Arc<stripe::Client>>,
    pub stripe_base_url: String,
    pub turnstile_secret_key: Option<String>,
}
```

**Middleware:** `enforce_actor_identity_middleware` validates `userId`/`sellerId` in request bodies match the authenticated user (or admin). Rejects unauthenticated identity claims.

**Stripe Integration:**
- Secret key from `config.secrets["stripe_secret_key"]`
- Base URL configurable (defaults to `https://api.stripe.com/v1`)
- Operations: Checkout Session, PaymentIntent, Refund, Transfer (Connect), webhook signature verification

**Rate Limiting:** `check_user_rate_limit()` uses PostgreSQL-backed sliding window rate limiter per user + action.

**Router Assembly:** `handlers_router(state)` merges all domain routers into a single axum Router with actor identity middleware.

**Dependencies:** ob-core, ob-database, ob-auth, ob-storage, reqwest, stripe

---

### ob-mcp

**Purpose:** Model Context Protocol (MCP) server exposing marketplace operations as AI-agent tools.

**Modules:**
- `server.rs` — `OrignaGtaMcp` MCP server
- `tools/` — Tool definitions (search, cart, orders, admin)
- `auth.rs` — JWT authentication for MCP connections
- `safeguards.rs` — Idempotency keys, confirmation tokens, spend limits
- `transport.rs` — JSON-RPC 2.0 over HTTP/SSE and stdio
- `errors.rs` — MCP-specific error types

**Key Types:**
```rust
pub struct McpState {
    pub db: Arc<DatabaseClient>,
    pub search: Option<Arc<SearchClient>>,
    pub config: Arc<Config>,
    pub jwt_keys: Arc<JwtKeys>,
}
```

**Transport:** Supports HTTP/SSE (production) and stdio (local development).

**Dependencies:** ob-core, ob-database, ob-auth, ob-search

---

### orignabase (binary)

**Purpose:** Main binary — CLI entry point, server assembly, route composition.

**Subcommands (clap):**
| Command | Purpose |
|---------|---------|
| `serve` | Start the server |
| `config` | Print current configuration |
| `login` | Authenticate with remote server |
| `logout` | Remove stored credentials |
| `whoami` | Show login status |
| `status` | Check server health |
| `init` | Initialize new project (orignabase.toml, rules.ob, indexes.toml) |
| `schema {inspect,create,up,down,indexes}` | Schema migration management |
| `users {list,get}` | User management |
| `backup` | Export database to JSON |
| `restore` | Import database from JSON |
| `migrate from-firebase` | Migrate from Firestore JSON export |
| `codegen dart` | Generate Dart models from GraphQL introspection |

**Server Assembly (serve function):**
1. Connect to PostgreSQL
2. Parse security rules file
3. Initialize SubscriptionRegistry + ChangeDispatcher
4. Initialize SearchClient + SearchSyncer
5. Initialize WASM runtime + FunctionRegistry
6. Build fan-out channel for change events
7. Build GraphQL schema with limits
8. Initialize JWT keys (RS256 auto-generate or HS256 fallback)
9. Initialize AuthState, StorageState, FunctionsState, AnalyticsState, NotificationsState, AdminState, HandlersState
10. Configure rate limiting (governor) — auth: 10 req/60s per IP, API: 100 req/60s per IP
11. Assemble axum Router with all sub-routers
12. Apply tower middleware stack (CORS, compression, timeout, tracing, panic catching, security headers)
13. Bind and serve

**Router Composition:**
```
Router
├── GET /health
├── GET /graphql (GraphiQL)
├── POST /graphql
├── /auth/* (auth_router with stricter rate limiting)
├── /realtime (WebSocket)
├── /storage/* (storage_router)
├── /functions/* (functions_router)
├── /analytics/* (analytics_router)
├── /notifications/* (notifications_router)
├── /_admin/* (admin_router)
├── /payments, /orders, /products, /chat, ... (handlers_router)
├── /fn/* (WASM HTTP triggers)
└── /static/* (static file hosting)
```

**Middleware Stack (outermost first):**
1. Tenant middleware (header/subdomain extraction)
2. Auth extractor (JWT verification)
3. CORS
4. Tracing
5. Timeout (30s)
6. Compression (gzip)
7. Security headers (X-Content-Type-Options: nosniff, X-Frame-Options: DENY)
8. CatchPanic
9. DefaultBodyLimit (2MB)
10. API rate limiting (tower-governor, PeerIpKeyExtractor)

**Dependencies:** all workspace crates, axum, tower, tower-http, tower-governor, clap, tracing-subscriber

## Decision Log

### 1. PostgreSQL v2 Required (Not v14)
PostgreSQL v14+ is required. The codebase uses `sqlx` with the `postgres` feature for connection pooling and query execution.

### 2. PostgreSQL Connection: `host:port` Format
The PostgreSQL client endpoint must be in `host:port` format (e.g., `localhost:5432`). Using incorrect format causes connection failures.

### 3. PostgreSQL RecordId: Use `Record` Wrapper with `#[serde(flatten)]`
Direct deserialization of PostgreSQL responses into `Value` loses the `id` field because `RecordId` doesn't serialize as a plain string. Solution:
```rust
struct Record {
    id: RecordId,
    #[serde(flatten)]
    rest: HashMap<String, Value>,
}
```
`#[serde(flatten)]` captures all other fields. `Record::into_value()` converts `RecordId` to string.

### 4. GraphQL Filters: OBJECT Format, Not Array
Server calls `filters.as_object()` expecting `{field: {_op: val}}`. Array format `[{"field": "x", "op": "eq"}]` silently returns no results. The Flutter SDK must send OBJECT format.

### 5. tower_governor: Use `PeerIpKeyExtractor` Behind Docker/Caddy
`SmartIpKeyExtractor` tries to read `X-Forwarded-For` but Docker's internal networking means the IP is always the Docker gateway. `PeerIpKeyExtractor` uses the raw TCP peer address, which works correctly with Caddy as the outermost reverse proxy.

### 6. Meilisearch Optional — Warns But Starts Without
If `[search]` section is absent from config or Meilisearch is unreachable, the server logs a warning and continues. `SearchClient::is_enabled()` returns false. All search operations return empty results. This avoids a hard dependency on an external service.

### 7. PostgreSQL for Local Dev and Production
`orignabase.toml` uses `url = "postgres://orignabase:orignabase_dev@localhost:5432/orignabase"` for local development. Production uses a managed PostgreSQL instance.

### 8. Serde Aliases for Backward Compatibility
Search result fields use multiple aliases for cross-version compatibility:
```rust
#[serde(rename = "processingTimeMs", alias = "processing_time_ms")]
pub processing_time_ms: u64,
```
This handles both Meilisearch v1.6 (camelCase) and v1.7+ (snake_case) response formats.

### 9. Release LTO OOMs on 8GB
Full LTO (`lto = true`, `codegen-units = 1`) causes out-of-memory on the 8GB Mac. Solution: add 4GB swap before release build (`sudo sysctl vm.swapusage`). The CI/CD pipeline builds on the VPS with more RAM.

### 10. Config Secrets Separation
Secrets (`stripe_secret_key`, `turnstile_secret_key`) are stored in `Config.secrets` (populated from `OB_SECRETS__*` env vars) rather than in named config fields. This avoids accidentally exposing secrets in the `orignabase config` command output (which only prints non-secret config).

## API Surface

### GraphQL Endpoint
- `GET /graphql` — GraphiQL UI (HTML)
- `POST /graphql` — GraphQL queries and mutations
- Auth: `Authorization: Bearer <jwt>` header (optional — anonymous access supported)
- Limits: max depth 12, max complexity 100, introspection disabled in production

### Auth Endpoints
| Method | Path | Purpose |
|--------|------|---------|
| POST | `/auth/register` | Email/password registration |
| POST | `/auth/login` | Email/password login |
| POST | `/auth/logout` | Invalidate session |
| POST | `/auth/refresh` | Refresh access token |
| POST | `/auth/mfa/setup` | Enable TOTP MFA |
| POST | `/auth/mfa/verify` | Verify TOTP code |
| POST | `/auth/magic-link/send` | Send magic link email |
| POST | `/auth/magic-link/verify` | Verify magic link token |
| POST | `/auth/password/reset` | Request password reset |
| POST | `/auth/password/reset/confirm` | Confirm password reset |
| POST | `/auth/email/verify` | Verify email address |
| GET | `/auth/oauth/google/start` | Google OAuth redirect |
| GET | `/auth/oauth/google/callback` | Google OAuth callback |
| POST | `/auth/oauth/apple/callback` | Apple Sign In callback |

### Storage Endpoints
| Method | Path | Purpose |
|--------|------|---------|
| POST | `/storage/presign/upload` | Get presigned upload URL |
| GET | `/storage/:path` | Download file |
| DELETE | `/storage/:path` | Delete file |
| POST | `/storage/resumable/start` | Start resumable upload |
| PATCH | `/storage/resumable/:id` | Upload chunk |

### Realtime WebSocket
- `ws://host:port/realtime?token=<jwt>`
- Protocol: JSON messages with `type` tag
- Max 100 subscriptions per connection

### MCP Endpoints
- `POST /mcp` — JSON-RPC 2.0 over HTTP
- `GET /mcp/sse` — Server-Sent Events transport

### Admin Endpoints
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/_admin/health` | Health check |
| GET | `/_admin/collections` | List tables |
| POST | `/_admin/collections` | Create table |
| GET | `/_admin/dashboard` | HTML dashboard |
| GET | `/_admin/config` | Public config |
| POST | `/_admin/indexes` | Create index |

### Handler Endpoints
REST endpoints for marketplace operations under `/payments`, `/orders`, `/products`, `/chat`, `/coupons`, `/geocoding`, `/shipping`, `/users`, `/warehouses`, `/addresses`, `/digital`.

## Testing Strategy

### Unit Tests (per crate)
- Run via `cargo test` (488+ across 13 crates)
- Use in-memory PostgreSQL for database tests
- Mock HTTP servers via `wiremock` for Stripe/external API tests
- Each crate has `#[cfg(test)] mod tests` inline

### Integration Tests
- Located in `crates/orignabase/tests/integration_test.rs`
- 50+ live tests requiring a running server
- Run via `cargo test -- --ignored` with `OB_TEST_URL` env var
- `OB_TEST_MODE=1` enables relaxed rate limits, test security rules, and permissive CORS

### Test Tags
- Default: unit tests only (fast)
- `--exclude-tags golden` for Flutter SDK tests
- Golden/visual tests run separately

### Proptest Validation
- Used for input validation fuzzing in `ob-core/validate.rs`
- Tests identifier validation, document ID validation, string escaping

### Snapshot Tests
- `insta` crate for snapshot testing of GraphQL responses and security rule evaluations

### Test Commands
```bash
cargo test                           # unit tests
cargo test -- --ignored             # integration (needs server)
cargo test -p ob-database           # single crate
cargo test --name "test_name"       # single test by name
cargo clippy -- -D warnings         # lint
```

### Pre-Deploy Checklist
```bash
cargo test && cargo clippy -- -D warnings
```

## Deployment

### Docker Setup
- Single-stage Rust build → minimal runtime image
- Entrypoint: `orignabase serve`
- Environment variables injected via Docker secrets or `.env`

### VPS Configuration
- **Server:** 204.168.137.16 (Hetzner)
- **Reverse Proxy:** Caddy (TLS termination, rate limiting at edge)
- **Database:** Standalone PostgreSQL (HTTP protocol)
- **Search:** Meilisearch instance
- **Storage:** Local filesystem at `/var/lib/orignabase/storage`

### Environment Variables (Critical)
| Variable | Purpose |
|----------|---------|
| `OB_AUTH__JWT_SECRET` | JWT signing secret (≥32 chars) |
| `OB_DATABASE__ENDPOINT` | PostgreSQL connection string |
| `OB_SECRETS__STRIPE_SECRET_KEY` | Stripe API key |
| `OB_SEARCH__URL` | Meilisearch URL |
| `OB_SEARCH__API_KEY` | Meilisearch API key |
| `OB_BASE_URL` | Public-facing base URL |
| `OB_FCM_PROJECT_ID` | Firebase project ID (optional) |
| `OB_ENABLE_INTROSPECTION` | Enable GraphQL introspection |
| `OB_TEST_MODE` | Enable test mode (relaxed limits) |

### Build Notes
- Release profile: `lto = true`, `codegen-units = 1`, `strip = true`
- On 8GB Mac: add 4GB swap before `cargo build --release`
- Dev profile: used for local builds to avoid OOM
