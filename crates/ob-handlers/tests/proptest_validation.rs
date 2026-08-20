//! Property-based tests for validation functions using proptest.
//!
//! Tests validate string, amount, email, HTML sanitization, contact info
//! redaction, and UID validation using generated inputs to ensure correctness
//! across a wide range of inputs.

use proptest::prelude::*;

use ob_core::validate;
use ob_handlers::shared::validation;

// ═══════════════════════════════════════════════════════════════════
// STRING VALIDATION PROPERTY TESTS
// ═══════════════════════════════════════════════════════════════════

proptest! {
    #[test]
    fn prop_valid_strings_accepted(s in "[a-zA-Z0-9 ]{1,100}") {
        prop_assert!(validation::validate_string("field", &s, 100).is_ok());
    }

    #[test]
    fn prop_overlength_strings_rejected(s in "[ -~]{101,200}") {
        prop_assert!(validation::validate_string("field", &s, 100).is_err());
    }

    #[test]
    fn prop_empty_string_rejected(max_len in 1usize..1000) {
        prop_assert!(validation::validate_string("field", "", max_len).is_err());
    }
}

// ═══════════════════════════════════════════════════════════════════
// AMOUNT (CENTS) VALIDATION PROPERTY TESTS
// ═══════════════════════════════════════════════════════════════════

proptest! {
    #[test]
    fn prop_valid_amounts_accepted(cents in 0i64..=10_000_000) {
        prop_assert!(validation::validate_amount_cents("price", cents).is_ok());
    }

    #[test]
    fn prop_negative_amounts_rejected(cents in i64::MIN..0i64) {
        prop_assert!(validation::validate_amount_cents("price", cents).is_err());
    }

    #[test]
    fn prop_huge_amounts_rejected(cents in 10_000_001i64..=i64::MAX) {
        prop_assert!(validation::validate_amount_cents("price", cents).is_err());
    }
}

// ═══════════════════════════════════════════════════════════════════
// EMAIL VALIDATION PROPERTY TESTS
// ═══════════════════════════════════════════════════════════════════

proptest! {
    #[test]
    fn prop_valid_emails_accepted(user in "[a-z]{1,10}", domain in "[a-z]{1,10}") {
        let email = format!("{user}@{domain}.com");
        prop_assert!(validation::validate_email(&email).is_ok());
    }

    #[test]
    fn prop_no_at_rejected(s in "[a-zA-Z0-9]{1,50}") {
        prop_assert!(validation::validate_email(&s).is_err());
    }

    #[test]
    fn prop_overlength_emails_rejected(local in "[a-z]{243,250}") {
        let email = format!("{local}@example.com");
        prop_assert!(email.len() > 254);
        prop_assert!(validation::validate_email(&email).is_err());
    }
}

// ═══════════════════════════════════════════════════════════════════
// HTML SANITIZATION PROPERTY TESTS
// ═══════════════════════════════════════════════════════════════════

proptest! {
    #[test]
    fn prop_sanitized_has_no_angle_brackets(input in ".*") {
        let result = validation::sanitize_html(&input);
        prop_assert!(!result.contains('<'));
        prop_assert!(!result.contains('>'));
    }

    #[test]
    fn prop_script_tags_removed(payload in "[a-zA-Z0-9 .,!?]{1,20}") {
        let input = format!("<script>{payload}</script>");
        let result = validation::sanitize_html(&input);
        prop_assert!(!result.contains("<script>"));
    }

    #[test]
    fn prop_plain_text_unchanged(input in "[a-zA-Z0-9 .,!?]{0,80}") {
        let result = validation::sanitize_html(&input);
        prop_assert_eq!(result, input);
    }
}

// ═══════════════════════════════════════════════════════════════════
// CONTACT INFO REDACTION PROPERTY TESTS
// ═══════════════════════════════════════════════════════════════════

