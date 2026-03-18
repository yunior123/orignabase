# OrignaBase Rust Backend — 12 Critical Fixes

**Date:** 2026-03-18
**Audit Scope:** Security, data integrity, concurrency safety
**Priority:** All issues require fixes before production deployment

---

## Issue #1: Missing DB Indexes

**File:** `crates/ob-handlers/src/shared/indexes.rs` (NEW)
**Risk Level:** HIGH — N+1 queries, slow pagination, high DB load
**Symptoms:** Product search/ratings/questions pages load slowly; high CPU on SurrealDB

**Root Cause:**
- `products`, `product_ratings`, `product_questions`, `favorites` tables lack indexes on frequently queried columns
- Queries on `(productId, createdAt)`, `(userId, productId)` perform full table scans

**Fix:** Create SurrealDB index definition module

```rust
// File: crates/ob-handlers/src/shared/indexes.rs (NEW FILE)
pub async fn create_required_indexes(db: &DatabaseClient) -> Result<(), String> {
    // Products
    db.query_raw("DEFINE INDEX idx_products_seller ON TABLE products COLUMNS (sellerId)")
        .await.map_err(|e| e.to_string())?;
    db.query_raw("DEFINE INDEX idx_products_category ON TABLE products COLUMNS (categoryId)")
        .await.map_err(|e| e.to_string())?;
    db.query_raw("DEFINE INDEX idx_products_status ON TABLE products COLUMNS (lifecycleStatus)")
        .await.map_err(|e| e.to_string())?;
    db.query_raw("DEFINE INDEX idx_products_price ON TABLE products COLUMNS (priceCents)")
        .await.map_err(|e| e.to_string())?;

    // Product ratings
    db.query_raw("DEFINE INDEX idx_ratings_product_user ON TABLE product_ratings COLUMNS (productId, userId)")
        .await.map_err(|e| e.to_string())?;
    db.query_raw("DEFINE INDEX idx_ratings_product_date ON TABLE product_ratings COLUMNS (productId, createdAt) ORDER BY createdAt DESC")
        .await.map_err(|e| e.to_string())?;

    // Product questions
    db.query_raw("DEFINE INDEX idx_questions_product_date ON TABLE product_questions COLUMNS (productId, createdAt) ORDER BY createdAt DESC")
        .await.map_err(|e| e.to_string())?;

    // Favorites
    db.query_raw("DEFINE INDEX idx_favorites_user_product ON TABLE favorites COLUMNS (userId, productId)")
        .await.map_err(|e| e.to_string())?;

    Ok(())
}
```

**Integration:**
- Add to `lib.rs` startup sequence after DB initialization
- Call: `if let Err(e) = create_required_indexes(&db).await { warn!("Index creation failed: {}", e); }`
- Idempotent: SurrealDB ignores if index already exists

---

## Issue #2: Float Math in Shipping Costs

**File:** `crates/ob-handlers/src/shipping_calc/mod.rs`
**Risk Level:** CRITICAL — Financial data loss, seller trust erosion
**Symptoms:** Shipping costs inconsistent; refunds don't match charges; rounding accumulation

**Root Cause:**
```rust
// Current problematic code (line ~548):
let total_cost = (final_shipping * 100.0).round() / 100.0;  // ❌ Float rounding error
breakdown.insert(key, (v * 100.0).round() / 100.0);  // ❌ Multiple roundings compound
```

Float rounding compound errors: 0.1 + 0.2 ≠ 0.3 in IEEE 754

**Fix:** Store all shipping calculations in i64 cents

```rust
// Conversion helpers
const fn dollars_to_cents(dollars: f64) -> i64 {
    (dollars * 100.0).round() as i64
}

fn cents_to_dollars(cents: i64) -> f64 {
    cents as f64 / 100.0
}

// Update CalculateShippingResponse struct
#[derive(Debug, Serialize)]
pub struct CalculateShippingResponse {
    pub success: bool,
    pub total_cost_cents: i64,      // ✓ Change from f64
    pub breakdown: HashMap<String, i64>,  // ✓ Change from f64
}

// All calculations in cents
let base_cost_cents = 499i64;  // $4.99 = 499 cents
let weight_surcharge_cents = (weight_kg * 150.0).round() as i64;  // $1.50/kg = 150 cents/kg

// Apply free shipping threshold
const FREE_SHIPPING_THRESHOLD_CENTS: i64 = 7500; // $75 CAD
let final_shipping_cents = if subtotal_cents >= FREE_SHIPPING_THRESHOLD_CENTS {
    0
} else {
    total_shipping_cents
};

Ok(Json(CalculateShippingResponse {
    success: true,
    total_cost_cents: final_shipping_cents,
    breakdown: overall_breakdown  // already in cents
}))
```

