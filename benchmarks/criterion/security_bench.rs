use criterion::{Criterion, black_box, criterion_group, criterion_main};
use ob_security::{RuleEngine, SecurityContext, parse_rules};
use serde_json::json;

fn bench_parse_rules(c: &mut Criterion) {
    let rules_text = r#"
rules products {
    read: true;
    create: isAuthenticated();
    update: isOwner(resource.seller_id) || hasRole("admin");
    delete: hasRole("admin");
}

rules users {
    read: isAuthenticated();
    create: true;
    update: isOwner(resource.id);
    delete: hasRole("admin");
}

rules orders {
    read: isOwner(resource.user_id) || hasRole("admin");
    create: isAuthenticated();
    update: isOwner(resource.user_id);
    delete: false;
}
"#;

    c.bench_function("parse_rules_3_collections", |b| {
        b.iter(|| {
            let _ = parse_rules(black_box(rules_text)).unwrap();
        });
    });
}

fn bench_evaluate_rule(c: &mut Criterion) {
    let rules_text = r#"
rules products {
    read: true;
    create: isAuthenticated();
    update: isOwner(resource.seller_id) || hasRole("admin");
    delete: hasRole("admin");
}
"#;
    let rules = parse_rules(rules_text).unwrap();
    let engine = RuleEngine::new(rules);

    let ctx_anon = SecurityContext {
        user_id: None,
        roles: vec![],
        resource: Some(json!({"seller_id": "user_123", "title": "Widget"})),
        incoming: Some(serde_json::Value::Null),
        authenticated: false,
    };

    let ctx_auth = SecurityContext {
        user_id: Some("user_123".to_string()),
        roles: vec!["seller".to_string()],
        resource: Some(json!({"seller_id": "user_123", "title": "Widget"})),
        incoming: Some(serde_json::Value::Null),
        authenticated: true,
    };

    let ctx_admin = SecurityContext {
        user_id: Some("admin_1".to_string()),
        roles: vec!["admin".to_string()],
        resource: Some(json!({"seller_id": "user_123", "title": "Widget"})),
        incoming: Some(serde_json::Value::Null),
        authenticated: true,
    };

    let mut group = c.benchmark_group("evaluate_rule");

    group.bench_function("read_public", |b| {
        b.iter(|| {
            let _ = engine.check(
                black_box("products"),
                black_box("read"),
                black_box(&ctx_anon),
            );
        });
    });

    group.bench_function("create_authenticated", |b| {
        b.iter(|| {
            let _ = engine.check(
                black_box("products"),
                black_box("create"),
                black_box(&ctx_auth),
            );
        });
    });

    group.bench_function("update_owner_check", |b| {
        b.iter(|| {
            let _ = engine.check(
                black_box("products"),
                black_box("update"),
                black_box(&ctx_auth),
            );
        });
    });

    group.bench_function("delete_admin_role", |b| {
        b.iter(|| {
            let _ = engine.check(
                black_box("products"),
                black_box("delete"),
                black_box(&ctx_admin),
            );
        });
    });

    group.finish();
}

fn bench_field_filtering(c: &mut Criterion) {
    let rules_text = r#"
rules users {
    read: true;
}
"#;
    let rules = parse_rules(rules_text).unwrap();
    let engine = RuleEngine::new(rules);

    let doc = json!({
        "id": "user_1",
        "name": "John Doe",
        "email": "john@example.com",
        "phone": "+1234567890",
        "address": {"street": "123 Main St", "city": "Toronto"},
        "created_at": "2026-01-01T00:00:00Z",
        "role": "user",
        "status": "active"
    });

    let ctx = SecurityContext {
        user_id: Some("other_user".to_string()),
        roles: vec![],
        resource: Some(serde_json::Value::Null),
        incoming: Some(serde_json::Value::Null),
        authenticated: true,
    };

    c.bench_function("filter_fields_8_fields", |b| {
        b.iter(|| {
            let _ = engine.filter_fields(black_box("users"), black_box(&doc), black_box(&ctx));
        });
    });
}

criterion_group!(
    benches,
    bench_parse_rules,
    bench_evaluate_rule,
    bench_field_filtering
);
criterion_main!(benches);
