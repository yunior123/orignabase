# OrignaBase

A lightweight, blazingly fast, self-hosted Backend-as-a-Service. Firebase/Supabase alternative built in Rust.

<p align="center">
  <img src="docs/images/orignabase-architecture.svg" alt="OrignaBase architecture overview" width="100%">
</p>

<p align="center"><strong>One Rust service for auth, data, realtime, storage, search, functions, analytics, and administration.</strong></p>

## Features

- **GraphQL API** — Auto-generated CRUD with filters, ordering, pagination
- **Authentication** — Email/password (Argon2id), JWT (access + refresh tokens)
- **Security Rules** — Custom DSL for fine-grained access control
- **Realtime** — WebSocket subscriptions with change tracking
- **File Storage** — Local filesystem with HMAC-signed URLs
- **Full-Text Search** — Meilisearch integration with auto-sync
- **Serverless Functions** — WASM runtime (wasmi) with fuel metering
- **Analytics** — Privacy-first event tracking (hashed IPs, no cookies)
- **Admin API** — Schema management, user management
- **PostgreSQL** — ACID-compliant relational database with full transaction support
- **Single Binary** — One process, all services, <30MB Docker image

## Quick Start

### Docker Compose (recommended)

```bash
cd docker
docker compose up
```

OrignaBase runs at `http://localhost:8080`. GraphiQL playground at `http://localhost:8080/graphql`.

### From Source

```bash
# Prerequisites: Rust 1.85+, PostgreSQL running on localhost:5432

source ./scripts/cargo-target-dir.sh
export_orignabase_cargo_target_dir dev
cargo run -- serve
```

## Rust Build Artifact Hygiene

OrignaBase now keeps Cargo build outputs in separate buckets under `target/` so normal development, tests, and coverage runs do not accumulate into one giant debug tree.

```bash
source ./scripts/cargo-target-dir.sh

export_orignabase_cargo_target_dir dev
cargo run -- serve

export_orignabase_cargo_target_dir test
cargo test --workspace

export_orignabase_cargo_target_dir coverage
./scripts/coverage.sh --html
```

Clean managed Rust artifacts when needed:

```bash
./scripts/clean_rust_artifacts.sh --all
```

Enforce a hard size cap for Rust build artifacts:

```bash
ORIGNABASE_MAX_TARGET_GB=30 ./scripts/check_rust_artifacts_size.sh
./scripts/install_git_hooks.sh
```

The installed `pre-push` hook blocks pushes if `target/` grows past the configured limit.

### Configuration

Configure via `orignabase.toml` or environment variables:

```bash
OB_HOST=0.0.0.0
OB_PORT=8080
OB_DATABASE__URL=postgres://orignabase:orignabase@localhost:5432/orignabase
OB_AUTH__JWT_SECRET=your-secret-here
OB_SECRETS__STRIPE_SECRET_KEY=sk_test_...
OB_SECRETS__STRIPE_WEBHOOK_SECRET=whsec_...
```

Handler integrations such as checkout, refunds, subscriptions, webhooks, shipping,
and email require process-level `OB_SECRETS__...` variables. Storing a Stripe key
in another system is not enough unless it is injected into the OrignaBase process
environment under the exact `OB_SECRETS__STRIPE_SECRET_KEY` name.

If Stripe CLI is already authenticated on your machine, you can source the real
test key directly into the OrignaBase process without committing it:

```bash
eval "$(./scripts/stripe-cli-env.sh test)"
```

## Security Rules

Define access control in `rules.ob`:

```
rules products {
    read: true;
    create: isAuthenticated() && hasRole("seller");
    update: isOwner(resource.seller_id) || hasRole("admin");
    delete: hasRole("admin");
}
```

Built-in helpers: `isAuthenticated()`, `isOwner(field)`, `hasRole(role)`.

## Auth API

```bash
# Register
curl -X POST http://localhost:8080/auth/register \
  -H "Content-Type: application/json" \
  -d '{"email": "user@example.com", "password": "securepass123"}'

# Login
curl -X POST http://localhost:8080/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email": "user@example.com", "password": "securepass123"}'

# Refresh
curl -X POST http://localhost:8080/auth/refresh \
  -H "Content-Type: application/json" \
  -d '{"refresh_token": "..."}'
```

## GraphQL API

```graphql
# List documents
query {
  list(collection: "products", limit: 20, orderBy: "created_at", descending: true)
}

# Create document
mutation {
  create(collection: "products", data: { title: "Widget", price: 29.99 })
}
```

## Endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /health` | Health check |
| `GET /graphql` | GraphiQL playground |
| `POST /graphql` | GraphQL API |
| `POST /auth/register` | Register new user |
| `POST /auth/login` | Login |
| `POST /auth/refresh` | Refresh token |
| `WS /realtime` | WebSocket subscriptions |
| `PUT /storage/upload/*` | Upload file (signed URL) |
| `GET /storage/download/*` | Download file (signed URL) |
| `POST /functions/deploy` | Deploy WASM function |
| `POST /functions/invoke/:name` | Invoke function |
| `POST /analytics/event` | Ingest analytics event |
| `GET /_admin/` | Admin dashboard (SPA) |
| `GET /_admin/health` | Admin health check |
| `POST /_admin/collections` | Create collection |
| `GET /_admin/collections` | List collections |

