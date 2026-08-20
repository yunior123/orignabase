//! Snapshot tests for ob-handlers using the `insta` crate.
//!
//! These tests capture serialized output of:
//! - Security rules parser (RuleSet structures)
//! - JWT Claims serialization
//! - Validation error messages
//! - Config defaults
//! - HTML sanitization
//! - Contact information redaction
//!
//! Run with: `cargo test --test snapshot_tests -- --nocapture`
//! Review snapshots with: `cargo insta review`

use ob_core::config::Config;
use ob_handlers::shared::validation::{
    redact_contact_info, sanitize_html, validate_amount_cents, validate_email, validate_string,
    validate_uid,
};
use serde_json::json;

// =============================================================================
// 1. HTML SANITIZATION SNAPSHOT TESTS
// =============================================================================

#[test]
fn snapshot_sanitize_html_basic() {
    let input = "<p>Hello World</p>";
    let output = sanitize_html(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_sanitize_html_with_script_tag() {
    let input = "<script>alert('xss')</script>Hello";
    let output = sanitize_html(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_sanitize_html_nested_tags() {
    let input = "<div><span>nested <b>bold</b></span></div>text";
    let output = sanitize_html(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_sanitize_html_with_attributes() {
    let input = r#"<img src="x" onerror="alert('xss')"/>"#;
    let output = sanitize_html(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_sanitize_html_with_entities() {
    let input = "&lt;script&gt;alert('safe')&lt;/script&gt;";
    let output = sanitize_html(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_sanitize_html_mixed_content() {
    let input =
        r#"<h1>Title</h1><script>bad()</script><p>Safe text</p><iframe src="evil"></iframe>End"#;
    let output = sanitize_html(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_sanitize_html_no_tags() {
    let input = "Plain text without any HTML tags";
    let output = sanitize_html(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_sanitize_html_unclosed_tags() {
    let input = "<p>Unclosed paragraph <span>and span";
    let output = sanitize_html(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_sanitize_html_malformed() {
    let input = "<<script>>double<<</script>>";
    let output = sanitize_html(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_sanitize_html_empty() {
    let input = "";
    let output = sanitize_html(input);
    insta::assert_snapshot!(output);
}

// =============================================================================
// 2. CONTACT REDACTION SNAPSHOT TESTS
// =============================================================================

#[test]
fn snapshot_redact_contact_info_phone_and_email() {
    let input = "Call me at 416-555-1234 or email john@example.com";
    let output = redact_contact_info(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_redact_contact_info_phone_with_plus() {
    let input = "International: +1-416-555-1234";
    let output = redact_contact_info(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_redact_contact_info_phone_no_separator() {
    let input = "Quick call: 4165551234";
    let output = redact_contact_info(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_redact_contact_info_multiple_emails() {
    let input = "Contact alice@example.com or bob.smith@company.co.uk for assistance";
    let output = redact_contact_info(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_redact_contact_info_mixed_formats() {
    let input = "Reach us: (416) 555-1234, support@orignaventures.ca, or 1 800-555-0000";
    let output = redact_contact_info(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_redact_contact_info_no_contact_data() {
    let input = "Just a regular message without any contact info";
    let output = redact_contact_info(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_redact_contact_info_email_edge_cases() {
    let input = "test+tag@example.com and user_name.123@sub.domain.co";
    let output = redact_contact_info(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_redact_contact_info_phone_spaces() {
    let input = "Phone: 416 555 1234";
    let output = redact_contact_info(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_redact_contact_info_empty() {
    let input = "";
    let output = redact_contact_info(input);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_redact_contact_info_fake_data() {
    let input = "Fake phone like 555-1234 (too short), and almost-email test@";
    let output = redact_contact_info(input);
    insta::assert_snapshot!(output);
}

// =============================================================================
// 3. VALIDATION ERROR MESSAGES SNAPSHOT TESTS
// =============================================================================

#[test]
fn snapshot_validate_string_empty() {
    let result = validate_string("username", "", 50);
    let err_msg = result.unwrap_err().to_string();
    insta::assert_snapshot!(err_msg);
}

#[test]
fn snapshot_validate_string_too_long() {
    let long_string = "a".repeat(200);
    let result = validate_string("description", &long_string, 100);
    let err_msg = result.unwrap_err().to_string();
    insta::assert_snapshot!(err_msg);
}

#[test]
fn snapshot_validate_string_ok() {
    let result = validate_string("name", "John Doe", 50);
    assert!(result.is_ok());
}

#[test]
fn snapshot_validate_email_invalid_no_at() {
    let result = validate_email("notanemail.com");
    let err_msg = result.unwrap_err().to_string();
    insta::assert_snapshot!(err_msg);
}

#[test]
fn snapshot_validate_email_invalid_no_dot() {
    let result = validate_email("user@domain");
    let err_msg = result.unwrap_err().to_string();
    insta::assert_snapshot!(err_msg);
}

#[test]
fn snapshot_validate_email_too_long() {
    let long_email = format!("{}@example.com", "a".repeat(300));
    let result = validate_email(&long_email);
    let err_msg = result.unwrap_err().to_string();
    insta::assert_snapshot!(err_msg);
}

#[test]
fn snapshot_validate_email_ok() {
    let result = validate_email("user@example.com");
    assert!(result.is_ok());
}

#[test]
fn snapshot_validate_uid_empty() {
    let result = validate_uid("user_id", "");
    let err_msg = result.unwrap_err().to_string();
    insta::assert_snapshot!(err_msg);
}

#[test]
fn snapshot_validate_uid_too_long() {
    let long_uid = "x".repeat(200);
    let result = validate_uid("user_id", &long_uid);
    let err_msg = result.unwrap_err().to_string();
    insta::assert_snapshot!(err_msg);
}

#[test]
fn snapshot_validate_uid_ok() {
    let result = validate_uid("user_id", "user_12345");
    assert!(result.is_ok());
}

#[test]
fn snapshot_validate_amount_cents_negative() {
    let result = validate_amount_cents("price", -100);
    let err_msg = result.unwrap_err().to_string();
    insta::assert_snapshot!(err_msg);
}

#[test]
fn snapshot_validate_amount_cents_exceeds_max() {
    let result = validate_amount_cents("price", 200_000_000);
    let err_msg = result.unwrap_err().to_string();
    insta::assert_snapshot!(err_msg);
}

#[test]
fn snapshot_validate_amount_cents_ok() {
    let result = validate_amount_cents("price", 5000);
    assert!(result.is_ok());
}

#[test]
fn snapshot_validate_amount_cents_zero() {
    // Zero amount is now correctly rejected (P2-NEW: validate_amount_cents allows zero)
    let result = validate_amount_cents("price", 0);
    assert!(result.is_err());
}

#[test]
fn snapshot_validate_amount_cents_max_allowed() {
    let result = validate_amount_cents("price", 10_000_000);
    assert!(result.is_ok());
}

// =============================================================================
// 4. CONFIG DEFAULTS SNAPSHOT TESTS
// =============================================================================

#[test]
fn snapshot_config_default_values() {
    let config = Config::load(None).unwrap();
    insta::assert_debug_snapshot!(config);
}

#[test]
fn snapshot_config_as_json() {
    let config = Config::load(None).unwrap();
    let txt = format!("{:#?}", config);
    insta::assert_snapshot!(txt);
}

#[test]
fn snapshot_config_auth_defaults() {
    let config = Config::load(None).unwrap();
    let auth_snapshot = json!({
        "jwt_secret": config.auth.jwt_secret,
        "access_token_ttl_secs": config.auth.access_token_ttl_secs,
        "refresh_token_ttl_secs": config.auth.refresh_token_ttl_secs,
    });
    insta::assert_json_snapshot!(auth_snapshot);
}

#[test]
fn snapshot_config_database_defaults() {
    let config = Config::load(None).unwrap();
    let db_snapshot = json!({
        "url": config.database.url,
        "max_connections": config.database.max_connections,
    });
    insta::assert_json_snapshot!(db_snapshot);
}

#[test]
fn snapshot_config_security_defaults() {
    let config = Config::load(None).unwrap();
    let sec_snapshot = json!({
        "rules_path": config.security.rules_path,
    });
    insta::assert_json_snapshot!(sec_snapshot);
}

#[test]
fn snapshot_config_cluster_defaults() {
    let config = Config::load(None).unwrap();
    let cluster_snapshot = json!({
        "enabled": config.cluster.enabled,
        "nats_url": config.cluster.nats_url,
        "node_id": config.cluster.node_id,
    });
    insta::assert_json_snapshot!(cluster_snapshot);
}

#[test]
fn snapshot_config_tenant_defaults() {
    let config = Config::load(None).unwrap();
    let tenant_snapshot = json!({
        "multi_tenant": config.tenant.multi_tenant,
        "header_name": config.tenant.header_name,
    });
    insta::assert_json_snapshot!(tenant_snapshot);
}

// =============================================================================
// 5. JWT CLAIMS SERIALIZATION SNAPSHOT TESTS
// =============================================================================

#[test]
fn snapshot_jwt_claims_access_token() {
    use ob_auth::jwt::Claims;

    let now = 1700000000;
    let claims = Claims {
        sub: "user_12345".to_string(),
        iat: now,
        exp: now + 900,
        roles: vec!["buyer".to_string(), "verified".to_string()],
        typ: "access".to_string(),
        email_verified: true,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    let json = serde_json::to_value(&claims).unwrap();
    insta::assert_json_snapshot!(json, @r#"
    {
      "email_verified": true,
      "exp": 1700000900,
      "iat": 1700000000,
      "mfa_required": false,
      "roles": [
        "buyer",
        "verified"
      ],
      "sub": "user_12345",
      "typ": "access"
    }
    "#);
}

#[test]
fn snapshot_jwt_claims_refresh_token() {
    use ob_auth::jwt::Claims;

    let now = 1700000000;
    let claims = Claims {
        sub: "user_67890".to_string(),
        iat: now,
        exp: now + 604800,
        roles: vec![],
        typ: "refresh".to_string(),
        email_verified: false,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    let json = serde_json::to_value(&claims).unwrap();
    insta::assert_json_snapshot!(json, @r#"
    {
      "email_verified": false,
      "exp": 1700604800,
      "iat": 1700000000,
      "mfa_required": false,
      "roles": [],
      "sub": "user_67890",
      "typ": "refresh"
    }
    "#);
}

#[test]
#[cfg_attr(not(feature = "integration"), ignore)]
fn snapshot_jwt_claims_with_custom_claims() {
    use ob_auth::jwt::Claims;

    let now = 1700000000;
    let custom = json!({
        "role": "seller",
        "plan": "pro",
        "store_id": "store_abc123"
    });

    let claims = Claims {
        sub: "seller_001".to_string(),
        iat: now,
        exp: now + 900,
        roles: vec!["seller".to_string()],
        typ: "access".to_string(),
        email_verified: true,
        mfa_required: false,
        custom_claims: custom,
    };

    let json = serde_json::to_value(&claims).unwrap();
    insta::assert_json_snapshot!(json, @r###"
    {
      "custom_claims": {
        "plan": "pro",
        "role": "seller",
        "store_id": "store_abc123"
      },
      "email_verified": true,
      "exp": 1700000900,
      "iat": 1700000000,
      "mfa_required": false,
      "roles": [
        "seller"
      ],
      "sub": "seller_001",
      "typ": "access"
    }
    "###);
}

#[test]
fn snapshot_jwt_claims_mfa_challenge() {
    use ob_auth::jwt::Claims;

    let now = 1700000000;
    let claims = Claims {
        sub: "user_mfa".to_string(),
        iat: now,
        exp: now + 300, // 5 minutes
        roles: vec![],
        typ: "mfa_challenge".to_string(),
        email_verified: false,
        mfa_required: true,
        custom_claims: serde_json::Value::Null,
    };

    let json = serde_json::to_value(&claims).unwrap();
    insta::assert_json_snapshot!(json, @r#"
    {
      "email_verified": false,
      "exp": 1700000300,
      "iat": 1700000000,
      "mfa_required": true,
      "roles": [],
      "sub": "user_mfa",
      "typ": "mfa_challenge"
    }
    "#);
}

#[test]
fn snapshot_jwt_claims_verification_token() {
    use ob_auth::jwt::Claims;

    let now = 1700000000;
    let claims = Claims {
        sub: "user_verify".to_string(),
        iat: now,
        exp: now + 86400, // 24 hours
        roles: vec![],
        typ: "email_verify".to_string(),
        email_verified: false,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    let json = serde_json::to_value(&claims).unwrap();
    insta::assert_json_snapshot!(json, @r#"
    {
      "email_verified": false,
      "exp": 1700086400,
      "iat": 1700000000,
      "mfa_required": false,
      "roles": [],
      "sub": "user_verify",
      "typ": "email_verify"
    }
    "#);
}

#[test]
fn snapshot_jwt_claims_password_reset_token() {
    use ob_auth::jwt::Claims;

    let now = 1700000000;
    let claims = Claims {
        sub: "user_reset".to_string(),
        iat: now,
        exp: now + 3600, // 1 hour
        roles: vec![],
        typ: "password_reset".to_string(),
        email_verified: false,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    let json = serde_json::to_value(&claims).unwrap();
    insta::assert_json_snapshot!(json, @r#"
    {
      "email_verified": false,
      "exp": 1700003600,
      "iat": 1700000000,
      "mfa_required": false,
      "roles": [],
      "sub": "user_reset",
      "typ": "password_reset"
    }
    "#);
}

#[test]
fn snapshot_jwt_claims_magic_link_token() {
    use ob_auth::jwt::Claims;

    let now = 1700000000;
    let claims = Claims {
        sub: "user_magic".to_string(),
        iat: now,
        exp: now + 900, // 15 minutes
        roles: vec![],
        typ: "magic_link".to_string(),
        email_verified: false,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    let json = serde_json::to_value(&claims).unwrap();
    insta::assert_json_snapshot!(json, @r#"
    {
      "email_verified": false,
      "exp": 1700000900,
      "iat": 1700000000,
      "mfa_required": false,
      "roles": [],
      "sub": "user_magic",
      "typ": "magic_link"
    }
    "#);
}

// =============================================================================
// 6. SECURITY RULES PARSER SNAPSHOT TESTS
// =============================================================================

#[test]
fn snapshot_parse_basic_rules() {
    use ob_security::parser::parse_rules;

    let input = r#"
        rules products {
            read: true;
            create: isAuthenticated();
        }
    "#;

    let result = parse_rules(input).unwrap();
    insta::assert_debug_snapshot!(result);
}

#[test]
fn snapshot_parse_complex_rules() {
    use ob_security::parser::parse_rules;

    let input = r#"
        rules products {
            read: true;
            create: isAuthenticated() && hasRole("seller");
            update: isOwner(resource.seller_id) || hasRole("admin");
            delete: hasRole("admin");
        }
    "#;

    let result = parse_rules(input).unwrap();
    insta::assert_debug_snapshot!(result);
}

#[test]
fn snapshot_parse_comparison_rules() {
    use ob_security::parser::parse_rules;

    let input = r#"
        rules products {
            read: resource.price > 100;
            update: resource.status == "active";
            delete: resource.rating < 2.5;
        }
    "#;

    let result = parse_rules(input).unwrap();
    insta::assert_debug_snapshot!(result);
}

#[test]
fn snapshot_parse_wildcard_rules() {
    use ob_security::parser::parse_rules;

    let input = r#"
        rules * {
            read: true;
            create: isAuthenticated();
        }
    "#;

    let result = parse_rules(input).unwrap();
    insta::assert_debug_snapshot!(result);
}

#[test]
#[cfg_attr(not(feature = "integration"), ignore)]
fn snapshot_parse_multi_collection_rules() {
    use ob_security::parser::parse_rules;
    use std::collections::BTreeMap;

    let input = r#"
        rules users {
            read: isAuthenticated();
            update: isOwner(resource.uid) || hasRole("admin");
        }
        rules orders {
            read: isAuthenticated();
            create: isAuthenticated();
            update: isOwner(resource.buyer_id) || hasRole("admin");
        }
        rules * {
            read: true;
        }
    "#;

    let result = parse_rules(input).unwrap();
    let sorted: BTreeMap<_, _> = result.into_iter().collect();
    insta::assert_debug_snapshot!(sorted);
}

#[test]
fn snapshot_parse_validate_rules() {
    use ob_security::parser::parse_rules;

    let input = r#"
        rules products {
            create: {
                validate: incoming.price > 0;
            }
        }
    "#;

    let result = parse_rules(input).unwrap();
    insta::assert_debug_snapshot!(result);
}

#[test]
fn snapshot_parse_or_expression() {
    use ob_security::parser::parse_rules;

    let input = r#"
        rules products {
            delete: hasRole("admin") || hasRole("super_admin");
        }
    "#;

    let result = parse_rules(input).unwrap();
    insta::assert_debug_snapshot!(result);
}

#[test]
fn snapshot_parse_and_expression() {
    use ob_security::parser::parse_rules;

    let input = r#"
        rules products {
            update: isAuthenticated() && hasRole("seller") && isOwner(resource.seller_id);
        }
    "#;

    let result = parse_rules(input).unwrap();
    insta::assert_debug_snapshot!(result);
}

#[test]
fn snapshot_parse_function_with_args() {
    use ob_security::parser::parse_rules;

    let input = r#"
        rules products {
            update: customCheck("admin", 42, resource.field);
        }
    "#;

    let result = parse_rules(input).unwrap();
    insta::assert_debug_snapshot!(result);
}

#[test]
fn snapshot_parse_nested_expressions() {
    use ob_security::parser::parse_rules;

    let input = r#"
        rules products {
            read: (isAuthenticated() && hasRole("seller")) || hasRole("admin");
        }
    "#;

    let result = parse_rules(input).unwrap();
    insta::assert_debug_snapshot!(result);
}

// =============================================================================
// 7. SCHEMA ENUM SERIALIZATION SNAPSHOT TESTS
// =============================================================================

#[test]
fn snapshot_order_status_all_variants() {
    use ob_handlers::shared::schema::OrderStatus;

    let variants = [
        OrderStatus::PendingPayment,
        OrderStatus::PaymentAuthorized,
        OrderStatus::AwaitingShippingApproval,
        OrderStatus::Processing,
        OrderStatus::Shipped,
        OrderStatus::Delivered,
        OrderStatus::Cancelled,
        OrderStatus::Refunded,
    ];

    let json: Vec<_> = variants
        .iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect();

    insta::assert_json_snapshot!(json);
}

#[test]
fn snapshot_payment_status_all_variants() {
    use ob_handlers::shared::schema::PaymentStatus;

    let variants = [
        PaymentStatus::Pending,
        PaymentStatus::Authorized,
        PaymentStatus::Captured,
        PaymentStatus::Refunded,
        PaymentStatus::PartialRefund,
        PaymentStatus::Failed,
        PaymentStatus::Cancelled,
        PaymentStatus::Disputed,
    ];

    let json: Vec<_> = variants
        .iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect();

    insta::assert_json_snapshot!(json);
}

#[test]
fn snapshot_user_role_all_variants() {
    use ob_handlers::shared::schema::UserRole;

    let variants = [UserRole::Buyer, UserRole::Seller, UserRole::Admin];

    let json: Vec<_> = variants
        .iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect();

    insta::assert_json_snapshot!(json);
}

#[test]
fn snapshot_subscription_status_all_variants() {
    use ob_handlers::shared::schema::SubscriptionStatus;

    let variants = [
        SubscriptionStatus::Active,
        SubscriptionStatus::Cancelled,
        SubscriptionStatus::PastDue,
        SubscriptionStatus::Expired,
    ];

    let json: Vec<_> = variants
        .iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect();

    insta::assert_json_snapshot!(json);
}

#[test]
fn snapshot_coupon_type_all_variants() {
    use ob_handlers::shared::schema::CouponType;

    let variants = [
        CouponType::Percentage,
        CouponType::FixedAmount,
        CouponType::FreeShipping,
    ];

    let json: Vec<_> = variants
        .iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect();

    insta::assert_json_snapshot!(json);
}

#[test]
fn snapshot_filter_value_all_variants() {
    use ob_handlers::shared::schema::FilterValue;

    let variants = [
        FilterValue::Recent,
        FilterValue::Popular,
        FilterValue::PriceLowToHigh,
        FilterValue::PriceHighToLow,
        FilterValue::TopRated,
    ];

    let json: Vec<_> = variants
        .iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect();

    insta::assert_json_snapshot!(json);
}

#[test]
fn snapshot_return_request_status_all_variants() {
    use ob_handlers::shared::schema::ReturnRequestStatus;

    let variants = [
        ReturnRequestStatus::Requested,
        ReturnRequestStatus::Approved,
        ReturnRequestStatus::Rejected,
        ReturnRequestStatus::Completed,
    ];

    let json: Vec<_> = variants
        .iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect();

    insta::assert_json_snapshot!(json);
}

// =============================================================================
// 8. COMPREHENSIVE REGRESSION TESTS
// =============================================================================

#[test]
fn snapshot_comprehensive_sanitization_and_redaction() {
    let input = r#"
        <script>alert('xss')</script>
        Contact support@example.com or call 416-555-0123
        <p>Safe content here</p>
    "#;

    let sanitized = sanitize_html(input);
    let redacted = redact_contact_info(&sanitized);

    let combined = json!({
        "original": input,
        "sanitized": sanitized,
        "redacted": redacted,
    });

    insta::assert_json_snapshot!(combined);
}

#[test]
fn snapshot_all_validation_patterns() {
    let patterns = json!({
        "valid_string": validate_string("field", "valid", 100).is_ok(),
        "invalid_empty_string": validate_string("field", "", 100).is_err(),
        "invalid_too_long": validate_string("field", &"x".repeat(200), 100).is_err(),
        "valid_email": validate_email("user@example.com").is_ok(),
        "invalid_email": validate_email("notanemail").is_err(),
        "valid_amount": validate_amount_cents("price", 5000).is_ok(),
        "invalid_negative_amount": validate_amount_cents("price", -1).is_err(),
        "valid_uid": validate_uid("id", "abc123").is_ok(),
        "invalid_empty_uid": validate_uid("id", "").is_err(),
    });

    insta::assert_json_snapshot!(patterns);
}
