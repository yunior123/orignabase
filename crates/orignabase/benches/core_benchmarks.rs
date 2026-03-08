use criterion::{Criterion, black_box, criterion_group, criterion_main};

// ── Query Translation Benchmarks ──

fn bench_query_translation(c: &mut Criterion) {
    use ob_database::query::QueryTranslator;

    let mut group = c.benchmark_group("query_translation");

    group.bench_function("empty_select", |b| {
        b.iter(|| {
            black_box(QueryTranslator::build_select(
                "products", None, None, false, None, None,
            ));
        });
    });

    let simple_filter = serde_json::json!({"status": {"_eq": "active"}});
    group.bench_function("simple_eq_filter", |b| {
        b.iter(|| {
            black_box(QueryTranslator::build_select(
                "products",
                Some(&simple_filter),
                None,
                false,
                None,
                None,
            ));
        });
    });

    let complex_filter = serde_json::json!({
        "status": {"_eq": "active"},
        "price": {"_gt": 10, "_lt": 1000},
        "category": {"_in": ["electronics", "books", "toys"]},
        "title": {"_contains": "widget"},
        "brand": {"_starts_with": "Acme"}
    });
    group.bench_function("complex_5_field_filter", |b| {
        b.iter(|| {
            black_box(QueryTranslator::build_select(
                "products",
                Some(&complex_filter),
                Some("created_at"),
                true,
                Some(20),
                Some(40),
            ));
        });
    });

    group.finish();
}

// ── Security Rules Benchmarks ──

fn bench_security_rules(c: &mut Criterion) {
    use ob_security::{RuleEngine, SecurityContext, parse_rules};

    let rules_source = r#"
        rules products {
            read: true;
            create: isAuthenticated() && hasRole("seller");
            update: isOwner(resource.seller_id) || hasRole("admin");
            delete: hasRole("admin");
        }
        rules orders {
            read: isAuthenticated();
            create: isAuthenticated();
            update: isOwner(resource.user_id);
            delete: false;
        }
    "#;

    let rules = parse_rules(rules_source).expect("parse rules");
    let engine = RuleEngine::new(rules);

    let mut group = c.benchmark_group("security_rules");

    group.bench_function("public_read", |b| {
        let ctx = SecurityContext {
            user_id: None,
            roles: vec![],
            authenticated: false,
            resource: Some(serde_json::json!({})),
            incoming: None,
        };
        b.iter(|| black_box(engine.check("products", "read", &ctx)));
    });

    group.bench_function("authenticated_create", |b| {
        let ctx = SecurityContext {
            user_id: Some("user_123".to_string()),
            roles: vec!["seller".to_string()],
            authenticated: true,
            resource: Some(serde_json::json!({})),
            incoming: None,
        };
        b.iter(|| black_box(engine.check("products", "create", &ctx)));
    });

    group.bench_function("owner_check", |b| {
        let ctx = SecurityContext {
            user_id: Some("user_123".to_string()),
            roles: vec![],
            authenticated: true,
            resource: Some(serde_json::json!({"seller_id": "user_123"})),
            incoming: None,
        };
        b.iter(|| black_box(engine.check("products", "update", &ctx)));
    });

    group.bench_function("denied_check", |b| {
        let ctx = SecurityContext {
            user_id: Some("user_123".to_string()),
            roles: vec![],
            authenticated: true,
            resource: Some(serde_json::json!({})),
            incoming: None,
        };
        b.iter(|| black_box(engine.check("products", "delete", &ctx)));
    });

    group.finish();
}

fn bench_rule_parsing(c: &mut Criterion) {
    use ob_security::parse_rules;

    let rules_source = r#"
        rules products {
            read: true;
            create: isAuthenticated() && hasRole("seller");
            update: isOwner(resource.seller_id) || hasRole("admin");
            delete: hasRole("admin");
        }
    "#;

    c.bench_function("parse_rules_single_collection", |b| {
        b.iter(|| black_box(parse_rules(rules_source).unwrap()));
    });
}

// ── Auth Benchmarks ──

fn bench_password_hashing(c: &mut Criterion) {
    use ob_auth::password::{hash_password, verify_password};

    let mut group = c.benchmark_group("auth_password");
    group.sample_size(10); // Argon2id is intentionally slow

    group.bench_function("argon2id_hash", |b| {
        b.iter(|| black_box(hash_password("TestPassword123!").unwrap()));
    });

    let hash = hash_password("TestPassword123!").unwrap();
    group.bench_function("argon2id_verify", |b| {
        b.iter(|| black_box(verify_password("TestPassword123!", &hash).unwrap()));
    });

    group.finish();
}

fn bench_jwt(c: &mut Criterion) {
    use ob_auth::jwt::{issue_access_token, verify_token};

    let mut group = c.benchmark_group("jwt");

    group.bench_function("issue_token", |b| {
        b.iter(|| {
            black_box(
                issue_access_token("user_123", &["user".to_string()], "secret", 900).unwrap(),
            );
        });
    });

    let token = issue_access_token("user_123", &["user".to_string()], "secret", 900).unwrap();
    group.bench_function("verify_token", |b| {
        b.iter(|| black_box(verify_token(&token, "secret").unwrap()));
    });

    group.finish();
}

// ── Signed URL Benchmarks ──

fn bench_signed_urls(c: &mut Criterion) {
    use ob_storage::SignedUrlGenerator;

    let signer = SignedUrlGenerator::new("supersecretkey", "http://localhost:8080");

    let mut group = c.benchmark_group("signed_urls");

    group.bench_function("sign_download", |b| {
        b.iter(|| black_box(signer.sign_download("users/123/avatar.jpg", 3600).unwrap()));
    });

    let url = signer.sign_download("users/123/avatar.jpg", 3600).unwrap();
    let parts: Vec<&str> = url.split('?').collect();
    let params: Vec<&str> = parts[1].split('&').collect();
    let expires: u64 = params[0].strip_prefix("expires=").unwrap().parse().unwrap();
    let sig = params[1].strip_prefix("sig=").unwrap();

    group.bench_function("verify_signature", |b| {
        b.iter(|| {
            black_box(
                signer
                    .verify("GET", "users/123/avatar.jpg", expires, sig)
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ── Analytics Benchmarks ──

fn bench_analytics(c: &mut Criterion) {
    use ob_analytics::event::{extract_domain, hash_ip};

    let mut group = c.benchmark_group("analytics");

    group.bench_function("hash_ip", |b| {
        b.iter(|| black_box(hash_ip("192.168.1.100", "daily_salt_2026")));
    });

    group.bench_function("extract_domain", |b| {
        b.iter(|| black_box(extract_domain("https://www.google.com/search?q=orignabase")));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_query_translation,
    bench_security_rules,
    bench_rule_parsing,
    bench_password_hashing,
    bench_jwt,
    bench_signed_urls,
    bench_analytics,
);
criterion_main!(benches);