## Admin Dashboard

Access the built-in admin dashboard at `http://localhost:8080/_admin/`. Features:

- **Overview** — System health, version, collection/user/function counts
- **Collections** — Create, list, and drop database collections
- **Users** — View, manage roles, delete users
- **Functions** — View deployed WASM functions
- **Analytics** — Event tracking overview

The dashboard is embedded in the binary (no external files needed).

## Firebase Migration

Migrate from Firestore JSON exports:

```bash
# Dry run — see what would be migrated
orignabase migrate from-firebase --export-path ./firestore-export --dry-run

# Migrate specific collections
orignabase migrate from-firebase \
  --export-path ./firestore-export \
  --collections users,products,orders \
  --target-url http://localhost:8080

# Migrate all collections
orignabase migrate from-firebase --export-path ./firestore-export
```

Supports Firestore typed values (`stringValue`, `integerValue`, `arrayValue`, `mapValue`, etc.) and extracts document IDs from Firestore paths.

## Benchmarks

Run criterion benchmarks for hot paths:

```bash
cargo bench --bench core_benchmarks
```

Covers: query translation, security rules evaluation, rule parsing, Argon2id hashing, JWT issue/verify, signed URLs, analytics helpers.

## Architecture

```
orignabase (single binary)
├── ob-core       — Config, AppState, server assembly
├── ob-database   — PostgreSQL client, CRUD, query translator
├── ob-auth       — JWT, Argon2id, email/password auth
├── ob-graphql    — Dynamic GraphQL schema + resolvers
├── ob-security   — Rules DSL parser (pest) + evaluator
├── ob-realtime   — WebSocket subscriptions, change dispatcher
├── ob-storage    — File storage, HMAC-signed URLs
├── ob-search     — Meilisearch client + auto-sync
├── ob-functions  — WASM runtime (wasmi), function registry
├── ob-analytics  — Privacy-first event tracking
└── ob-admin      — Schema + user management API
```

## Flutter SDK

```dart
final ob = OrignaBase(url: 'http://localhost:8080');

// Auth
await ob.auth.signInWithEmail('user@example.com', 'password');

// Database — Firestore-like fluent API
final products = ob.collection('products');
final docs = await products
    .where('status', isEqualTo: 'active')
    .where('price', isGreaterThan: 1000)
    .orderBy('created_at', descending: true)
    .limit(20)
    .get();

// Realtime
ob.realtime.subscribe('products', onEvent: (event) => print(event));

// Storage
await ob.storage.upload('users/123/avatar.jpg', bytes);
```

See `sdks/flutter/orignabase/` for the full SDK.

## Horizontal Scaling

### PostgreSQL Connection Pooling

Use PgBouncer for connection pooling and horizontal scaling:

```bash
# Start PgBouncer in front of PostgreSQL
pgbouncer /etc/pgbouncer/pgbouncer.ini

# Point OrignaBase to PgBouncer
OB_DATABASE__URL=REDACTED_SECRET/orignabase orignabase serve
```

### Multi-Node Clustering (NATS JetStream)

Enable NATS JetStream for cross-node realtime sync:

```bash
OB_CLUSTER__ENABLED=true \
OB_CLUSTER__NATS_URL=nats://nats:4222 \
orignabase serve
```

Or in `orignabase.toml`:

```toml
[cluster]
enabled = true
nats_url = "nats://nats:4222"
# node_id = "node-1"  # Optional, auto-generated if not set
```

Build with cluster support: `cargo build -p orignabase --features cluster`

## Change Events And Native Triggers

Write operations emitted through GraphQL now produce richer change events with:

- `action`
- `collection`
- `document_id`
- `data`
- `before_data`
- `after_data`
- `timestamp`

That event stream fans out inside `orignabase` to:

- realtime WebSocket subscribers
- WASM DB triggers
- native Rust trigger handlers in `ob-handlers`
- search sync
- optional NATS JetStream cluster replication

Native Rust triggers currently cover:

- product create, update, and delete search-sync hooks
- order status, payment refund, item shipped, and item delivered notifications
- return status notifications
- stock notification cleanup after successful purchase
- perishable-order urgent seller alerts

Cluster behavior is intentional:

- local writes execute local side effects once
- remote cluster events are forwarded to realtime subscribers only
- remote events are not re-broadcast into native triggers or search sync, which avoids duplicate side effects

Current notification behavior:

- in-app notifications are written to `notifications`
- email attempts go through Postal when configured
- push attempts go through FCM when configured
- fallback records are written to `_mail_logs` and `_pending_notifications`

Idempotency and retention:

- notification-side trigger sends claim deterministic records in `webhook_events`
- Stripe webhooks also log to `webhook_events`
- these records now include `timestamp`, so the existing stale-webhook cleanup cron can purge them correctly

## CLI Commands

```bash
orignabase serve                    # Start server
orignabase config                   # Print configuration
orignabase migrate from-firebase    # Migrate from Firestore
```

## License

Dual-licensed under [Apache 2.0](LICENSE-APACHE) and [MIT](LICENSE-MIT).
