# New Integration Test Files

This document summarizes the new integration test files created to extend coverage of OrignaBase handlers.

## Files Created

All files are located in `crates/orignabase/tests/`

### 1. `search_integration_test.rs` (425 lines)
**Coverage**: Meilisearch product search, filtering, autocomplete

- **Section 1** (4 tests): Basic search queries
  - `test_400`: Empty query browse
  - `test_401`: Text query search
  - `test_402`: Pagination with limit/offset
  - `test_403`: Invalid limit handling

- **Section 2** (4 tests): Search filtering
  - `test_404`: Category filter
  - `test_405`: Price range filter
  - `test_406`: Seller ID filter
  - `test_407`: Multiple combined filters

- **Section 3** (3 tests): Sort operations
  - `test_408`: Sort by price ascending
  - `test_409`: Sort by price descending
  - `test_410`: Sort by creation date

- **Section 4** (2 tests): Autocomplete
  - `test_411`: Autocomplete with query
  - `test_412`: Empty autocomplete

### 2. `push_notifications_integration_test.rs` (468 lines)
**Coverage**: FCM push token registration, notifications, rate limiting

- **Section 1** (4 tests): Push token registration
  - `test_500`: Register token successfully
  - `test_501`: Missing required fields rejection
  - `test_502`: Empty token rejection
  - `test_503`: Multiple tokens per user

- **Section 2** (3 tests): Push token unregistration
  - `test_504`: Unregister token
  - `test_505`: Unregister nonexistent token (idempotent)
  - `test_506`: Empty token rejection

- **Section 3** (4 tests): Notification management
  - `test_507`: Get notification list
  - `test_508`: Mark single notification as read
  - `test_509`: Mark all notifications as read
  - `test_510`: Delete notification

- **Section 4** (2 tests): Rate limiting and pagination
  - `test_511`: Rate limit enforcement
  - `test_512`: Notification list pagination

### 3. `extended_handlers_test.rs` (506 lines)
**Coverage**: Shipping calculations, email validation, authentication edge cases

- **Section 1** (5 tests): Advanced shipping calculation
  - `test_600`: Perishable items local delivery (≤50km)
  - `test_601`: Cross-province shipping
  - `test_602`: Free shipping threshold ($75 CAD)
  - `test_603`: Multiple items weight combination
  - `test_604`: Invalid coordinate rejection

- **Section 2** (4 tests): Email validation and resend
  - `test_605`: Email verification resend
  - `test_606`: Invalid email format rejection
  - `test_607`: Email case insensitivity
  - `test_608`: Duplicate email prevention (409 Conflict)

- **Section 3** (3 tests): Unauthenticated access
  - `test_609`: Public product listing
  - `test_610`: Protected endpoints require auth
  - `test_611`: Invalid token rejection

- **Section 4** (2 tests): Sequential request handling
  - `test_612`: Sequential user operations
  - `test_613`: Sequential cart additions

### 4. `miscellaneous_handlers_test.rs` (484 lines)
**Coverage**: PDF generation, file operations, state transitions, idempotency

- **Section 1** (3 tests): PDF invoice generation
  - `test_700`: English invoice generation
  - `test_701`: French invoice generation
  - `test_702`: Missing order handling

- **Section 2** (3 tests): File operations
  - `test_703`: Product image upload
  - `test_704`: Digital product download
  - `test_705`: Invalid file type rejection

- **Section 3** (4 tests): State transition edge cases
  - `test_706`: Invalid order status transitions
  - `test_707`: Product draft → active lifecycle
  - `test_708`: Double payment prevention
  - `test_709`: Cancelled order reactivation (terminal state)

- **Section 4** (3 tests): Idempotency and error recovery
  - `test_710`: Idempotent cart additions
  - `test_711`: Double refund prevention
  - `test_712`: Connection timeout recovery

## Test Pattern

All tests follow the standard pattern used in `handlers_integration_test.rs`:

```rust
#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_NNN_description() {
    let client = reqwest::Client::new();
    let (token, user_id, email) = register_test_user(&client).await;
    
    let (status, body) = make_request(
        &client,
        "POST",
        "/api/endpoint",
        Some(&token),
        Some(json!({ /* body */ }))
    ).await;
    
    assert!(status == 200 || status == 400, "Expected behavior");
}
```

## Running the Tests

```bash
# Run all new tests
cargo test --test search_integration_test -- --ignored
cargo test --test push_notifications_integration_test -- --ignored
cargo test --test extended_handlers_test -- --ignored
cargo test --test miscellaneous_handlers_test -- --ignored

# Run specific test
cargo test --test search_integration_test test_400 -- --ignored
```

## Prerequisites

```bash
# Start SurrealDB
surreal start --user root --pass root memory

# Start OrignaBase
cargo run -- serve

# Optionally set custom URL
export OB_TEST_URL=http://localhost:8080
```

## Coverage Summary

| Area | File | Tests | Coverage |
|------|------|-------|----------|
| Search | search_integration_test.rs | 13 | Queries, filters, sort, autocomplete |
| Push/Notifications | push_notifications_integration_test.rs | 13 | Registration, management, rate limits |
| Shipping/Email/Auth | extended_handlers_test.rs | 14 | Advanced shipping, email validation, auth |
| PDF/Files/State/Idempotency | miscellaneous_handlers_test.rs | 13 | PDFs, files, state machines, idempotency |
| **TOTAL** | **4 files** | **53 tests** | **Gaps in existing coverage** |

## Gaps Addressed

These new tests cover previously untested or sparsely-tested areas:

- ✅ Meilisearch search product endpoints (no prior tests)
- ✅ Push notification FCM endpoints (no prior tests)
- ✅ Advanced shipping calculation scenarios (perishable, cross-province)
- ✅ Email format/case handling edge cases
- ✅ State transition validation (prevents invalid state changes)
- ✅ Idempotency patterns (cart, refunds, payments)
- ✅ PDF invoice generation (bilingual support)
- ✅ Rate limiting on push notifications
- ✅ File operations (upload/download with type validation)

## Notes

- All tests use `#[ignore]` to allow selective execution
- Tests are designed to be independently runnable
- No test data cleanup required (each uses unique IDs)
- Tests handle both success (200/201) and expected failures (400/404/409)
- Compatible with existing Cargo test infrastructure (auto-discovered)
