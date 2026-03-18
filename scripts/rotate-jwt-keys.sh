#!/bin/bash
# JWT Key Rotation Script — Run quarterly via cron
# Rotates JWT signing keys on VPS, archives old ones, keeps last 4 backups
# Usage: ./rotate-jwt-keys.sh
# Cron: 0 3 1 */3 * /opt/orignabase/scripts/rotate-jwt-keys.sh
# (3 AM on 1st of every 3rd month: Jan, Apr, Jul, Oct)

set -euo pipefail

# Configuration
KEYS_DIR="/opt/orignabase/data/keys"
LOG_FILE="/var/log/orignabase/jwt-rotation.log"
WEBHOOK_URL="${ROTATION_WEBHOOK_URL:-}"  # Optional: webhook to notify on rotation

# Ensure log directory exists
mkdir -p "$(dirname "$LOG_FILE")"

# Logging function
log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" | tee -a "$LOG_FILE"
}

log "Starting JWT key rotation..."

# Verify keys directory exists
if [ ! -d "$KEYS_DIR" ]; then
    log "ERROR: Keys directory not found: $KEYS_DIR"
    exit 1
fi

# Backup current keys
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_PRIVATE="$KEYS_DIR/jwt_private_${TIMESTAMP}.pem.bak"
BACKUP_PUBLIC="$KEYS_DIR/jwt_public_${TIMESTAMP}.pem.bak"

if [ -f "$KEYS_DIR/jwt_private.pem" ] && [ -f "$KEYS_DIR/jwt_public.pem" ]; then
    cp "$KEYS_DIR/jwt_private.pem" "$BACKUP_PRIVATE" || {
        log "ERROR: Failed to backup private key"
        exit 1
    }
    cp "$KEYS_DIR/jwt_public.pem" "$BACKUP_PUBLIC" || {
        log "ERROR: Failed to backup public key"
        exit 1
    }
    log "Backed up current keys to:"
    log "  Private: $BACKUP_PRIVATE"
    log "  Public:  $BACKUP_PUBLIC"
else
    log "WARNING: Current keys not found, will generate new pair"
fi

# Generate new RS256 key pair (2048-bit)
log "Generating new RS256 key pair..."
if ! openssl genpkey -algorithm RSA -out "$KEYS_DIR/jwt_private.pem" -pkeyopt rsa_keygen_bits:2048 2>> "$LOG_FILE"; then
    log "ERROR: Failed to generate private key"
    exit 1
fi

if ! openssl rsa -in "$KEYS_DIR/jwt_private.pem" -pubout -out "$KEYS_DIR/jwt_public.pem" 2>> "$LOG_FILE"; then
    log "ERROR: Failed to extract public key"
    exit 1
fi

log "New keys generated successfully"

# Restart OrignaBase services to pick up new keys
log "Restarting OrignaBase services..."
cd /opt/orignabase || exit 1

if ! docker compose restart orignabase-dev orignabase-staging orignabase-prod >> "$LOG_FILE" 2>&1; then
    log "ERROR: Failed to restart services"
    exit 1
fi

log "Services restarted successfully"

# Wait for services to be ready
sleep 5

# Cleanup old backups: keep only last 4
log "Cleaning up old backups (keeping last 4)..."
BACKUP_COUNT=$(ls -1 "$KEYS_DIR"/jwt_*.pem.bak 2>/dev/null | wc -l)
if [ "$BACKUP_COUNT" -gt 4 ]; then
    # Remove oldest backups
    ls -1t "$KEYS_DIR"/jwt_*.pem.bak | tail -n +5 | xargs -r rm -v >> "$LOG_FILE"
    log "Removed old backups, kept 4 most recent"
fi

# Optional: Send webhook notification
if [ -n "$WEBHOOK_URL" ]; then
    PAYLOAD=$(cat <<WEBHOOK
{
    "event": "jwt_keys_rotated",
    "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
    "keys_dir": "$KEYS_DIR",
    "backup_private": "$BACKUP_PRIVATE",
    "backup_public": "$BACKUP_PUBLIC"
}
WEBHOOK
    )
    if curl -s -X POST "$WEBHOOK_URL" -H "Content-Type: application/json" -d "$PAYLOAD" >> "$LOG_FILE" 2>&1; then
        log "Webhook notification sent"
    else
        log "WARNING: Failed to send webhook notification"
    fi
fi

log "JWT key rotation completed successfully"
log "Next scheduled rotation: $(date -d '+3 months' '+%Y-%m-%d 03:00:00')"
log "See $LOG_FILE for details"
