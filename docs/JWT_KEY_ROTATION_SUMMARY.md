# JWT Key Rotation — Implementation Summary

## What Was Implemented

Complete quarterly JWT key rotation system for OrignaBase Rust backend with zero-downtime rolling updates and fallback verification.

## Files Created/Modified

### New Files
1. **`crates/ob-auth/src/key_rotation.rs`** (155 lines)
   - `KeyRotationManager`: Tracks current + previous keys (max 2 old)
   - `KeyMetadata`: Fingerprint, timestamp, is_current flag
   - SHA256 fingerprinting for key identification
   - Load/save metadata to JSON file
   - Unit tests included

2. **`scripts/rotate-jwt-keys.sh`** (130 lines, executable)
   - Quarterly rotation script for VPS cron
   - Generates new RSA 2048-bit key pair via OpenSSL
   - Archives old keys with timestamp
   - Restarts OrignaBase services (dev/staging/prod)
   - Cleanup: keeps last 4 backups
   - Optional webhook notifications
   - Structured logging to `/var/log/orignabase/jwt-rotation.log`

3. **`docs/JWT_KEY_ROTATION.md`** (comprehensive guide)
   - Architecture and design principles
   - API endpoint documentation
   - VPS setup and cron configuration
   - Manual rotation procedures
   - Disaster recovery playbook
   - Security considerations
   - Monitoring and alerting

### Modified Files
1. **`crates/ob-auth/src/jwt.rs`** (550 lines, updated)
   - `JwtKeys::Rsa` now includes `previous_decoding: Vec<DecodingKey>`
   - New method: `from_rsa_pem_with_rotation()` to load with previous keys
   - `verify_token()` now tries current key first, then previous keys (fallback)
   - New function: `rotate_keys()` to generate new pair, archive old, update metadata
   - New function: `cleanup_old_backups()` to remove old archives
   - All existing tests pass, new rotation tests added

2. **`crates/ob-auth/src/lib.rs`** (updated exports)
   - Export: `rotate_keys`, `KeyRotationManager`, `fingerprint_public_key`

3. **`crates/ob-admin/src/routes.rs`** (updated)
   - New admin handlers:
     - `rotate_jwt_keys()`: POST /_admin/jwt/rotate (admin-only)
     - `jwt_key_status()`: GET /_admin/jwt/status (admin-only)
   - Routes registered in `admin_router()`

## How It Works

### Token Lifecycle
```
Generation:  issue_*_token() → signed with CURRENT key
Verification: verify_token() → tries CURRENT, then PREVIOUS[0], then PREVIOUS[1]
```

### Key Rotation Process
1. Admin calls `POST /_admin/jwt/rotate` or cron runs `/opt/orignabase/scripts/rotate-jwt-keys.sh`
2. System archives current keys: `jwt_private_TIMESTAMP.pem.bak`
3. Generates new RS256 2048-bit key pair
4. Updates rotation metadata: current → previous[0], new → current
5. Cleans up: keeps last 4 backups
6. Restarts services (zero downtime — new tokens use new key, old tokens verified with fallback)

### Metadata Tracking
File: `/opt/orignabase/data/keys/key_rotation.json`
```json
{
  "current_key_metadata": {
    "created_at": "2026-03-18T03:00:00Z",
    "fingerprint": "a1b2c3d4e5f6g7h8",
    "is_current": true
  },
  "previous_keys_metadata": [
    {"created_at": "...", "fingerprint": "...", "is_current": false},
    {"created_at": "...", "fingerprint": "...", "is_current": false}
  ]
}
```

## API Usage

### Rotate Keys (Admin)
```bash
curl -X POST https://api.orignagta.ca/_admin/jwt/rotate \
  -H "Authorization: Bearer $ADMIN_TOKEN"

# Response
{
  "status": "rotated",
  "new_fingerprint": "a1b2c3d4e5f6g7h8",
  "timestamp": "2026-03-18T03:00:00Z",
  "message": "New JWT keys generated. Tokens signed before rotation remain valid."
}
```

### Check Key Status (Admin)
```bash
curl https://api.orignagta.ca/_admin/jwt/status \
  -H "Authorization: Bearer $ADMIN_TOKEN"

# Response
{
  "current_key": {...},
  "previous_keys": [...],
  "total_keys": 3
}
```

## VPS Deployment

### Setup (One-time)
```bash
# Copy script to VPS
scp -i ~/.ssh/id_ed25519 scripts/rotate-jwt-keys.sh root@204.168.137.16:/opt/orignabase/scripts/

# SSH in and add cron
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16
crontab -e
# Add: 0 3 1 1,4,7,10 * /opt/orignabase/scripts/rotate-jwt-keys.sh
```

### Manual Rotation (Testing)
```bash
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 \
  '/opt/orignabase/scripts/rotate-jwt-keys.sh'

# Check log
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 \
  'tail -50 /var/log/orignabase/jwt-rotation.log'
```

## Security Properties

- **Key strength**: RSA 2048-bit (NIST FIPS 140-2)
- **Rotation frequency**: Quarterly (90-day grace period with 2 previous keys)
- **Token coverage**: Access tokens (15m) renewed before each rotation; refresh (7d) covered by fallback; verification links (24h-15m) all covered
- **Backup security**: Owned by root, archived with timestamp, kept for audit trail
- **Audit trail**: All rotations logged with fingerprints and timestamps
- **Zero downtime**: New/old services can coexist; new tokens use new key, old tokens verified with fallback

## Testing

### Unit Tests
```bash
cd crates/ob-auth
cargo test key_rotation::tests
cargo test jwt::tests
```

### Integration Test (Dev)
```bash
# 1. Get admin token
TOKEN=$(curl -s -X POST https://api.dev.orignagta.ca/auth/login \
  -d '{"email":"e2e-admin@test.origna.ca","password":"TestPass123!"}' | jq -r .access_token)

# 2. Check initial status
curl https://api.dev.orignagta.ca/_admin/jwt/status \
  -H "Authorization: Bearer $TOKEN" | jq .

# 3. Rotate
curl -X POST https://api.dev.orignagta.ca/_admin/jwt/rotate \
  -H "Authorization: Bearer $TOKEN" | jq .

# 4. Verify new fingerprint
curl https://api.dev.orignagta.ca/_admin/jwt/status \
  -H "Authorization: Bearer $TOKEN" | jq .current_key.fingerprint
```

## Disaster Recovery

### Quick Restore (If Needed)
```bash
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 << 'SSH'
cd /opt/orignabase/data/keys
# Restore from most recent backup
cp jwt_private_20260318_030000.pem.bak jwt_private.pem
cp jwt_public_20260318_030000.pem.bak jwt_public.pem
# Restart
cd /opt/orignabase && docker compose restart orignabase-prod
SSH
```

## Next Steps (Optional)

1. **Deploy to prod** — Run cron setup on VPS
2. **Monitor** — Watch `/var/log/orignabase/jwt-rotation.log` for first rotation
3. **Enhance** — Add Slack/email webhook notifications
4. **Scale** — Database-backed metadata for multi-node deployments

## Files Summary

| File | Size | Purpose |
|------|------|---------|
| `key_rotation.rs` | 155 lines | Key metadata manager |
| `jwt.rs` | 550 lines | JWT module (updated with fallback verification) |
| `rotate-jwt-keys.sh` | 130 lines | VPS automation script |
| `JWT_KEY_ROTATION.md` | ~400 lines | Comprehensive documentation |
| `lib.rs` | Updated | Exports new rotation functions |
| `routes.rs` | Updated | Admin endpoints |

---
**Implementation complete and ready for testing/deployment.**
