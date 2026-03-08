#!/usr/bin/env bash
# End-to-end test for OrignaBase Todo App example
# Prerequisites: OrignaBase running at localhost:8080, jq installed
set -uo pipefail

BASE_URL="${OB_URL:-http://localhost:8080}"
PASS=0
FAIL=0

check() {
    local desc="$1" expected="$2" actual="$3"
    if echo "$actual" | grep -q "$expected"; then
        echo "  PASS: $desc"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: $desc (expected '$expected', got '$actual')"
        FAIL=$((FAIL + 1))
    fi
}

echo "=== OrignaBase Todo App E2E Test ==="
echo "Target: $BASE_URL"
echo

# 1. Health check
echo "[1] Health check"
HEALTH=$(curl -s "$BASE_URL/health" || echo "FAILED")
check "health endpoint" "ok" "$HEALTH"

# 2. Register user
echo "[2] Register user"
TEST_EMAIL="todo-test-$(date +%s)@example.com"
REG=$(curl -s -X POST "$BASE_URL/auth/register" \
  -H "Content-Type: application/json" \
  -d "{\"email\": \"$TEST_EMAIL\", \"password\": \"testpass123\"}" || echo "FAILED")
check "registration" "access_token" "$REG"

# 3. Login (use access_token from registration directly)
echo "[3] Login"
TOKEN=$(echo "$REG" | jq -r '.access_token // empty' 2>/dev/null)
if [ -n "$TOKEN" ] && [ "$TOKEN" != "null" ]; then
    echo "  PASS: got access token"
    PASS=$((PASS + 1))
else
    # Try explicit login
    LOGIN=$(curl -s -X POST "$BASE_URL/auth/login" \
      -H "Content-Type: application/json" \
      -d "{\"email\": \"$TEST_EMAIL\", \"password\": \"testpass123\"}" || echo "FAILED")
    TOKEN=$(echo "$LOGIN" | jq -r '.access_token // empty' 2>/dev/null)
    if [ -n "$TOKEN" ] && [ "$TOKEN" != "null" ]; then
        echo "  PASS: got access token via login"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: no access token"
        FAIL=$((FAIL + 1))
    fi
fi

# 4. Create collection
echo "[4] Create todos collection"
COL=$(curl -s -X POST "$BASE_URL/_admin/collections" \
  -H "Content-Type: application/json" \
  -d '{"name": "todos_test", "fields": [{"name": "title", "field_type": "string", "required": true}, {"name": "completed", "field_type": "bool"}]}' || echo "FAILED")
check "create collection" "todos_test" "$COL"

# 5. Create a todo via GraphQL
echo "[5] Create todo"
CREATE=$(curl -s -X POST "$BASE_URL/graphql" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"query": "mutation { create(collection: \"todos_test\", data: {title: \"Test todo\", completed: false}) }"}' || echo "FAILED")
check "create todo" "data" "$CREATE"

# 6. List todos
echo "[6] List todos"
LIST=$(curl -s -X POST "$BASE_URL/graphql" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"query": "query { list(collection: \"todos_test\", limit: 10) }"}' || echo "FAILED")
check "list todos" "data" "$LIST"

# 7. Analytics event
echo "[7] Track analytics event"
ANALYTICS=$(curl -s -X POST "$BASE_URL/analytics/event" \
  -H "Content-Type: application/json" \
  -d '{"event": "todo_created", "properties": {"source": "test"}}' || echo "FAILED")
check "analytics" "ok" "$ANALYTICS"

# 8. Admin health
echo "[8] Admin health"
ADMIN=$(curl -s "$BASE_URL/_admin/health" || echo "FAILED")
check "admin health" "ok" "$ADMIN"

# 9. List collections
echo "[9] List collections"
COLS=$(curl -s "$BASE_URL/_admin/collections" || echo "FAILED")
check "list collections" "collections" "$COLS"

# 10. Clean up — drop test collection
echo "[10] Drop test collection"
DROP=$(curl -s -X DELETE "$BASE_URL/_admin/collections/todos_test" || echo "FAILED")
check "drop collection" "dropped" "$DROP"

echo
echo "=== Results: $PASS passed, $FAIL failed ==="
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
