import http from 'k6/http';
import { check, sleep } from 'k6';

const BASE_URL = __ENV.OB_BASE_URL || 'http://localhost:8080';

export const options = {
  scenarios: {
    large: { executor: 'constant-vus', vus: 5, duration: '1m' },
  },
  thresholds: {
    http_req_failed: ['rate<0.20'],
  },
};

function generateNestedObject(depth) {
  if (depth <= 0) return 'leaf';
  return { nested: generateNestedObject(depth - 1), level: depth };
}

function generateLargeArray(size) {
  return Array.from({ length: size }, (_, i) => ({ idx: i, data: 'x'.repeat(100) }));
}

export function setup() {
  const email = `k6_large_${Date.now()}@test.com`;
  const res = http.post(`${BASE_URL}/auth/register`, JSON.stringify({
    email, password: 'TestPassword123!'
  }), { headers: { 'Content-Type': 'application/json' } });
  return { token: JSON.parse(res.body).access_token };
}

export default function (data) {
  const headers = {
    'Content-Type': 'application/json',
    Authorization: `Bearer ${data.token}`,
  };

  // 1MB string body
  const bigString = { content: 'x'.repeat(1024 * 1024) };
  const res1 = http.post(`${BASE_URL}/graphql`,
    JSON.stringify({ query: `mutation { create(collection: "stress_large", data: ${JSON.stringify(JSON.stringify(bigString))}) }` }),
    { headers }
  );
  check(res1, { '1MB handled': (r) => r.status === 200 || r.status === 400 || r.status === 413 });

  // Deeply nested (50 levels)
  const nested = { deep: generateNestedObject(50) };
  const res2 = http.post(`${BASE_URL}/graphql`,
    JSON.stringify({ query: `mutation { create(collection: "stress_nested", data: ${JSON.stringify(JSON.stringify(nested))}) }` }),
    { headers }
  );
  check(res2, { 'nested handled': (r) => r.status < 500 });

  // Large array (1K elements)
  const arr = { items: generateLargeArray(1000) };
  const res3 = http.post(`${BASE_URL}/graphql`,
    JSON.stringify({ query: `mutation { create(collection: "stress_array", data: ${JSON.stringify(JSON.stringify(arr))}) }` }),
    { headers }
  );
  check(res3, { 'array handled': (r) => r.status < 500 });

  sleep(1);
}
