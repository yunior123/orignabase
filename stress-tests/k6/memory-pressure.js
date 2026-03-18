import http from 'k6/http';
import exec from 'k6/execution';
import { check, fail, sleep } from 'k6';

const BASE_URL = __ENV.OB_BASE_URL || 'http://localhost:8080';
const PASSWORD = __ENV.OB_STRESS_PASSWORD || 'TestPassword123!';
const TOTAL_DOCS = Number(__ENV.OB_MEMORY_DOCS || 10000);
const PAYLOAD_BYTES = Number(__ENV.OB_MEMORY_PAYLOAD_BYTES || 262144);

export const options = {
  scenarios: {
    memory_pressure: {
      executor: 'ramping-vus',
      startVUs: 0,
      stages: [
        { duration: '10s', target: 50 },
        { duration: '40s', target: 150 },
        { duration: '10s', target: 0 },
      ],
      gracefulRampDown: '5s',
    },
  },
  thresholds: {
    http_req_failed: ['rate<0.10'],
    http_req_duration: ['p(95)<5000'],
    oom_errors: ['count==0'],
  },
};

const OOM_PATTERNS = [
  'out of memory',
  'oom',
  'allocation failed',
  'memory exhausted',
  'cannot allocate',
];

function randomEmail(prefix) {
  return `${prefix}_${exec.vu.idInTest}_${Date.now()}_${Math.random().toString(36).slice(2)}@example.com`;
}

function registerUser() {
  const email = randomEmail('memory');
  const response = http.post(
    `${BASE_URL}/auth/register`,
    JSON.stringify({ email, password: PASSWORD }),
    { headers: { 'Content-Type': 'application/json' } }
  );

  check(response, {
    'register succeeded': (r) => r.status === 200,
  });

  if (response.status !== 200) {
    fail(`auth/register failed with ${response.status}: ${response.body}`);
  }

  const body = JSON.parse(response.body);
  return body.access_token;
}

function createPayload(docIndex) {
  return JSON.stringify({
    title: `memory-doc-${docIndex}`,
    sequence: docIndex,
    createdAt: new Date().toISOString(),
    metadata: {
      source: 'k6-memory-pressure',
      vu: exec.vu.idInTest,
      iteration: exec.scenario.iterationInTest,
    },
    tags: Array.from({ length: 32 }, (_, i) => `tag-${i}`),
    blob: 'x'.repeat(PAYLOAD_BYTES),
  });
}

function gql(token, query) {
  return http.post(
    `${BASE_URL}/graphql`,
    JSON.stringify({ query }),
    {
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${token}`,
      },
      tags: { endpoint: 'graphql' },
    }
  );
}

export function setup() {
  return {
    token: registerUser(),
    collection: `memory_pressure_${Date.now()}`,
  };
}

export default function (data) {
  const docIndex = exec.scenario.iterationInTest;
  if (docIndex >= TOTAL_DOCS) {
    sleep(1);
    return;
  }

  const payload = createPayload(docIndex);
  const escaped = JSON.stringify(payload);
  const query = `mutation { create(collection: "${data.collection}", data: ${escaped}) }`;
  const response = gql(data.token, query);
  const body = (response.body || '').toLowerCase();

  const sawOom = OOM_PATTERNS.some((pattern) => body.includes(pattern));
  if (sawOom) {
    exec.vu.metrics.tags = { failure: 'oom' };
  }

  check(response, {
    'create handled request': (r) => r.status === 200 || r.status === 400 || r.status === 413,
    'server did not report oom': () => !sawOom,
  });

  if (sawOom) {
    fail(`possible OOM signal in response body: ${response.body.slice(0, 400)}`);
  }
}

