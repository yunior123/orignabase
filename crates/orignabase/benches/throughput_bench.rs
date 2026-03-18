//! Throughput benchmarks for OrignaBase.
//!
//! Requires a running OrignaBase instance at http://localhost:8080 (or OB_TEST_URL).
//! Run with: `cargo bench --bench throughput_bench`

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use serde_json::{Value, json};

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

async fn register_test_user(client: &reqwest::Client) -> (String, String) {
    let email = format!("bench_{}@example.com", uuid::Uuid::new_v4());
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "BenchPassword123!" }))
        .send()
        .await
        .expect("register failed");
    let body: Value = resp.json().await.unwrap();
    let token = body["access_token"]
        .as_str()
        .expect("missing access_token")
        .to_string();
    (token, email)
}

async fn graphql_req(client: &reqwest::Client, token: &str, query: &str) -> Value {
    let resp = client
        .post(format!("{}/graphql", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "query": query }))
        .send()
        .await
        .expect("graphql request failed");
    resp.json().await.unwrap()
}

async fn create_doc(
    client: &reqwest::Client,
    token: &str,
    collection: &str,
    data: &Value,
) -> String {
    let data_str = serde_json::to_string(data).unwrap();
    let escaped = serde_json::to_string(&data_str).unwrap();
    let query = format!(r#"mutation {{ create(collection: "{collection}", data: {escaped}) }}"#);
    let body = graphql_req(client, token, &query).await;
    let result = &body["data"]["create"];
    result["id"]
        .as_str()
        .or_else(|| result["_id"].as_str())
        .unwrap_or("")
        .to_string()
}

// =============================================================================
// 1. Single Document CRUD
// =============================================================================

fn bench_single_crud(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_crud_{}", uuid::Uuid::new_v4().simple());

    let mut group = c.benchmark_group("single_document_crud");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);

    // Create
    group.bench_function("create", |b| {
        b.iter(|| {
            rt.block_on(async {
                create_doc(
                    &client,
                    &token,
                    &col,
                    &json!({"title": "bench item", "price": 42}),
                )
                .await
            })
        });
    });

    // Seed a doc for read/update/delete benchmarks
    let doc_id = rt.block_on(create_doc(
        &client,
        &token,
        &col,
        &json!({"title": "seed", "price": 1}),
    ));

    // Read
    group.bench_function("read", |b| {
        let query = format!(r#"{{ get(collection: "{col}", id: "{doc_id}") }}"#);
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    // Update
    group.bench_function("update", |b| {
        let data = json!({"title": "updated", "price": 99});
        let data_str = serde_json::to_string(&data).unwrap();
        let escaped = serde_json::to_string(&data_str).unwrap();
        let query = format!(
            r#"mutation {{ update(collection: "{col}", id: "{doc_id}", data: {escaped}) }}"#
        );
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    // Delete (creates a new doc each iteration)
    group.bench_function("delete", |b| {
        b.iter(|| {
            rt.block_on(async {
                let id = create_doc(
                    &client,
                    &token,
                    &col,
                    &json!({"title": "to_delete", "price": 0}),
                )
                .await;
                let query = format!(r#"mutation {{ delete(collection: "{col}", id: "{id}") }}"#);
                graphql_req(&client, &token, &query).await
            })
        });
    });

    group.finish();
}

// =============================================================================
// 2. Batch Write Throughput
// =============================================================================

fn bench_batch_write(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));

    let mut group = c.benchmark_group("batch_write_throughput");
    group.sample_size(10);

    for batch_size in [10u64, 50, 100, 500] {
        group.throughput(Throughput::Elements(batch_size));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &size| {
                b.iter(|| {
                    rt.block_on(async {
                        let col = format!("bench_batch_{}", uuid::Uuid::new_v4().simple());
                        let mut handles = Vec::with_capacity(size as usize);
                        for i in 0..size {
                            let client = client.clone();
                            let token = token.clone();
                            let col = col.clone();
                            handles.push(tokio::spawn(async move {
                                create_doc(
                                    &client,
                                    &token,
                                    &col,
                                    &json!({"title": format!("batch_{i}"), "price": i}),
                                )
                                .await
                            }));
                        }
                        for h in handles {
                            let _ = h.await;
                        }
                    })
                });
            },
        );
    }

    group.finish();
}

// =============================================================================
// 3. Concurrent Write Scaling
// =============================================================================

fn bench_concurrent_writes(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));

    let mut group = c.benchmark_group("concurrent_write_scaling");
    group.sample_size(10);

    for concurrency in [10u64, 50, 100, 200, 500] {
        group.throughput(Throughput::Elements(concurrency));
        group.bench_with_input(
            BenchmarkId::from_parameter(concurrency),
            &concurrency,
            |b, &n| {
                b.iter(|| {
                    rt.block_on(async {
                        let col = format!("bench_cw_{}", uuid::Uuid::new_v4().simple());
                        let mut handles = Vec::with_capacity(n as usize);
                        for i in 0..n {
                            let client = client.clone();
                            let token = token.clone();
                            let col = col.clone();
                            handles.push(tokio::spawn(async move {
                                create_doc(
                                    &client,
                                    &token,
                                    &col,
                                    &json!({"item": i, "data": "concurrent_write_test"}),
                                )
                                .await
                            }));
                        }
                        let mut ok = 0u64;
                        for h in handles {
                            if h.await.is_ok() {
                                ok += 1;
                            }
                        }
                        ok
                    })
                });
            },
        );
    }

    group.finish();
}

// =============================================================================
// 4. Concurrent Read Scaling
// =============================================================================

fn bench_concurrent_reads(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_cr_{}", uuid::Uuid::new_v4().simple());

    // Seed 20 docs for reads
    rt.block_on(async {
        for i in 0..20 {
            create_doc(
                &client,
                &token,
                &col,
                &json!({"title": format!("seed_{i}"), "price": i * 5}),
            )
            .await;
        }
    });

    let mut group = c.benchmark_group("concurrent_read_scaling");
    group.sample_size(10);

    for concurrency in [10u64, 50, 100, 500, 1000] {
        group.throughput(Throughput::Elements(concurrency));
        group.bench_with_input(
            BenchmarkId::from_parameter(concurrency),
            &concurrency,
            |b, &n| {
                b.iter(|| {
                    rt.block_on(async {
                        let mut handles = Vec::with_capacity(n as usize);
                        for _ in 0..n {
                            let client = client.clone();
                            let token = token.clone();
                            let col = col.clone();
                            handles.push(tokio::spawn(async move {
                                let query =
                                    format!(r#"{{ list(collection: "{col}", limit: 10) }}"#);
                                graphql_req(&client, &token, &query).await
                            }));
                        }
                        let mut ok = 0u64;
                        for h in handles {
                            if h.await.is_ok() {
                                ok += 1;
                            }
                        }
                        ok
                    })
                });
            },
        );
    }

    group.finish();
}

// =============================================================================
// 5. Query Performance
// =============================================================================

fn bench_query_performance(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_qp_{}", uuid::Uuid::new_v4().simple());

    // Seed 50 docs with varied data
    rt.block_on(async {
        for i in 0..50 {
            create_doc(
                &client,
                &token,
                &col,
                &json!({
                    "title": format!("product_{i}"),
                    "price": (i % 10) * 15 + 5,
                    "category": if i % 3 == 0 { "electronics" } else if i % 3 == 1 { "books" } else { "toys" },
                    "rating": (i % 5) + 1
                }),
            )
            .await;
        }
    });

    let mut group = c.benchmark_group("query_performance");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);

    // Simple list
    group.bench_function("simple_list", |b| {
        let query = format!(r#"{{ list(collection: "{col}", limit: 20) }}"#);
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    // Filtered query
    group.bench_function("filtered_query", |b| {
        let filter = r#"{\"category\":{\"_eq\":\"electronics\"}}"#;
        let query = format!(r#"{{ list(collection: "{col}", filter: "{filter}", limit: 20) }}"#);
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    // Ordered query
    group.bench_function("ordered_query", |b| {
        let query = format!(
            r#"{{ list(collection: "{col}", orderBy: "price", orderDesc: true, limit: 20) }}"#
        );
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    // Paginated query
    group.bench_function("paginated_query", |b| {
        let query = format!(r#"{{ list(collection: "{col}", limit: 10, offset: 20) }}"#);
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    group.finish();
}

// =============================================================================
// 6. Auth Overhead
// =============================================================================

fn bench_auth_overhead(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();

    let mut group = c.benchmark_group("auth_overhead");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);

    // Register
    group.bench_function("register", |b| {
        b.iter(|| {
            rt.block_on(async {
                let email = format!("authbench_{}@example.com", uuid::Uuid::new_v4());
                let resp = client
                    .post(format!("{}/auth/register", base_url()))
                    .json(&json!({ "email": email, "password": "BenchPassword123!" }))
                    .send()
                    .await
                    .unwrap();
                resp.status().as_u16()
            })
        });
    });

    // Login (register once, then benchmark login)
    let (_, login_email) = rt.block_on(register_test_user(&client));
    group.bench_function("login", |b| {
        b.iter(|| {
            rt.block_on(async {
                let resp = client
                    .post(format!("{}/auth/login", base_url()))
                    .json(&json!({ "email": login_email, "password": "BenchPassword123!" }))
                    .send()
                    .await
                    .unwrap();
                resp.status().as_u16()
            })
        });
    });

    // JWT verification cost (measured via a simple authenticated request)
    let (token, _) = rt.block_on(register_test_user(&client));
    group.bench_function("jwt_verified_request", |b| {
        b.iter(|| {
            rt.block_on(async {
                let resp = client
                    .get(format!("{}/health", base_url()))
                    .header("Authorization", format!("Bearer {token}"))
                    .send()
                    .await
                    .unwrap();
                resp.status().as_u16()
            })
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_single_crud,
    bench_batch_write,
    bench_concurrent_writes,
    bench_concurrent_reads,
    bench_query_performance,
    bench_auth_overhead,
);
criterion_main!(benches);
