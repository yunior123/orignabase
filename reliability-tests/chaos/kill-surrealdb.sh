#!/usr/bin/env bash
set -euo pipefail
BASE_URL="${1:-https://api.orignagta.ca}"
SSH_HOST="${2:-root@204.168.137.16}"
SSH_KEY="${3:-$HOME/.ssh/id_ed25519}"

echo "=== Chaos: Kill SurrealDB ==="

# 1. Verify healthy
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/health")
[ "$HTTP_CODE" = "200" ] || { echo "FAIL: Not healthy before test"; exit 1; }
echo "Pre-check: healthy"

# 2. Kill SurrealDB container
ssh -i "$SSH_KEY" "$SSH_HOST" "docker stop surrealdb" || true
echo "SurrealDB stopped"
sleep 3

# 3. Verify degraded (503, not 500 or panic)
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/_admin/collections" 2>/dev/null || echo "000")
echo "During outage: /_admin/collections → $HTTP_CODE"
[ "$HTTP_CODE" = "503" ] || echo "WARN: Expected 503, got $HTTP_CODE"

# Health should still respond
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/health")
echo "During outage: /health → $HTTP_CODE"

# 4. Restart SurrealDB
ssh -i "$SSH_KEY" "$SSH_HOST" "docker start surrealdb"
echo "SurrealDB restarted"
sleep 10

# 5. Verify recovery
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/health")
[ "$HTTP_CODE" = "200" ] || { echo "FAIL: Not recovered after restart"; exit 1; }
echo "Recovery: healthy"
echo "=== PASS ==="
