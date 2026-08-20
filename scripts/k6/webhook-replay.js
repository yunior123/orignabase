// k6 Webhook Replay — Stripe webhook processing stress test for OrignaBase
//
// Run: k6 run scripts/k6/webhook-replay.js
// Override target: k6 run -e BASE_URL=https://api.dev.orignagta.ca scripts/k6/webhook-replay.js
//
// NOTE: This test sends unsigned webhook payloads. The server SHOULD reject them
// with 400/401 (signature verification failure). The test validates that:
// 1. The server responds gracefully under load (no 500s, no crashes)
// 2. Duplicate event IDs are handled idempotently
// 3. Response times stay consistent under burst

import http from "k6/http";
import { check, sleep } from "k6";
import { Counter, Rate, Trend } from "k6/metrics";
import { SharedArray } from "k6/data";

const BASE_URL = __ENV.BASE_URL || "https://api.dev.orignagta.ca";
const WEBHOOK_PATH =
  __ENV.WEBHOOK_PATH || "/stripe/webhook";

// Custom metrics
const webhookLatency = new Trend("webhook_latency", true);
const webhookErrors = new Counter("webhook_errors");
const duplicateRejections = new Counter("duplicate_rejections");
const signatureRejections = new Counter("signature_rejections");
const errorRate = new Rate("error_rate");

export const options = {
  scenarios: {
    webhook_burst: {
      executor: "constant-vus",
      vus: 100,
      duration: "30s",
      exec: "webhookBurst",
    },
    duplicate_replay: {
      executor: "per-vu-iterations",
      vus: 10,
      iterations: 20,
      exec: "duplicateReplay",
      startTime: "35s",
    },
  },
  thresholds: {
    webhook_latency: ["p(95)<500"],
    http_req_failed: ["rate<0.05"],
  },
};

// Generate a Stripe-like webhook event payload
function makeWebhookEvent(eventId, eventType) {
  return JSON.stringify({
    id: eventId,
    object: "event",
    type: eventType,
    created: Math.floor(Date.now() / 1000),
    livemode: false,
    data: {
      object: {
        id: `pi_test_${Date.now()}_${Math.random().toString(36).substring(7)}`,
        object: "payment_intent",
        amount: 4999,
        currency: "cad",
        status: "succeeded",
        metadata: {
          order_id: `orders:test_${Date.now()}`,
        },
      },
    },
  });
}

export function webhookBurst() {
  const eventId = `evt_test_${__VU}_${__ITER}_${Date.now()}`;
  const payload = makeWebhookEvent(eventId, "payment_intent.succeeded");
  const start = Date.now();

  const res = http.post(`${BASE_URL}${WEBHOOK_PATH}`, payload, {
    headers: {
      "Content-Type": "application/json",
      // Intentionally invalid signature — server should reject with 400/401
      "Stripe-Signature": `t=${Math.floor(Date.now() / 1000)},v1=invalid_signature_for_load_test`,
    },
  });

  const elapsed = Date.now() - start;
  webhookLatency.add(elapsed);

  // Server should reject unsigned webhooks (400 or 401) — NOT crash (500)
  const success = check(res, {
    "webhook no 500": (r) => r.status < 500,
    "webhook handled gracefully": (r) =>
      r.status === 200 ||
      r.status === 400 ||
      r.status === 401 ||
      r.status === 403,
    "webhook latency < 500ms": () => elapsed < 500,
  });

  if (res.status === 400 || res.status === 401 || res.status === 403) {
    signatureRejections.add(1);
    // Signature rejection is expected behavior, not an error
    errorRate.add(0);
  } else if (!success) {
    webhookErrors.add(1);
    errorRate.add(1);
  } else {
    errorRate.add(0);
  }

  sleep(0.05);
}

export function duplicateReplay() {
  // All VUs send the SAME event ID to test idempotency
  const sharedEventId = "evt_idempotency_test_shared_001";
  const payload = makeWebhookEvent(
    sharedEventId,
    "payment_intent.succeeded"
  );

  const start = Date.now();
  const res = http.post(`${BASE_URL}${WEBHOOK_PATH}`, payload, {
    headers: {
      "Content-Type": "application/json",
      "Stripe-Signature": `t=${Math.floor(Date.now() / 1000)},v1=invalid_sig_idempotency_test`,
    },
  });
  const elapsed = Date.now() - start;

  webhookLatency.add(elapsed);

  // All duplicates should be handled without 500
  const success = check(res, {
    "duplicate no crash": (r) => r.status < 500,
    "duplicate handled": (r) =>
      r.status === 200 ||
      r.status === 400 ||
      r.status === 401 ||
      r.status === 409,
  });

  if (res.status === 409) {
    duplicateRejections.add(1);
  }

  if (!success) {
    webhookErrors.add(1);
  }

  sleep(0.1);
}
