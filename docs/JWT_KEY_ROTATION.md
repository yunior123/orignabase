# JWT Key Rotation — OrignaBase Implementation

Implements quarterly JWT signing key rotation with fallback verification support. Tokens signed before rotation remain valid. Old keys are archived for audit trails and disaster recovery.

## Overview

### Design Principles
- **Zero downtime**: New keys are generated, old keys remain valid for verification
- **Fallback verification**: Token verification tries current key first, then falls back to up to 2 previous keys
- **Audit trail**: Key metadata (fingerprints, creation timestamps) stored in `key_rotation.json`
- **Cleanup**: Only keep 3 total keys (1 current + 2 previous), drop oldest beyond that
- **Backup**: Old keys archived with timestamp, keep last 4 backups

### Key Files
- `crates/ob-auth/src/key_rotation.rs` — Key rotation manager (metadata tracking, fingerprinting)
- `crates/ob-auth/src/jwt.rs` — Updated JWT module with rotation support
- `crates/ob-admin/src/routes.rs` — Admin endpoints for rotation
- `scripts/rotate-jwt-keys.sh` — Bash script for VPS cron execution
- `data/keys/key_rotation.json` — Rotation metadata (created automatically on first rotation)

## Architecture

### KeyRotationManager
Tracks current and previous keys. Rotates by:
1. Moving current key metadata to `previous_keys_metadata` (VecDeque, max 2)
2. Creating new key as current
3. Saving metadata to JSON file

```rust
pub struct KeyRotationManager {
    pub current_key_metadata: KeyMetadata,
    pub previous_keys_metadata: VecDeque<KeyMetadata>,
}
```

### JwtKeys with Rotation
RS256 keys now support fallback verification:

```rust
pub enum JwtKeys {
    Rsa {
        encoding: EncodingKey,      // Current signing key
        decoding: DecodingKey,      // Current verification key
        previous_decoding: Vec<DecodingKey>, // Up to 2 old keys
    },
    Hmac { secret: String },
}
```

### Token Verification Flow
```
verify_token(token) →
  Try current key → ✓ return claims
  Try previous key 1 → ✓ return claims
  Try previous key 2 → ✓ return claims
  ✗ return error
```

## API Endpoints (Admin-Only)

All endpoints require `admin` role in JWT claims.

### POST /_admin/jwt/rotate
Immediately rotate JWT signing keys.

**Request:**
```bash
curl -X POST https://api.orignagta.ca/_admin/jwt/rotate \
  -H "Authorization: Bearer $ADMIN_TOKEN"
```

**Response:**
```json
{
  "status": "rotated",
  "new_fingerprint": "a1b2c3d4e5f6g7h8",
  "timestamp": "2026-03-18T03:00:00Z",
  "message": "New JWT keys generated. Tokens signed before rotation remain valid. Old backups archived."
}
```

### GET /_admin/jwt/status
View current key metadata and rotation history.

**Request:**
```bash
curl https://api.orignagta.ca/_admin/jwt/status \
  -H "Authorization: Bearer $ADMIN_TOKEN"
```

**Response:**
```json
{
  "current_key": {
    "fingerprint": "a1b2c3d4e5f6g7h8",
    "created_at": "2026-03-18T03:00:00Z"
  },
  "previous_keys": [
    {
      "fingerprint": "x1y2z3w4v5u6t7s8",
      "created_at": "2025-12-18T03:00:00Z"
    },
    {
      "fingerprint": "p1q2r3s4t5u6v7w8",
      "created_at": "2025-09-18T03:00:00Z"
    }
  ],
  "all_fingerprints": ["a1b2c3d4e5f6g7h8", "x1y2z3w4v5u6t7s8", "p1q2r3s4t5u6v7w8"],
  "total_keys": 3
}
```

## File Structure

