import http from 'k6/http';
import { check, sleep } from 'k6';
import { randomString } from 'https://jslib.k6.io/k6-utils/1.4.0/index.js';

const BASE_URL = __ENV.K6_BASE_URL || 'https://api.orignagta.ca';

export const options = {
    scenarios: {
        smoke: { executor: 'constant-vus', vus: 1, duration: '30s', tags: { profile: 'smoke' } },
    },
};

export function setup() {
    const email = `k6_crud_${randomString(8)}@test.com`;
    const res = http.post(`${BASE_URL}/auth/register`, JSON.stringify({
        email, password: 'TestPassword123!'
    }), { headers: { 'Content-Type': 'application/json' } });
    const body = JSON.parse(res.body);
    return { token: body.access_token, collection: `k6_crud_${Date.now()}` };
}

export default function (data) {
    const headers = {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${data.token}`,
    };

    // Create via GraphQL
    const docData = JSON.stringify({ name: randomString(10), value: Math.random() });
    const escaped = JSON.stringify(docData);
    const createRes = http.post(`${BASE_URL}/graphql`,
        JSON.stringify({ query: `mutation { create(collection: "${data.collection}", data: ${escaped}) }` }),
        { headers }
    );
    check(createRes, { 'create 200': (r) => r.status === 200 });

    // List via GraphQL
    const listRes = http.post(`${BASE_URL}/graphql`,
        JSON.stringify({ query: `{ list(collection: "${data.collection}", limit: 10) }` }),
        { headers }
    );
    check(listRes, { 'list 200': (r) => r.status === 200 });

    sleep(0.5);
}
