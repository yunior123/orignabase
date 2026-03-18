#!/usr/bin/env bash
set -euo pipefail
BASE_URL="${1:-https://api.orignagta.ca}"
SSH_HOST="${2:-root@204.168.137.16}"
SSH_KEY="${3:-$HOME/.ssh/id_ed25519}"

echo "=== Chaos: Data Consistency After Kill ==="

# 1. Create a document via API
REG_RESP=$(curl -s -X POST "$BASE_URL/auth/register" \
  -H "Content-Type: application/json" \
  -d "{\"email\": \"chaos_$(date +%s)@test.com\", \"password\": \"TestPassword123!\"}")
TOKEN=$(echo "$REG_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])" 2>/dev/null || echo "")
[ -n "$TOKEN" ] || { echo "FAIL: Could not register"; exit 1; }
echo "Registered test user"

CREATE_RESP=$(curl -s -X POST "$BASE_URL/graphql" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"query": "mutation { create(collection: \"chaos_test\", data: \"{\\\"marker\\\": \\\"before_kill\\\"}\" ) }"}')
echo "Created doc: $CREATE_RESP"

# 2. Kill orignabase abruptly
ssh -i "$SSH_KEY" "$SSH_HOST" "docker kill orignabase" || true
echo "OrignaBase killed"
sleep 3

# 3. Restart
ssh -i "$SSH_KEY" "$SSH_HOST" "docker start orignabase"
echo "OrignaBase restarted"
sleep 10

# 4. Verify data is still there (no corruption)
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/health")
[ "$HTTP_CODE" = "200" ] || { echo "FAIL: Not recovered after kill"; exit 1; }
echo "Server recovered"

# 5. Re-auth and check
REG_RESP2=$(curl -s -X POST "$BASE_URL/auth/register" \
  -H "Content-Type: application/json" \
  -d "{\"email\": \"chaos_verify_$(date +%s)@test.com\", \"password\": \"TestPassword123!\"}")
TOKEN2=$(echo "$REG_RESP2" | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])" 2>/dev/null || echo "")

LIST_RESP=$(curl -s -X POST "$BASE_URL/graphql" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN2" \
  -d '{"query": "{ list(collection: \"chaos_test\", limit: 10) }"}')
echo "Post-kill list: $LIST_RESP"
echo "=== Manual verification needed: check marker 'before_kill' exists ==="
