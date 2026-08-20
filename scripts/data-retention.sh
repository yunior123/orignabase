#!/usr/bin/env bash
# data-retention.sh — TTL cleanup for PostgreSQL
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
#   - psql
#   - Access to PostgreSQL (localhost:5432 by default)

set -euo pipefail

ENV="${1:-dev}"

case "$ENV" in
  dev|staging|production) DB="${POSTGRES_DB:-orignabase}" ;;
  *)
    echo "Unknown env: $ENV (use dev|staging|production)"
    exit 1
    ;;
esac

PGHOST="${POSTGRES_HOST:-localhost}"
PGPORT="${POSTGRES_PORT:-5432}"
PGUSER="${POSTGRES_USER:-orignabase}"
PGPASSWORD="${POSTGRES_PASSWORD:-orignabase_dev}"
export PGPASSWORD

NOW_EPOCH=$(date +%s)
DAYS_90_AGO=$(( NOW_EPOCH - 90 * 86400 ))
DAYS_30_AGO=$(( NOW_EPOCH - 30 * 86400 ))

echo "[$(date -Iseconds)] Starting data retention cleanup (env=$ENV, db=$DB)"

# Delete webhook_events older than 90 days
echo "  Deleting webhook_events older than 90 days (before epoch $DAYS_90_AGO)..."
RESULT_WEBHOOKS=$(psql \
  --host "$PGHOST" \
  --port "$PGPORT" \
  --username "$PGUSER" \
  --dbname "$DB" \
  --tuples-only \
  --no-align \
  -c "DELETE FROM webhook_events WHERE timestamp < $DAYS_90_AGO;")
echo "  Result: $RESULT_WEBHOOKS"

# Delete _pending_notifications older than 30 days
echo "  Deleting _pending_notifications older than 30 days (before epoch $DAYS_30_AGO)..."
RESULT_NOTIFS=$(psql \
  --host "$PGHOST" \
  --port "$PGPORT" \
  --username "$PGUSER" \
  --dbname "$DB" \
  --tuples-only \
  --no-align \
  -c "DELETE FROM _pending_notifications WHERE created_at < to_timestamp($DAYS_30_AGO);")
echo "  Result: $RESULT_NOTIFS"

echo "[$(date -Iseconds)] Data retention cleanup complete."
