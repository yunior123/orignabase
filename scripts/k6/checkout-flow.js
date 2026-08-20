// k6 Checkout Flow — End-to-end checkout stress test for OrignaBase
//
// Run: k6 run scripts/k6/checkout-flow.js
// Override target: k6 run -e BASE_URL=https://api.dev.orignagta.ca scripts/k6/checkout-flow.js

import http from "k6/http";
import { check, sleep, group } from "k6";
import { Counter, Rate, Trend } from "k6/metrics";

const BASE_URL = __ENV.BASE_URL || "https://api.dev.orignagta.ca";

// Custom metrics
const checkoutLatency = new Trend("checkout_flow_latency", true);
const cartAddLatency = new Trend("cart_add_latency", true);
const checkoutErrors = new Counter("checkout_errors");
const errorRate = new Rate("error_rate");

export const options = {
  scenarios: {
    checkout_load: {
      executor: "constant-vus",
      vus: 50,
      duration: "120s",
      exec: "checkoutFlow",
    },
  },
  thresholds: {
    checkout_flow_latency: ["p(95)<2000"],
    cart_add_latency: ["p(95)<500"],
    error_rate: ["rate<0.005"],
    http_req_failed: ["rate<0.01"],
  },
};

function registerUser() {
  const email = `k6_checkout_${__VU}_${__ITER}_${Date.now()}@example.com`;
  const res = http.post(
    `${BASE_URL}/auth/register`,
    JSON.stringify({ email, password: "TestPassword123!" }),
    { headers: { "Content-Type": "application/json" } }
  );

  if (res.status !== 200) {
    return null;
  }

  try {
    const body = JSON.parse(res.body);
    return { token: body.access_token, email };
  } catch {
    return null;
  }
}

function graphql(token, query) {
  return http.post(
    `${BASE_URL}/graphql`,
    JSON.stringify({ query }),
    {
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${token}`,
      },
    }
  );
}

function createProduct(token) {
  const data = JSON.stringify({
    title: `k6-product-${Date.now()}`,
    priceCents: 1999,
    stockQuantity: 100,
    lifecycleStatus: "active",
    categoryId: "test",
  });
  const escaped = JSON.stringify(data);
  const query = `mutation { create(collection: "products", data: ${escaped}) }`;
  const res = graphql(token, query);

  try {
    const body = JSON.parse(res.body);
    const result = body.data?.create;
    return result?.id || result?._id || "";
  } catch {
    return "";
  }
}

function addToCart(token, productId) {
  const data = JSON.stringify({
    productId: productId,
    quantity: 1,
    priceCents: 1999,
    name: "k6 test product",
  });
  const escaped = JSON.stringify(data);
  const query = `mutation { create(collection: "cart", data: ${escaped}) }`;
  return graphql(token, query);
}

function initiateCheckout(token) {
  const res = http.post(
    `${BASE_URL}/payments/checkout`,
    JSON.stringify({
      shippingAddress: {
        line1: "123 Test St",
        city: "Toronto",
        state: "ON",
        postalCode: "M5V 1J2",
        country: "CA",
      },
    }),
    {
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${token}`,
      },
    }
  );
  return res;
}

export function checkoutFlow() {
  const flowStart = Date.now();

  group("register", () => {
    const user = registerUser();
    if (!user) {
      checkoutErrors.add(1);
      errorRate.add(1);
      return;
    }

    group("create_product", () => {
      const productId = createProduct(user.token);

      check(productId, {
        "product created": (id) => id.length > 0,
      });

      if (!productId) {
        checkoutErrors.add(1);
        errorRate.add(1);
        return;
      }

      group("add_to_cart", () => {
        const cartStart = Date.now();
        const cartRes = addToCart(user.token, productId);
        cartAddLatency.add(Date.now() - cartStart);

        const cartOk = check(cartRes, {
          "cart add status 200": (r) => r.status === 200,
        });

        if (!cartOk) {
          checkoutErrors.add(1);
          errorRate.add(1);
          return;
        }

        group("checkout", () => {
          const checkoutRes = initiateCheckout(user.token);
          const totalElapsed = Date.now() - flowStart;
          checkoutLatency.add(totalElapsed);

          // Checkout may return 200 (session URL) or 400/422 (no real Stripe in test)
          // We accept either — the point is the server handles it gracefully
          const ok = check(checkoutRes, {
            "checkout handled gracefully": (r) =>
              r.status === 200 ||
              r.status === 400 ||
              r.status === 422 ||
              r.status === 402,
            "checkout no 500": (r) => r.status < 500,
            "checkout latency < 2000ms": () => totalElapsed < 2000,
          });

          if (!ok) {
            checkoutErrors.add(1);
            errorRate.add(1);
          } else {
            errorRate.add(0);
          }
        });
      });
    });
  });

  sleep(1);
}
