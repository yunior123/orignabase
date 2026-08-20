// k6 Search Stress — Meilisearch endpoint stress test for OrignaBase
//
// Run: k6 run scripts/k6/search-stress.js
// Override target: k6 run -e BASE_URL=https://api.dev.orignagta.ca scripts/k6/search-stress.js

import http from "k6/http";
import { check, sleep } from "k6";
import { Counter, Rate, Trend } from "k6/metrics";

const BASE_URL = __ENV.BASE_URL || "https://api.dev.orignagta.ca";

// Custom metrics
const searchLatency = new Trend("search_latency", true);
const searchErrors = new Counter("search_errors");
const errorRate = new Rate("error_rate");

export const options = {
  scenarios: {
    search_flood: {
      executor: "constant-vus",
      vus: 200,
      duration: "60s",
      exec: "searchFlood",
    },
  },
  thresholds: {
    search_latency: ["p(95)<300"],
    error_rate: ["rate<0.01"],
    http_req_failed: ["rate<0.02"],
  },
};

// Random search queries that simulate real user behavior
const SEARCH_QUERIES = [
  "shoes",
  "electronics",
  "laptop",
  "phone case",
  "organic food",
  "winter jacket",
  "maple syrup",
  "headphones",
  "camera",
  "backpack",
  "coffee maker",
  "yoga mat",
  "desk lamp",
  "water bottle",
  "running shoes",
  "bluetooth speaker",
  "kitchen knife",
  "notebook",
  "sunglasses",
  "watch",
  "t-shirt",
  "vitamins",
  "candle",
  "pillow",
  "umbrella",
  // Typos / partial matches (common in real search)
  "lapt",
  "sho",
  "elec",
  "phon",
  "cam",
];

// Register a shared user for authenticated search
let sharedToken = "";

export function setup() {
  const email = `k6_search_${Date.now()}@example.com`;
  const res = http.post(
    `${BASE_URL}/auth/register`,
    JSON.stringify({ email, password: "TestPassword123!" }),
    { headers: { "Content-Type": "application/json" } }
  );

  if (res.status === 200) {
    try {
      const body = JSON.parse(res.body);
      return { token: body.access_token };
    } catch {
      // Fall through
    }
  }

  // Fallback: try login
  const loginRes = http.post(
    `${BASE_URL}/auth/login`,
    JSON.stringify({ email, password: "TestPassword123!" }),
    { headers: { "Content-Type": "application/json" } }
  );

  try {
    const body = JSON.parse(loginRes.body);
    return { token: body.access_token || "" };
  } catch {
    return { token: "" };
  }
}

export function searchFlood(data) {
  const query = SEARCH_QUERIES[Math.floor(Math.random() * SEARCH_QUERIES.length)];
  const start = Date.now();

  // GraphQL search
  const graphqlQuery = `{ search(collection: "products", query: "${query}", limit: 20) }`;
  const headers = {
    "Content-Type": "application/json",
  };

  if (data.token) {
    headers["Authorization"] = `Bearer ${data.token}`;
  }

  const res = http.post(
    `${BASE_URL}/graphql`,
    JSON.stringify({ query: graphqlQuery }),
    { headers }
  );

  const elapsed = Date.now() - start;
  searchLatency.add(elapsed);

  const success = check(res, {
    "search status 200": (r) => r.status === 200,
    "search returns JSON": (r) => {
      try {
        JSON.parse(r.body);
        return true;
      } catch {
        return false;
      }
    },
    "search has data or errors": (r) => {
      try {
        const body = JSON.parse(r.body);
        return body.data !== undefined || body.errors !== undefined;
      } catch {
        return false;
      }
    },
    "search latency < 300ms": () => elapsed < 300,
  });

  if (!success) {
    searchErrors.add(1);
    errorRate.add(1);
  } else {
    errorRate.add(0);
  }

  // Simulate user typing delay (debounce 300ms)
  sleep(0.1);
}