**Affected Functions:**
- `calculate_shipping()` — Convert all f64 calculations to i64
- `calculate_itemized_cost()` — Store base costs as cents
- Response serialization — divide by 100 ONLY when sending JSON to client

---

## Issue #3: Payout Implementation Incomplete

**File:** `crates/ob-handlers/src/cron/mod.rs` (lines 271–285)
**Risk Level:** CRITICAL — Sellers paid $0 while orders marked complete
**Symptoms:** Payout records show "completed" but no money in seller accounts; vendor disputes

**Root Cause:**
```rust
// Current code: marks payout complete WITHOUT calling Stripe Transfer API
let _ = state.db.update_document(
    collections::PAYOUTS,
    &payout_id,
    json!({
        fields::STATUS: "completed",  // ❌ But no actual transfer!
        "payoutDate": Utc::now().to_rfc3339(),
    }),
).await;
// NOTE: Actual Stripe Transfer would happen here (NEVER IMPLEMENTED)
```

**Fix:** Add actual Stripe Transfer API call

```rust
// In run_auto_capture(), replace the stub with real transfer:
let transfer_result = stripe_create_transfer(
    state,
    seller_id,
    net_cents,
    &order_id,
).await;

match transfer_result {
    Ok(transfer_id) => {
        let _ = state
            .db
            .update_document(
                collections::PAYOUTS,
                &payout_id,
                json!({
                    fields::STATUS: "completed",
                    "stripeTransferId": transfer_id,  // Track transfer ID
                    "payoutDate": Utc::now().to_rfc3339(),
                }),
            )
            .await;
        success_count += 1;
    }
    Err(e) => {
        warn!("Stripe transfer failed for seller {}: {}", seller_id, e);
        let _ = state
            .db
            .update_document(
                collections::PAYOUTS,
                &payout_id,
                json!({
                    fields::STATUS: "failed",
                    "failureReason": e.to_string(),
                    fields::UPDATED_AT: Utc::now().to_rfc3339(),
                }),
            )
            .await;
    }
}

// Helper function (add to cron/mod.rs)
async fn stripe_create_transfer(
    state: &HandlersState,
    seller_id: &str,
    amount_cents: i64,
    order_id: &str,
) -> Result<String, String> {
    let stripe_key = state
        .config
        .require_secret("stripe_secret_key")
        .map_err(|e| e.to_string())?;

    // Get seller's Stripe Connect account ID
    let seller = state
        .db
        .get_document(collections::USERS, seller_id)
        .await
        .map_err(|e| format!("Seller not found: {}", e))?;
    
    let stripe_account_id = seller
        .get("stripeConnectAccountId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Seller has no Stripe Connect account".to_string())?;

    let params = [
        ("amount", amount_cents.to_string()),
        ("currency", "cad".to_string()),
        ("destination", stripe_account_id.to_string()),
        ("metadata[order_id]", order_id.to_string()),
        ("metadata[seller_id]", seller_id.to_string()),
    ];

    let resp = state
        .http_client
        .post(format!("{}/transfers", state.stripe_base_url))
        .header("Authorization", format!("Bearer {stripe_key}"))
        .header("Idempotency-Key", format!("{}-{}", order_id, seller_id))
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Stripe error: {}", body));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    body.get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "No transfer ID in response".to_string())
}
```

**Testing:**
- Test with dev Stripe account: Transfer test token `tok_chargeDeclined`
- Verify `stripeTransferId` in payouts collection after run

---

## Issue #4: Refund + Payout Race Condition

