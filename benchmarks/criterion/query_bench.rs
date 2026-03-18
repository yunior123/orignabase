use criterion::{black_box, criterion_group, criterion_main, Criterion};
use serde_json::json;

/// Benchmark GraphQL filter translation (the logic that converts
/// `{field: {_eq: value}}` style filters into SurrealQL WHERE clauses).
///
/// This benchmarks the core hot path of every database query.
fn bench_filter_translation(c: &mut Criterion) {
    let mut group = c.benchmark_group("filter_translation");

    // Simple equality filter
    group.bench_function("single_eq", |b| {
        let filter = json!({"status": {"_eq": "active"}});
        b.iter(|| {
            translate_filter(black_box(&filter));
        });
    });

    // Multiple filters
    group.bench_function("three_filters", |b| {
        let filter = json!({
            "status": {"_eq": "active"},
            "price": {"_gt": 100},
            "category": {"_in": ["electronics", "books"]}
        });
        b.iter(|| {
            translate_filter(black_box(&filter));
        });
    });

    // Nested field access
    group.bench_function("nested_field", |b| {
        let filter = json!({
            "address.city": {"_eq": "Toronto"},
            "address.country": {"_eq": "CA"}
        });
        b.iter(|| {
            translate_filter(black_box(&filter));
        });
    });

    group.finish();
}

/// Simplified filter translation for benchmarking.
/// Mirrors the real translation in ob-graphql resolvers.
fn translate_filter(filter: &serde_json::Value) -> String {
    let mut clauses = Vec::new();
    if let Some(obj) = filter.as_object() {
        for (field, ops) in obj {
            if let Some(ops_obj) = ops.as_object() {
                for (op, value) in ops_obj {
                    let surreal_op = match op.as_str() {
                        "_eq" => "=",
                        "_ne" => "!=",
                        "_gt" => ">",
                        "_gte" => ">=",
                        "_lt" => "<",
                        "_lte" => "<=",
                        "_in" => "IN",
                        "_contains" => "CONTAINS",
                        _ => "=",
                    };
                    let val_str = match value {
                        serde_json::Value::String(s) => format!("'{s}'"),
                        serde_json::Value::Array(arr) => {
                            let items: Vec<String> = arr
                                .iter()
                                .map(|v| match v {
                                    serde_json::Value::String(s) => format!("'{s}'"),
                                    other => other.to_string(),
                                })
                                .collect();
                            format!("[{}]", items.join(", "))
                        }
                        other => other.to_string(),
                    };
                    clauses.push(format!("{field} {surreal_op} {val_str}"));
                }
            }
        }
    }
    clauses.join(" AND ")
}

/// Benchmark JSON document serialization (common path for all responses).
fn bench_document_serialization(c: &mut Criterion) {
    let doc = json!({
        "id": "product:abc123",
        "title": "Premium Widget",
        "description": "A high-quality widget for all your needs",
        "price": {"amount": 2999, "currency": "CAD"},
        "tags": ["electronics", "widgets", "premium"],
        "seller": {"id": "user:xyz", "name": "WidgetCo"},
        "created_at": "2026-01-15T10:30:00Z",
        "updated_at": "2026-03-08T14:22:00Z",
        "status": "active",
        "inventory": 42
    });

    c.bench_function("serialize_document", |b| {
        b.iter(|| {
            let _ = serde_json::to_string(black_box(&doc)).unwrap();
        });
    });

    c.bench_function("deserialize_document", |b| {
        let json_str = serde_json::to_string(&doc).unwrap();
        b.iter(|| {
            let _: serde_json::Value = serde_json::from_str(black_box(&json_str)).unwrap();
        });
    });
}

criterion_group!(benches, bench_filter_translation, bench_document_serialization);
criterion_main!(benches);
