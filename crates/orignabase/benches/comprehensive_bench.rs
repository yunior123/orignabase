//! Comprehensive benchmarks for OrignaBase covering Auth, CRUD, Queries, Concurrency, and sustained load.
//!
//! Requires a running OrignaBase instance at http://localhost:8080 (or OB_TEST_URL).
//! Run with: `cargo bench --bench comprehensive_bench`

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use serde_json::{Value, json};
use std::time::Instant;

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

async fn update_doc(
    client: &reqwest::Client,
    token: &str,
    collection: &str,
    doc_id: &str,
    data: &Value,
) -> bool {
    let data_str = serde_json::to_string(data).unwrap();
    let escaped = serde_json::to_string(&data_str).unwrap();
    let query = format!(
        r#"mutation {{ update(collection: "{collection}", id: "{doc_id}", data: {escaped}) }}"#
    );
    let body = graphql_req(client, token, &query).await;
    body["data"]["update"].is_object()
}

async fn delete_doc(client: &reqwest::Client, token: &str, collection: &str, doc_id: &str) -> bool {
    let query = format!(r#"mutation {{ delete(collection: "{collection}", id: "{doc_id}") }}"#);
    let body = graphql_req(client, token, &query).await;
    body["data"]["delete"].is_object()
}

// =============================================================================
// 1. AUTH OPERATIONS
// =============================================================================

fn bench_auth_register(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();

    let mut group = c.benchmark_group("auth_register");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);

    group.bench_function("register", |b| {
        b.iter(|| {
            rt.block_on(async {
                let email = format!("bench_{}@example.com", uuid::Uuid::new_v4());
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

    group.finish();
}

fn bench_auth_login(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (_, email) = rt.block_on(register_test_user(&client));

    let mut group = c.benchmark_group("auth_login");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);

    group.bench_function("login", |b| {
        b.iter(|| {
            rt.block_on(async {
                let resp = client
                    .post(format!("{}/auth/login", base_url()))
                    .json(&json!({ "email": &email, "password": "BenchPassword123!" }))
                    .send()
                    .await
                    .unwrap();
                resp.status().as_u16()
            })
        });
    });

    group.finish();
}

fn bench_auth_token_refresh(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));

    let mut group = c.benchmark_group("auth_token_refresh");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);

    group.bench_function("refresh_token", |b| {
        b.iter(|| {
            rt.block_on(async {
                let resp = client
                    .post(format!("{}/auth/refresh", base_url()))
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

// =============================================================================
// 2. CRUD AT SCALE
// =============================================================================

fn bench_single_doc_crud(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_crud_{}", uuid::Uuid::new_v4().simple());

    let mut group = c.benchmark_group("single_doc_crud");
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
                    &json!({"title": "bench item", "price": 42, "tags": ["tag1", "tag2"]}),
                )
                .await
            })
        });
    });

    // Seed a doc for read/update/delete
    let doc_id = rt.block_on(create_doc(
        &client,
        &token,
        &col,
        &json!({"title": "seed", "price": 1, "tags": ["seed"]}),
    ));

    // Read (full)
    group.bench_function("read_full", |b| {
        let query = format!(r#"{{ get(collection: "{col}", id: "{doc_id}") }}"#);
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    // Read (projection - selected fields)
    group.bench_function("read_projection", |b| {
        let query = format!(
            r#"{{ get(collection: "{col}", id: "{doc_id}", fields: ["title", "price"]) }}"#
        );
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

    // Delete
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

fn bench_batch_create(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));

    let mut group = c.benchmark_group("batch_create");
    group.sample_size(10);

    for batch_size in [10u64, 100, 1000] {
        group.throughput(Throughput::Elements(batch_size));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &size| {
                b.iter(|| {
                    rt.block_on(async {
                        let col = format!("batch_create_{}", uuid::Uuid::new_v4().simple());
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
                                    &json!({"title": format!("item_{i}"), "price": i, "index": i}),
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

fn bench_batch_update(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));

    let mut group = c.benchmark_group("batch_update");
    group.sample_size(10);

    for batch_size in [10u64, 100, 1000] {
        let col = format!("batch_update_{}", uuid::Uuid::new_v4().simple());
        let mut doc_ids = Vec::new();

        // Pre-create documents
        rt.block_on(async {
            for i in 0..batch_size {
                let id = create_doc(
                    &client,
                    &token,
                    &col,
                    &json!({"title": format!("item_{i}"), "price": i}),
                )
                .await;
                doc_ids.push(id);
            }
        });

        group.throughput(Throughput::Elements(batch_size));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &size| {
                b.iter(|| {
                    rt.block_on(async {
                        let mut handles = Vec::with_capacity(size as usize);
                        for (i, doc_id) in doc_ids.iter().take(size as usize).enumerate() {
                            let client = client.clone();
                            let token = token.clone();
                            let col = col.clone();
                            let doc_id = doc_id.clone();
                            handles.push(tokio::spawn(async move {
                                update_doc(
                                    &client,
                                    &token,
                                    &col,
                                    &doc_id,
                                    &json!({"title": format!("updated_{i}"), "price": i + 100}),
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

fn bench_batch_delete(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));

    let mut group = c.benchmark_group("batch_delete");
    group.sample_size(10);

    for batch_size in [10u64, 100, 1000] {
        group.throughput(Throughput::Elements(batch_size));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &size| {
                b.iter(|| {
                    rt.block_on(async {
                        let col = format!("batch_delete_{}", uuid::Uuid::new_v4().simple());
                        // Create docs to delete
                        let mut doc_ids = Vec::new();
                        for i in 0..size {
                            let id = create_doc(
                                &client,
                                &token,
                                &col,
                                &json!({"title": format!("item_{i}"), "price": i}),
                            )
                            .await;
                            doc_ids.push(id);
                        }
                        // Delete them
                        let mut handles = Vec::with_capacity(size as usize);
                        for doc_id in doc_ids {
                            let client = client.clone();
                            let token = token.clone();
                            let col = col.clone();
                            handles.push(tokio::spawn(async move {
                                delete_doc(&client, &token, &col, &doc_id).await
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
// 3. QUERY PERFORMANCE
// =============================================================================

fn bench_query_simple_list(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_qp_{}", uuid::Uuid::new_v4().simple());

    // Seed 100 docs
    rt.block_on(async {
        for i in 0..100 {
            create_doc(
                &client,
                &token,
                &col,
                &json!({"title": format!("product_{i}"), "price": i * 10, "category": "general"}),
            )
            .await;
        }
    });

    let mut group = c.benchmark_group("query_simple_list");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);

    group.bench_function("list_no_filter", |b| {
        let query = format!(r#"{{ list(collection: "{col}", limit: 50) }}"#);
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    group.finish();
}

fn bench_query_single_filter(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_qf1_{}", uuid::Uuid::new_v4().simple());

    // Seed 100 docs with category
    rt.block_on(async {
        for i in 0..100 {
            let cat = if i % 3 == 0 {
                "electronics"
            } else if i % 3 == 1 {
                "books"
            } else {
                "toys"
            };
            create_doc(
                &client,
                &token,
                &col,
                &json!({"title": format!("product_{i}"), "price": i * 10, "category": cat}),
            )
            .await;
        }
    });

    let mut group = c.benchmark_group("query_single_filter");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);

    group.bench_function("single_field_eq", |b| {
        let filter = r#"{\"category\":{\"_eq\":\"electronics\"}}"#;
        let query = format!(r#"{{ list(collection: "{col}", filter: "{filter}", limit: 50) }}"#);
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    group.finish();
}

fn bench_query_multi_filter(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_qfm_{}", uuid::Uuid::new_v4().simple());

    // Seed 100 docs with multiple fields
    rt.block_on(async {
        for i in 0..100 {
            let cat = if i % 3 == 0 {
                "electronics"
            } else if i % 3 == 1 {
                "books"
            } else {
                "toys"
            };
            create_doc(
                &client,
                &token,
                &col,
                &json!({
                    "title": format!("product_{i}"),
                    "price": (i % 10) * 15 + 5,
                    "category": cat,
                    "rating": (i % 5) + 1,
                    "in_stock": i % 2 == 0
                }),
            )
            .await;
        }
    });

    let mut group = c.benchmark_group("query_multi_filter");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);

    group.bench_function("multi_field_filter", |b| {
        let filter = r#"{\"category\":{\"_eq\":\"electronics\"},\"price\":{\"_gt\":20,\"_lt\":100},\"in_stock\":{\"_eq\":true}}"#;
        let query = format!(r#"{{ list(collection: "{col}", filter: "{filter}", limit: 50) }}"#);
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    group.finish();
}

fn bench_query_orderby_limit(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_qob_{}", uuid::Uuid::new_v4().simple());

    // Seed 100 docs
    rt.block_on(async {
        for i in 0..100 {
            create_doc(
                &client,
                &token,
                &col,
                &json!({"title": format!("product_{i}"), "price": (i % 50) * 20, "created_at": 1000 + i}),
            )
            .await;
        }
    });

    let mut group = c.benchmark_group("query_orderby_limit");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);

    group.bench_function("orderby_desc_limit", |b| {
        let query = format!(
            r#"{{ list(collection: "{col}", orderBy: "price", orderDesc: true, limit: 20) }}"#
        );
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    group.finish();
}

fn bench_query_offset_pagination(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_qop_{}", uuid::Uuid::new_v4().simple());

    // Seed 200 docs
    rt.block_on(async {
        for i in 0..200 {
            create_doc(
                &client,
                &token,
                &col,
                &json!({"title": format!("product_{i}"), "price": i * 5}),
            )
            .await;
        }
    });

    let mut group = c.benchmark_group("query_offset_pagination");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);

    group.bench_function("offset_pagination", |b| {
        let query = format!(r#"{{ list(collection: "{col}", limit: 20, offset: 100) }}"#);
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    group.finish();
}

fn bench_query_cursor_pagination(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_qcp_{}", uuid::Uuid::new_v4().simple());

    // Seed 200 docs
    let mut first_doc_id = String::new();
    rt.block_on(async {
        for i in 0..200 {
            let id = create_doc(
                &client,
                &token,
                &col,
                &json!({"title": format!("product_{i}"), "price": i * 5}),
            )
            .await;
            if i == 50 {
                first_doc_id = id;
            }
        }
    });

    let mut group = c.benchmark_group("query_cursor_pagination");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);

    group.bench_function("cursor_pagination", |b| {
        let query =
            format!(r#"{{ list(collection: "{col}", limit: 20, startAfter: "{first_doc_id}") }}"#);
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    group.finish();
}

fn bench_query_field_projection(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_qfp_{}", uuid::Uuid::new_v4().simple());

    // Seed 100 docs with multiple fields
    rt.block_on(async {
        for i in 0..100 {
            create_doc(
                &client,
                &token,
                &col,
                &json!({
                    "title": format!("product_{i}"),
                    "price": i * 10,
                    "description": "A long description text for testing field projection",
                    "category": "test",
                    "rating": 4.5,
                    "stock": 100
                }),
            )
            .await;
        }
    });

    let mut group = c.benchmark_group("query_field_projection");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);

    group.bench_function("full_fields", |b| {
        let query = format!(r#"{{ list(collection: "{col}", limit: 50) }}"#);
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    group.bench_function("projected_fields", |b| {
        let query =
            format!(r#"{{ list(collection: "{col}", fields: ["title", "price"], limit: 50) }}"#);
        b.iter(|| rt.block_on(graphql_req(&client, &token, &query)));
    });

    group.finish();
}

// =============================================================================
// 4. CONCURRENT USER SIMULATION
// =============================================================================

fn bench_concurrent_mixed_10(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_cm10_{}", uuid::Uuid::new_v4().simple());

    let mut group = c.benchmark_group("concurrent_mixed_10");
    group.throughput(Throughput::Elements(10));
    group.sample_size(10);

    group.bench_function("10_users_mixed", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut handles = Vec::new();
                for i in 0..10 {
                    let client = client.clone();
                    let token = token.clone();
                    let col = col.clone();
                    handles.push(tokio::spawn(async move {
                        if i % 2 == 0 {
                            // Write
                            create_doc(
                                &client,
                                &token,
                                &col,
                                &json!({"user": i, "action": "write"}),
                            )
                            .await
                        } else {
                            // Read
                            let query = format!(r#"{{ list(collection: "{col}", limit: 10) }}"#);
                            let _ = graphql_req(&client, &token, &query).await;
                            String::new()
                        }
                    }));
                }
                let mut ok = 0;
                for h in handles {
                    if h.await.is_ok() {
                        ok += 1;
                    }
                }
                ok
            })
        });
    });

    group.finish();
}

fn bench_concurrent_mixed_50(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_cm50_{}", uuid::Uuid::new_v4().simple());

    let mut group = c.benchmark_group("concurrent_mixed_50");
    group.throughput(Throughput::Elements(50));
    group.sample_size(10);

    group.bench_function("50_users_mixed", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut handles = Vec::new();
                for i in 0..50 {
                    let client = client.clone();
                    let token = token.clone();
                    let col = col.clone();
                    handles.push(tokio::spawn(async move {
                        if i % 2 == 0 {
                            create_doc(
                                &client,
                                &token,
                                &col,
                                &json!({"user": i, "action": "write"}),
                            )
                            .await
                        } else {
                            let query = format!(r#"{{ list(collection: "{col}", limit: 10) }}"#);
                            let _ = graphql_req(&client, &token, &query).await;
                            String::new()
                        }
                    }));
                }
                let mut ok = 0;
                for h in handles {
                    if h.await.is_ok() {
                        ok += 1;
                    }
                }
                ok
            })
        });
    });

    group.finish();
}

fn bench_concurrent_mixed_100(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_cm100_{}", uuid::Uuid::new_v4().simple());

    let mut group = c.benchmark_group("concurrent_mixed_100");
    group.throughput(Throughput::Elements(100));
    group.sample_size(10);

    group.bench_function("100_users_mixed", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut handles = Vec::new();
                for i in 0..100 {
                    let client = client.clone();
                    let token = token.clone();
                    let col = col.clone();
                    handles.push(tokio::spawn(async move {
                        if i % 2 == 0 {
                            create_doc(
                                &client,
                                &token,
                                &col,
                                &json!({"user": i, "action": "write"}),
                            )
                            .await
                        } else {
                            let query = format!(r#"{{ list(collection: "{col}", limit: 10) }}"#);
                            let _ = graphql_req(&client, &token, &query).await;
                            String::new()
                        }
                    }));
                }
                let mut ok = 0;
                for h in handles {
                    if h.await.is_ok() {
                        ok += 1;
                    }
                }
                ok
            })
        });
    });

    group.finish();
}

fn bench_concurrent_readonly_500(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_cr500_{}", uuid::Uuid::new_v4().simple());

    // Seed some data
    rt.block_on(async {
        for i in 0..50 {
            create_doc(
                &client,
                &token,
                &col,
                &json!({"title": format!("item_{i}"), "price": i * 10}),
            )
            .await;
        }
    });

    let mut group = c.benchmark_group("concurrent_readonly_500");
    group.throughput(Throughput::Elements(500));
    group.sample_size(5);

    group.bench_function("500_users_readonly", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut handles = Vec::new();
                for _ in 0..500 {
                    let client = client.clone();
                    let token = token.clone();
                    let col = col.clone();
                    handles.push(tokio::spawn(async move {
                        let query = format!(r#"{{ list(collection: "{col}", limit: 5) }}"#);
                        graphql_req(&client, &token, &query).await
                    }));
                }
                let mut ok = 0;
                for h in handles {
                    if h.await.is_ok() {
                        ok += 1;
                    }
                }
                ok
            })
        });
    });

    group.finish();
}

fn bench_write_contention(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_wc_{}", uuid::Uuid::new_v4().simple());

    // Create a single doc that all users will update
    let doc_id = rt.block_on(create_doc(
        &client,
        &token,
        &col,
        &json!({"counter": 0, "title": "contention_test"}),
    ));

    let mut group = c.benchmark_group("write_contention");
    group.throughput(Throughput::Elements(50));
    group.sample_size(5);

    group.bench_function("50_users_update_same_doc", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut handles = Vec::new();
                for i in 0..50 {
                    let client = client.clone();
                    let token = token.clone();
                    let col = col.clone();
                    let doc_id = doc_id.clone();
                    handles.push(tokio::spawn(async move {
                        update_doc(
                            &client,
                            &token,
                            &col,
                            &doc_id,
                            &json!({"counter": i, "updated_at": i}),
                        )
                        .await
                    }));
                }
                let mut ok = 0;
                for h in handles {
                    if let Ok(true) = h.await {
                        ok += 1;
                    }
                }
                ok
            })
        });
    });

    group.finish();
}

// =============================================================================
// 5. SUSTAINED LOAD BENCHMARKS
// =============================================================================

fn bench_sustained_write_60s(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_sw_{}", uuid::Uuid::new_v4().simple());

    let mut group = c.benchmark_group("sustained_write_60s");
    group.measurement_time(std::time::Duration::from_secs(60));
    group.sample_size(1);

    group.bench_function("write_60s", |b| {
        b.iter(|| {
            rt.block_on(async {
                let start = Instant::now();
                let mut count = 0u64;
                while start.elapsed().as_secs() < 60 {
                    let _ = create_doc(
                        &client,
                        &token,
                        &col,
                        &json!({"timestamp": std::time::SystemTime::now().elapsed().unwrap().as_millis(), "index": count}),
                    )
                    .await;
                    count += 1;
                }
                count
            })
        });
    });

    group.finish();
}

fn bench_sustained_mixed_60s(c: &mut Criterion) {
    let rt = rt();
    let client = reqwest::Client::new();
    let (token, _) = rt.block_on(register_test_user(&client));
    let col = format!("bench_sm_{}", uuid::Uuid::new_v4().simple());

    let mut group = c.benchmark_group("sustained_mixed_60s");
    group.measurement_time(std::time::Duration::from_secs(60));
    group.sample_size(1);

    group.bench_function("mixed_60s", |b| {
        b.iter(|| {
            rt.block_on(async {
                let start = Instant::now();
                let mut count = 0u64;
                while start.elapsed().as_secs() < 60 {
                    if count.is_multiple_of(3) {
                        let _ = create_doc(
                            &client,
                            &token,
                            &col,
                            &json!({"action": "write", "index": count}),
                        )
                        .await;
                    } else {
                        let query = format!(r#"{{ list(collection: "{col}", limit: 10) }}"#);
                        let _ = graphql_req(&client, &token, &query).await;
                    }
                    count += 1;
                }
                count
            })
        });
    });

    group.finish();
}

// =============================================================================
// Benchmark Groups
// =============================================================================

criterion_group!(
    benches,
    // Auth
    bench_auth_register,
    bench_auth_login,
    bench_auth_token_refresh,
    // CRUD
    bench_single_doc_crud,
    bench_batch_create,
    bench_batch_update,
    bench_batch_delete,
    // Queries
    bench_query_simple_list,
    bench_query_single_filter,
    bench_query_multi_filter,
    bench_query_orderby_limit,
    bench_query_offset_pagination,
    bench_query_cursor_pagination,
    bench_query_field_projection,
    // Concurrency
    bench_concurrent_mixed_10,
    bench_concurrent_mixed_50,
    bench_concurrent_mixed_100,
    bench_concurrent_readonly_500,
    bench_write_contention,
    // Sustained Load
    bench_sustained_write_60s,
    bench_sustained_mixed_60s,
);

criterion_main!(benches);
