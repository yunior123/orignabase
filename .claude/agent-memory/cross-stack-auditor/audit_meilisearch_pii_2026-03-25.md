---
name: Meilisearch PII Exposure + Error Handling Audit
description: Findings from 2026-03-25 audit of ob-search and ob-core for PII indexing, ID sanitization, sync error handling, and error message safety
type: project
---

## Audit: Meilisearch Sync & Error Handling (2026-03-25)

**Why:** Pre-release security review of ob-search and ob-core for PII leakage into search index and HTTP error responses.

**How to apply:** Use as baseline for any future changes to ob-search/src/, ob-core/src/error.rs, or main.rs fan-out logic.

## Findings

### CRITICAL: No collection filter before search sync (main.rs:844-856)
Every ChangeEvent — regardless of collection — is forwarded raw to the SearchSyncer. This means `users`, `orders`, `payouts`, `seller_profiles`, `addresses`, and other PII-containing collections are indexed wholesale into Meilisearch with no field stripping.

### CRITICAL: Meilisearch HTTP error body echoed in Error::Internal (client.rs:103-105)
The Meilisearch HTTP response body is included verbatim in the Internal error string. The Internal variant IS sanitized to "Internal server error" in IntoResponse, but the raw string propagates through tracing logs — so if Meilisearch echoes back index data in an error, it lands in structured logs.

### WARNING: origId field naming — uses `record_id` not `origId` (sync.rs:98-99)
The schema memory and CLAUDE.md specify `origId` as the original PostgreSQL ID field in Meilisearch. The actual code uses `record_id`. Not a data-loss risk, but a naming contract violation that will break any Flutter-side code expecting `origId`.

### WARNING: `Invalid order status: {req.new_status}` echoes user input (orders/status.rs:486)
User-supplied `new_status` string is reflected directly in a Validation error message returned to the client. Low severity (no PII) but enables enum value enumeration and is a minor input reflection issue.

### OK: Error::Database / Internal / Config are sanitized to "Internal server error" in HTTP responses (error.rs:52-57)
Confirmed working correctly. SQL query text, DB connection strings, and config secrets do not reach HTTP clients.

### OK: PostgreSQL ID sanitization works correctly (sync.rs:78-89)
`:` is replaced with `_`. `record_id` preserves the original ID. All non-alphanumeric chars except `-` and `_` are replaced.

### OK: Sync failures are logged with tracing::error (sync.rs:51-55, 64-68) — no retry mechanism
Failures are logged (index + doc_id, no document data). A DLQ (`meilisearch_sync_failures` collection) exists in ob-handlers but is only used for products. The realtime fan-out syncer (main.rs) has no retry — failures are fire-and-forget.
