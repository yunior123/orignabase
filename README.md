# OrignaBase

A lightweight, blazingly fast, self-hosted Backend-as-a-Service. Firebase/Supabase alternative built in Rust.

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
- **SurrealDB** — Multi-model database (document + graph), LIVE queries
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
# Prerequisites: Rust 1.85+, SurrealDB v2 running on localhost:8000

cargo run -- serve
```

### Configuration

Configure via `orignabase.toml` or environment variables:

```bash
OB_HOST=0.0.0.0
OB_PORT=8080
OB_DATABASE__ENDPOINT=ws://localhost:8000
OB_DATABASE__USERNAME=root
OB_DATABASE__PASSWORD=root
OB_AUTH__JWT_SECRET=your-secret-here
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
| `GET /_admin/health` | Admin health check |
| `POST /_admin/collections` | Create collection |
| `GET /_admin/collections` | List collections |

## Architecture

```
orignabase (single binary)
├── ob-core       — Config, AppState, server assembly
├── ob-database   — SurrealDB client, CRUD, query translator
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

## License

Dual-licensed under [Apache 2.0](LICENSE-APACHE) and [MIT](LICENSE-MIT).
