use crate::{Error, Result};

/// Validate that a string is a safe SurrealDB identifier (collection name, field name, index name).
/// Only allows: ASCII alphanumeric + underscore, must start with letter or underscore, max 255 chars.
pub fn validate_identifier(name: &str) -> Result<&str> {
    if name.is_empty() {
        return Err(Error::Validation("Identifier cannot be empty".into()));
    }
    if name.len() > 255 {
        return Err(Error::Validation("Identifier too long (max 255)".into()));
    }
    let first = name.chars().next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return Err(Error::Validation(format!(
            "Identifier must start with a letter or underscore, got: '{}'",
            first
        )));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(Error::Validation(format!(
            "Identifier '{}' contains invalid characters (only ASCII alphanumeric and underscore allowed)",
            name
        )));
    }
    Ok(name)
}

/// Validate a document ID (more permissive — allows hyphens and dots).
pub fn validate_document_id(id: &str) -> Result<&str> {
    if id.is_empty() {
        return Err(Error::Validation("Document ID cannot be empty".into()));
    }
    if id.len() > 512 {
        return Err(Error::Validation("Document ID too long (max 512)".into()));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err(Error::Validation(format!(
            "Document ID '{}' contains invalid characters",
            id
        )));
    }
    Ok(id)
}

/// Escape a string value for use in SurrealQL string literals.
/// Prevents injection via single quotes.
pub fn escape_surreal_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_identifiers() {
        assert!(validate_identifier("users").is_ok());
        assert!(validate_identifier("_config").is_ok());
        assert!(validate_identifier("order_items").is_ok());
        assert!(validate_identifier("Products123").is_ok());
        assert!(validate_identifier("a").is_ok());
    }

    #[test]
    fn test_invalid_identifiers() {
        assert!(validate_identifier("").is_err()); // empty
        assert!(validate_identifier("123abc").is_err()); // starts with digit
        assert!(validate_identifier("my-table").is_err()); // hyphen
        assert!(validate_identifier("my table").is_err()); // space
        assert!(validate_identifier("users;DROP").is_err()); // semicolon
        assert!(validate_identifier("t\u{00e0}ble").is_err()); // unicode
        assert!(validate_identifier("tab\nle").is_err()); // newline
        assert!(validate_identifier("SELECT").is_ok()); // keywords are ok (SurrealDB handles)
    }

    #[test]
    fn test_identifier_length_limit() {
        let long = "a".repeat(255);
        assert!(validate_identifier(&long).is_ok());
        let too_long = "a".repeat(256);
        assert!(validate_identifier(&too_long).is_err());
    }

    #[test]
    fn test_valid_document_ids() {
        assert!(validate_document_id("abc123").is_ok());
        assert!(validate_document_id("user-uuid-here").is_ok());
        assert!(validate_document_id("doc.v2").is_ok());
        assert!(validate_document_id("a_b-c.d").is_ok());
    }

    #[test]
    fn test_invalid_document_ids() {
        assert!(validate_document_id("").is_err());
        assert!(validate_document_id("id;DROP TABLE").is_err());
        assert!(validate_document_id("id' OR 1=1").is_err());
        assert!(validate_document_id("id\n").is_err());
    }

    #[test]
    fn test_escape_surreal_string() {
        assert_eq!(escape_surreal_string("hello"), "hello");
        assert_eq!(escape_surreal_string("it's"), "it\\'s");
        assert_eq!(escape_surreal_string("a\\b"), "a\\\\b");
        assert_eq!(
            escape_surreal_string("'; DROP TABLE--"),
            "\\'; DROP TABLE--"
        );
    }

    #[test]
    fn test_escape_surreal_string_combined() {
        let input = "O'Reilly's \\ Book";
        let escaped = escape_surreal_string(input);
        assert_eq!(escaped, "O\\'Reilly\\'s \\\\ Book");
    }
}

