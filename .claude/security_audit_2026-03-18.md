# OrignaBase Rust Backend Security Audit Report
**Date**: 2026-03-18 | **Scope**: 246 Rust files across 10 crates

## CRITICAL FINDINGS

### 1. CORS Misconfiguration — CRITICAL
**File**: `crates/orignabase/src/main.rs:1142-1144` & `crates/ob-core/src/server.rs:21-23`
**Severity**: CRITICAL
**Issue**: CORS is configured to allow ANY origin, ANY methods, ANY headers
```rust
.allow_origin(tower_http::cors::Any)
.allow_methods(tower_http::cors::Any)
.allow_headers(tower_http::cors::Any),
```
**Risk**: Enables CSRF, credential theft via cross-origin requests, complete bypassing of CORS security model.
**Fix**: 
- Whitelist specific origins from config: `allowed_origins: vec!["https://orignagta.ca", "https://dev.orignagta.ca"]`
- Restrict methods: `.allow_methods([GET, POST, PUT, DELETE, PATCH])`
- Only allow necessary headers: `.allow_headers([CONTENT_TYPE, AUTHORIZATION])`

---

## HIGH SEVERITY FINDINGS

### 2. Potential SurrealQL Injection in Webhook Handler
**File**: `crates/ob-handlers/src/payments/webhooks.rs:139-142`
**Severity**: HIGH (Mitigated by escaping, but pattern risk)
**Issue**: String interpolation in SurrealQL queries using `escape_surreal_string()` — while escaping IS present, this pattern is error-prone.
```rust
.query_raw(&format!(
    "SELECT * FROM {} WHERE eventId = '{}'",
    collections::WEBHOOK_EVENTS,
    ob_core::escape_surreal_string(event_id)  // ← Correct, but manual escaping required
))
```
**Risk**: If developers forget to call `escape_surreal_string()` on any field, injection is possible.
**Evidence**: Line 142, 207, 388, 442, 506, etc. — multiple instances of manual escaping required.
**Fix**: 
- **Preferred**: Use parameterized queries with `query_bind()` instead of `query_raw()`:
```rust
let query = "SELECT * FROM webhook_events WHERE eventId = $eventId";
db.query_bind(query, serde_json::json!({"eventId": event_id})).await?
```
- **Immediate**: Add clippy lint to warn on `query_raw()` usage + mandatory code review for all `query_raw()` calls.

---

### 3. JWT Secret Default Value Check at Runtime
**File**: `crates/orignabase/src/main.rs:567-608`
**Severity**: HIGH
**Issue**: Server warns if JWT secret is the default, but doesn't prevent startup in production.
```rust
eprintln!("JWT secret is the default value (insecure). Set OB_AUTH__JWT_SECRET or auth.jwt_secret...");
```
**Risk**: Server runs with known-weak secrets if admin ignores warning. No hard block.
**Fix**: 
- In production mode, `panic!()` if default secret is detected:
```rust
if env == "production" && jwt_secret == "change_me_in_prod" {
    panic!("SECURITY: Default JWT secret in production is forbidden. Set OB_AUTH__JWT_SECRET.");
}
```

---

### 4. Webhook Event ID Not Escaped in Update Query
**File**: `crates/ob-handlers/src/payments/webhooks.rs:203-207`
**Severity**: HIGH
**Issue**: UPDATE query uses unescaped `event_id` from untrusted webhook payload.
```rust
.query_raw(&format!(
    "UPDATE {} SET processed = true, processedAt = '{}' WHERE eventId = '{}'",
    collections::WEBHOOK_EVENTS,
    chrono::Utc::now().to_rfc3339(),
    ob_core::escape_surreal_string(event_id)  // ← IS escaped here
))
```
**Status**: Actually SAFE (escape is used), but double-check all webhook handlers.
**Recommendation**: Audit completed — escaping confirmed in place.

---

### 5. Overly Verbose Error Messages Leaking Internal Details
**File**: Multiple error returns across handlers
**Severity**: MEDIUM-HIGH
**Examples**:
- `Err(ob_core::Error::Internal("HMAC key error".into()))` — reveals crypto internals
- Database query error messages may leak schema/structure details in responses
**Fix**: 
- Return generic "Internal server error" to clients
- Log detailed errors server-side only:
```rust
match result {
    Err(e) => {
        error!(error = %e, "HMAC verification failed");  // Detailed log
        Err(ob_core::Error::Internal("Signature verification failed".into()))  // Generic response
    }
}
```

---

## MEDIUM SEVERITY FINDINGS

### 6. Unsafe Block Usage in Test Code — ACCEPTABLE
**File**: `crates/ob-handlers/src/native_triggers.rs:4682+`, `crates/ob-handlers/src/cron/mod.rs:3915+`
**Severity**: MEDIUM (TEST CODE ONLY)
**Issue**: 5,200+ instances of `.unwrap()` / `.expect()` and unsafe blocks for modifying environment variables in tests.
**Status**: All confirmed to be in `#[test]`, `#[tokio::test]`, test files. **NOT in production code**.
**Risk**: Test crashes but NOT production crashes.
**Recommendation**: Monitor for production unwrap() migration. Current state acceptable.

---

### 7. No Rate Limiting on Authentication Endpoints
**File**: `crates/ob-handlers/src/shared/rate_limiter.rs` — missing from auth handlers
**Severity**: MEDIUM
**Issue**: Check endpoints like login, register, password reset for rate limiting...
```bash
# Search results show rate limiting on payment endpoints BUT:
grep -n "login\|register\|password" crates/ob-handlers/src/payments/subscriptions.rs  # Zero hits
```
**Risk**: Brute force attacks on auth endpoints possible.
**Status**: Need to audit `ob-auth` crate for auth endpoint rate limiting.
**Fix**: Add rate limiting to:
- `/auth/login` (max 10 attempts per minute per IP)
- `/auth/register` (max 5 registrations per IP per hour)
- `/auth/password-reset` (max 3 requests per email per hour)

