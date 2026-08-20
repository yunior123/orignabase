---
name: GraphQL Resolver Authorization Audit — 2026-03-25
description: First-ever security audit of ob-graphql crate resolvers. Documents all findings by severity.
type: project
---

GraphQL authorization audit completed 2026-03-25. Never audited before.

**Why:** GraphQL is the primary data access surface for the Flutter app. Authorization bypass here means full data exposure.

**How to apply:** Use this as baseline for regression testing. Any change to resolvers.rs, schema.rs, or evaluator.rs should re-verify these findings are still addressed.

## Findings summary

### CRITICAL
1. `list` resolver passes `resource: None` to RuleEngine — `isOwner` always returns false on list, but any rule using only `isAuthenticated()` will list ALL documents cross-user (IDOR). File: resolvers.rs:94-111.
2. `config` and `config_all` queries have NO authentication check — any unauthenticated caller can read all remote config keys/values. File: resolvers.rs:150-195.
3. GraphQL body read limit (10MB, line 1195) exceeds router-level DefaultBodyLimit (2MB, line 1267). Because the GraphQL handler reads the body manually before the middleware layer can enforce it, the 2MB limit is bypassed — allows 10MB DoS payloads.
4. `batch_delete` IDs array is unbounded — no cap on `ids.len()`. Attacker sends 100K IDs triggering 100K DB round-trips (N+1 DoS). File: resolvers.rs:683-754.

### WARNING
5. `list` resolver limit clamped to 10,000 (line 122) — extremely high; a single query can dump 10K documents. Should be 100 max for non-admin.
6. `vector_search` top_k clamped to 10,000 (line 237) — same concern; unbounded scan with high k triggers full-collection similarity search.
7. `search` filter param passed unsanitized to Meilisearch (line 294) — Meilisearch filter injection possible if filter DSL is not escaped.
8. `batch_update` update_list is unbounded — no cap on number of update entries. File: resolvers.rs:778.
9. Introspection enabled in `is_test_mode` (OB_TEST_MODE=1) — if dev env is accidentally exposed, full schema is revealed.
10. `config_all` returns ALL config key-value pairs — if any config value contains internal secrets (webhook secrets, etc.) this leaks them to unauthenticated callers.

### INFO
11. GraphQL endpoint GET handler returns GraphiQL UI — should be disabled in production builds.
12. No per-user rate limiting on GraphQL mutations — only per-IP (tower_governor). Attacker behind shared IP bypasses this.
13. `normalize_data` double-decode logic could be abused: attacker sends a JSON string that decodes to a different payload than what was validated at the SDK layer.
14. `batch_create` flattens nested arrays — attacker could pass `[[doc1, doc2], [doc3]]` and bypass per-document count limits if any are added later.
