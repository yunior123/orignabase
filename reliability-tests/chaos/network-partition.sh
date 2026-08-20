#!/usr/bin/env bash
set -euo pipefail
BASE_URL="${1:-https://api.orignagta.ca}"
SSH_HOST="${2:-root@204.168.137.16}"
SSH_KEY="${3:-$HOME/.ssh/id_ed25519}"

echo "=== Chaos: Network Partition (PostgreSQL) ==="

# Get PostgreSQL container IP
DB_IP=$(ssh -i "$SSH_KEY" "$SSH_HOST" "docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' orignabase-postgres-1")
echo "PostgreSQL IP: $DB_IP"

# Block traffic from the dev app container to PostgreSQL
ssh -i "$SSH_KEY" "$SSH_HOST" "docker exec orignabase-orignabase-dev-1 sh -c 'apt-get update -qq && apt-get install -qq -y iptables > /dev/null 2>&1; iptables -A OUTPUT -d $DB_IP -j DROP'" || true
echo "Network partition active"
sleep 5

# Verify error handling
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 10 "$BASE_URL/health")
echo "During partition: /health → $HTTP_CODE"

# Remove partition
ssh -i "$SSH_KEY" "$SSH_HOST" "docker exec orignabase-orignabase-dev-1 iptables -D OUTPUT -d $DB_IP -j DROP" || true
echo "Network partition removed"
sleep 5

# Verify recovery
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/health")
[ "$HTTP_CODE" = "200" ] || { echo "FAIL: Not recovered after partition heal"; exit 1; }
echo "Recovery: healthy"
echo "=== PASS ==="
