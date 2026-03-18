#!/usr/bin/env bash
set -euo pipefail
BASE_URL="${1:-https://api.orignagta.ca}"
SSH_HOST="${2:-root@204.168.137.16}"
SSH_KEY="${3:-$HOME/.ssh/id_ed25519}"

echo "=== Chaos: Kill Meilisearch ==="

# 1. Kill Meilisearch
ssh -i "$SSH_KEY" "$SSH_HOST" "docker stop meilisearch" || true
echo "Meilisearch stopped"
sleep 3

# 2. CRUD should still work (graceful degradation)
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/health")
echo "During outage: /health → $HTTP_CODE"
[ "$HTTP_CODE" = "200" ] || echo "WARN: Health degraded to $HTTP_CODE"

# 3. Restart
ssh -i "$SSH_KEY" "$SSH_HOST" "docker start meilisearch"
echo "Meilisearch restarted"
sleep 5

# 4. Verify recovery
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/health")
[ "$HTTP_CODE" = "200" ] || { echo "FAIL: Not recovered"; exit 1; }
echo "Recovery: healthy"
echo "=== PASS ==="