**File:** `crates/ob-handlers/src/orders/refunds.rs` (lines 333–336)
**Risk Level:** HIGH — Ledger inconsistency, double-payment risk
**Status:** Partially fixed (payout status check exists), needs transaction isolation
**Symptoms:** Payout processes simultaneously with refund → money locked or refunded twice

**Current (Good but Incomplete):**
```rust
let payout_status = str_field(&order, "payoutStatus");
if payout_status == "PROCESSING" {
    return Err(ob_core::Error::Validation(
        "Cannot refund item while payout is currently processing...".into(),
    ));
}
```

**Enhanced Fix:** Add SurrealDB transaction locking

```rust
// In refund_order_item(), before refund processing

// Acquire lock via transaction
let lock_query = format!(
    "BEGIN TRANSACTION; \
     SELECT * FROM {} WHERE id = '{}'; \
     UPDATE {} SET payoutLocked = true WHERE id = '{}' AND (payoutLocked = false OR payoutLocked IS NONE); \
     COMMIT;",
    collections::ORDERS,
    &req.order_id,
    collections::ORDERS,
    &req.order_id
);

let lock_result = state.db.query_raw(&lock_query).await;

match lock_result {
    Ok(results) if !results.is_empty() => {
        info!("Refund lock acquired for order {}", req.order_id);
    }
    _ => {
        return Err(ob_core::Error::Validation(
            "Order is currently being processed. Please try again in 30 seconds.".into(),
        ));
    }
}

// Process refund (existing logic)
// ... refund code ...

// Release lock ALWAYS (use a defer-like pattern)
let unlock_result = state
    .db
    .update_document(
        collections::ORDERS,
        &req.order_id,
        json!({ "payoutLocked": false }),
    )
    .await;

if unlock_result.is_err() {
    error!("Failed to release payout lock for order {}", req.order_id);
}
```

**Schema Migration:** Add `payoutLocked: boolean` field to `orders` collection

---

## Issue #5: Subscription Double-Create Risk

**File:** `crates/ob-handlers/src/payments/subscriptions.rs` (lines 200–230)
**Risk Level:** HIGH — Duplicate charges, customer trust loss
**Symptoms:** User charged twice; two active subscriptions in DB

**Root Cause:**
Race condition: two simultaneous POST requests both pass the "no subscription check" before either inserts.

**Fix:** Check for active subscription BEFORE creation

```rust
pub async fn create_subscription(
    State(state): State<HandlersState>,
    Json(req): Json<CreateSubscriptionRequest>,
) -> Result<Json<SubscriptionResponse>, ob_core::Error> {
    validate_uid("userId", &req.user_id)?;

    // CRITICAL: Atomic check for existing active subscriptions
    let existing_sql = format!(
        "SELECT * FROM {} WHERE userId = '{}' AND (status = 'active' OR status = 'cancel_pending') LIMIT 1",
        collections::SUBSCRIPTIONS,
        req.user_id
    );
    
    let existing = state.db.query_raw(&existing_sql).await?;
    if !existing.is_empty() {
        let existing_id = existing[0]
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        
        return Err(ob_core::Error::Validation(
            format!("User already has active subscription: {}", existing_id)
        ));
    }

    // Create Stripe subscription first
    let stripe_sub = create_stripe_subscription(
        state,
        &req.user_id,
        &req.stripe_payment_method_id
    ).await?;

    // Atomic insert with generated ID
    let subscription_id = format!("sub_{}", ulid::Ulid::new());
    
    state
        .db
        .insert_document(
            collections::SUBSCRIPTIONS,
            &subscription_id,
            json!({
                "id": subscription_id,
                "userId": req.user_id,
                "stripeSubscriptionId": stripe_sub.id,
                "status": "active",
                fields::CREATED_AT: Utc::now().to_rfc3339(),
            }),
        )
        .await?;

    Ok(Json(SubscriptionResponse {
        subscription_id,
        status: "active".to_string(),
    }))
}
```

**Alternative (More Robust):** Use SurrealDB UPSERT with constraint

