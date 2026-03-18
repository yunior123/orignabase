#!/bin/bash
# SurrealDB backup script — run daily via cron
# Usage: ./scripts/backup.sh [--dry-run]
# Cron: 0 2 * * * /opt/orignabase/scripts/backup.sh >> /var/log/surrealdb_backup.log 2>&1

set -euo pipefail

DRY_RUN="${1:-}"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_DIR="${BACKUP_DIR:-/opt/backups/surrealdb}"
RETENTION_DAYS="${RETENTION_DAYS:-30}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] [INFO] $*"
}

log_error() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] [ERROR] $*" >&2
}

log_warn() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] [WARN] $*"
}

# Ensure backup directory exists
mkdir -p "$BACKUP_DIR"
chmod 700 "$BACKUP_DIR"

log_info "Starting SurrealDB backup"
log_info "Backup directory: $BACKUP_DIR"
log_info "Retention period: $RETENTION_DAYS days"

if [ -n "$DRY_RUN" ]; then
    log_warn "DRY-RUN mode enabled (no backups will be created)"
fi

# Function to export database
export_database() {
    local db_name="$1"
    local output_file="$BACKUP_DIR/${db_name}_${TIMESTAMP}.surql"

    log_info "Exporting database: $db_name"

    if [ -n "$DRY_RUN" ]; then
        log_info "DRY-RUN: Would export to $output_file"
        return 0
    fi

    # Use surreal CLI to export
    # Assumes surreal is in PATH or use full path
    if ! surreal export \
        --conn http://localhost:8000 \
        --user "${SURREALDB_USER:-root}" \
        --pass "${SURREALDB_PASS:-orignabase_root_2026}" \
        --ns orignabase \
        --db "$db_name" \
        "$output_file" 2>/dev/null; then
        log_error "Failed to export database: $db_name"
        return 1
    fi

    # Verify file was created and has content
    if [ ! -s "$output_file" ]; then
        log_error "Backup file is empty: $output_file"
        rm -f "$output_file"
        return 1
    fi

    local file_size=$(du -h "$output_file" | cut -f1)
    log_info "Successfully exported database: $db_name ($file_size)"
}

# Function to rotate old backups
rotate_backups() {
    log_info "Rotating old backups (older than $RETENTION_DAYS days)"

    if [ -n "$DRY_RUN" ]; then
        log_info "DRY-RUN: Would delete backups older than $RETENTION_DAYS days"
        find "$BACKUP_DIR" -name "*.surql" -mtime "+$RETENTION_DAYS" -type f
        return 0
    fi

    local deleted_count=0
    while IFS= read -r file; do
        if rm -f "$file"; then
            log_info "Deleted old backup: $(basename "$file")"
            ((deleted_count++))
        else
            log_warn "Failed to delete: $file"
        fi
    done < <(find "$BACKUP_DIR" -name "*.surql" -mtime "+$RETENTION_DAYS" -type f)

    if [ "$deleted_count" -gt 0 ]; then
        log_info "Rotation complete: deleted $deleted_count old backup(s)"
    else
        log_info "No old backups to delete"
    fi
}

# Main execution
main() {
    local success=true

    # Backup primary database
    if ! export_database "main"; then
        log_error "Primary database backup failed"
        success=false
    fi

    # Backup additional databases if configured
    for db in ${BACKUP_ADDITIONAL_DBS:-}; do
        if ! export_database "$db"; then
            log_warn "Additional database backup failed: $db"
            success=false
        fi
    done

    # Rotate old backups
    if ! rotate_backups; then
        log_error "Backup rotation failed"
        success=false
    fi

    if [ "$success" = true ]; then
        log_info "Backup completed successfully"
        exit 0
    else
        log_error "Backup completed with errors"
        exit 1
    fi
}

main
