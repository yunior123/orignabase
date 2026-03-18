#!/usr/bin/env bash
set -euo pipefail
BASE_URL="${1:-https://api.orignagta.ca}"
echo "Smoke testing $BASE_URL..."

# Health
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/health")
[ "$HTTP_CODE" = "200" ] || { echo "FAIL: /health returned $HTTP_CODE"; exit 1; }
echo "OK: /health -> 200"

# Auth register (bad request)
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$BASE_URL/auth/register" -H "Content-Type: application/json" -d '{}')
echo "$HTTP_CODE" | grep -qE "^(400|422)$" || { echo "FAIL: /auth/register empty -> $HTTP_CODE"; exit 1; }
echo "OK: /auth/register empty -> $HTTP_CODE"

# GraphQL introspection
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$BASE_URL/graphql" -H "Content-Type: application/json" -d '{"query":"{ __schema { types { name } } }"}')
[ "$HTTP_CODE" = "200" ] || { echo "FAIL: /graphql introspection -> $HTTP_CODE"; exit 1; }
echo "OK: /graphql introspection -> 200"

# Storage presign without auth
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$BASE_URL/storage/presign/upload" -H "Content-Type: application/json" -d '{"path":"test.txt","content_type":"text/plain"}')
echo "$HTTP_CODE" | grep -qE "^(401|400|422)$" || { echo "FAIL: /storage/presign/upload unauth -> $HTTP_CODE"; exit 1; }
echo "OK: /storage/presign/upload unauth -> $HTTP_CODE"

echo "All smoke tests passed!"