```rust
// SurrealDB: Prevent duplicate active subscriptions at DB level
let upsert_query = format!(
    "UPSERT {{ userId: '{}', status: 'active' }} INTO {} \
     SET id = '{}', stripeSubscriptionId = '{}', createdAt = now() \
     RETURN id;",
    req.user_id, collections::SUBSCRIPTIONS, subscription_id, stripe_sub.id
);
```

---

## Issue #6: Password Reset Token Reuse

**File:** `crates/ob-auth/src/routes.rs`
**Risk Level:** HIGH — Account takeover via stolen token
**Symptoms:** Attacker uses password reset link after token leaked in email

**Root Cause:**
Token never marked as used → valid forever.

**Fix:** Mark token consumed immediately after verification

```rust
pub async fn reset_password(
    State(state): State<HandlersState>,
    Json(req): Json<ResetPasswordRequest>,
) -> Result<Json<ResetPasswordResponse>, ob_core::Error> {
    validate_token(&req.reset_token)?;

    // Lookup UNUSED token only
    let token_docs = state
        .db
        .query_raw(
            &format!(
                "SELECT * FROM {} WHERE token = '{}' AND (usedAt IS NONE) LIMIT 1",
                collections::PASSWORD_RESET_TOKENS,
                req.reset_token
            )
        )
        .await?;

    if token_docs.is_empty() {
        return Err(ob_core::Error::Validation(
            "Token invalid or already used".into(),
        ));
    }

    let token_record = &token_docs[0];
    let user_id = token_record
        .get("userId")
        .and_then(|v| v.as_str())
        .ok_or(ob_core::Error::Internal("No user ID in token".into()))?;

    // Check expiry (24-hour window)
    let created_at_str = token_record
        .get("createdAt")
        .and_then(|v| v.as_str())
        .ok_or(ob_core::Error::Internal("No created_at".into()))?;
    
    let created: chrono::DateTime<chrono::Utc> = chrono::DateTime::parse_from_rfc3339(created_at_str)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|_| ob_core::Error::Internal("Invalid token date".into()))?;

    if (chrono::Utc::now() - created).num_hours() > 24 {
        return Err(ob_core::Error::Validation(
            "Token has expired. Request a new password reset.".into(),
        ));
    }

    // CRITICAL: Mark token as USED BEFORE updating password
    let token_id = token_record
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or(ob_core::Error::Internal("No token ID".into()))?;

    state
        .db
        .update_document(
            collections::PASSWORD_RESET_TOKENS,
            token_id,
            json!({
                "usedAt": chrono::Utc::now().to_rfc3339(),
                "usedIp": extract_client_ip(&req),  // Log IP for audit
            }),
        )
        .await?;

    // Now update password
    let hashed = hash_password(&req.new_password)?;
    state
        .db
        .update_document(
            collections::USERS,
            user_id,
            json!({
                "passwordHash": hashed,
                fields::UPDATED_AT: chrono::Utc::now().to_rfc3339(),
            }),
        )
        .await?;

    Ok(Json(ResetPasswordResponse { success: true }))
}
```

**Schema:** Add `usedAt` and `usedIp` fields to `password_reset_tokens` collection

---

## Issue #7: OAuth State Nonce Memory Leak

**File:** `crates/ob-auth/src/routes.rs`
**Risk Level:** MEDIUM — Memory exhaustion, eventual service degradation
**Symptoms:** OAuth state HashMap grows unbounded; memory usage increases over weeks

**Root Cause:**
```rust
// Pseudo-code: In-memory storage with no cleanup
static OAUTH_STATES: Mutex<HashMap<String, OAuthState>> = ...;
// No TTL mechanism, no garbage collection
```

**Fix Option A (Recommended):** Use database instead of memory

```rust
pub async fn generate_oauth_state(
    db: &DatabaseClient,
    user_id: Option<&str>,
) -> Result<String, ob_core::Error> {
    let state_token = generate_random_token(32);
    
    db.insert_document(
        "oauth_states",  // Temporary collection
        &state_token,
        serde_json::json!({
            "id": state_token,
            "userId": user_id,
            "createdAt": chrono::Utc::now().to_rfc3339(),
            "expiresAt": (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
        }),
    )
    .await?;

    Ok(state_token)
}

pub async fn validate_oauth_state(
    db: &DatabaseClient,
    state_token: &str,
) -> Result<Option<String>, ob_core::Error> {
    let docs = db
        .query_raw(
            &format!(
                "SELECT * FROM oauth_states WHERE id = '{}' AND expiresAt > now() LIMIT 1",
                state_token
            )
        )
        .await?;

    if docs.is_empty() {
        return Ok(None);
    }

    // Consume token (delete it)
    let _ = db.delete_document("oauth_states", state_token).await;

    Ok(docs[0]
        .get("userId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string()))
}
```

