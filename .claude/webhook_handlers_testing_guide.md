# Stripe Webhook Handlers — Testing Guide

## Test Environment Setup

### Prerequisites
- OrignaBase dev server running: `https://api.dev.orignagta.ca`
- Stripe test account with webhook endpoint configured
- Test orders with items in products table
- curl or Postman for manual webhook testing

### Test Data
```sql
-- Create test product
INSERT INTO products {
  id: 'e2e_test_product',
  productId: 'test-product-001',
  title: 'Test Widget',
  priceCents: 2999,  -- $29.99
  stockQuantity: 100,
  lifecycleStatus: 'active',
  sellerId: 'e2e_seller'
};

-- Create test user (buyer)
INSERT INTO users {
  id: 'e2e_buyer',
  email: 'buyer@test.com',
  roles: ['buyer']
};

-- Create test coupon
INSERT INTO coupons {
  id: 'test_coupon',
  code: 'SAVE10',
  discountPercent: 10,
  maxUses: 100,
  expiresAt: <future-date>
};
```

---

## Unit Tests (Run Locally)

### Signature Verification
```bash
cd crates/ob-handlers
cargo test payments::webhooks::tests --lib
```

**Expected output**:
```
test payments::webhooks::tests::test_signature_verification_valid ok
test payments::webhooks::tests::test_signature_verification_invalid ok
```

---

## Integration Tests

### Test 1: Happy Path (Payment Success)

**Scenario**: Customer completes checkout, payment succeeds.

**Steps**:
1. Create order in `pending` state:
```sql
INSERT INTO orders {
  id: 'orders:order_001',
  orderId: 'order-001',
  buyerId: 'e2e_buyer',
  sellerId: 'e2e_seller',
  items: [{
    productId: 'test-product-001',
    name: 'Test Widget',
    quantity: 2,
    unitPriceCents: 2999
  }],
  subtotalCents: 5998,
  taxAmountCents: 780,
  shippingCostCents: 1000,
  totalAmountCents: 7778,
  orderStatus: 'PENDING_PAYMENT',
  createdAt: now(),
  couponCode: 'SAVE10'  -- optional
};
```

2. Send webhook: `payment_intent.succeeded`
```bash
curl -X POST http://localhost:8080/api/webhooks/stripe \
  -H "Content-Type: application/json" \
  -H "stripe-signature: $(generate-stripe-sig.sh)" \
  -d '{
    "id": "evt_test_001",
    "type": "payment_intent.succeeded",
    "data": {
      "object": {
        "id": "pi_test_001",
        "metadata": {
          "order_id": "order-001",
          "coupon_code": "SAVE10"
        }
      }
    }
  }'
```

3. Verify order state:
```sql
SELECT * FROM orders WHERE orderId = 'order-001';
-- Expected: orderStatus = 'PAYMENT_AUTHORIZED', paymentIntentId = 'pi_test_001'
```

4. Verify stock decremented:
```sql
SELECT stockQuantity FROM products WHERE productId = 'test-product-001';
-- Expected: 98 (was 100, decremented by 2)
```

5. Verify coupon marked used:
```sql
SELECT * FROM coupon_uses WHERE orderId = 'order-001';
-- Expected: redeemedAt IS NOT NULL
```

**Expected outcome**: ✓ Order confirmed, stock decremented, coupon marked used

---

### Test 2: Failure Path (Payment Failed)

**Scenario**: Payment fails → order should be cancelled, coupon released.

**Setup**: Same order as Test 1, in `PENDING_PAYMENT` state.

**Steps**:
1. Send webhook: `payment_intent.payment_failed`
```bash
curl -X POST http://localhost:8080/api/webhooks/stripe \
  -H "Content-Type: application/json" \
  -H "stripe-signature: $(generate-stripe-sig.sh)" \
  -d '{
    "id": "evt_test_002",
    "type": "payment_intent.payment_failed",
    "data": {
      "object": {
        "id": "pi_test_002",
        "metadata": {
          "order_id": "order-002"
        }
      }
    }
  }'
```

2. Verify order cancelled:
```sql
SELECT orderStatus FROM orders WHERE orderId = 'order-002';
-- Expected: 'CANCELLED'
```

3. Verify coupon released:
```sql
SELECT COUNT(*) FROM coupon_uses WHERE orderId = 'order-002' AND redeemedAt IS NULL;
-- Expected: 0 (record deleted)
```

4. Verify stock NOT decremented:
```sql
SELECT stockQuantity FROM products WHERE productId = 'test-product-001';
-- Expected: 100 (unchanged, payment failed before decrement)
```

**Expected outcome**: ✓ Order cancelled, coupon released, stock untouched

---

### Test 3: Refund Path (Charge Refunded)

**Scenario**: Order delivered → customer requests refund → refund approved.

**Setup**:
1. Create order in `PAYMENT_AUTHORIZED` state (from Test 1)
2. Stock already decremented (quantity = 98)

