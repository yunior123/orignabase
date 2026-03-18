# JWT Key Rotation — Implementation Index

## Quick Access

| Document | Purpose | Audience |
|----------|---------|----------|
| **JWT_KEY_ROTATION_SUMMARY.md** | Quick overview (5 min read) | Developers |
| **JWT_KEY_ROTATION.md** | Complete architecture (30 min read) | Tech leads, DevOps |
| **JWT_ROTATION_CHECKLIST.md** | Deployment procedures | DevOps engineers |
| This file | Navigation guide | Everyone |

## Code Files

### Rust Code
- **crates/ob-auth/src/key_rotation.rs** (151 lines) — Key metadata manager
  - `KeyRotationManager` — tracks current + previous keys
  - `KeyMetadata` — fingerprint, timestamp, is_current
  - Unit tests included
  
- **crates/ob-auth/src/jwt.rs** (735 lines) — Updated JWT module
  - Fallback verification (current → previous[0] → previous[1])
  - `rotate_keys()` function
  - `from_rsa_pem_with_rotation()` method
  - All existing tests pass

- **crates/ob-auth/src/lib.rs** — Updated exports
  - `rotate_keys`, `KeyRotationManager`, `fingerprint_public_key`

- **crates/ob-admin/src/routes.rs** — Admin endpoints
  - `POST /_admin/jwt/rotate` — Trigger rotation
  - `GET /_admin/jwt/status` — View key metadata

### Bash Scripts
- **scripts/rotate-jwt-keys.sh** (110 lines, executable)
  - Quarterly rotation automation
  - Cron: `0 3 1 1,4,7,10 * /opt/orignabase/scripts/rotate-jwt-keys.sh`
  - Creates backups, restarts services, cleans up old files

## Documentation

### For Quick Overview
→ **JWT_KEY_ROTATION_SUMMARY.md**
- What was built
- How it works
- Security properties
- Testing procedures

### For Architecture & Design
→ **JWT_KEY_ROTATION.md**
- Key files reference
- Architecture details
- Metadata format
- VPS directory layout
- API endpoint examples
- Disaster recovery playbook
- Security considerations
- Monitoring guidelines

### For Deployment
→ **JWT_ROTATION_CHECKLIST.md**
- Pre-deployment checklist
- Dev testing procedures
- Production deployment steps
- Post-deployment monitoring
- Rollback procedures
- Troubleshooting guide

## Key Features

**Zero Downtime**: Old and new keys valid simultaneously
**Fallback Verification**: Tokens remain valid ~180 days after rotation
**Audit Trail**: Fingerprints and timestamps tracked
**Backup Management**: Keep last 4 backups for recovery
**VPS Automation**: Cron script handles quarterly rotations
**Admin API**: Manual rotation via `POST /_admin/jwt/rotate`

## Security Properties

| Property | Value |
|----------|-------|
| Algorithm | RS256 (RSA 2048-bit) |
| Compliance | NIST FIPS 140-2 |
| Rotation Frequency | Quarterly (90 days) |
| Grace Period | ~180 days (2 previous keys) |
| Token TTL Coverage | Access (15m) + Refresh (7d) + Links (15m-24h) |
| Backup Count | Keep last 4, delete oldest |
| Metadata Persistence | JSON file (key_rotation.json) |

## Quick Start

### Local Testing (Dev)
```bash
# 1. Get admin token
TOKEN=$(curl -s -X POST https://api.dev.orignagta.ca/auth/login \
  -d '{"email":"e2e-admin@test.origna.ca","password":"TestPass123!"}' | jq -r .access_token)

# 2. Check current keys
curl https://api.dev.orignagta.ca/_admin/jwt/status -H "Authorization: Bearer $TOKEN" | jq .

# 3. Rotate
curl -X POST https://api.dev.orignagta.ca/_admin/jwt/rotate -H "Authorization: Bearer $TOKEN" | jq .

# 4. Verify rotation succeeded
curl https://api.dev.orignagta.ca/_admin/jwt/status -H "Authorization: Bearer $TOKEN" | jq .current_key
```

### VPS Testing (Production)
```bash
# 1. SSH to VPS
ssh -i ~/.ssh/id_ed25519 root@204.168.137.16

# 2. Run rotation script
/opt/orignabase/scripts/rotate-jwt-keys.sh

# 3. Check log
tail -50 /var/log/orignabase/jwt-rotation.log

# 4. Verify keys
ls -la /opt/orignabase/data/keys/jwt_*.pem*
cat /opt/orignabase/data/keys/key_rotation.json | jq .
```

## Deployment Sequence

1. **Code Review** → Review key_rotation.rs, jwt.rs, routes.rs
2. **Unit Tests** → `cargo test --package ob-auth -- key_rotation::tests`
3. **Dev Testing** → Test endpoints on dev.orignagta.ca
4. **VPS Deployment** → Copy script, test manual run
5. **Cron Setup** → Add to root crontab with quarterly schedule
6. **Monitoring** → Watch logs for first rotation

See **JWT_ROTATION_CHECKLIST.md** for detailed procedures.

## File Locations

```
orignabase/
├─ crates/ob-auth/src/
│  ├─ key_rotation.rs (NEW)
│  ├─ jwt.rs (MODIFIED)
│  └─ lib.rs (MODIFIED)
├─ crates/ob-admin/src/
│  └─ routes.rs (MODIFIED)
├─ scripts/
│  └─ rotate-jwt-keys.sh (NEW)
├─ docs/
│  ├─ JWT_KEY_ROTATION.md (NEW)
│  └─ JWT_KEY_ROTATION_SUMMARY.md (NEW)
├─ JWT_ROTATION_CHECKLIST.md (NEW)
└─ JWT_KEY_ROTATION_INDEX.md (NEW — this file)
```

## FAQ

**Q: Will old tokens stop working after rotation?**
A: No. Old tokens remain valid and are verified with fallback keys (current → previous[0] → previous[1]).

**Q: How many old keys are kept?**
A: 2 previous keys plus current = 3 total. Oldest beyond that is dropped.

**Q: What happens if rotation fails?**
A: Check `/var/log/orignabase/jwt-rotation.log`. If needed, restore from backup: `cp jwt_private_TIMESTAMP.pem.bak jwt_private.pem`.

**Q: Can I rotate manually without waiting for cron?**
A: Yes. Call `POST /_admin/jwt/rotate` or run `/opt/orignabase/scripts/rotate-jwt-keys.sh`.

**Q: How long do backups stay?**
A: Last 4 backups are kept. Oldest backups are deleted automatically.

**Q: What's the grace period for old tokens?**
A: ~180 days. After rotation, tokens signed before rotation remain valid for up to 90 days, and the previous key before that remains valid for ~90 days.

## Support

For issues, check these in order:
1. **JWT_ROTATION_CHECKLIST.md** — Troubleshooting section
2. **JWT_KEY_ROTATION.md** — Security considerations & monitoring
3. **logs**: `/var/log/orignabase/jwt-rotation.log` (VPS), Sentry (application)

## Related Documentation

- [NIST SP 800-57 — Key Management](https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-57pt1r5.pdf)
- [OWASP Key Management Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Key_Management_Cheat_Sheet.html)
- [JWT.io — Introduction to JWT](https://jwt.io/)

---

**Implementation Status**: Complete ✓
**Last Updated**: 2026-03-18
**Ready for**: Code review, testing, deployment
