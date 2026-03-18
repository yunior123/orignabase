import http from 'k6/http';
import { check, sleep, group } from 'k6';
import { randomString } from 'https://jslib.k6.io/k6-utils/1.4.0/index.js';

const BASE_URL = __ENV.K6_BASE_URL || 'https://api.orignagta.ca';

export const options = {
    scenarios: {
        smoke:  { executor: 'constant-vus', vus: 1,   duration: '30s',  tags: { profile: 'smoke' } },
        low:    { executor: 'constant-vus', vus: 10,  duration: '2m',   startTime: '31s', tags: { profile: 'low' } },
        medium: { executor: 'constant-vus', vus: 50,  duration: '5m',   startTime: '3m',  tags: { profile: 'medium' } },
    },
    thresholds: {
        http_req_duration: ['p(95)<2000'],
        http_req_failed: ['rate<0.05'],
    },
};

export function setup() {
    const email = `k6_mixed_${randomString(8)}@test.com`;
    const res = http.post(`${BASE_URL}/auth/register`, JSON.stringify({
        email, password: 'TestPassword123!'
    }), { headers: { 'Content-Type': 'application/json' } });
    const body = JSON.parse(res.body);
    return { token: body.access_token, collection: `k6_mixed_${Date.now()}` };
}

export default function (data) {
    const headers = {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${data.token}`,
    };

    group('health', () => {
        check(http.get(`${BASE_URL}/health`), { 'health 200': (r) => r.status === 200 });
    });

    group('auth', () => {
        const email = `k6_${randomString(8)}@test.com`;
        http.post(`${BASE_URL}/auth/register`, JSON.stringify({
            email, password: 'TestPassword123!'
        }), { headers: { 'Content-Type': 'application/json' } });
    });

    group('crud', () => {
        const docData = JSON.stringify({ name: randomString(10) });
        const escaped = JSON.stringify(docData);
        http.post(`${BASE_URL}/graphql`,
            JSON.stringify({ query: `mutation { create(collection: "${data.collection}", data: ${escaped}) }` }),
            { headers });
        http.post(`${BASE_URL}/graphql`,
            JSON.stringify({ query: `{ list(collection: "${data.collection}", limit: 10) }` }),
            { headers });
    });

    group('graphql', () => {
        http.post(`${BASE_URL}/graphql`, JSON.stringify({
            query: '{ __schema { types { name } } }'
        }), { headers });
    });

    sleep(Math.random() * 2);
}