/// Validate a SurrealDB record ID (format: "collection:record_id").
/// Must contain exactly one colon separating valid collection and record parts.
pub fn validate_surreal_record_id(id: &str) -> Result<&str> {
    if id.is_empty() {
        return Err(Error::Validation("Record ID cannot be empty".into()));
    }
    if id.len() > 512 {
        return Err(Error::Validation("Record ID too long (max 512)".into()));
    }
    
    let parts: Vec<&str> = id.split(':').collect();
    if parts.len() != 2 {
        return Err(Error::Validation(
            format!("Record ID '{}' must be in format 'collection:record_id'", id)
        ));
    }
    
    let [collection, record_id] = [parts[0], parts[1]];
    
    // Validate collection name (identifier rules)
    if collection.is_empty() {
        return Err(Error::Validation("Collection name cannot be empty".into()));
    }
    validate_identifier(collection)?;
    
    // Validate record ID part (document ID rules)
    if record_id.is_empty() {
        return Err(Error::Validation("Record ID part cannot be empty".into()));
    }
    validate_document_id(record_id)?;
    
    Ok(id)
}

#[cfg(test)]
mod record_id_tests {
    use super::*;

    #[test]
    fn test_valid_surreal_record_ids() {
        assert!(validate_surreal_record_id("users:abc123").is_ok());
        assert!(validate_surreal_record_id("orders:ord_12345").is_ok());
        assert!(validate_surreal_record_id("products:prod-uuid").is_ok());
        assert!(validate_surreal_record_id("_internal:rec123").is_ok());
    }

    #[test]
    fn test_invalid_surreal_record_ids() {
        assert!(validate_surreal_record_id("").is_err()); // empty
        assert!(validate_surreal_record_id("no_colon").is_err()); // missing colon
        assert!(validate_surreal_record_id("123abc:rec").is_err()); // invalid collection
        assert!(validate_surreal_record_id("users:").is_err()); // empty record part
        assert!(validate_surreal_record_id(":record").is_err()); // empty collection
        assert!(validate_surreal_record_id("users:rec:extra").is_err()); // multiple colons
        assert!(validate_surreal_record_id("users:rec;DROP").is_err()); // invalid chars
    }
}

    #[test]
    fn test_postal_code_validation_valid() {
        assert!(is_valid_canadian_postal("M5V2H1"));
        assert!(is_valid_canadian_postal("K1A0B1"));
        assert!(is_valid_canadian_postal("V8W3Z5"));
    }

    #[test]
    fn test_postal_code_validation_lowercase() {
        // Validator handles lowercase
        assert!(is_valid_canadian_postal("m5v2h1"));
    }

    #[test]
    fn test_postal_code_validation_with_spaces() {
        // These should fail (no spaces in validator)
        assert!(!is_valid_canadian_postal("M5V 2H1"));
    }

    #[test]
    fn test_postal_code_validation_too_short() {
        assert!(!is_valid_canadian_postal("M5V2H"));
    }

    #[test]
    fn test_postal_code_validation_too_long() {
        assert!(!is_valid_canadian_postal("M5V2H1X"));
    }

    #[test]
    fn test_postal_code_validation_invalid_pattern() {
        assert!(!is_valid_canadian_postal("12A4B6")); // Starts with digit
        assert!(!is_valid_canadian_postal("A5A5A5")); // All consonants
        assert!(!is_valid_canadian_postal("M1V1H1")); // Valid pattern actually
    }

    #[test]
    fn test_postal_code_validation_empty() {
        assert!(!is_valid_canadian_postal(""));
    }

    #[test]
    fn test_postal_code_validation_special_chars() {
        assert!(!is_valid_canadian_postal("M5V-2H1"));
        assert!(!is_valid_canadian_postal("M5V/2H1"));
    }

    #[test]
    fn test_record_id_with_hyphens() {
        assert!(validate_surreal_record_id("users:abc-123-def").is_ok());
    }

    #[test]
    fn test_record_id_with_dots() {
        assert!(validate_surreal_record_id("products:v1.2.3").is_ok());
    }

    #[test]
    fn test_record_id_with_underscores() {
        assert!(validate_surreal_record_id("orders:ord_2024_001").is_ok());
    }

    #[test]
    fn test_record_id_mixed_separators() {
        assert!(validate_surreal_record_id("items:item-v1.2_test").is_ok());
    }

    #[test]
    fn test_record_id_max_length() {
        let long_id = format!("users:{}", "a".repeat(505));
        assert!(validate_surreal_record_id(&long_id).is_err());
    }

    #[test]
    fn test_record_id_double_colon() {
        assert!(validate_surreal_record_id("users::invalid").is_err());
    }

    #[test]
    fn test_record_id_special_characters() {
        assert!(validate_surreal_record_id("users:id;DROP").is_err());
        assert!(validate_surreal_record_id("users:id'quote").is_err());
    }

    #[test]
    fn test_identifier_underscore_prefix() {
        assert!(validate_identifier("_internal_table").is_ok());
        assert!(validate_identifier("__dunder__").is_ok());
    }

    #[test]
    fn test_identifier_numbers() {
        assert!(validate_identifier("table123").is_ok());
        assert!(validate_identifier("t123t").is_ok());
    }

    #[test]
    fn test_identifier_uppercase() {
        assert!(validate_identifier("USERS").is_ok());
        assert!(validate_identifier("UserProfile").is_ok());
    }

    #[test]
    fn test_escape_surreal_string_quotes() {
        assert_eq!(
            escape_surreal_string("It's"),
            "It\\'s"
        );
    }

    #[test]
    fn test_escape_surreal_string_backslash() {
        assert_eq!(
            escape_surreal_string("path\\to\\file"),
            "path\\\\to\\\\file"
        );
    }

    #[test]
    fn test_escape_surreal_string_mixed() {
        assert_eq!(
            escape_surreal_string("O'Neil's\\path"),
            "O\\'Neil\\'s\\\\path"
        );
    }

    #[test]
    fn test_escape_surreal_string_sql_injection_attempt() {
        let input = "'; DROP TABLE users; --";
        let escaped = escape_surreal_string(input);
        assert!(escaped.contains("\\'"));
    }

    #[test]
    fn test_document_id_numeric() {
        assert!(validate_document_id("123456").is_ok());
    }

    #[test]
    fn test_document_id_alphanumeric() {
        assert!(validate_document_id("abc123xyz789").is_ok());
    }

    #[test]
    fn test_document_id_with_all_allowed_chars() {
        assert!(validate_document_id("a_b-c.d123").is_ok());
    }

    #[test]
    fn test_document_id_empty() {
        assert!(validate_document_id("").is_err());
    }

    #[test]
    fn test_document_id_too_long() {
        let long_id = "a".repeat(513);
        assert!(validate_document_id(&long_id).is_err());
    }

    #[test]
    fn test_document_id_invalid_space() {
        assert!(validate_document_id("abc 123").is_err());
    }

    #[test]
    fn test_document_id_invalid_colon() {
        assert!(validate_document_id("abc:123").is_err());
    }

    #[test]
    fn test_document_id_hash_like() {
        assert!(validate_document_id("abc#123").is_err());
    }
}