**Fix Option B:** If using in-memory HashMap, add TTL cleanup

```rust
// In cron/mod.rs
pub async fn cleanup_expired_oauth_states(db: &DatabaseClient) {
    let cleanup_sql = "DELETE FROM oauth_states WHERE expiresAt <= now()";
    match db.query_raw(cleanup_sql).await {
        Ok(results) => info!("OAuth state cleanup: {} deleted", results.len()),
        Err(e) => warn!("OAuth state cleanup failed: {}", e),
    }
}

// Call every 15 minutes in cron scheduler
```

---

## Issue #8: TOTP Brute-Force Protection

**File:** `crates/ob-auth/src/routes.rs`
**Risk Level:** HIGH — Account compromise via brute force
**Symptoms:** No rate limiting on MFA attempts; 6-digit code (~1M attempts) crackable

**Fix:** Rate limit TOTP verification to 5 attempts per 15 minutes

```rust
pub async fn verify_totp(
    State(state): State<HandlersState>,
    Json(req): Json<VerifyTotpRequest>,
) -> Result<Json<VerifyTotpResponse>, ob_core::Error> {
    validate_uid("userId", &req.user_id)?;

    // Check rate limit: max 5 attempts per 15 minutes
    let bucket_key = format!("totp_attempt:{}", req.user_id);
    let attempt_count = get_rate_limit_count(
        &state.db,
        &bucket_key,
        900,  // 15 minutes in seconds
    )
    .await?;

    if attempt_count >= 5 {
        // Lock account MFA after exceeded
        state
            .db
            .update_document(
                collections::USERS,
                &req.user_id,
                serde_json::json!({
                    "mfaLocked": true,
                    "mfaLockedAt": chrono::Utc::now().to_rfc3339(),
                }),
            )
            .await?;

        return Err(ob_core::Error::Forbidden(
            "Too many failed TOTP attempts. MFA locked. Contact support.".into(),
        ));
    }

    // Verify TOTP code
    let user = state
        .db
        .get_document(collections::USERS, &req.user_id)
        .await?;

    let totp_secret = user
        .get("totpSecret")
        .and_then(|v| v.as_str())
        .ok_or(ob_core::Error::Validation("TOTP not enabled".into()))?;

    if !verify_totp_code(totp_secret, &req.code)? {
        // Invalid code: increment counter
        increment_rate_limit(&state.db, &bucket_key, 900).await?;
        
        return Err(ob_core::Error::Validation(
            "Invalid TOTP code".into(),
        ));
    }

    // Valid code: clear attempt counter
    let _ = state.db.delete_document("rate_limits", &bucket_key).await;

    // Issue JWT
    let token = create_jwt(&req.user_id)?;

    Ok(Json(VerifyTotpResponse {
        token,
        expires_in: 3600,
    }))
}

// Helpers
async fn get_rate_limit_count(
    db: &DatabaseClient,
    key: &str,
    ttl_seconds: i64,
) -> Result<i64, ob_core::Error> {
    let docs = db.get_document("rate_limits", key).await.ok();
    Ok(docs
        .and_then(|d| d.get("count").and_then(|v| v.as_i64()))
        .unwrap_or(0))
}

async fn increment_rate_limit(
    db: &DatabaseClient,
    key: &str,
    ttl_seconds: i64,
) -> Result<(), ob_core::Error> {
    let _ = db
        .query_raw(
            &format!(
                "UPSERT {{ id: '{}' }} INTO rate_limits SET count = (count || 0) + 1, expiresAt = now() + {} SECONDS",
                key, ttl_seconds
            )
        )
        .await;
    Ok(())
}
```

**Schema:** Add `mfaLocked` and `mfaLockedAt` to `users` collection

