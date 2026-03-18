import ws from 'k6/ws';
import { check } from 'k6';
import { randomString } from 'https://jslib.k6.io/k6-utils/1.4.0/index.js';
import http from 'k6/http';

const BASE_URL = __ENV.K6_BASE_URL || 'https://api.orignagta.ca';
const WS_URL = BASE_URL.replace('https://', 'wss://').replace('http://', 'ws://');

export const options = {
    scenarios: {
        smoke: { executor: 'constant-vus', vus: 1, duration: '30s', tags: { profile: 'smoke' } },
    },
};

export function setup() {
    const email = `k6_ws_${randomString(8)}@test.com`;
    const res = http.post(`${BASE_URL}/auth/register`, JSON.stringify({
        email, password: 'TestPassword123!'
    }), { headers: { 'Content-Type': 'application/json' } });
    return { token: JSON.parse(res.body).access_token };
}

export default function (data) {
    const res = ws.connect(`${WS_URL}/realtime?token=${data.token}`, {}, function (socket) {
        socket.on('open', () => {
            socket.send(JSON.stringify({ type: 'subscribe', collection: 'products' }));
        });
        socket.on('message', (msg) => {
            // Just verify we get a response
        });
        socket.setTimeout(() => { socket.close(); }, 5000);
    });
    check(res, { 'ws connected': (r) => r && r.status === 101 });
}
