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
    const email = `k6_search_${randomString(8)}@test.com`;
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
    const queries = ['shoe', 'shirt', 'hat', 'test', randomString(5)];
    const q = queries[Math.floor(Math.random() * queries.length)];

    // Search via GraphQL
    const res = http.post(`${BASE_URL}/graphql`,
        JSON.stringify({ query: `{ search(collection: "products", query: "${q}", limit: 5) }` }),
        { headers }
    );
    check(res, { 'search responds': (r) => r.status === 200 });
    sleep(0.5);
}
