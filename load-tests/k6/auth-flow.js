import http from 'k6/http';
import { check, sleep } from 'k6';
import { randomString } from 'https://jslib.k6.io/k6-utils/1.4.0/index.js';

const BASE_URL = __ENV.K6_BASE_URL || 'https://api.orignagta.ca';

export const options = {
    scenarios: {
        smoke: { executor: 'constant-vus', vus: 1, duration: '30s', tags: { profile: 'smoke' } },
    },
};

export default function () {
    const email = `k6_${randomString(8)}@test.com`;
    const password = 'TestPassword123!';

    // Register
    const regRes = http.post(`${BASE_URL}/auth/register`, JSON.stringify({ email, password }), {
        headers: { 'Content-Type': 'application/json' },
    });
    check(regRes, { 'register 200': (r) => r.status === 200 });

    // Login
    const loginRes = http.post(`${BASE_URL}/auth/login`, JSON.stringify({ email, password }), {
        headers: { 'Content-Type': 'application/json' },
    });
    check(loginRes, { 'login 200': (r) => r.status === 200 });

    if (loginRes.status === 200) {
        const body = JSON.parse(loginRes.body);
        const refreshToken = body.refresh_token;

        // Token refresh
        const refreshRes = http.post(`${BASE_URL}/auth/refresh`,
            JSON.stringify({ refresh_token: refreshToken }),
            { headers: { 'Content-Type': 'application/json' } }
        );
        check(refreshRes, { 'refresh 200': (r) => r.status === 200 });
    }

    sleep(1);
}
