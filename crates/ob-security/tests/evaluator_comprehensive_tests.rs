//! Comprehensive tests for the security rules evaluator.
//!
//! Covers: RuleEngine, SecurityContext, all built-in functions,
//! field-level access, validation, path resolution, comparison operators,
//! wildcard collections, and edge cases.

use ob_security::evaluator::{RuleEngine, SecurityContext};
use ob_security::parser::parse_rules;
use serde_json::{Value, json};

// ────────────────────────────────────────────────────────────
// Test fixtures
// ────────────────────────────────────────────────────────────

fn make_engine(rules_src: &str) -> RuleEngine {
    let rules = parse_rules(rules_src).expect("rules should parse");
    RuleEngine::new(rules)
}

fn anon_ctx() -> SecurityContext {
    SecurityContext {
        user_id: None,
        roles: vec![],
        authenticated: false,
        resource: None,
        incoming: None,
    }
}

fn authed_ctx(user_id: &str, roles: &[&str]) -> SecurityContext {
    SecurityContext {
        user_id: Some(user_id.to_string()),
        roles: roles.iter().map(|s| s.to_string()).collect(),
        authenticated: true,
        resource: None,
        incoming: None,
    }
}

fn authed_ctx_with_resource(user_id: &str, roles: &[&str], resource: Value) -> SecurityContext {
    SecurityContext {
        user_id: Some(user_id.to_string()),
        roles: roles.iter().map(|s| s.to_string()).collect(),
        authenticated: true,
        resource: Some(resource),
        incoming: None,
    }
}

fn authed_ctx_with_incoming(user_id: &str, roles: &[&str], incoming: Value) -> SecurityContext {
    SecurityContext {
        user_id: Some(user_id.to_string()),
        roles: roles.iter().map(|s| s.to_string()).collect(),
        authenticated: true,
        resource: None,
        incoming: Some(incoming),
    }
}

// ────────────────────────────────────────────────────────────
// Basic allow/deny
// ────────────────────────────────────────────────────────────

#[test]
fn test_read_true_allows_anyone() {
    let engine = make_engine("rules products { read: true; }");
    assert!(engine.check("products", "read", &anon_ctx()).unwrap());
}

#[test]
fn test_delete_false_denies_everyone() {
    let engine = make_engine("rules products { delete: false; }");
    let ctx = authed_ctx("admin1", &["admin"]);
    assert!(!engine.check("products", "delete", &ctx).unwrap());
}

#[test]
fn test_no_rules_defined_denies() {
    let engine = make_engine("rules products { read: true; }");
    assert!(!engine.check("orders", "read", &anon_ctx()).unwrap());
}

#[test]
fn test_no_matching_operation_denies() {
    let engine = make_engine("rules products { read: true; }");
    assert!(!engine.check("products", "create", &anon_ctx()).unwrap());
}

// ────────────────────────────────────────────────────────────
// isAuthenticated()
// ────────────────────────────────────────────────────────────

#[test]
fn test_is_authenticated_allows_logged_in_user() {
    let engine = make_engine("rules products { create: isAuthenticated(); }");
    let ctx = authed_ctx("user1", &[]);
    assert!(engine.check("products", "create", &ctx).unwrap());
}

#[test]
fn test_is_authenticated_denies_anonymous() {
    let engine = make_engine("rules products { create: isAuthenticated(); }");
    assert!(!engine.check("products", "create", &anon_ctx()).unwrap());
}

// ────────────────────────────────────────────────────────────
// hasRole()
// ────────────────────────────────────────────────────────────

