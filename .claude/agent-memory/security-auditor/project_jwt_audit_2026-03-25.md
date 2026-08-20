---
name: JWT Auth Security Audit — 2026-03-25
description: First-ever JWT authentication audit of ob-auth crate. Findings, severities, and confirmed non-issues for future reference.
type: project
---

First JWT auth audit of ob-auth (crates/ob-auth/src/jwt.rs + middleware.rs + routes.rs).

**Why:** This area was never audited before. Access tokens are long-lived (default 900s in tests but configurable), refresh tokens 7 days, no revocation blacklist.

**How to apply:** When making changes to ob-auth, check against these findings first.

## Key Findings

### P1 (High — fix before release)
- No token revocation / blacklist mechanism. Stolen access tokens valid until expiry. Stolen refresh tokens valid for 7 days.
- Refresh token rotation is non-atomic: issues new refresh token but does NOT invalidate old one — two valid refresh tokens coexist post-rotation.
- `verify_token` uses a single shared `Validation` object for all previous keys in rotation fallback — the algorithm enforcement is correct per key type BUT previous_decoding keys are Vec<DecodingKey> with no bound on how long they're accepted (no expiry on old key acceptance).

### P2 (Medium)
- `custom_claims` field is `serde_json::Value` — arbitrary JSON. A compromised admin endpoint or DB write can inject `{"roles": ["admin"]}` into custom_claims. Authorization code reads from `auth.roles` (Vec<String> from Claims.roles) not custom_claims, but this vector originates from the DB `roles` field at refresh time — if DB is compromised, roles escalate on next refresh.
- Apple redirect URI hardcoded to `https://orignagta.ca/auth/apple/callback` in oauth.rs:979 — breaks staging/dev Apple Sign-In.
- Turnstile validation is conditional: if `turnstile_secret_key` is None AND `OB_TEST_MODE != 1`, registration still requires turnstile_token to be sent, but if turnstile_secret_key is None the token is NOT validated server-side.

### Confirmed SAFE
- Algorithm confusion: Validation::new(Algorithm::RS256) for RSA, Validation::default() for HMAC — algorithms are locked to key type, no downgrade possible.
- Algorithm "none" attack: jsonwebtoken crate explicitly rejects `alg: none` by default.
- Expired token rejection: Validation has exp check enabled by default — confirmed by test_expired_token_fails.
- MFA bypass: challenge_token type "mfa_challenge" is checked at routes.rs:1374 — you can't use an access token to complete MFA.
- Default secret in production: panics at startup if CHANGE_ME_IN_PRODUCTION in production env (middleware.rs:11 + main.rs:626).
- Timing attack on login: dummy_verify() runs even for unknown emails (routes.rs:416).
- Account lockout: check_account_lockout called before password hash (routes.rs:397).
