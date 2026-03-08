# Chat App Example

A realtime chat app demonstrating OrignaBase WebSocket subscriptions, auth, and GraphQL.

## Setup

1. Start OrignaBase:
```bash
cd ../../docker && docker compose up -d
```

2. Create the `messages` collection:
```bash
curl -X POST http://localhost:8080/_admin/collections \
  -H "Content-Type: application/json" \
  -d '{
    "name": "messages",
    "fields": [
      {"name": "text", "field_type": "string", "required": true},
      {"name": "sender_id", "field_type": "string", "required": true},
      {"name": "sender_name", "field_type": "string"},
      {"name": "channel", "field_type": "string", "required": true, "indexed": true},
      {"name": "created_at", "field_type": "datetime"}
    ]
  }'
```

3. Add security rules in `rules.ob`:
```
rules messages {
    read: isAuthenticated();
    create: isAuthenticated();
    update: isOwner(resource.sender_id);
    delete: isOwner(resource.sender_id) || hasRole("admin");
}
```

## Usage

### Register Two Users

```bash
# User 1
curl -X POST http://localhost:8080/auth/register \
  -H "Content-Type: application/json" \
  -d '{"email": "alice@example.com", "password": "securepass123"}'

TOKEN_ALICE=$(curl -s -X POST http://localhost:8080/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email": "alice@example.com", "password": "securepass123"}' | jq -r '.access_token')

# User 2
curl -X POST http://localhost:8080/auth/register \
  -H "Content-Type: application/json" \
  -d '{"email": "bob@example.com", "password": "securepass123"}'

TOKEN_BOB=$(curl -s -X POST http://localhost:8080/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email": "bob@example.com", "password": "securepass123"}' | jq -r '.access_token')
```

### Subscribe to Realtime Updates (Terminal 1)

```bash
# Connect via WebSocket (using websocat or similar)
# Install: cargo install websocat
websocat ws://localhost:8080/realtime

# Send subscription message:
{"type": "subscribe", "collection": "messages", "filter": {"channel": "general"}}
```

### Send Messages (Terminal 2)

```bash
# Alice sends a message
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN_ALICE" \
  -d '{
    "query": "mutation { create(collection: \"messages\", data: {text: \"Hello everyone!\", sender_name: \"Alice\", channel: \"general\"}) }"
  }'

# Bob replies
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN_BOB" \
  -d '{
    "query": "mutation { create(collection: \"messages\", data: {text: \"Hey Alice!\", sender_name: \"Bob\", channel: \"general\"}) }"
  }'
```

### Query Message History

```bash
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN_ALICE" \
  -d '{
    "query": "query { list(collection: \"messages\", limit: 50, orderBy: \"created_at\", descending: false, filter: {channel: {_eq: \"general\"}}) }"
  }'
```

## What This Demonstrates

- Multi-user authentication
- Realtime WebSocket subscriptions
- Collection with indexed fields for fast filtering
- Message history via GraphQL queries
- Owner-based edit/delete permissions
