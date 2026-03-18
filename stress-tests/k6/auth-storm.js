import http from 'k6/http';
import { check } from 'k6';
import { Counter } from 'k6/metrics';

const BASE_URL = __ENV.OB_BASE_URL || 'http://localhost:8080';
const rateLimited = new Counter('rate_limited');

export const options = {
  scenarios: {
    storm: {
      executor: 'constant-arrival-rate',
      rate: 100,
      timeUnit: '1s',
      duration: '10s',
      preAllocatedVUs: 200,
    },
  },
};

export default function () {
  const res = http.post(`${BASE_URL}/auth/login`, JSON.stringify({
    email: 'nonexistent@test.com',
    password: 'WrongPassword123!',
  }), { headers: { 'Content-Type': 'application/json' } });

  check(res, { 'not 5xx': (r) => r.status < 500 });
  if (res.status === 429) { rateLimited.add(1); }
}

export function handleSummary(data) {
  const limited = data.metrics.rate_limited ? data.metrics.rate_limited.values.count : 0;
  const total = data.metrics.http_reqs.values.count;
  console.log(`Rate limited: ${limited}/${total} (${((limited/total)*100).toFixed(1)}%)`);
  return {};
}