**Steps**:
1. Send webhook: `charge.refunded`
```bash
curl -X POST http://localhost:8080/api/webhooks/stripe \
  -H "Content-Type: application/json" \
  -H "stripe-signature: $(generate-stripe-sig.sh)" \
  -d '{
    "id": "evt_test_003",
    "type": "charge.refunded",
    "data": {
      "object": {
        "id": "ch_test_003",
        "payment_intent": "pi_test_001",
        "amount_refunded": 7778
      }
    }
  }'
```

2. Verify stock restored:
```sql
SELECT stockQuantity FROM products WHERE productId = 'test-product-001';
-- Expected: 100 (was 98, restored by 2)
```

3. Verify refund recorded:
```sql
SELECT refundedAmountCents, refundedAt FROM orders WHERE orderId = 'order-001';
-- Expected: refundedAmountCents = 7778, refundedAt = current timestamp
```

**Expected outcome**: ✓ Stock restored, refund recorded

---

### Test 4: Idempotency (Duplicate Webhook)

**Scenario**: Webhook received twice (network retry).

**Setup**: Use same event ID from Test 1.

**Steps**:
1. Send `payment_intent.succeeded` webhook (first time)
   - Order status updates to `PAYMENT_AUTHORIZED`
   - Stock decrements from 100 → 98

2. Send identical webhook again (same event ID)
   ```bash
   curl -X POST http://localhost:8080/api/webhooks/stripe \
     -H "stripe-signature: $(generate-stripe-sig.sh)" \
     -d '{ ... same data as first request ... }'
   ```

3. Verify order NOT double-processed:
   ```sql
   SELECT orderStatus, COUNT(*) FROM orders WHERE orderId = 'order-001';
   -- Expected: orderStatus = 'PAYMENT_AUTHORIZED', count = 1 (not duplicated)
   ```

4. Verify stock NOT decremented twice:
   ```sql
   SELECT stockQuantity FROM products WHERE productId = 'test-product-001';
   -- Expected: 98 (not 96, second webhook ignored)
   ```

5. Check webhook_events table:
   ```sql
   SELECT COUNT(*) FROM webhook_events WHERE id = 'evt_test_001';
   -- Expected: 1 (stored only once)
   ```

**Expected outcome**: ✓ Duplicate ignored, state unchanged

---

### Test 5: Bounds Checking (Refund Exceeds Order Total)

**Scenario**: Refund amount > order total (payment processing bug prevention).

**Setup**: Create order with totalAmountCents = 5000.

**Steps**:
1. Send webhook: `charge.refunded` with amount_refunded = 6000 (exceeds total)
```bash
curl -X POST http://localhost:8080/api/webhooks/stripe \
  -H "stripe-signature: $(generate-stripe-sig.sh)" \
  -d '{
    "id": "evt_test_005",
    "type": "charge.refunded",
    "data": {
      "object": {
        "id": "ch_test_005",
        "payment_intent": "pi_test_005",
        "amount_refunded": 6000  -- EXCEEDS order total
      }
    }
  }'
```

2. Verify error returned (HTTP 200 but webhook marked failed):
```sql
SELECT processed, data FROM webhook_events WHERE id = 'evt_test_005';
-- Expected: processed = false (error logged)
```

3. Verify order NOT modified:
```sql
SELECT orderStatus, refundedAmountCents FROM orders WHERE orderId = 'order-005';
-- Expected: unchanged from before webhook
```

4. Verify stock NOT restored:
```sql
SELECT stockQuantity FROM products WHERE productId = 'test-product-001';
-- Expected: unchanged
```

**Expected outcome**: ✓ Validation error prevented invalid refund

---

### Test 6: Order Not Found

**Scenario**: Webhook arrives for non-existent order (data mismatch).

**Steps**:
1. Send webhook with order_id that doesn't exist:
```bash
curl -X POST http://localhost:8080/api/webhooks/stripe \
  -H "stripe-signature: $(generate-stripe-sig.sh)" \
  -d '{
    "id": "evt_test_006",
    "type": "payment_intent.succeeded",
    "data": {
      "object": {
        "id": "pi_test_006",
        "metadata": {
          "order_id": "nonexistent-order"
        }
      }
    }
  }'
```

2. Verify error logged:
```sql
SELECT data FROM webhook_events WHERE id = 'evt_test_006';
-- Expected: contains "Order ... not found"
```

3. Verify HTTP 200 returned (webhook accepted to prevent Stripe retries):
   - Manual curl should show: `{ "status": "error", ... }`

**Expected outcome**: ✓ Error handled gracefully, no duplicate retries

---

### Test 7: Missing Metadata

**Scenario**: Payment intent has no order_id in metadata.

**Steps**:
1. Send webhook without metadata:
```bash
curl -X POST http://localhost:8080/api/webhooks/stripe \
  -H "stripe-signature: $(generate-stripe-sig.sh)" \
  -d '{
    "id": "evt_test_007",
    "type": "payment_intent.succeeded",
    "data": {
      "object": {
        "id": "pi_test_007",
        "metadata": {}  -- empty
      }
    }
  }'
```

2. Verify validation error:
```sql
SELECT data FROM webhook_events WHERE id = 'evt_test_007';
-- Expected: error about missing "order_id"
```

**Expected outcome**: ✓ Validation caught before processing