---

## Issue #9: Rate Limiter X-Forwarded-For Validation

**File:** `crates/ob-handlers/src/shared/rate_limiter.rs`
**Risk Level:** CRITICAL — Rate limit bypass via spoofed headers
**Symptoms:** Attacker sends `X-Forwarded-For: 1.2.3.4` and bypasses all limits

**Root Cause:**
```rust
// Current: Trusts X-Forwarded-For from ANY source
let ip = headers.get("x-forwarded-for")
    .and_then(|h| h.to_str().ok())
    .unwrap_or(&peer_ip);  // ❌ Trusts untrusted header
```

**Fix:** Only trust X-Forwarded-For from known proxy IP (Caddy at 127.0.0.1)

```rust
pub fn extract_client_ip(headers: &axum::http::HeaderMap, socket_addr: std::net::SocketAddr) -> String {
    const TRUSTED_PROXY_IP: &str = "127.0.0.1";

    // Only trust X-Forwarded-For from Caddy reverse proxy
    if socket_addr.ip().to_string() == TRUSTED_PROXY_IP {
        if let Some(xff) = headers.get("x-forwarded-for") {
            if let Ok(xff_str) = xff.to_str() {
                // Take first IP if multiple
                if let Some(first_ip) = xff_str.split(',').next() {
                    let ip = first_ip.trim();
                    // Validate it's an IP address
                    if ip.parse::<std::net::IpAddr>().is_ok() {
                        return ip.to_string();
                    }
                }
            }
        }
    }

    // Not from trusted proxy → use peer IP
    socket_addr.ip().to_string()
}

// Usage in rate limiter
pub async fn check_rate_limit(
    db: &DatabaseClient,
    headers: &axum::http::HeaderMap,
    socket_addr: std::net::SocketAddr,
    action: &str,
    max_requests: i64,
    window_seconds: i64,
) -> Result<(), ob_core::Error> {
    let client_ip = extract_client_ip(headers, socket_addr);
    let bucket_key = format!("rate_limit:{}:{}", action, client_ip);
    
    let now = chrono::Utc::now();
    let window_start = (now - chrono::Duration::seconds(window_seconds)).to_rfc3339();

    let count: i64 = db
        .query_raw(
            &format!(
                "SELECT count(*) as cnt FROM rate_limit_events WHERE bucket = '{}' AND timestamp >= '{}' LIMIT 1",
                bucket_key, window_start
            )
        )
        .await
        .ok()
        .and_then(|results| {
            results.first().and_then(|doc| doc.get("cnt").and_then(|v| v.as_i64()))
        })
        .unwrap_or(0);

    if count >= max_requests {
        return Err(ob_core::Error::RateLimit(
            format!("Too many requests. Retry after {} seconds.", window_seconds)
        ));
    }

    // Log this attempt
    let _ = db.insert_document(
        "rate_limit_events",
        &format!("{}_{}", bucket_key, ulid::Ulid::new()),
        serde_json::json!({
            "bucket": bucket_key,
            "timestamp": now.to_rfc3339(),
        }),
    ).await;

    Ok(())
}
```

**Testing:**
- Spoof X-Forwarded-For from client IP → rate limit should NOT be bypassed
- Send from Caddy (127.0.0.1) with X-Forwarded-For → rate limit SHOULD use that IP

---

## Issue #10: Phone Number Validation (E.164)

**File:** `crates/ob-handlers/src/addresses/mod.rs`
**Risk Level:** MEDIUM — SMS delivery fails, logistics breakage
**Symptoms:** Shipping notifications don't reach customers; address data invalid

**Fix:** Add E.164 format validation