### VPS Directory Layout
```
/opt/orignabase/
├── data/
│   └── keys/
│       ├── jwt_private.pem                      ← Current private key
│       ├── jwt_public.pem                       ← Current public key
│       ├── key_rotation.json                    ← Metadata (current + prev keys)
│       ├── jwt_private_20260318_030000.pem.bak  ← Archived keys (max 4 backups)
│       ├── jwt_private_20251218_030000.pem.bak
│       ├── jwt_private_20250918_030000.pem.bak
│       └── jwt_private_20250618_030000.pem.bak
└── scripts/
    └── rotate-jwt-keys.sh                      ← Cron script

/var/log/orignabase/
└── jwt-rotation.log                            ← Rotation audit log
```

### Metadata Format
```json
{
  "current_key_metadata": {
    "created_at": "2026-03-18T03:00:00Z",
    "fingerprint": "a1b2c3d4e5f6g7h8",
    "is_current": true
  },
  "previous_keys_metadata": [
    {
      "created_at": "2025-12-18T03:00:00Z",
      "fingerprint": "x1y2z3w4v5u6t7s8",
      "is_current": false
    },
    {
      "created_at": "2025-09-18T03:00:00Z",
      "fingerprint": "p1q2r3s4t5u6t7w8",
      "is_current": false
    }
  ]
}
```

## Cron Setup (VPS)

### Add to root crontab
```bash
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 <<'CRON'
# Edit crontab
crontab -e

# Add this line:
# Rotate JWT keys quarterly: 3 AM on 1st of Jan, Apr, Jul, Oct
0 3 1 1,4,7,10 * /opt/orignabase/scripts/rotate-jwt-keys.sh
CRON
```

Or manually without cron:
```bash
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 \
  '/opt/orignabase/scripts/rotate-jwt-keys.sh'
```

### Verify Setup
```bash
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 \
  'crontab -l | grep rotate-jwt-keys'

# Check log
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 \
  'tail -50 /var/log/orignabase/jwt-rotation.log'
```

## Manual Rotation

### Option 1: API (Recommended)
```bash
# Get admin token (manual login as admin)
ADMIN_TOKEN="..."

curl -X POST https://api.orignagta.ca/_admin/jwt/rotate \
  -H "Authorization: Bearer $ADMIN_TOKEN"
```

### Option 2: SSH (VPS)
```bash
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 \
  '/opt/orignabase/scripts/rotate-jwt-keys.sh'
```

## Testing

### Unit Tests
```bash
cd crates/ob-auth
cargo test key_rotation
cargo test verify_token  # Tests fallback verification
```

### Integration Test (Dev Environment)
```bash
# Get dev admin token
curl -X POST https://api.dev.orignagta.ca/auth/login \
  -H "Content-Type: application/json" \
  -d '{
    "email": "e2e-admin@test.origna.ca",
    "password": "TestPass123!"
  }' | jq '.access_token'

ADMIN_TOKEN="..."

# Check initial status
curl https://api.dev.orignagta.ca/_admin/jwt/status \
  -H "Authorization: Bearer $ADMIN_TOKEN"

# Trigger rotation
curl -X POST https://api.dev.orignagta.ca/_admin/jwt/rotate \
  -H "Authorization: Bearer $ADMIN_TOKEN"

# Verify new keys are in use
curl https://api.dev.orignagta.ca/_admin/jwt/status \
  -H "Authorization: Bearer $ADMIN_TOKEN"

# Old tokens should still be valid (fallback verification)
# Create a token before rotation, then verify it still works
```

### Backup Verification
```bash
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 << 'SSH'
# Verify backups exist
ls -la /opt/orignabase/data/keys/*.bak

# Verify metadata
cat /opt/orignabase/data/keys/key_rotation.json | jq .

# Count backups (should be ≤ 4)
ls -1 /opt/orignabase/data/keys/*.bak | wc -l
SSH
```

## Disaster Recovery

### Restore Previous Key
If a recent key rotation caused issues and you need to revert:

