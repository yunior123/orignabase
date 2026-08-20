---
name: WebSocket Realtime Security Audit 2026-03-25
description: First-ever security audit of ob-realtime crate (websocket.rs, registry.rs, dispatcher.rs, cluster.rs). 2 CRITICAL, 5 WARNING, 3 INFO findings.
type: project
---

Audit date: 2026-03-25. Scope: ob-realtime crate only. No prior audit existed.

## CRITICAL

1. **Unauthenticated presence endpoints** — `GET /presence` and `GET /presence/{user_id}` at websocket.rs:249–255 have zero auth. Any anonymous caller gets full list of online user IDs, connection IDs, and metadata. Fix: add JWT middleware to both routes before they are merged in main.rs:1210.

2. **No per-user authorization on dispatched change events** — dispatcher.rs:32 calls `find_all_for_collection()` which returns ALL subscribers for a collection regardless of user_id. Every subscriber to `orders` receives every other user's order change. `Subscription.user_id` is stored (registry.rs:49) but never checked by the dispatcher before sending. Fix: filter in dispatcher — check `sub.user_id` against `event.data["buyerId"]` / `event.data["sellerId"]`, or block collection-wide subscriptions to private collections (orders, users, addresses, return_requests) for non-admin roles.

## WARNING

3. **No collection allowlist** — websocket.rs:177–206, clients can subscribe to any collection string including `webhook_events`, `seller_metrics`, `admin_*`. Fix: maintain collection→allowed roles mapping.

4. **No inbound WS message size limit** — websocket.rs:175, `DefaultBodyLimit::max(2MB)` does not apply to WS frames. Attacker sends 50MB JSON blob causing OOM. Fix: guard `if text.len() > 64_000 { continue }` before `serde_json::from_str`.

5. **Slow-consumer bridge task can stall** — Bridge task (websocket.rs:144–154) uses `.await` send into 256-slot channel. Slow client fills channel, bridge stalls indefinitely. No timeout, no connection kill. Fix: use `try_send` or per-send timeout in bridge task.

6. **No per-user/per-IP WebSocket connection limit** — MAX_SUBS_PER_CONNECTION=100 (websocket.rs:16) but no limit on concurrent connections per user_id or IP. 1000 connections × 100 subs = 100K registry entries. Fix: track active WS connections per user_id in registry, reject upgrade at >5 per user.

7. **JWT in URL query param logged by Caddy/access logs** — websocket.rs:27–30, `?token=<jwt>` appears in VPS access logs. Fix: log only first 8 chars; keep JWT expiry short.

8. **Unbounded presence metadata** — websocket.rs:213–220, client supplies arbitrary JSON stored and broadcast to all presence observers. Fix: allowlist keys, enforce 1KB max, flat string-only values.

9. **NATS cluster events have no authentication** — cluster.rs:163–183, only guard is `origin_node != self_node`. Any internal service can forge a ClusterEvent. Fix: NATS mTLS + HMAC signature field on ClusterEvent.

## INFO

- No idle connection timeout — server never evicts clients that stop sending Pings. Fix: watchdog per connection checking last_seen.
- Dispatcher channel capacity 1024 drops order status events silently under burst. Fix: client-side fallback polling for critical collections.
- JetStream 1h retention causes stale event replay on node restart. Fix: `deliver_policy: DeliverNew` or client dedup on timestamp.

**Why this matters:** The critical authorization gap (finding #2) means a buyer connected to the realtime channel can receive payment and address data from other buyers' orders in real time — direct PCI/privacy violation. Fix before any production load.
