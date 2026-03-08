# Todo App Example

A simple todo list demonstrating OrignaBase CRUD, auth, and security rules.

## Setup

1. Start OrignaBase:
```bash
cd ../../docker && docker compose up -d
```

2. Create the `todos` collection and security rules:
```bash
# Create collection with schema
curl -X POST http://localhost:8080/_admin/collections \
  -H "Content-Type: application/json" \
  -d '{
    "name": "todos",
    "fields": [
      {"name": "title", "field_type": "string", "required": true},
      {"name": "completed", "field_type": "bool"},
      {"name": "owner_id", "field_type": "string"},
      {"name": "created_at", "field_type": "datetime"}
    ]
  }'
```

3. Add security rules in `rules.ob`:
```
rules todos {
    read: isAuthenticated() && isOwner(resource.owner_id);
    create: isAuthenticated();
    update: isOwner(resource.owner_id);
    delete: isOwner(resource.owner_id);
}
```

## Usage

### Register & Login

```bash
# Register
curl -X POST http://localhost:8080/auth/register \
  -H "Content-Type: application/json" \
  -d '{"email": "alice@example.com", "password": "securepass123"}'

# Login — save the access_token
TOKEN=$(curl -s -X POST http://localhost:8080/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email": "alice@example.com", "password": "securepass123"}' | jq -r '.access_token')

echo "Token: $TOKEN"
```

### Create Todos

```bash
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "query": "mutation { create(collection: \"todos\", data: {title: \"Buy groceries\", completed: false}) }"
  }'

curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "query": "mutation { create(collection: \"todos\", data: {title: \"Write tests\", completed: false}) }"
  }'
```

### List Todos

```bash
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "query": "query { list(collection: \"todos\", limit: 50, orderBy: \"created_at\", descending: true) }"
  }'
```

### Update a Todo

```bash
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "query": "mutation { update(collection: \"todos\", id: \"TODO_ID_HERE\", data: {completed: true}) }"
  }'
```

### Delete a Todo

```bash
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "query": "mutation { delete(collection: \"todos\", id: \"TODO_ID_HERE\") }"
  }'
```

## What This Demonstrates

- User registration and JWT authentication
- Collection creation via Admin API
- CRUD operations via GraphQL
- Security rules (owner-based access control)
