#!/usr/bin/env bash
# data-retention.sh — TTL cleanup for SurrealDB
#
# Policy:
#   - webhook_events: DELETE after 90 days
#   - _pending_notifications: DELETE after 30 days
#
# Usage:
#   ./data-retention.sh [env]
#   env: dev | staging | production (default: dev)
#
# Schedule via cron:
#   0 3 * * * /opt/orignabase/scripts/data-retention.sh production >> /var/log/data-retention.log 2>&1
#
# Requirements:
#   - curl
#   - Access to SurrealDB HTTP endpoint (localhost:8000)
#   - SurrealDB credentials (root / orignabase_root_2026)

set -euo pipefail

ENV="${1:-dev}"

case "$ENV" in
  dev)        DB="dev" ;;
  staging)    DB="staging" ;;
  production) DB="main" ;;
  *)
    echo "Unknown env: $ENV (use dev|staging|production)"
    exit 1
    ;;
esac

SURREAL_URL="http://localhost:8000/sql"
SURREAL_NS="orignabase"
SURREAL_DB="$DB"
SURREAL_USER="root"
SURREAL_PASS="orignabase_root_2026"

NOW_EPOCH=$(date +%s)
DAYS_90_AGO=$(( NOW_EPOCH - 90 * 86400 ))
DAYS_30_AGO=$(( NOW_EPOCH - 30 * 86400 ))

echo "[$(date -Iseconds)] Starting data retention cleanup (env=$ENV, db=$DB)"

# Delete webhook_events older than 90 days
echo "  Deleting webhook_events older than 90 days (before epoch $DAYS_90_AGO)..."
RESULT_WEBHOOKS=$(curl -sf -X POST "$SURREAL_URL" \
  -H "Accept: application/json" \
  -H "NS: $SURREAL_NS" \
  -H "DB: $SURREAL_DB" \
  -u "$SURREAL_USER:$SURREAL_PASS" \
  -d "DELETE FROM webhook_events WHERE timestamp < $DAYS_90_AGO;")
echo "  Result: $RESULT_WEBHOOKS"

# Delete _pending_notifications older than 30 days
echo "  Deleting _pending_notifications older than 30 days (before epoch $DAYS_30_AGO)..."
RESULT_NOTIFS=$(curl -sf -X POST "$SURREAL_URL" \
  -H "Accept: application/json" \
  -H "NS: $SURREAL_NS" \
  -H "DB: $SURREAL_DB" \
  -u "$SURREAL_USER:$SURREAL_PASS" \
  -d "DELETE FROM _pending_notifications WHERE createdAt < $DAYS_30_AGO;")
echo "  Result: $RESULT_NOTIFS"

echo "[$(date -Iseconds)] Data retention cleanup complete."
