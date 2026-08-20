# Integration Tests for OrignaBase

These integration tests cover the HTTP API endpoints for all major repository operations. All tests require a running OrignaBase instance.

## Running Tests

### Start OrignaBase
```bash
# Terminal 1: Start PostgreSQL + Meilisearch
docker compose -f docker/docker-compose.yml up -d postgres meilisearch

# Terminal 2: OrignaBase server
cargo run -- serve
```

### Run All Integration Tests
```bash
# Auth tests (36 tests)
cargo test --test auth_repository_test -- --ignored

# User tests (16 tests) 
cargo test --test user_repository_test -- --ignored

# Product tests (25 tests)
cargo test --test product_repository_test -- --ignored

# Cart tests (20 tests)
cargo test --test cart_repository_test -- --ignored

# Order tests (14 tests)
cargo test --test order_repository_test -- --ignored

# Run all integration tests
cargo test -- --ignored
```

### Custom Test URL
```bash
OB_TEST_URL=http://localhost:3000 cargo test --test auth_repository_test -- --ignored
```

## Test Coverage

### auth_repository_test.rs (36 tests)
- Registration: success, missing fields, invalid email, weak password, duplicate email
- Login: success, missing fields, invalid password, nonexistent email
- Token validation: missing token, invalid token, expired token
- Logout & refresh
- Password reset (anti-enumeration tested)
- Rate limiting on auth endpoints

**Key assertions:**
- Registration returns `access_token` and `user` object
- Duplicate emails rejected with 400/409
- Invalid passwords return 401
- Nonexistent emails don't reveal existence (401/400, anti-enumeration)
- Protected endpoints require valid Bearer token

### user_repository_test.rs (16 tests)
- Profile: get (success), update (success, invalid phone), requires auth
- Addresses: add (success, validation errors), list, delete, requires auth
- Authorization: users cannot access other user profiles/addresses

**Key assertions:**
- Profile retrieval returns user object
- Phone validation enforces E.164 format
- Postal code validation for Canadian format
- Address deletion is idempotent (200 even if not found)
- Row-level security prevents cross-user access

### product_repository_test.rs (25 tests)
- CRUD: create (success, validation), list (with pagination & filters), get, update, delete
- Digital products (no shipping required)
- Perishable products (local delivery only)
- Search integration (Meilisearch)

**Key assertions:**
- Product creation returns `productId`
- Prices/stock must be non-negative integers (cents)
- Pagination with `limit` + `offset`
- Search filters by category, price range
- Deletion is idempotent (404 expected for missing products)

### cart_repository_test.rs (20 tests)
- Add items: success, validation (missing productId, invalid qty/price)
- Retrieve cart (may be empty)
- Remove items: success, validation
- Clear cart
- Update quantity: validation for zero/negative

**Key assertions:**
- Cart operations require authentication
- Quantities must be positive integers
- Prices in integer cents
- Missing productId rejected with 400/422
- Removing nonexistent items is safe (200 or 404)

### order_repository_test.rs (14 tests)
- Create: success, validation (missing items/sellerId)
- Listing: buyer orders, seller orders (with pagination)
- Get order by ID
- Cancel order (validation: must be pending)
- Confirm receipt (delivery confirmation)
- Money validation (no negative amounts)

**Key assertions:**
- Order creation requires: items[], sellerId, addresses, money values
- All monetary fields in integer cents (no floats)
- Buyer/seller order listings paginate separately
- Order IDs use format `orders:*`
- Cancellation only allowed in pending state
- Authentication required for all operations

## Test Patterns

### Helper Functions
```rust
// Register a test user
let (token, user_id, email) = register_test_user(&client).await;

// Make HTTP requests
let (status, body) = make_request(
    &client,
    "POST",
    "/api/endpoint",
    Some(&token),  // Bearer token (optional)
    Some(payload), // JSON body (optional)
).await;
```

### Authentication
- All tests register unique users via `test_<uuid>@example.com`
- Bearer token extracted from registration response
- Passed in `Authorization: Bearer <token>` header

### Assertions
- Status codes: expect 200/201 for success, 400/422 for validation, 401/403 for auth, 404 for not found
- Responses validated as JSON objects with expected fields
- Anti-enumeration tested (e.g., forgot-password doesn't reveal if email exists)

### Error Handling
- Missing required fields: 400 or 422
- Invalid authentication: 401 or 403
- Not found: 404
- Rate limiting: 429
- Invalid data format: 400 or 422

## Database Schema Notes

All monetary values are **integer cents** (e.g., $29.99 = 2999 cents):
- `priceCents`, `subtotalCents`, `taxAmountCents`, `totalAmountCents`

Timestamp fields vary by collection:
- Orders, users, payouts, return_requests: `createdAt`
- Products, cart: `dateCreated`
- Webhook events: `timestamp`

Document IDs use the `collection:id` format (e.g., `products:abc123`)

## Known Limitations

1. **No emulators**: Tests use dev OrignaBase server, not Firebase Emulator
2. **Test data**: Assumes test products/categories may not exist (some 404s expected)
3. **Payment flow**: Stripe webhook tests not included (require webhook replay)
4. **Search**: Meilisearch tests may return 501 if search backend disabled
5. **Seller features**: Requires proper seller role setup (some tests may fail if user isn't seller)

## Debugging Failed Tests

### Check server is running
```bash
curl http://localhost:8080/health
```

### Check test user can register
```bash
curl -X POST http://localhost:8080/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"test@example.com","password":"TestPassword123!"}'
```

### View full test output
```bash
cargo test --test auth_repository_test -- --ignored --nocapture
```

### Check database state
```bash
ssh root@204.168.137.16
# PostgreSQL
psql postgres://orignabase:orignabase_dev@localhost:5432/orignabase
```

## CI Integration

These tests are designed to run in GitHub Actions CI:
- Requires `OB_TEST_URL` environment variable pointing to running server
- All tests marked with `#[ignore]` to require explicit `--ignored` flag
- Recommended: run only on merge to main (too slow for every PR)
- Parallel workers: limit to 2-4 due to 8GB RAM constraint

## Future Enhancements

- [ ] Webhook signature verification tests
- [ ] Rate limiting threshold tests
- [ ] Concurrent order creation race conditions
- [ ] Stock restoration transaction atomicity
- [ ] Stripe Connect payout flow
- [ ] Email notification delivery
- [ ] Search index synchronization