---

## Load Testing

### Concurrent Webhook Simulation
```bash
#!/bin/bash
# Send 50 webhooks concurrently (same order, different payment intents)
for i in {1..50}; do
  curl -X POST http://localhost:8080/api/webhooks/stripe \
    -H "stripe-signature: ..." \
    -d "{ ... order-$i ... }" &
done
wait

# Verify all processed exactly once
SELECT COUNT(DISTINCT orderId) FROM orders WHERE orderStatus = 'PAYMENT_AUTHORIZED';
# Expected: 50

# Verify stock correct (no over-decrement)
SELECT stockQuantity FROM products WHERE productId = 'test-product-001';
# Expected: 100 - (50 orders * 2 items) = 0 (or correct total)
```

---

## Stripe CLI Testing (Local Dev)

### Forward Webhooks to Local Server
```bash
stripe listen --forward-to localhost:8080/api/webhooks/stripe
# Output: Ready! Your webhook signing secret is: whsec_test_...
```

### Trigger Test Events
```bash
stripe trigger payment_intent.succeeded
stripe trigger payment_intent.payment_failed
stripe trigger charge.refunded
```

### Monitor Webhook Logs
```bash
stripe logs tail
```

---

## Logging Checklist

After running each test, verify logs contain:

### Successful Payment
```
INFO: Payment intent succeeded: order confirmed, stock decremented
  order_id=order-001, payment_intent_id=pi_test_001
INFO: Stock decremented for order items
  order_id=order-001, item_count=2
INFO: Order status updated
  order_id=order-001, new_status=PAYMENT_AUTHORIZED
INFO: Coupon marked as redeemed
  order_id=order-001, coupon_code=SAVE10
```

### Failed Payment
```
WARN: Payment intent failed: order cancelled, coupon released
  order_id=order-002, payment_intent_id=pi_test_002
INFO: Coupon reservation released
  order_id=order-002
```

### Refund
```
INFO: Charge refunded: stock restored, order updated
  order_id=order-001, charge_id=ch_test_003, refunded_amount_cents=7778
INFO: Stock restored for order items
  order_id=order-001, item_count=2
```

### Duplicate
```
INFO: Duplicate webhook, skipping
  event_id=evt_test_001, event_type=payment_intent.succeeded
```

---

## Performance Benchmarks

Expected latencies (dev environment):

| Operation | Target | Notes |
|-----------|--------|-------|
| Find order | <10ms | Indexed query |
| Decrement stock | <20ms | Single transaction |
| Update order status | <10ms | Single UPDATE |
| Mark coupon | <15ms | Single UPDATE |
| Full webhook (happy path) | <100ms | 4–5 queries total |
| Duplicate check | <5ms | Webhook already stored |

**Load test goal**: 100+ webhooks/sec with <200ms p99 latency.

---

## Troubleshooting

### Order not confirming
1. Check webhook received: `SELECT * FROM webhook_events WHERE type = 'payment_intent.succeeded'`
2. Check order status: `SELECT orderStatus FROM orders WHERE orderId = 'order-001'`
3. Check logs for errors: `grep -i "error\|failed" webhook.log`
4. Verify payment intent ID matches: `SELECT paymentIntentId FROM orders WHERE orderId = 'order-001'`

### Stock not decremented
1. Verify order status is `PAYMENT_AUTHORIZED`: webhook must succeed first
2. Check transaction commit: `SELECT * FROM products WHERE productId = 'test-product-001'`
3. Verify product record ID format: `products:test-product-001` (not `test-product-001`)

### Coupon not marked used
1. Check coupon_uses table: `SELECT * FROM coupon_uses WHERE orderId = 'order-001'`
2. Verify coupon code in metadata: `SELECT data FROM webhook_events WHERE type = 'payment_intent.succeeded'`
3. Coupon not found is silent (OK) — only logs if update succeeds

### Duplicate webhook not ignored
1. Verify webhook_events record created: `SELECT * FROM webhook_events WHERE id = 'evt_test_001'`
2. Second webhook should hit `is_duplicate_webhook()` before processing
3. Check logs: should see "Duplicate webhook, skipping"

---

## Deployment Checklist

Before deploying to production:

- [ ] All 7 tests pass locally
- [ ] Load test: 100+ webhooks/sec
- [ ] Webhook signature verification enabled (`stripe_webhook_secret` set)
- [ ] Database migrations run (if any schema changes)
- [ ] Logs configured (Sentry, CloudWatch, etc.)
- [ ] Stripe webhook endpoint registered: `https://api.orignagta.ca/api/webhooks/stripe`
- [ ] Rate limiting verified (10 req/sec per IP)
- [ ] Error monitoring alerts set up
- [ ] Backup webhook queue (if infrastructure allows)

---

## Rollback Plan

If issues found in production:

1. **Immediate**: Disable webhook processing (set webhook secret to invalid)
2. **Investigation**: Review error logs, identify root cause
3. **Fix**: Deploy corrected code
4. **Validation**: Run full test suite locally
5. **Reprocess**: Manually replay failed webhooks from `webhook_events` table

---
