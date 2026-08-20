//! Extended validation tests for ob-core
//! Tests for postal codes, phone numbers, and additional edge cases

use ob_core::validate::*;

#[test]
fn test_postal_code_valid_canadian() {
    assert!(is_valid_canadian_postal("M5V2H1"));
    assert!(is_valid_canadian_postal("K1A0B1"));
    assert!(is_valid_canadian_postal("V8W3Z5"));
}

#[test]
fn test_postal_code_lowercase() {
    assert!(is_valid_canadian_postal("m5v2h1"));
}

#[test]
fn test_postal_code_with_spaces_invalid() {
    assert!(!is_valid_canadian_postal("M5V 2H1"));
}

#[test]
fn test_postal_code_length_validation() {
    assert!(!is_valid_canadian_postal("M5V2H"));
    assert!(!is_valid_canadian_postal("M5V2H1X"));
    assert!(!is_valid_canadian_postal(""));
}

#[test]
fn test_postal_code_pattern_validation() {
    assert!(!is_valid_canadian_postal("12A4B6"));
    assert!(!is_valid_canadian_postal("MVVVVV"));
}

#[test]
fn test_postal_code_special_characters() {
    assert!(!is_valid_canadian_postal("M5V-2H1"));
    assert!(!is_valid_canadian_postal("M5V/2H1"));
}

#[test]
fn test_record_id_valid() {
    assert!(validate_record_id("users:abc123").is_ok());
    assert!(validate_record_id("orders:ord_12345").is_ok());
    assert!(validate_record_id("products:prod-uuid").is_ok());
}

#[test]
fn test_record_id_missing_colon() {
    assert!(validate_record_id("users_abc123").is_err());
}

#[test]
fn test_record_id_multiple_colons() {
    assert!(validate_record_id("users:rec:extra").is_err());
}

#[test]
fn test_record_id_empty_parts() {
    assert!(validate_record_id(":record").is_err());
    assert!(validate_record_id("users:").is_err());
}

#[test]
fn test_record_id_invalid_collection() {
    assert!(validate_record_id("123users:rec").is_err());
}

#[test]
fn test_record_id_special_characters() {
    assert!(validate_record_id("users:id;DROP").is_err());
    assert!(validate_record_id("users:id'quote").is_err());
}

#[test]
fn test_identifier_valid() {
    assert!(validate_identifier("users").is_ok());
    assert!(validate_identifier("_config").is_ok());
    assert!(validate_identifier("order_items").is_ok());
    assert!(validate_identifier("Products123").is_ok());
}

#[test]
fn test_identifier_invalid_start() {
    assert!(validate_identifier("123abc").is_err());
    assert!(validate_identifier("-table").is_err());
}

#[test]
fn test_identifier_invalid_characters() {
    assert!(validate_identifier("my-table").is_err());
    assert!(validate_identifier("my table").is_err());
    assert!(validate_identifier("users;DROP").is_err());
}

#[test]
fn test_document_id_valid() {
    assert!(validate_document_id("abc123").is_ok());
    assert!(validate_document_id("user-uuid-here").is_ok());
    assert!(validate_document_id("doc.v2").is_ok());
}

#[test]
fn test_document_id_invalid_characters() {
    assert!(validate_document_id("id;DROP TABLE").is_err());
    assert!(validate_document_id("id' OR 1=1").is_err());
    assert!(validate_document_id("id:with:colon").is_err());
}

#[test]
fn test_document_id_empty() {
    assert!(validate_document_id("").is_err());
}

#[test]
fn test_escape_sql_string_single_quotes() {
    assert_eq!(escape_sql_string("it's"), "it\\'s");
}

#[test]
fn test_escape_sql_string_backslashes() {
    assert_eq!(escape_sql_string("path\\to\\file"), "path\\\\to\\\\file");
}

#[test]
fn test_escape_sql_string_mixed() {
    assert_eq!(escape_sql_string("O'Neil's\\path"), "O\\'Neil\\'s\\\\path");
}

#[test]
fn test_escape_sql_string_sql_injection() {
    let input = "'; DROP TABLE users; --";
    let output = escape_sql_string(input);
    assert!(output.contains("\\'"));
}

#[test]
fn test_escape_sql_string_normal() {
    assert_eq!(escape_sql_string("hello"), "hello");
    assert_eq!(escape_sql_string("test123"), "test123");
}
