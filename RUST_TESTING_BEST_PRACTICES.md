# Rust Testing Best Practices, 2025-2026

As of **March 10, 2026**.

## 1. Integration Testing: `axum`, `tower::ServiceExt`, test clients

**Current crates**
- [`axum` 0.8.8](https://docs.rs/crate/axum/latest)
- [`tower` 0.5.3](https://docs.rs/crate/tower/latest)
- [`axum-test` 19.1.1](https://docs.rs/crate/axum-test/latest)
- [`tokio` 1.50.0](https://docs.rs/crate/tokio/latest)

**Use when**
- You want to test routing, extractors, middleware, headers, auth, and serialization together.
- You want either in-process service tests or HTTP-style end-to-end tests.

### Best practice
- Use `tower::ServiceExt::oneshot` for **fast in-process handler/router tests**.
- Use `axum_test::TestServer` for **higher-level HTTP tests** with cookies, JSON, headers, and websocket flows.
- Keep app construction in a helper like `fn app() -> Router`.
- Prefer asserting status, headers, and response body together.

### Example: fast in-process router test
```rust
use axum::{body::Body, http::{Request, StatusCode}, routing::get, Router};
use tower::util::ServiceExt;

fn app() -> Router {
    Router::new().route("/health", get(|| async { "ok" }))
}

#[tokio::test]
async fn health_check() {
    let response = app()
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
```

### Example: HTTP-style test client
```rust
use axum::{routing::post, Json, Router};
use axum_test::TestServer;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct CreateUser {
    name: String,
}

fn app() -> Router {
    Router::new().route("/users", post(|Json(payload): Json<CreateUser>| async move {
        Json(payload)
    }))
}

#[tokio::test]
async fn create_user_round_trip() {
    let server = TestServer::new(app()).unwrap();

    let response = server
        .post("/users")
        .json(&CreateUser { name: "Ada".into() })
        .await;

    response.assert_status_ok();
    response.assert_json(&serde_json::json!({ "name": "Ada" }));
}
```

### Gotchas
- `oneshot` bypasses a real socket; it is not a substitute for true network-level tests.
- Remember Tower utilities often require the `util` feature.
- For stateful tests, avoid global mutable state; inject state per test.
- If middleware depends on time or randomness, make those injectable.

---

## 2. Property-Based Testing: `proptest`

**Current crate**
- [`proptest` 1.10.0](https://docs.rs/crate/proptest/latest)

**Use when**
- You want to validate invariants across large input spaces.
- You want automatic shrinking to a minimal failing example.

### Best practice
- Write **properties**, not examples.
- Prefer **strategy composition** (`prop_map`, tuples, enums) over `prop_filter`.
- Keep generated domains realistic and biased toward valid inputs.
- Start with 1-2 strong invariants per test.

### Example
```rust
use proptest::prelude::*;

fn reverse_twice(xs: Vec<u8>) -> Vec<u8> {
    let mut ys = xs.clone();
    ys.reverse();
    ys.reverse();
    ys
}

proptest! {
    #[test]
    fn reversing_twice_is_identity(xs in proptest::collection::vec(any::<u8>(), 0..100)) {
        prop_assert_eq!(reverse_twice(xs.clone()), xs);
    }
}
```

### Shrinking advice
- Good shrinking comes from good strategies.
- Prefer structured strategies:
```rust
use proptest::prelude::*;

#[derive(Debug, Clone)]
struct User {
    name: String,
    age: u8,
}

prop_compose! {
    fn arb_user()(
        name in "[a-z]{1,12}",
        age in 0u8..=120
    ) -> User {
        User { name, age }
    }
}
```

### Gotchas
- `prop_filter` can create lots of rejects and slow tests badly.
- `prop_flat_map` is powerful, but nested flat-maps can make shrinking much slower.
- Property tests complement unit tests; they do not replace hand-picked edge cases.
- Persist failing seeds/regressions in tests once bugs are found.

---

## 3. Snapshot Testing: `insta`

**Current crate**
- [`insta` 1.46.3](https://docs.rs/crate/insta/latest)

**Use when**
- Output is large, structured, or changes infrequently.
- You want approval-style review of diffs.

### Best practice
- Snapshot **stable, intentional output**, not volatile data.
- Redact timestamps, UUIDs, paths, and randomized fields.
- Prefer small focused snapshots over giant all-in-one snapshots.
- Use inline snapshots for tiny values; file snapshots for larger payloads.

### Example
```rust
#[test]
fn json_shape_snapshot() {
    let body = serde_json::json!({
        "id": "REDACTED",
        "name": "Ada",
        "roles": ["admin", "user"]
    });

    insta::assert_json_snapshot!(body, @r#"
    {
      "id": "REDACTED",
      "name": "Ada",
      "roles": [
        "admin",
        "user"
      ]
    }
    "#);
}
```

### Gotchas
- Unstable fields make snapshots noisy and useless.
- Snapshot tests can hide semantic mistakes if reviewers rubber-stamp updates.
- Use snapshots for output shape, formatting, and regression detection; not as your only behavioral test.

---

## 4. Table-Driven Tests: `test-case` and `rstest`

**Current crates**
- [`test-case` 3.3.1](https://docs.rs/test-case/latest/test_case/)
- [`rstest` 0.26.1](https://docs.rs/crate/rstest/latest)

**Use when**
- You have a finite set of meaningful cases.
- You want each case reported separately in `cargo test`.

### Best practice
- Use table-driven tests for known business rules and edge cases.
- Use `rstest` when you also want fixtures and async ergonomics.
- Use `test-case` when you want simple parameterization with minimal machinery.

### Example: `test-case`
```rust
use test_case::test_case;

#[test_case("", 0; "empty")]
#[test_case("a", 1; "single")]
#[test_case("rust", 4; "word")]
fn len_cases(input: &str, expected: usize) {
    assert_eq!(input.len(), expected);
}
```

### Example: `rstest`
```rust
use rstest::rstest;

#[rstest]
#[case("", 0)]
#[case("a", 1)]
#[case("rust", 4)]
fn len_cases(#[case] input: &str, #[case] expected: usize) {
    assert_eq!(input.len(), expected);
}
```

### Gotchas
- Don't turn hundreds of pseudo-random rows into a table test; that belongs in `proptest`.
- Keep each row meaningful and named when possible.
- `test-case` tracks current stable aggressively; pin exact version if your toolchain lags.

---

## 5. Coverage Tooling: `cargo-llvm-cov` vs `cargo-tarpaulin`

**Current tools**
- [`cargo-llvm-cov` 0.8.4](https://docs.rs/crate/cargo-llvm-cov/latest)
- [`cargo-tarpaulin` 0.35.2](https://docs.rs/crate/cargo-tarpaulin/latest)

### Recommendation
- Prefer **`cargo-llvm-cov`** by default in 2025-2026.
- Keep **`cargo-tarpaulin`** for teams already invested in it, or where its workflow fits existing CI.

### Why `cargo-llvm-cov` usually wins
- More accurate modern coverage path.
- Better fit with current Rust coverage instrumentation.
- Good support for branch/region coverage and `nextest`.
- Generally the default choice on current Rust teams.

### Example
```bash
cargo llvm-cov --workspace --all-features --html
cargo llvm-cov --workspace --all-features --lcov --output-path lcov.info
```

### Tarpaulin example
```bash
cargo tarpaulin --workspace --all-features --out Html
```

### Gotchas
- Coverage is a reachability metric, not a test quality metric.
- High coverage can still miss assertions and behavior.
- Tarpaulin's historical ptrace-based behavior has platform/backend caveats; on non-Linux or some setups it uses LLVM mode.
- Don't gate merges on a single magic percentage without excluding generated code, benches, or irrelevant glue.

---

## 6. Mutation Testing: `cargo-mutants`

**Current tool**
- [`cargo-mutants` 27.0.0](https://docs.rs/crate/cargo-mutants/latest)

**Use when**
- You want to know whether tests actually detect behavioral changes.

### Best practice
- Run mutation testing on critical crates, not the whole workspace at first.
- Start with changed files or modules in CI, full runs on nightly/weekly jobs.
- Treat surviving mutants as leads, not automatic bugs.

### Example
```bash
cargo mutants
cargo mutants -f src/parser.rs
cargo mutants --in-diff
```

### Gotchas
- Mutation testing is much slower than normal test runs.
- Flaky tests make results noisy and hard to trust.
- Tests with side effects can be dangerous under machine-generated mutations.
- Use it after basic unit/integration coverage is already healthy.

---

## 7. Test Fixtures

**Useful crates**
- [`rstest` 0.26.1](https://docs.rs/crate/rstest/latest) for fixture injection
- [`assert_fs` 1.1.3](https://docs.rs/crate/assert_fs/latest) for filesystem fixtures
- [`tempfile` 3.25.0](https://docs.rs/crate/tempfile/3.25.0)
- [`test-context` 0.4.1](https://docs.rs/test-context/latest/test_context/)

### Best practice
- Prefer **fresh per-test fixtures** over shared mutable fixtures.
- Build fixtures with helper functions/builders.
- Use temp dirs for filesystem/database state.
- If setup is expensive, encapsulate it behind a fixture/context type.

### Example: `rstest` fixture
```rust
use rstest::{fixture, rstest};

#[fixture]
fn user_name() -> String {
    "Ada".to_string()
}

#[rstest]
fn uses_fixture(user_name: String) {
    assert_eq!(user_name, "Ada");
}
```

### Example: filesystem fixture
```rust
use assert_fs::prelude::*;

#[test]
fn reads_fixture_file() {
    let temp = assert_fs::TempDir::new().unwrap();
    temp.child("input.txt").write_str("hello").unwrap();

    let content = std::fs::read_to_string(temp.child("input.txt").path()).unwrap();
    assert_eq!(content, "hello");
}
```

### Gotchas
- Shared fixtures create hidden test order dependencies.
- Avoid fixed ports, fixed temp paths, and singleton DBs in parallel tests.
- Clean teardown matters less if you use isolated temp resources and RAII.

---

## 8. Concurrency Testing: `tokio::test`, `loom`

**Current crates**
- [`tokio` 1.50.0](https://docs.rs/crate/tokio/latest)
- [`tokio-test` 0.4.5](https://docs.rs/tokio-test/latest/tokio_test/)
- [`loom` 0.7.2](https://docs.rs/crate/loom/latest)

### Best practice
- Use `#[tokio::test]` for async integration/unit tests.
- Use `loom` for **small, critical concurrent primitives**: channels, caches, state machines, lock-free code.
- Keep loom models tiny; model-check the core synchronization logic, not the full app.

### Example: async test
```rust
#[tokio::test]
async fn async_logic_works() {
    let value = async { 2 + 2 }.await;
    assert_eq!(value, 4);
}
```

### Example: loom model test
```rust
#[test]
fn loom_smoke_test() {
    loom::model(|| {
        use loom::sync::{Arc, Mutex};
        use loom::thread;

        let n = Arc::new(Mutex::new(0));
        let n2 = n.clone();

        let t = thread::spawn(move || {
            *n2.lock().unwrap() += 1;
        });

        *n.lock().unwrap() += 1;
        t.join().unwrap();
    });
}
```

### Gotchas
- `tokio::test` does not explore interleavings; it only runs one schedule.
- `loom` requires writing code or abstractions that can be modeled cleanly.
- Loom state explosion is real; keep tests minimal.
- If tests share global state, use serialization sparingly and only when truly necessary.

---

## 9. Fuzzing: `cargo-fuzz` / `libfuzzer`

**Current crates**
- [`cargo-fuzz` 0.13.1](https://docs.rs/crate/cargo-fuzz/latest)
- [`libfuzzer-sys` 0.4.12](https://docs.rs/crate/libfuzzer-sys/latest)

### Best practice
- Fuzz parsers, protocol decoders, deserializers, and state-machine transitions.
- Keep fuzz targets small and deterministic.
- Seed the corpus with real examples and past bug inputs.
- Minimize crashes into regression tests.

### Example fuzz target
```rust
#![no_main]
use libfuzzer_sys::fuzz_target;

fn parse(data: &[u8]) {
    let _ = std::str::from_utf8(data);
}

fuzz_target!(|data: &[u8]| {
    parse(data);
});
```

### Commands
```bash
cargo fuzz init
cargo fuzz add parse_bytes
cargo fuzz run parse_bytes
cargo fuzz tmin parse_bytes artifacts/parse_bytes/crash-...
```

### Gotchas
- `cargo-fuzz` still depends on LLVM sanitizer support and nightly-oriented tooling constraints.
- Fuzz targets must be deterministic; time, randomness, and network calls ruin signal.
- Fuzzing finds crashers and bad states well, but not all semantic bugs.
- Always convert minimized crash inputs into normal regression tests.

---

## Practical Stack for Most Rust Teams

If you want a modern default stack:

- `cargo test` + unit tests
- `axum` router tests with `tower::ServiceExt::oneshot`
- `axum-test` for HTTP-level API tests
- `rstest` for fixtures/table-driven tests
- `proptest` for invariants
- `insta` for stable structured outputs
- `cargo-llvm-cov` for coverage
- `cargo-mutants` in scheduled CI
- `loom` only for concurrency-sensitive internals
- `cargo-fuzz` for parsers and unsafe/complex input surfaces

## Sources
- `axum`: https://docs.rs/crate/axum/latest
- `tower`: https://docs.rs/crate/tower/latest
- `axum-test`: https://docs.rs/crate/axum-test/latest
- `tokio`: https://docs.rs/crate/tokio/latest
- `tokio-test`: https://docs.rs/tokio-test/latest/tokio_test/
- `proptest`: https://docs.rs/crate/proptest/latest
- proptest shrinking note: https://docs.rs/proptest/latest/proptest/strategy/trait.Strategy.html
- `insta`: https://docs.rs/crate/insta/latest
- `test-case`: https://docs.rs/test-case/latest/test_case/
- `rstest`: https://docs.rs/crate/rstest/latest
- `cargo-llvm-cov`: https://docs.rs/crate/cargo-llvm-cov/latest
- `cargo-tarpaulin`: https://docs.rs/crate/cargo-tarpaulin/latest
- `cargo-mutants`: https://docs.rs/crate/cargo-mutants/latest
- `assert_fs`: https://docs.rs/crate/assert_fs/latest
- `tempfile`: https://docs.rs/crate/tempfile/3.25.0
- `test-context`: https://docs.rs/test-context/latest/test_context/
- `loom`: https://docs.rs/crate/loom/latest
- `cargo-fuzz`: https://docs.rs/crate/cargo-fuzz/latest
- `libfuzzer-sys`: https://docs.rs/crate/libfuzzer-sys/latest
