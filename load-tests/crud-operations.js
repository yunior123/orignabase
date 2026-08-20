import http from 'k6/http';
import { check, sleep } from 'k6';
import { Rate } from 'k6/metrics';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';
const errorRate = new Rate('errors');

export const options = {
  stages: [
    { duration: '10s', target: 5 },
    { duration: '30s', target: 8 },
    { duration: '10s', target: 0 },
  ],
  thresholds: {
    errors: ['rate<0.1'],
    http_req_duration: ['p(95)<3000'],
  },
};

function getToken() {
  const uniqueId = `${__VU}_${__ITER}_${Date.now()}`;
  const email = `k6crud_${uniqueId}@loadtest.origna.ca`;
  const res = http.post(`${BASE_URL}/auth/register`, JSON.stringify({
    email,
    password: 'LoadTest123!',
  }), { headers: { 'Content-Type': 'application/json' } });
  try { return JSON.parse(res.body).access_token; }
  catch { return ''; }
}

export default function () {
  const token = getToken();
  if (!token) { errorRate.add(1); return; }

  const headers = {
    'Content-Type': 'application/json',
    'Authorization': `Bearer ${token}`,
  };

  // Create product via GraphQL
  const createRes = http.post(`${BASE_URL}/graphql`, JSON.stringify({
    query: `mutation { create(collection: "products", data: {name: "K6 Product ${__VU}", priceCents: 1000, stockQuantity: 5, lifecycleStatus: "draft", isDigital: false, isPerishable: false}) }`,
  }), { headers });

  const createOk = check(createRes, {
    'create product 200': (r) => r.status === 200,
    'create no errors': (r) => {
      try { return !JSON.parse(r.body).errors; }
      catch { return false; }
    },
  });
  if (!createOk) errorRate.add(1);

  // List products
  const listRes = http.post(`${BASE_URL}/graphql`, JSON.stringify({
    query: '{ list(collection: "products", limit: 10) }',
  }), { headers });

  check(listRes, {
    'list products 200': (r) => r.status === 200,
  }) || errorRate.add(1);

  sleep(0.3);
}
