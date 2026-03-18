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