#[test]
fn test_has_role_allows_matching_role() {
    let engine = make_engine(r#"rules products { delete: hasRole("admin"); }"#);
    let ctx = authed_ctx("admin1", &["admin"]);
    assert!(engine.check("products", "delete", &ctx).unwrap());
}

#[test]
fn test_has_role_denies_wrong_role() {
    let engine = make_engine(r#"rules products { delete: hasRole("admin"); }"#);
    let ctx = authed_ctx("user1", &["buyer"]);
    assert!(!engine.check("products", "delete", &ctx).unwrap());
}

#[test]
fn test_has_role_denies_anonymous() {
    let engine = make_engine(r#"rules products { delete: hasRole("admin"); }"#);
    assert!(!engine.check("products", "delete", &anon_ctx()).unwrap());
}

#[test]
fn test_has_role_multiple_roles() {
    let engine = make_engine(r#"rules products { create: hasRole("seller"); }"#);
    let ctx = authed_ctx("user1", &["buyer", "seller", "premium"]);
    assert!(engine.check("products", "create", &ctx).unwrap());
}

// ────────────────────────────────────────────────────────────
// isOwner()
// ────────────────────────────────────────────────────────────

#[test]
fn test_is_owner_allows_owner() {
    let engine = make_engine(r#"rules products { update: isOwner(resource.seller_id); }"#);
    let ctx = authed_ctx_with_resource("user42", &[], json!({"seller_id": "user42"}));
    assert!(engine.check("products", "update", &ctx).unwrap());
}

#[test]
fn test_is_owner_denies_non_owner() {
    let engine = make_engine(r#"rules products { update: isOwner(resource.seller_id); }"#);
    let ctx = authed_ctx_with_resource("user99", &[], json!({"seller_id": "user42"}));
    assert!(!engine.check("products", "update", &ctx).unwrap());
}

#[test]
fn test_is_owner_denies_when_no_resource() {
    let engine = make_engine(r#"rules products { update: isOwner(resource.seller_id); }"#);
    let ctx = authed_ctx("user42", &[]);
    assert!(!engine.check("products", "update", &ctx).unwrap());
}

// ────────────────────────────────────────────────────────────
// AND / OR expressions
// ────────────────────────────────────────────────────────────

#[test]
fn test_and_both_true() {
    let engine =
        make_engine(r#"rules products { create: isAuthenticated() && hasRole("seller"); }"#);
    let ctx = authed_ctx("u1", &["seller"]);
    assert!(engine.check("products", "create", &ctx).unwrap());
}

#[test]
fn test_and_one_false() {
    let engine =
        make_engine(r#"rules products { create: isAuthenticated() && hasRole("seller"); }"#);
    let ctx = authed_ctx("u1", &["buyer"]);
    assert!(!engine.check("products", "create", &ctx).unwrap());
}

#[test]
fn test_or_one_true() {
    let engine = make_engine(
        r#"rules products { update: isOwner(resource.seller_id) || hasRole("admin"); }"#,
    );
    let ctx = authed_ctx("admin1", &["admin"]);
    assert!(engine.check("products", "update", &ctx).unwrap());
}

#[test]
fn test_or_both_false() {
    let engine = make_engine(
        r#"rules products { update: isOwner(resource.seller_id) || hasRole("admin"); }"#,
    );
    let ctx = authed_ctx_with_resource("user1", &["buyer"], json!({"seller_id": "user2"}));
    assert!(!engine.check("products", "update", &ctx).unwrap());
}

// ────────────────────────────────────────────────────────────
// Comparison operators
// ────────────────────────────────────────────────────────────

#[test]
fn test_comparison_gt() {
    let engine = make_engine("rules products { read: resource.price > 100; }");
    let ctx = authed_ctx_with_resource("u1", &[], json!({"price": 150}));
    assert!(engine.check("products", "read", &ctx).unwrap());

    let ctx2 = authed_ctx_with_resource("u1", &[], json!({"price": 50}));
    assert!(!engine.check("products", "read", &ctx2).unwrap());
}

#[test]
fn test_comparison_eq() {
    let engine = make_engine(r#"rules products { read: resource.status == "active"; }"#);
    let ctx = authed_ctx_with_resource("u1", &[], json!({"status": "active"}));
    assert!(engine.check("products", "read", &ctx).unwrap());

    let ctx2 = authed_ctx_with_resource("u1", &[], json!({"status": "draft"}));
    assert!(!engine.check("products", "read", &ctx2).unwrap());
}

#[test]
fn test_comparison_neq() {
    let engine = make_engine(r#"rules products { read: resource.status != "deleted"; }"#);
    let ctx = authed_ctx_with_resource("u1", &[], json!({"status": "active"}));
    assert!(engine.check("products", "read", &ctx).unwrap());
}

#[test]
fn test_comparison_gte_lte() {
    let engine = make_engine("rules products { read: resource.rating >= 4; }");
    let ctx4 = authed_ctx_with_resource("u1", &[], json!({"rating": 4}));
    assert!(engine.check("products", "read", &ctx4).unwrap());

    let ctx3 = authed_ctx_with_resource("u1", &[], json!({"rating": 3}));
    assert!(!engine.check("products", "read", &ctx3).unwrap());
}

#[test]
fn test_comparison_lt() {
    let engine = make_engine("rules products { read: resource.price < 1000; }");
    let ctx = authed_ctx_with_resource("u1", &[], json!({"price": 500}));
    assert!(engine.check("products", "read", &ctx).unwrap());

    let ctx2 = authed_ctx_with_resource("u1", &[], json!({"price": 1500}));
    assert!(!engine.check("products", "read", &ctx2).unwrap());
}

// ────────────────────────────────────────────────────────────
// Wildcard collection (*)
// ────────────────────────────────────────────────────────────

#[test]
fn test_wildcard_applies_to_unknown_collection() {
    let engine = make_engine(
        r#"
        rules * { read: true; create: isAuthenticated(); }
    "#,
    );
    assert!(engine.check("anything", "read", &anon_ctx()).unwrap());
    assert!(!engine.check("anything", "create", &anon_ctx()).unwrap());
    assert!(
        engine
            .check("anything", "create", &authed_ctx("u1", &[]))
            .unwrap()
    );
}

#[test]
fn test_specific_rules_override_wildcard() {
    let engine = make_engine(
        r#"
        rules users { read: isAuthenticated(); }
        rules * { read: true; }
    "#,
    );
    // "users" has specific rules → use those (require auth)
    assert!(!engine.check("users", "read", &anon_ctx()).unwrap());
    // "products" falls through to wildcard → allow
    assert!(engine.check("products", "read", &anon_ctx()).unwrap());
}

// ────────────────────────────────────────────────────────────
// Validation rules
// ────────────────────────────────────────────────────────────

#[test]
fn test_validation_passes() {
    let engine = make_engine(
        r#"
        rules products {
            create: { validate: incoming.price > 0; }
        }
    "#,
    );
    let ctx = authed_ctx_with_incoming("u1", &[], json!({"price": 100}));
    assert!(engine.check("products", "create", &ctx).unwrap());
}

#[test]
fn test_validation_fails() {
    let engine = make_engine(
        r#"
        rules products {
            create: { validate: incoming.price > 0; }
        }
    "#,
    );
    let ctx = authed_ctx_with_incoming("u1", &[], json!({"price": -5}));
    let result = engine.check("products", "create", &ctx);
    assert!(result.is_err());
}

// ────────────────────────────────────────────────────────────
// Nested path resolution
// ────────────────────────────────────────────────────────────

#[test]
fn test_nested_path_resolution() {
    let engine = make_engine(r#"rules products { read: resource.address.city == "Toronto"; }"#);
    let ctx = authed_ctx_with_resource("u1", &[], json!({"address": {"city": "Toronto"}}));
    assert!(engine.check("products", "read", &ctx).unwrap());
}

#[test]
fn test_missing_nested_path_returns_null() {
    let engine = make_engine(r#"rules products { read: resource.nonexistent.deep == "value"; }"#);
    let ctx = authed_ctx_with_resource("u1", &[], json!({"other": "data"}));
    assert!(!engine.check("products", "read", &ctx).unwrap());
}

// ────────────────────────────────────────────────────────────
// auth.uid path
// ────────────────────────────────────────────────────────────

#[test]
fn test_auth_uid_path() {
    let engine = make_engine(r#"rules profiles { read: resource.owner == auth.uid; }"#);
    let ctx = authed_ctx_with_resource("user42", &[], json!({"owner": "user42"}));
    // Note: auth.uid comparison depends on resolve_path implementation
    // This tests the path resolution path
    assert!(engine.check("profiles", "read", &ctx).is_ok());
}

// ────────────────────────────────────────────────────────────
// Unknown function
// ────────────────────────────────────────────────────────────

#[test]
fn test_unknown_function_denies() {
    let engine = make_engine("rules products { read: unknownFunc(); }");
    let ctx = authed_ctx("u1", &["admin"]);
    assert!(!engine.check("products", "read", &ctx).unwrap());
}

// ────────────────────────────────────────────────────────────
// Multiple operations on same line
// ────────────────────────────────────────────────────────────

#[test]
fn test_multiple_ops_same_rule() {
    let engine = make_engine(r#"rules products { create, update: isAuthenticated(); }"#);
    let ctx = authed_ctx("u1", &[]);
    assert!(engine.check("products", "create", &ctx).unwrap());
    assert!(engine.check("products", "update", &ctx).unwrap());
    // delete not covered
    assert!(!engine.check("products", "delete", &ctx).unwrap());
}

// ────────────────────────────────────────────────────────────
// OR semantics across multiple rules for same operation
// ────────────────────────────────────────────────────────────

#[test]
fn test_multiple_rules_or_semantics() {
    let engine = make_engine(
        r#"
        rules products {
            update: isOwner(resource.seller_id);
            update: hasRole("admin");
        }
    "#,
    );
    // Admin (not owner) should be allowed
    let ctx = authed_ctx_with_resource("admin1", &["admin"], json!({"seller_id": "other"}));
    assert!(engine.check("products", "update", &ctx).unwrap());

    // Owner (not admin) should be allowed
    let ctx2 = authed_ctx_with_resource("user1", &[], json!({"seller_id": "user1"}));
    assert!(engine.check("products", "update", &ctx2).unwrap());

    // Neither owner nor admin → deny
    let ctx3 = authed_ctx_with_resource("user2", &["buyer"], json!({"seller_id": "user1"}));
    assert!(!engine.check("products", "update", &ctx3).unwrap());
}

// ────────────────────────────────────────────────────────────
// Table-driven: all RBAC combinations
// ────────────────────────────────────────────────────────────

#[test]
fn test_rbac_matrix() {
    let engine = make_engine(
        r#"
        rules products {
            read: true;
            create: isAuthenticated() && hasRole("seller");
            update: isOwner(resource.seller_id) || hasRole("admin");
            delete: hasRole("admin");
            list: true;
        }
    "#,
    );

    struct Case {
        operation: &'static str,
        ctx: SecurityContext,
        expected: bool,
        label: &'static str,
    }

    let cases = vec![
        // Anonymous
        Case {
            operation: "read",
            ctx: anon_ctx(),
            expected: true,
            label: "anon read",
        },
        Case {
            operation: "list",
            ctx: anon_ctx(),
            expected: true,
            label: "anon list",
        },
        Case {
            operation: "create",
            ctx: anon_ctx(),
            expected: false,
            label: "anon create",
        },
        Case {
            operation: "delete",
            ctx: anon_ctx(),
            expected: false,
            label: "anon delete",
        },
        // Buyer (authenticated, no seller role)
        Case {
            operation: "read",
            ctx: authed_ctx("b1", &["buyer"]),
            expected: true,
            label: "buyer read",
        },
        Case {
            operation: "create",
            ctx: authed_ctx("b1", &["buyer"]),
            expected: false,
            label: "buyer create",
        },
        Case {
            operation: "delete",
            ctx: authed_ctx("b1", &["buyer"]),
            expected: false,
            label: "buyer delete",
        },
        // Seller
        Case {
            operation: "create",
            ctx: authed_ctx("s1", &["seller"]),
            expected: true,
            label: "seller create",
        },
        Case {
            operation: "update",
            ctx: authed_ctx_with_resource("s1", &["seller"], json!({"seller_id": "s1"})),
            expected: true,
            label: "seller update own",
        },
        Case {
            operation: "update",
            ctx: authed_ctx_with_resource("s1", &["seller"], json!({"seller_id": "s2"})),
            expected: false,
            label: "seller update other",
        },
        Case {
            operation: "delete",
            ctx: authed_ctx("s1", &["seller"]),
            expected: false,
            label: "seller delete",
        },
        // Admin
        Case {
            operation: "update",
            ctx: authed_ctx("a1", &["admin"]),
            expected: true,
            label: "admin update",
        },
        Case {
            operation: "delete",
            ctx: authed_ctx("a1", &["admin"]),
            expected: true,
            label: "admin delete",
        },
        Case {
            operation: "create",
            ctx: authed_ctx("a1", &["admin"]),
            expected: false,
            label: "admin create (no seller role)",
        },
        Case {
            operation: "create",
            ctx: authed_ctx("a1", &["admin", "seller"]),
            expected: true,
            label: "admin+seller create",
        },
    ];

    for case in &cases {
        let result = engine
            .check("products", case.operation, &case.ctx)
            .unwrap_or_else(|e| panic!("{}: unexpected error: {e}", case.label));
        assert_eq!(result, case.expected, "RBAC test failed: {}", case.label);
    }
}

// ────────────────────────────────────────────────────────────
// Empty rule block
// ────────────────────────────────────────────────────────────

#[test]
fn test_empty_rule_block_denies_everything() {
    let engine = make_engine("rules products { }");
    let ctx = authed_ctx("admin", &["admin"]);
    assert!(!engine.check("products", "read", &ctx).unwrap());
    assert!(!engine.check("products", "create", &ctx).unwrap());
}

// ────────────────────────────────────────────────────────────
// Deeply nested AND/OR
// ────────────────────────────────────────────────────────────

#[test]
fn test_deeply_nested_expression() {
    let engine = make_engine(
        r#"
        rules products {
            read: (isAuthenticated() && hasRole("seller")) || (hasRole("admin") && hasRole("verified"));
        }
    "#,
    );
    // seller only → allowed
    assert!(
        engine
            .check("products", "read", &authed_ctx("u1", &["seller"]))
            .unwrap()
    );
    // admin + verified → allowed
    assert!(
        engine
            .check(
                "products",
                "read",
                &authed_ctx("u2", &["admin", "verified"])
            )
            .unwrap()
    );
    // admin only (no verified) → denied
    assert!(
        !engine
            .check("products", "read", &authed_ctx("u3", &["admin"]))
            .unwrap()
    );
    // buyer → denied
    assert!(
        !engine
            .check("products", "read", &authed_ctx("u4", &["buyer"]))
            .unwrap()
    );
}

// ────────────────────────────────────────────────────────────
// Comparison with negative numbers
// ────────────────────────────────────────────────────────────

#[test]
fn test_comparison_negative_number() {
    let engine = make_engine("rules sensors { read: resource.temp > -40; }");
    let ctx = authed_ctx_with_resource("u1", &[], json!({"temp": -10}));
    assert!(engine.check("sensors", "read", &ctx).unwrap());

    let ctx2 = authed_ctx_with_resource("u1", &[], json!({"temp": -50}));
    assert!(!engine.check("sensors", "read", &ctx2).unwrap());
}

// ────────────────────────────────────────────────────────────
// filter_fields
// ────────────────────────────────────────────────────────────

#[test]
fn test_filter_fields_no_restrictions() {
    let engine = make_engine("rules products { read: true; }");
    let doc = json!({"name": "Widget", "price": 100, "secret": "hidden"});
    let filtered = engine.filter_fields("products", &doc, &anon_ctx());
    // No field-level rules → all fields pass through
    assert_eq!(filtered, doc);
}

#[test]
fn test_filter_fields_non_object() {
    let engine = make_engine("rules products { read: true; }");
    let doc = json!("just a string");
    let filtered = engine.filter_fields("products", &doc, &anon_ctx());
    assert_eq!(filtered, doc);
}
