import http from 'k6/http';
import { check, sleep } from 'k6';

const BASE_URL = __ENV.OB_BASE_URL || 'http://localhost:8080';
const CONCURRENCY = Number(__ENV.OB_CONNECTION_VUS || 600);

export const options = {
  vus: CONCURRENCY,
  duration: __ENV.OB_CONNECTION_DURATION || '30s',
  noVUConnectionReuse: true,
  thresholds: {
    http_req_failed: ['rate<0.20'],
    http_req_duration: ['p(95)<4000'],
  },
};

const PATHS = ['/health', '/_admin/health', '/graphql'];

function pickPath(iteration) {
  return PATHS[iteration % PATHS.length];
}

export default function () {
  const path = pickPath(__ITER);
  const params = {
    headers: { 'Content-Type': 'application/json' },
    timeout: __ENV.OB_CONNECTION_TIMEOUT || '10s',
    tags: { endpoint: path },
  };

  const response =
    path === '/graphql'
      ? http.post(`${BASE_URL}${path}`, JSON.stringify({ query: '{ __typename }' }), params)
      : http.get(`${BASE_URL}${path}`, params);

  check(response, {
    'server stayed responsive': (r) => [200, 400, 401, 404, 429, 503].includes(r.status),
    'no generic 5xx storm': (r) => !(r.status >= 500 && r.status !== 503),
  });

  sleep(0.1);
}

