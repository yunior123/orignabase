# Security Auditor Memory Index

## Audits
- `storage_audit_2026-03-25.md` — First audit of ob-storage crate. 3 CRITICAL (product overwrite by any user, OB_TEST_MODE MIME bypass, OB_TEST_MODE auth bypass), 5 WARNING, 4 INFO.
- `graphql_audit_2026-03-25.md` — First audit of ob-graphql resolvers. 4 CRITICAL, 6 WARNING, 4 INFO findings.
- `project_jwt_audit_2026-03-25.md` — First JWT auth audit (jwt.rs, middleware.rs, routes.rs). P1: no revocation, non-atomic refresh rotation. P2: custom_claims injection surface, Turnstile conditional skip, hardcoded Apple redirect URI.
- `websocket_audit_2026-03-25.md` — First WebSocket/realtime audit (ob-realtime crate). 2 CRITICAL: unauthenticated /presence endpoint + no per-user event authorization on dispatcher. 5 WARNING: no collection allowlist, no WS message size limit, slow-consumer stall, no per-user connection limit, JWT in URL logged. 3 INFO.
