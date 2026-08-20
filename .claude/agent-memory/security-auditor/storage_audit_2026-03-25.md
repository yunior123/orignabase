---
name: storage_audit_2026-03-25
description: First security audit of ob-storage crate (local.rs, s3.rs, routes.rs, signed_url.rs, resumable.rs, transform.rs). Never audited before.
type: project
---

# Storage Audit — 2026-03-25

## CRITICAL findings (3)

1. **Any authenticated user can overwrite any product image** (`routes.rs:262-273`)
   - `can_user_write_path` allows ALL auth'd users to write to `products/*` — not just the product's seller.
   - Same check used in `batch_delete` → buyers can delete product images.
   - Fix: embed seller ID in path: `products/{seller_id}/{product_id}/...`

2. **OB_TEST_MODE=1 disables MIME validation** (`routes.rs:34-38`)
   - `application/octet-stream` content type skips magic-byte check in test mode.
   - Dev VPS has OB_TEST_MODE=1 — any user on api.dev.orignagta.ca can upload executables.
   - Fix: remove the bypass; use real image bytes in tests instead.

3. **OB_TEST_MODE=1 disables all path-ownership authorization** (`routes.rs:263-266`)
   - `can_user_write_path` returns `true` unconditionally in test mode.
   - Fix: never bypass ownership in test mode; use a test-user prefix instead.

## WARNING findings (5)

4. **ttl_secs is caller-controlled with no cap** (`routes.rs:393-398`)
   - Client can request signed URLs valid for 1 year.
   - Fix: cap to MAX_UPLOAD_TTL=3600, MAX_DOWNLOAD_TTL=7*24*3600.

5. **Signed URL secret == JWT auth secret** (`main.rs:1011`)
   - Same key for both. Key rotation breaks both simultaneously.
   - Fix: add separate `OB_STORAGE__SIGNED_URL_SECRET` config key.

6. **Path traversal sanitization uses string replacement, not canonical path** (`local.rs:20-32`)
   - Iterative `..` removal works for common cases but not OS-level canonicalization.
   - Fix: use `canonicalize()` + `starts_with(root)` check after join.

7. **Resumable max size (5 GB) vs regular (500 MB) — 10x inconsistency** (`routes.rs:25-28`)
   - 100 sessions × 5 GB = 500 GB of disk reservation possible.
   - Fix: 50 MB regular, 200 MB resumable for this use case.

8. **Empty-owner sessions bypass ownership** (`resumable.rs:155`)
   - `if !session.owner.is_empty() && session.owner != owner` — empty owner bypasses check.
   - Fix: reject empty owner at session creation; remove is_empty() bypass.

## INFO findings (3)

9. S3Config derives Debug → secret_key leaks in logs
10. Image decoding lacks pixel budget → zip-bomb DoS possible in transform.rs
11. Empty HMAC secret accepted without error at startup (should panic if < 32 bytes)
12. Content-Disposition filename not sanitized for header injection characters

## What was GOOD (no finding needed)
- Magic-byte validation via `infer` crate — correct approach
- Signed URL HMAC uses constant-time comparison (verify_slice) — correct
- Signed URL expiry enforced — correct
- Resumable upload: owner binding, session TTL (24h), per-user session limit (10), global limit (100)
- MAX_BATCH_PATHS=100 cap on batch operations
- X-Content-Type-Options: nosniff set at Caddy + axum middleware level
- SVG excluded from inline display (forced attachment)
- Path sanitization present in both LocalStorage and ResumableUploadManager

**Why:** First audit of this area. Previous audit focused on GraphQL and JWT.
**How to apply:** When fixing any of the above, verify the seller ID path structure is coordinated with the Flutter SDK's StorageRef path construction.
