# OrignaBase Security Fixes — Implementation Guide

## FIX #1: CORS Configuration (CRITICAL) — 1 Hour

### Files to Change:
1. `crates/orignabase/src/main.rs` (lines 1142-1144)
2. `crates/ob-core/src/server.rs` (lines 21-23)

### Current Code:
```rust
.allow_origin(tower_http::cors::Any)
.allow_methods(tower_http::cors::Any)
.allow_headers(tower_http::cors::Any),
```

### Fixed Code:
```rust
use tower_http::cors::AllowOrigin;

// In main.rs or server.rs, load from config:
let allowed_origins = vec![
    "https://orignagta.ca",
    "https://staging.orignagta.ca",
    "https://dev.orignagta.ca",
    "http://localhost:3000", // dev only
];

.allow_origin(AllowOrigin::predicate(|origin, _| {
    allowed_origins
        .iter()
        .any(|o| origin.as_bytes() == o.as_bytes())
}))
.allow_methods([GET, POST, PUT, DELETE, PATCH])
.allow_headers([CONTENT_TYPE, AUTHORIZATION]),
```

### Testing:
```bash
# Should FAIL (403 Forbidden):
curl -H "Origin: https://evil.com" https://api.orignagta.ca/api/orders

# Should SUCCEED (200 OK):
curl -H "Origin: https://orignagta.ca" https://api.orignagta.ca/api/orders
```

---

## FIX #2: JWT Secret Enforcement (HIGH) — 30 Minutes

### File: `crates/orignabase/src/main.rs` (around line 567)

### Current Code:
```rust
if jwt_secret == "change_me_in_prod" {
    eprintln!("JWT secret is the default value (insecure). Set OB_AUTH__JWT_SECRET...");
}
```

### Fixed Code:
```rust
let env_mode = std::env::var("OB_ENV").unwrap_or_else(|_| "development".to_string());
if env_mode == "production" && jwt_secret == "change_me_in_prod" {
    panic!(
        "SECURITY VIOLATION: Default JWT secret detected in production mode.\n\
         This is a critical security issue.\n\
         Set OB_AUTH__JWT_SECRET environment variable to a strong secret.\n\
         Example: export OB_AUTH__JWT_SECRET=$(openssl rand -hex 32)"
    );
}
if env_mode != "production" {
    eprintln!("⚠️  WARNING: Using default JWT secret (development mode only)");
}
```

### Testing:
```bash
# Production should fail:
OB_ENV=production ./orignabase
# Output: thread 'main' panicked at 'SECURITY VIOLATION...'

# Dev should warn but start:
OB_ENV=development ./orignabase
# Output: ⚠️  WARNING: Using default JWT secret...
```

---

## FIX #3: Auth Endpoint Rate Limiting (HIGH) — 2 Hours

### File: `crates/ob-auth/src/routes.rs`

### Add to imports:
```rust
use crate::handlers::shared::rate_limiter::check_user_rate_limit;
use ob_core::DatabaseClient;
```

### Modify login handler:
```rust
async fn login(
    State(state): State<HandlersState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ob_core::Error> {
    // Add rate limiting (before any auth logic):
    check_user_rate_limit(
        &state.db,
        &req.email,  // or IP address if available
        "login_attempt",
        10,  // max 10 attempts
        1,   // per minute
    )
    .await?;

    // ... rest of login logic
}
```

### Repeat for:
- `register()` handler → 5 attempts/hour per email
- `password_reset()` handler → 3 attempts/hour per email
- `google_login()` handler → 10 attempts/minute per user

### Testing:
```bash
# Try login 11 times quickly:
for i in {1..11}; do curl -X POST https://api.orignagta.ca/auth/login ...; done

# Output on 11th: 429 Conflict / "Rate limit exceeded"
```

---

## FIX #4: SurrealQL Injection Pattern (HIGH) — 6 Hours (Gradual)

### Identify All `query_raw()` Calls:
```bash
grep -rn "query_raw" crates/ob-handlers/src/ | grep -v test
# Output will show all unsafe query patterns
```

