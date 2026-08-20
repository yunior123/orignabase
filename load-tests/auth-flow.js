import http from 'k6/http';
import { check, sleep } from 'k6';
import { Rate, Trend } from 'k6/metrics';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';
const errorRate = new Rate('errors');
const loginDuration = new Trend('login_duration');

export const options = {
  stages: [
    { duration: '10s', target: 5 },   // ramp up
    { duration: '20s', target: 10 },   // sustain (8GB RAM safe)
    { duration: '10s', target: 0 },    // ramp down
  ],
  thresholds: {
    errors: ['rate<0.1'],              // <10% error rate
    http_req_duration: ['p(95)<2000'],  // p95 < 2s
    login_duration: ['p(95)<1500'],
  },
};

export default function () {
  const uniqueId = `${__VU}_${__ITER}_${Date.now()}`;
  const email = `k6_${uniqueId}@loadtest.origna.ca`;
  const password = 'LoadTest123!';

  // Register
  const registerRes = http.post(`${BASE_URL}/auth/register`, JSON.stringify({
    email,
    password,
  }), { headers: { 'Content-Type': 'application/json' } });

  check(registerRes, {
    'register status 200': (r) => r.status === 200,
    'register has token': (r) => {
      try { return JSON.parse(r.body).access_token !== undefined; }
      catch { return false; }
    },
  }) || errorRate.add(1);

  // Login
  const loginStart = Date.now();
  const loginRes = http.post(`${BASE_URL}/auth/login`, JSON.stringify({
    email,
    password,
  }), { headers: { 'Content-Type': 'application/json' } });
  loginDuration.add(Date.now() - loginStart);

  check(loginRes, {
    'login status 200': (r) => r.status === 200,
    'login has token': (r) => {
      try { return JSON.parse(r.body).access_token !== undefined; }
      catch { return false; }
    },
  }) || errorRate.add(1);

  // Health check
  const healthRes = http.get(`${BASE_URL}/health`);
  check(healthRes, {
    'health ok': (r) => r.status === 200,
  });

  sleep(0.5);
}
