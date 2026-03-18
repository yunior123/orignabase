# JWT Key Rotation — Deployment Checklist

## Pre-Deployment (Dev Testing)

- [ ] Code Review
  - [ ] Review `crates/ob-auth/src/key_rotation.rs` for correctness
  - [ ] Review JWT fallback verification logic in `jwt.rs`
  - [ ] Review admin endpoints in `routes.rs`

- [ ] Unit Tests
  - [ ] `cargo test --package ob-auth -- key_rotation::tests`
  - [ ] `cargo test --package ob-auth -- jwt::tests` (verify_token with fallback)
  - [ ] All tests pass with zero failures

- [ ] Build & Compilation
  - [ ] `cargo check` passes (no warnings or errors)
  - [ ] `cargo build --release` succeeds

## Dev Environment Testing

- [ ] Manual Integration Test
  - [ ] Get dev admin token (login as e2e-admin@test.origna.ca)
  - [ ] Call `GET /_admin/jwt/status` — verify response format
  - [ ] Call `POST /_admin/jwt/rotate` — verify new keys generated
  - [ ] Call `GET /_admin/jwt/status` again — verify current key changed
  - [ ] Verify old key in previous_keys list
  - [ ] Check that old tokens still verify (fallback test):
    1. Create token before rotation
    2. Trigger rotation
    3. Verify old token still decodes

- [ ] Check Log Output
  - [ ] Review Sentry/structured logs for rotation events
  - [ ] Verify no error spam

## Production Deployment (VPS: 204.168.137.16)

- [ ] Deploy Rust Code
  - [ ] Push to repo or merge to main
  - [ ] Pull on VPS: `cd /opt/orignabase && git pull origin main`
  - [ ] Rebuild: `docker compose build --no-cache orignabase-dev orignabase-staging orignabase-prod`
  - [ ] Restart: `docker compose restart orignabase-dev orignabase-staging orignabase-prod`
  - [ ] Wait 30 seconds for services to start
  - [ ] Check health: `curl https://api.orignagta.ca/_admin/health`

- [ ] Deploy Rotation Script
  - [ ] Copy script to VPS:
    ```bash
    scp -i ~/.ssh/id_ed25519 scripts/rotate-jwt-keys.sh \
      root@204.168.137.16:/opt/orignabase/scripts/
    ```
  - [ ] SSH and verify permissions:
    ```bash
    ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 \
      'ls -lh /opt/orignabase/scripts/rotate-jwt-keys.sh'
    ```
    Should show: `-rwxr-xr-x ... root root ... rotate-jwt-keys.sh`

- [ ] Test Script (Manual)
  - [ ] SSH to VPS and run:
    ```bash
    ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 \
      '/opt/orignabase/scripts/rotate-jwt-keys.sh'
    ```
  - [ ] Check exit code: `echo $?` (should be 0)
  - [ ] Check log file:
    ```bash
    ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 \
      'cat /var/log/orignabase/jwt-rotation.log'
    ```
  - [ ] Verify no errors in log
  - [ ] Verify keys were rotated:
    ```bash
    ssh -i ~/.ssh/id_ed25519 root@204.168.137.16 \
      'ls -la /opt/orignabase/data/keys/jwt_*.pem*'
    ```
    Should show: new `jwt_private.pem` / `jwt_public.pem` + one backup `*.pem.bak`

- [ ] Setup Cron (Production)
  - [ ] SSH to VPS: `ssh -i ~/.ssh/id_ed25519 root@204.168.137.16`
  - [ ] Edit crontab: `crontab -e`
  - [ ] Add line (3 AM on 1st of Jan, Apr, Jul, Oct):
    ```
    0 3 1 1,4,7,10 * /opt/orignabase/scripts/rotate-jwt-keys.sh
    ```
  - [ ] Save and exit (`:wq` in vim)
  - [ ] Verify cron was added:
    ```bash
    crontab -l | grep rotate-jwt-keys
    ```

- [ ] Test Cron Simulation (if possible)
  - [ ] Manually test on at least one environment first
  - [ ] Reduce cron frequency temporarily to hourly if time permits:
    ```
    0 * * * * /opt/orignabase/scripts/rotate-jwt-keys.sh
    ```
  - [ ] Wait 1 hour and check log
  - [ ] Change back to quarterly after verification

## Post-Deployment Monitoring

- [ ] Daily (First Week)
  - [ ] Check log file for errors: `tail -50 /var/log/orignabase/jwt-rotation.log`
  - [ ] Verify services still healthy: `curl https://api.orignagta.ca/_admin/health`
  - [ ] Check for auth failures in Sentry (should be zero)

- [ ] Weekly (First Month)
  - [ ] Verify backups are accumulating: `ls -la /opt/orignabase/data/keys/*.bak`
  - [ ] Check that no more than 4 backups exist
  - [ ] Call `/_admin/jwt/status` endpoint — verify metadata correct

- [ ] Monthly (Ongoing)
  - [ ] Check rotation log for any anomalies
  - [ ] Verify no backup accumulation beyond 4 files
  - [ ] Monitor Sentry for JWT-related errors

- [ ] Quarterly (After Next Rotation)
  - [ ] Verify automatic rotation triggered (if cron active)
  - [ ] Check log output
  - [ ] Confirm no service downtime

## Rollback Plan

If critical issue discovered:

1. **Stop automatic rotation**: `ssh ... 'crontab -e'` → comment out or delete cron line
2. **Restore previous keys**:
   ```bash
   ssh root@204.168.137.16 <<'SSH'
   cd /opt/orignabase/data/keys
   # List backups
   ls -lt jwt_private_*.pem.bak | head -3
   # Restore (replace TIMESTAMP)
   cp jwt_private_YYYYMMDD_HHMMSS.pem.bak jwt_private.pem
   cp jwt_public_YYYYMMDD_HHMMSS.pem.bak jwt_public.pem
   # Restart services
   cd /opt/orignabase && docker compose restart orignabase-prod
   # Verify
   curl https://api.orignagta.ca/_admin/health
   SSH
   ```
3. **Investigate**: Check logs in `/var/log/orignabase/jwt-rotation.log` and Sentry
4. **Fix**: Address root cause, test on dev, redeploy
5. **Re-enable**: Update cron and monitor

## Troubleshooting

### Script Fails with "openssl: command not found"
- SSH to VPS and check: `which openssl`
- If not found, install: `apt-get install openssl`

### Services fail to restart
- Check Docker: `ssh ... 'docker ps'`
- Check logs: `ssh ... 'docker compose logs orignabase-prod'`
- Manual restart: `ssh ... 'cd /opt/orignabase && docker compose up -d'`

### Old backups not cleaning up (>4 files)
- Script may have failed. Check `/var/log/orignabase/jwt-rotation.log`
- Manual cleanup: `ssh ... 'ls -1t /opt/orignabase/data/keys/*.bak | tail -n +5 | xargs rm'`

### Tokens fail verification after rotation
- Verify `key_rotation.json` exists and is valid JSON
- Check that previous keys are being loaded correctly
- Test on dev first with manual rotation

## Documentation

- [ ] `docs/JWT_KEY_ROTATION.md` — Full architecture guide (shared with team)
- [ ] `docs/JWT_KEY_ROTATION_SUMMARY.md` — Quick reference
- [ ] This checklist — Deployment procedures

## Sign-Off

- [ ] Code reviewed by: _________________
- [ ] Dev testing completed by: _________________
- [ ] Production deployment by: _________________
- [ ] Post-deployment verification by: _________________
- Date: _________________
- Notes: _________________________________________________

---

**Do not merge until all items checked.**