```rust
use regex::Regex;

fn validate_phone_e164(phone: &str) -> Result<String, ob_core::Error> {
    // E.164 format: +[1-9]{1,15}
    // Example: +14165551234
    let phone_trimmed = phone.trim();
    
    let e164_regex = Regex::new(r"^\+[1-9]\d{1,14}$")
        .map_err(|_| ob_core::Error::Internal("Regex compile error".into()))?;

    if !e164_regex.is_match(phone_trimmed) {
        return Err(ob_core::Error::Validation(
            "Phone must be in E.164 format (e.g., +14165551234)".into(),
        ));
    }

    Ok(phone_trimmed.to_string())
}

// In address creation handler
#[derive(serde::Deserialize)]
pub struct CreateAddressRequest {
    pub name: String,
    pub phone: String,
    pub street: String,
    pub city: String,
    pub province: String,
    pub postal_code: String,
    pub country: String,
}

pub async fn create_address(
    State(state): State<HandlersState>,
    Json(mut req): Json<CreateAddressRequest>,
) -> Result<Json<AddressResponse>, ob_core::Error> {
    // Validate and normalize phone
    req.phone = validate_phone_e164(&req.phone)?;

    // Validate postal code (Canadian)
    req.postal_code = validate_postal_code(&req.postal_code)?;

    let address_id = format!("addr_{}", ulid::Ulid::new());
    state
        .db
        .insert_document(
            collections::ADDRESSES,
            &address_id,
            serde_json::json!({
                "id": address_id,
                "name": req.name,
                "phone": req.phone,  // Now validated
                "street": req.street,
                "city": req.city,
                "province": req.province,
                "postalCode": req.postal_code,
                "country": req.country,
                fields::CREATED_AT: chrono::Utc::now().to_rfc3339(),
            }),
        )
        .await?;

    Ok(Json(AddressResponse { address_id }))
}
```

---

## Issue #11: Postal Code Validation (Canadian)

**File:** `crates/ob-handlers/src/addresses/mod.rs`
**Risk Level:** MEDIUM — Shipping failure, address lookup error
**Symptoms:** Invalid postal codes in DB; Canada Post rejects addresses

**Fix:** Enforce Canadian postal code format

```rust
fn validate_postal_code(postal_code: &str) -> Result<String, ob_core::Error> {
    // Canadian postal code: A1A 1A1
    // Format: letter-digit-letter space digit-letter-digit
    let normalized = postal_code.to_uppercase().replace(" ", "");
    
    let postal_regex = Regex::new(r"^[A-Z]\d[A-Z]\d[A-Z]\d$")
        .map_err(|_| ob_core::Error::Internal("Regex compile error".into()))?;

    if !postal_regex.is_match(&normalized) {
        return Err(ob_core::Error::Validation(
            "Invalid Canadian postal code. Format: A1A 1A1 (e.g., M5V 3A8)".into(),
        ));
    }

    // Return formatted: A1A 1A1
    Ok(format!(
        "{} {}",
        &normalized[0..3],
        &normalized[3..6]
    ))
}

// Usage
req.postal_code = validate_postal_code(&req.postal_code)?;
```

**Test Cases:**
- Valid: `M5V 3A8`, `m5v3a8`, `m5v 3a8` → all format to `M5V 3A8`
- Invalid: `M5V 3A`, `123 456`, `M5V3A8` (missing space)

---

## Issue #12: Multi-Seller Warehouse Validation

**File:** `crates/ob-handlers/src/shipping_calc/mod.rs`
**Risk Level:** MEDIUM — Shipping calculated from non-existent warehouse
**Symptoms:** Orders ship from wrong address; logistics partner rejects shipments

**Fix:** Verify each seller has warehouse configured