#[cfg(test)]
mod phone_tests {
    use super::*;

    fn is_valid_e164_phone(phone: &str) -> bool {
        // E.164 format: +1-15 digits
        phone.starts_with('+')
            && phone.len() >= 10
            && phone.len() <= 15
            && phone.chars().skip(1).all(|c| c.is_ascii_digit())
    }

    #[test]
    fn test_e164_phone_valid_canada() {
        assert!(is_valid_e164_phone("+12345678901"));
        assert!(is_valid_e164_phone("+14165551234"));
    }

    #[test]
    fn test_e164_phone_valid_us() {
        assert!(is_valid_e164_phone("+15551234567"));
    }

    #[test]
    fn test_e164_phone_valid_international() {
        assert!(is_valid_e164_phone("+441632960000")); // UK
        assert!(is_valid_e164_phone("+33123456789")); // France
    }

    #[test]
    fn test_e164_phone_invalid_no_plus() {
        assert!(!is_valid_e164_phone("12345678901"));
    }

    #[test]
    fn test_e164_phone_invalid_short() {
        assert!(!is_valid_e164_phone("+123"));
    }

    #[test]
    fn test_e164_phone_invalid_long() {
        assert!(!is_valid_e164_phone("+1234567890123456")); // 16 digits
    }

    #[test]
    fn test_e164_phone_invalid_letters() {
        assert!(!is_valid_e164_phone("+1234567890a"));
    }

    #[test]
    fn test_e164_phone_invalid_dashes() {
        assert!(!is_valid_e164_phone("+1-234-567-8901"));
    }

    #[test]
    fn test_e164_phone_invalid_spaces() {
        assert!(!is_valid_e164_phone("+1 234 567 8901"));
    }

    #[test]
    fn test_e164_phone_empty() {
        assert!(!is_valid_e164_phone(""));
    }
}
