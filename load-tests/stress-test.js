import http from 'k6/http';
import { check, sleep } from 'k6';
import { Rate, Counter } from 'k6/metrics';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';
const errorRate = new Rate('errors');
const requests = new Counter('total_requests');

export const options = {
  stages: [
    { duration: '5s', target: 5 },     // warm up
    { duration: '15s', target: 15 },    // stress (pushing 8GB RAM)
    { duration: '10s', target: 20 },    // spike
    { duration: '10s', target: 5 },     // recovery
    { duration: '5s', target: 0 },      // cool down
  ],
  thresholds: {
    errors: ['rate<0.3'],               // allow higher errors under stress
    http_req_duration: ['p(95)<5000'],   // p95 < 5s under stress
  },
};

export default function () {
  requests.add(1);

  // Mix of operations
  const op = Math.random();

  if (op < 0.4) {
    // 40%: Health check (lightweight)
    const res = http.get(`${BASE_URL}/health`);
    check(res, { 'health ok': (r) => r.status === 200 });
  } else if (op < 0.7) {
    // 30%: Auth (register + login)
    const email = `stress_${__VU}_${__ITER}_${Date.now()}@test.origna.ca`;
    const res = http.post(`${BASE_URL}/auth/register`, JSON.stringify({
      email,
      password: 'StressTest123!',
    }), { headers: { 'Content-Type': 'application/json' } });

    check(res, {
      'register ok or rate limited': (r) => r.status === 200 || r.status === 429,
    }) || errorRate.add(1);
  } else if (op < 0.9) {
    // 20%: GraphQL introspection (read-only, no auth)
    const res = http.post(`${BASE_URL}/graphql`, JSON.stringify({
      query: '{ __schema { queryType { name } } }',
    }), { headers: { 'Content-Type': 'application/json' } });

    check(res, {
      'introspection ok': (r) => r.status === 200,
    }) || errorRate.add(1);
  } else {
    // 10%: Large payload (test body size limits)
    const largeBody = 'x'.repeat(50000);
    const res = http.post(`${BASE_URL}/auth/register`, JSON.stringify({
      email: `large_${Date.now()}@test.origna.ca`,
      password: 'StressTest123!',
      display_name: largeBody,
    }), { headers: { 'Content-Type': 'application/json' } });

    check(res, {
      'large payload handled': (r) => r.status !== 500,
    }) || errorRate.add(1);
  }

  sleep(0.1);
}