```bash
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 << 'SSH'
cd /opt/orignabase/data/keys

# List backups (most recent first)
ls -lt jwt_private_*.pem.bak | head -3

# Restore from most recent backup
BACKUP="jwt_private_20260318_030000.pem.bak"  # <— adjust date
cp "$BACKUP" jwt_private.pem
cp "${BACKUP/private/public}" jwt_public.pem

# Restart services
cd /opt/orignabase
docker compose restart orignabase-dev orignabase-staging orignabase-prod

# Verify
docker compose logs orignabase-prod | tail -20
SSH
```

### Full Key Regeneration (Emergency)
If key files are corrupted and backups unavailable:

```bash
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 << 'SSH'
cd /opt/orignabase/data/keys

# Backup metadata for audit
cp key_rotation.json key_rotation.json.emergency-backup

# Delete all keys and metadata (WARNING: all existing tokens become invalid!)
rm -f jwt_private.pem jwt_public.pem key_rotation.json

# Regenerate from OrignaBase (will auto-create on startup)
cd /opt/orignabase
docker compose restart orignabase-dev orignabase-staging orignabase-prod

# Wait for services to start
sleep 10

# Verify new keys generated
ls -la data/keys/jwt_*.pem

# All users must re-authenticate
SSH
```

## Security Considerations

### Key Strength
- RS256 with 2048-bit RSA keys (NIST FIPS 140-2 compliant)
- Generated via OpenSSL with cryptographically secure random seed
- No raw key material in source code

### Rotation Frequency
- **Quarterly** (90 days): balances security and token lifetime
- Access tokens: 15 minutes → automatically use new key if rotated
- Refresh tokens: 7 days → last rotation's key still valid
- Long-lived verification links (24h, 1h, 15m): fallback verification covers all cases

### Token Lifetime Window
```
Scenario: Quarterly rotation schedule
- Tokens issued before rotation: Valid with fallback keys (up to 6 months)
- Tokens issued after rotation: Valid with current key (15 min to 7 days)
- Grace period: 2 previous keys ≈ 180 days of coverage
```

### Backup Security
- Archived keys owned by `root` with `0400` permissions (read-only)
- Stored in same VPS (same threat model as current keys)
- For stronger protection: encrypt backups or store off-site
- Never commit keys to git (added to `.gitignore`)

### Audit Trail
- All rotations logged: `/var/log/orignabase/jwt-rotation.log`
- Fingerprints tracked in metadata for correlation
- Admin API calls logged separately (GlitchTip integration)

## Monitoring

### Health Check
```bash
# Verify service can sign tokens with new key
curl https://api.orignagta.ca/health
# Should return 200 with metadata about current keys

# Monitor rotation log
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 \
  'tail -f /var/log/orignabase/jwt-rotation.log'
```

### Alert Thresholds
- Rotation fails: Check `/var/log/orignabase/jwt-rotation.log` and restart services
- Old backups accumulate (>4): Manual cleanup with `rm /opt/orignabase/data/keys/*.bak`
- Metadata corrupt: Restore from `key_rotation.json` backup, re-run rotation

## Implementation Status

### Complete ✓
- [x] `KeyRotationManager` with metadata tracking
- [x] RS256 key generation and backup
- [x] Fallback verification (current + 2 previous keys)
- [x] Admin endpoints (`POST /_admin/jwt/rotate`, `GET /_admin/jwt/status`)
- [x] Bash script for VPS automation
- [x] Unit tests

### Tested
- [x] Single rotation cycle
- [x] Multiple rotations (verify cleanup to 3 total keys)
- [x] Fallback verification with old tokens
- [x] Backup archival and cleanup

### Next Steps (Optional Enhancements)
- [ ] Webhook notifications on rotation (e.g., to Slack)
- [ ] Metrics/observability (track rotation timing, failures)
- [ ] Database-backed metadata (for distributed setups)
- [ ] Automated alerting if rotation fails

## References
- [JWT.io — Key Management](https://jwt.io/)
- [NIST SP 800-57 — Recommendation for Key Management](https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-57pt1r5.pdf)
- [OWASP — Key Management Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Key_Management_Cheat_Sheet.html)