proptest! {
    #[test]
    fn prop_emails_redacted(user in "[a-z]{1,10}", domain in "[a-z]{1,10}") {
        let email = format!("{user}@{domain}.com");
        let input = format!("Contact {email} for info");
        let result = validation::redact_contact_info(&input);
        prop_assert!(!result.contains(&email));
        prop_assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn prop_phone_numbers_redacted(area in "[0-9]{3}", prefix in "[0-9]{3}", line in "[0-9]{4}") {
        let phone = format!("{area}-{prefix}-{line}");
        let input = format!("Call {phone} now");
        let result = validation::redact_contact_info(&input);
        prop_assert!(!result.contains(&phone));
        prop_assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn prop_plain_text_without_contact_info_unchanged(input in "[a-zA-Z ]{0,60}") {
        let result = validation::redact_contact_info(&input);
        prop_assert_eq!(result, input);
    }
}

// ═══════════════════════════════════════════════════════════════════
// UID VALIDATION PROPERTY TESTS
// ═══════════════════════════════════════════════════════════════════

proptest! {
    #[test]
    fn prop_valid_uids_accepted(uid in "[a-zA-Z0-9_-]{1,128}") {
        prop_assert!(validation::validate_uid("id", &uid).is_ok());
    }

    #[test]
    fn prop_empty_uid_rejected(field in "[a-z]{1,20}") {
        prop_assert!(validation::validate_uid(&field, "").is_err());
    }

    #[test]
    fn prop_overlength_uid_rejected(uid in "[a-zA-Z0-9]{129,256}") {
        prop_assert!(validation::validate_uid("id", &uid).is_err());
    }
}

// ═══════════════════════════════════════════════════════════════════
// CORE VALIDATION (SQL identifiers, document IDs)
// ═══════════════════════════════════════════════════════════════════

proptest! {
    #[test]
    fn prop_valid_identifiers_accepted(rest in "[a-zA-Z0-9_]{0,50}") {
        let name = format!("a{rest}");
        prop_assert!(validate::validate_identifier(&name).is_ok());
    }

    #[test]
    fn prop_numeric_start_rejected(first in "[0-9]", rest in "[a-zA-Z0-9_]{0,20}") {
        let name = format!("{first}{rest}");
        prop_assert!(validate::validate_identifier(&name).is_err());
    }

    #[test]
    fn prop_valid_doc_ids_accepted(id in "[a-zA-Z0-9._-]{1,128}") {
        prop_assert!(validate::validate_document_id(&id).is_ok());
    }
}

// ═══════════════════════════════════════════════════════════════════
// REGEX SAFETY TESTS (no panics on arbitrary input)
// ═══════════════════════════════════════════════════════════════════

proptest! {
    #[test]
    fn prop_escape_never_panics(s in "\\PC*") {
        let _ = validate::escape_sql_string(&s);
    }

    #[test]
    fn prop_sanitize_never_panics(s in "\\PC*") {
        let _ = validation::sanitize_html(&s);
    }

    #[test]
    fn prop_redact_never_panics(s in "\\PC*") {
        let _ = validation::redact_contact_info(&s);
    }
}

// ═══════════════════════════════════════════════════════════════════
// CROSS-PROPERTY TESTS (interactions between validations)
// ═══════════════════════════════════════════════════════════════════

proptest! {
    #[test]
    fn prop_escaped_string_safe_for_sql(s in ".*") {
        let escaped = validate::escape_sql_string(&s);
        let chars: Vec<char> = escaped.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            if c == '\'' {
                prop_assert!(i > 0);
                prop_assert_eq!(chars[i - 1], '\\');
            }
        }
    }

    #[test]
    fn prop_sanitize_then_validate_string(s in "[a-zA-Z ]{1,50}") {
        let sanitized = validation::sanitize_html(&s);
        prop_assert!(validation::validate_string("field", &sanitized, 100).is_ok());
    }

    #[test]
    fn prop_redacted_output_still_valid_string(
        s in "[a-zA-Z ]{1,20}",
        user in "[a-z]{1,10}",
        domain in "[a-z]{1,10}"
    ) {
        let input = format!("{s} {user}@{domain}.com");
        let redacted = validation::redact_contact_info(&input);
        prop_assert!(validation::validate_string("field", &redacted, 100).is_ok());
    }
}

// ═══════════════════════════════════════════════════════════════════
// SANITY CHECKS (ensure we're testing the right functions)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn sanity_validate_string_basic() {
    assert!(validation::validate_string("name", "hello", 10).is_ok());
    assert!(validation::validate_string("name", "", 10).is_err());
    assert!(validation::validate_string("name", "hello", 3).is_err());
}

#[test]
fn sanity_validate_amount_basic() {
    assert!(validation::validate_amount_cents("price", 1000).is_ok());
    assert!(validation::validate_amount_cents("price", -1).is_err());
    assert!(validation::validate_amount_cents("price", 10_000_001).is_err());
}

#[test]
fn sanity_validate_email_basic() {
    assert!(validation::validate_email("test@example.com").is_ok());
    assert!(validation::validate_email("invalid").is_err());
    assert!(validation::validate_email("no-dot@example").is_err());
}

#[test]
fn sanity_sanitize_html_basic() {
    let result = validation::sanitize_html("<script>alert('xss')</script>hello");
    assert!(!result.contains('<'));
    assert!(!result.contains('>'));
    assert!(result.contains("hello"));
}

#[test]
fn sanity_redact_contact_info_basic() {
    let result = validation::redact_contact_info("Email john@example.com");
    assert!(!result.contains("john@example.com") || result.contains("[REDACTED]"));
}

#[test]
fn sanity_validate_uid_basic() {
    assert!(validation::validate_uid("id", "user123").is_ok());
    assert!(validation::validate_uid("id", "").is_err());
    assert!(validation::validate_uid("id", &"a".repeat(129)).is_err());
}

#[test]
fn sanity_validate_identifier_basic() {
    assert!(validate::validate_identifier("users").is_ok());
    assert!(validate::validate_identifier("_config").is_ok());
    assert!(validate::validate_identifier("123invalid").is_err());
    assert!(validate::validate_identifier("my-table").is_err());
}

#[test]
fn sanity_validate_document_id_basic() {
    assert!(validate::validate_document_id("user-123").is_ok());
    assert!(validate::validate_document_id("doc.v2").is_ok());
    assert!(validate::validate_document_id("").is_err());
}

#[test]
fn sanity_escape_sql_string_basic() {
    assert_eq!(validate::escape_sql_string("hello"), "hello");
    assert_eq!(validate::escape_sql_string("it's"), "it\\'s");
    assert_eq!(validate::escape_sql_string("a\\b"), "a\\\\b");
}