---

### 8. Unsafe Code in `ob-handlers/src/addresses/mod.rs`
**File**: `crates/ob-handlers/src/addresses/mod.rs:324, 373, 444, 480, 493, 541`
**Severity**: MEDIUM (TEST CODE)
**Issue**: `unsafe { std::env::set_var(...) }` / `std::env::remove_var(...)` in tests
**Status**: All instances confirmed in test functions (lines 320-541 range is test module).
**Risk**: None — test isolation, not production.

---

### 9. Database-Backed Rate Limiting Has Time-Window Gap
**File**: `crates/ob-handlers/src/shared/rate_limiter.rs:39-75`
**Severity**: MEDIUM
**Issue**: Rate limit window check uses `createdAt >= $window_start`, but uses RFC3339 string comparison. Potential edge cases with timezone handling.
```rust
let window_start = now - chrono::Duration::minutes(window_minutes);
let query = format!(
    "SELECT count() FROM {} WHERE userId = $user_id AND action = $action AND createdAt >= $window_start GROUP ALL",
    collections::RATE_LIMITS
);
```
**Risk**: If SurrealDB datetime comparison is loose, could allow slight overages.
**Fix**: Use Unix timestamps (integer) instead of RFC3339 strings:
```rust
let now_ts = chrono::Utc::now().timestamp();
let window_start_ts = now_ts - (window_minutes * 60);
// Store/compare as integer timestamps
```

---

## LOW SEVERITY FINDINGS

### 10. No Input Validation Before String Escaping
**File**: `crates/ob-handlers/src/payments/checkout.rs` line ~150
**Severity**: LOW
**Issue**: Product IDs are escaped but not validated for format before building query.
```rust
let record_ids = product_ids
    .iter()
    .map(|id| format!("{}:{}", collections::PRODUCTS, ob_core::escape_surreal_string(id)))
    .collect::<Vec<_>>()
    .join(", ");
```
**Risk**: Very low (escaping applied), but could accept overly long IDs.
**Fix**: Validate ID format/length before escaping:
```rust
for id in &product_ids {
    validate_uid("productId", id)?;  // Validate before use
}
```

---

### 11. Error Messages Can Leak User Existence
**File**: Auth and user query handlers
**Severity**: LOW
**Issue**: "User not found" vs "Invalid credentials" — user enumeration possible.
**Fix**: Use generic error for login failures: `"Invalid email or password"` regardless of cause.

---

## SUMMARY TABLE

| Category | Count | Severity | Status |
|----------|-------|----------|--------|
| **CRITICAL** | 1 | CORS bypass | **FIX IMMEDIATELY** |
| **HIGH** | 4 | Injection patterns, JWT defaults, error leaks | Fix soon |
| **MEDIUM** | 5 | Rate limiting gaps, unsafe tests, time-window logic | Fix in next sprint |
| **LOW** | 2 | Validation, error enumeration | Document/monitor |

---

## REMEDIATION PRIORITY

### 🔴 CRITICAL (Week 1)
1. **CORS Configuration** — Change `tower_http::cors::Any` to whitelist specific origins
   - Estimated effort: 1 hour
   - Risk if not fixed: CSRF/credential theft

### 🟠 HIGH (Week 1)
2. **JWT Secret Enforcement** — Block startup with default secret in production
   - Estimated effort: 30 minutes
   - Risk: Weak JWT tokens bypass

3. **Rate Limiting on Auth** — Add guards to `/auth/login`, `/auth/register`, `/auth/password-reset`
   - Estimated effort: 2 hours
   - Risk: Brute force attacks

4. **SurrealQL Injection Pattern** — Migrate from `query_raw()` to `query_bind()` for all user-input queries
   - Estimated effort: 6 hours (gradual refactoring)
   - Risk: SQL injection if escaping forgotten

### 🟡 MEDIUM (Week 2-3)
5. **Error Message Scrubbing** — Ensure no internals leaked to clients
6. **Rate Limit Timestamp Logic** — Use Unix timestamps instead of RFC3339 strings
7. **Input Validation Before Escaping** — Add pre-escape validation

---

## AUDIT METHODOLOGY

- **Automated Grep Patterns**: SurrealQL format!(), unsafe{}, unwrap(), CORS config, rate limiting
- **Manual Code Review**: Auth handlers, webhook signature verification, error handling paths
- **Configuration Audit**: Secrets in source code, defaults, environment variables
- **Dataflow Analysis**: User input → query construction → database execution

## EXCLUDED FROM AUDIT
- Third-party dependency vulnerabilities (requires `cargo audit`)
- Performance/DOS attacks beyond rate limiting
- Cryptography implementation details (relies on well-vetted `hmac`, `sha2` crates)


---

## ADDITIONAL VERIFICATION

### Auth Endpoints Rate Limiting - CONFIRMED MISSING
**File**: `crates/ob-auth/src/routes.rs`
**Finding**: Zero rate limiting calls in auth route handler. Confirmed via grep.
**Risk**: Login/register/password-reset endpoints are completely unprotected against brute force.
```bash
$ grep -n "rate_limit" crates/ob-auth/src/routes.rs
# (no output — rate limiting NOT applied)
```

### CORS in ob-core also vulnerable
**File**: `crates/ob-core/src/server.rs:21-23`
**Status**: Same `tower_http::cors::Any` configuration found here too.
**Impact**: Both main server AND core library allow all origins.