### Example Migration:

#### BEFORE (Vulnerable to Injection):
```rust
let query = format!(
    "SELECT * FROM {} WHERE eventId = '{}'",
    collections::WEBHOOK_EVENTS,
    ob_core::escape_surreal_string(event_id)
);
let rows = db.query_raw(&query).await?;
```

#### AFTER (Safe):
```rust
let query = "SELECT * FROM webhook_events WHERE eventId = $event_id";
let rows = db.query_bind(
    query,
    serde_json::json!({ "event_id": event_id })
).await?;
```

### Checklist (Per File):
- [ ] payments/webhooks.rs (multiple instances)
- [ ] products/crud.rs
- [ ] orders/returns.rs
- [ ] cron/mod.rs

### Testing:
```bash
# Inject attempt should fail:
curl -X POST https://api.orignagta.ca/orders \
  -d '{"product_id": "'; DROP TABLE products; --"}'

# Should return validation error, not execute DROP
```

---

## FIX #5: Error Message Scrubbing (MEDIUM) — 2 Hours

### File: `crates/ob-handlers/src/payments/webhooks.rs` (line 52)

#### BEFORE:
```rust
.map_err(|_| ob_core::Error::Internal("HMAC key error".into()))?
```

#### AFTER:
```rust
.map_err(|e| {
    error!(error = ?e, "HMAC key initialization failed");
    ob_core::Error::Internal("Signature verification failed".into())
})?
```

### Pattern to Apply:
1. Find: `Err(ob_core::Error::Internal("...")`
2. Add: `error!()` log with details
3. Return: Generic message to client

### Testing:
```bash
# Browser DevTools → Network tab
# Response should NOT expose "HMAC key error", only generic message
```

---

## FIX #6: Rate Limit Timestamp Logic (MEDIUM) — 1 Hour

### File: `crates/ob-handlers/src/shared/rate_limiter.rs` (lines 39-75)

#### BEFORE:
```rust
let now = chrono::Utc::now();
let window_start = now - chrono::Duration::minutes(window_minutes);
// Uses RFC3339 string in query
let query = format!(
    "SELECT count() FROM {} WHERE ... AND createdAt >= $window_start",
    ...
);
```

#### AFTER:
```rust
let now_ts = chrono::Utc::now().timestamp();
let window_start_ts = now_ts - (window_minutes * 60);

let query = format!(
    "SELECT count() FROM {} WHERE ... AND createdAtUnix >= $window_start_ts",
    ...
);

db.query_bind(
    &query,
    serde_json::json!({
        "user_id": user_id,
        "action": action,
        "window_start_ts": window_start_ts
    })
).await?;
```

#### Database Migration:
```sql
-- Add Unix timestamp field to rate_limits collection
DEFINE FIELD createdAtUnix ON rate_limits TYPE number;
DEFINE INDEX idx_rate_limits_window ON rate_limits(userId, action, createdAtUnix);
```

### Testing:
```bash
# Rapid requests should trigger limit exactly at threshold
# No off-by-one errors from timezone handling
```

---

## VALIDATION CHECKLIST

- [ ] Cargo check — no compile errors
- [ ] cargo clippy — no warnings
- [ ] cargo test — all tests pass
- [ ] Manual browser test — CORS headers check:
  ```
  Response Headers: Access-Control-Allow-Origin: https://orignagta.ca
  (NOT: Access-Control-Allow-Origin: *)
  ```
- [ ] Auth rate limiting test — 11th login attempt returns 429
- [ ] Error message test — no internals exposed in response
- [ ] SurrealQL injection test — malicious input rejected safely

---

## DEPLOYMENT ORDER

1. **Fix CORS** (most critical, lowest risk)
2. **Enforce JWT secret** (blocks bad configs)
3. **Add auth rate limiting** (protects accounts)
4. **Migrate queries** (reduce injection risk)
5. **Scrub errors** (information disclosure)
6. **Fix timestamps** (reliability improvement)

**Total estimated time: 12-14 hours**
**Recommend spreading over 2-3 development days**

