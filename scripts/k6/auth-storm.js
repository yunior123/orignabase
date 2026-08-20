// k6 Auth Storm — Stress test for OrignaBase auth endpoints
//
// Run: k6 run scripts/k6/auth-storm.js
// Override target: k6 run -e BASE_URL=https://api.dev.orignagta.ca scripts/k6/auth-storm.js

import http from "k6/http";
import { check, sleep } from "k6";
import { Counter, Rate, Trend } from "k6/metrics";

const BASE_URL = __ENV.BASE_URL || "https://api.dev.orignagta.ca";

// Custom metrics
const loginLatency = new Trend("login_latency", true);
const registerLatency = new Trend("register_latency", true);
const loginErrors = new Counter("login_errors");
const registerErrors = new Counter("register_errors");
const errorRate = new Rate("error_rate");

export const options = {
  scenarios: {
    login_storm: {
      executor: "constant-vus",
      vus: 100,
      duration: "60s",
      exec: "loginStorm",
    },
    register_burst: {
      executor: "constant-vus",
      vus: 20,
      duration: "60s",
      exec: "registerBurst",
      startTime: "0s",
    },
  },
  thresholds: {
    login_latency: ["p(95)<500"],
    register_latency: ["p(95)<1000"],
    error_rate: ["rate<0.01"],
    http_req_failed: ["rate<0.01"],
  },
};

// Pre-register a shared test user for login storm
const SHARED_EMAIL = `k6_shared_${Date.now()}@example.com`;
const SHARED_PASSWORD = "TestPassword123!";

export function setup() {
  const registerRes = http.post(
    `${BASE_URL}/auth/register`,
    JSON.stringify({
      email: SHARED_EMAIL,
      password: SHARED_PASSWORD,
    }),
    { headers: { "Content-Type": "application/json" } }
  );

  if (registerRes.status !== 200) {
    console.error(
      `Setup registration failed: ${registerRes.status} ${registerRes.body}`
    );
  }

  return { email: SHARED_EMAIL, password: SHARED_PASSWORD };
}

export function loginStorm(data) {
  const start = Date.now();
  const res = http.post(
    `${BASE_URL}/auth/login`,
    JSON.stringify({
      email: data.email,
      password: data.password,
    }),
    { headers: { "Content-Type": "application/json" } }
  );
  const elapsed = Date.now() - start;

  loginLatency.add(elapsed);

  const success = check(res, {
    "login status 200": (r) => r.status === 200,
    "login has access_token": (r) => {
      try {
        const body = JSON.parse(r.body);
        return !!body.access_token;
      } catch {
        return false;
      }
    },
    "login latency < 500ms": () => elapsed < 500,
  });

  if (!success) {
    loginErrors.add(1);
    errorRate.add(1);
  } else {
    errorRate.add(0);
  }

  sleep(0.1);
}

export function registerBurst() {
  const email = `k6_reg_${__VU}_${__ITER}_${Date.now()}@example.com`;
  const start = Date.now();

  const res = http.post(
    `${BASE_URL}/auth/register`,
    JSON.stringify({
      email: email,
      password: "TestPassword123!",
    }),
    { headers: { "Content-Type": "application/json" } }
  );
  const elapsed = Date.now() - start;

  registerLatency.add(elapsed);

  const success = check(res, {
    "register status 200": (r) => r.status === 200,
    "register has access_token": (r) => {
      try {
        const body = JSON.parse(r.body);
        return !!body.access_token;
      } catch {
        return false;
      }
    },
    "register latency < 1000ms": () => elapsed < 1000,
  });

  if (!success) {
    registerErrors.add(1);
    errorRate.add(1);
  } else {
    errorRate.add(0);
  }

  sleep(0.5);
}