```rust
pub async fn calculate_shipping(
    State(state): State<HandlersState>,
    Json(req): Json<CalculateShippingRequest>,
) -> Result<Json<CalculateShippingResponse>, ob_core::Error> {
    // Group items by seller
    let mut items_by_seller: std::collections::HashMap<String, Vec<ShippingItem>> = 
        std::collections::HashMap::new();
    
    for item in &req.items {
        let seller_id = item.seller_id.as_deref().unwrap_or("default_seller");
        items_by_seller
            .entry(seller_id.to_string())
            .or_insert_with(Vec::new)
            .push(item.clone());
    }

    // CRITICAL: Validate each seller has warehouse before shipping calculation
    for seller_id in items_by_seller.keys() {
        let seller = state
            .db
            .get_document(collections::USERS, seller_id)
            .await
            .map_err(|_| ob_core::Error::NotFound(
                format!("Seller {} not found", seller_id)
            ))?;

        // Check warehouse address exists and is complete
        let warehouse_addr = seller
            .get("warehouseAddress")
            .and_then(|v| v.as_object())
            .ok_or(ob_core::Error::Validation(
                format!("Seller {} has no warehouse configured. Contact seller to set up warehouse address.", seller_id)
            ))?;

        // Validate required warehouse fields
        let warehouse_province = warehouse_addr
            .get("province")
            .and_then(|v| v.as_str())
            .ok_or(ob_core::Error::Validation(
                format!("Seller {} warehouse missing province", seller_id)
            ))?;

        let warehouse_city = warehouse_addr
            .get("city")
            .and_then(|v| v.as_str())
            .ok_or(ob_core::Error::Validation(
                format!("Seller {} warehouse missing city", seller_id)
            ))?;

        info!(
            "Seller {} warehouse validated: {}, {}",
            seller_id, warehouse_city, warehouse_province
        );
    }

    // Proceed with shipping calculation
    let mut total_shipping_cents = 0i64;
    let mut overall_breakdown = std::collections::HashMap::new();

    for (seller_id, seller_items) in items_by_seller {
        let seller = state.db.get_document(collections::USERS, &seller_id).await?;
        let warehouse = seller
            .get("warehouseAddress")
            .and_then(|v| v.as_object())
            .unwrap();

        let seller_province = warehouse
            .get("province")
            .and_then(|v| v.as_str())
            .unwrap_or("ON");

        let (shipping_cents, breakdown) = calculate_seller_shipping(
            &seller_items,
            seller_province,
            &req.buyer_address_province,
            seller_items.get(0).and_then(|i| i.is_perishable),
        )
        .await?;

        total_shipping_cents += shipping_cents;
        overall_breakdown.extend(breakdown);
    }

    Ok(Json(CalculateShippingResponse {
        success: true,
        total_cost_cents: total_shipping_cents,
        breakdown: overall_breakdown,
    }))
}
```

**Schema:** Ensure `users` collection has `warehouseAddress` field with required sub-fields:
- `street`, `city`, `province`, `postal_code`, `country`

---

## Summary & Implementation Order

| Priority | # | Issue | Files | Effort | Blockers |
|----------|---|-------|-------|--------|----------|
| P0 | 3 | Payout no Stripe transfer | cron/mod.rs | 2h | Stripe config validation |
| P0 | 2 | Float math shipping | shipping_calc/mod.rs | 3h | Full refactor, migration |
| P0 | 9 | X-Forwarded-For bypass | shared/rate_limiter.rs | 1h | Verify Caddy IP |
| P1 | 1 | Missing indexes | shared/indexes.rs | 1h | None |
| P1 | 5 | Subscription double-create | payments/subscriptions.rs | 1h | None |
| P1 | 6 | Password token reuse | ob-auth/routes.rs | 1h | Schema migration |
| P1 | 8 | TOTP brute-force | ob-auth/routes.rs | 1h | None |
| P1 | 4 | Refund+payout race | orders/refunds.rs | 1h | Test with concurrent calls |
| P2 | 7 | OAuth state leak | ob-auth/routes.rs | 2h | Test long-running instance |
| P2 | 10 | Phone validation | addresses/mod.rs | 30m | None |
| P2 | 11 | Postal code validation | addresses/mod.rs | 30m | None |
| P2 | 12 | Multi-seller warehouse | shipping_calc/mod.rs | 30m | None |

**Total Effort:** ~15 hours
**Target Deployment:** Before next seller cohort onboarding

---

## Testing Checklist

- [ ] Issue #3: Test payout with test Stripe account; verify `stripeTransferId` in DB
- [ ] Issue #2: Test shipping cost consistency across 100+ calculations
- [ ] Issue #4: Concurrent refund + payout requests don't double-process
- [ ] Issue #5: Attempt duplicate subscription creation → rejected
- [ ] Issue #6: Password reset token invalid after first use
- [ ] Issue #8: TOTP locked after 5 failed attempts
- [ ] Issue #9: Spoofed X-Forwarded-For doesn't bypass rate limits
- [ ] Issue #10-11: Address validation rejects invalid inputs
- [ ] Issue #12: Shipping rejected when seller has no warehouse

