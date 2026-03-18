//! Input validation helpers for handler endpoints.

use ob_core::Error;

/// Validate that a string is non-empty and within max length.
pub fn validate_string(field: &str, value: &str, max_len: usize) -> ob_core::Result<()> {
    if value.is_empty() {
        return Err(Error::Validation(format!("{field} cannot be empty")));
    }
    if value.len() > max_len {
        return Err(Error::Validation(format!(
            "{field} exceeds max length of {max_len}"
        )));
    }
    Ok(())
}

/// Validate that an amount in cents is positive and within bounds.
pub fn validate_amount_cents(field: &str, cents: i64) -> ob_core::Result<()> {
    if cents < 0 {
        return Err(Error::Validation(format!("{field} cannot be negative")));
    }
    if cents > 100_000_000 {
        // $1,000,000 max
        return Err(Error::Validation(format!("{field} exceeds maximum")));
    }
    Ok(())
}

/// Validate an email address (basic check).
pub fn validate_email(email: &str) -> ob_core::Result<()> {
    if !email.contains('@') || !email.contains('.') {
        return Err(Error::Validation("Invalid email address".into()));
    }
    if email.len() > 254 {
        return Err(Error::Validation("Email too long".into()));
    }
    Ok(())
}

/// Validate a UUID string.
pub fn validate_uid(field: &str, value: &str) -> ob_core::Result<()> {
    if value.is_empty() {
        return Err(Error::Validation(format!("{field} cannot be empty")));
    }
    if value.len() > 128 {
        return Err(Error::Validation(format!("{field} is too long")));
    }
    Ok(())
}

/// Sanitize HTML to prevent XSS (strips all tags).
pub fn sanitize_html(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result
}

/// Redact contact information (phone numbers, emails) from chat messages.
pub fn redact_contact_info(text: &str) -> String {
    let mut result = text.to_string();
    // Redact phone numbers (10+ digits with optional separators)
    let phone_pattern = regex_lite::Regex::new(r"\+?\d[\d\s\-().]{8,}\d").unwrap();
    result = phone_pattern.replace_all(&result, "[REDACTED]").to_string();
    // Redact emails
    let email_pattern =
        regex_lite::Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}").unwrap();
    result = email_pattern.replace_all(&result, "[REDACTED]").to_string();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_string_ok() {
        assert!(validate_string("name", "hello", 100).is_ok());
    }

    #[test]
    fn test_validate_string_empty() {
        assert!(validate_string("name", "", 100).is_err());
    }

    #[test]
    fn test_validate_string_too_long() {
        let long = "a".repeat(200);
        assert!(validate_string("name", &long, 100).is_err());
    }

    #[test]
    fn test_validate_amount_cents() {
        assert!(validate_amount_cents("price", 1000).is_ok());
        assert!(validate_amount_cents("price", -1).is_err());
        assert!(validate_amount_cents("price", 200_000_000).is_err());
    }

    #[test]
    fn test_sanitize_html() {
        assert_eq!(
            sanitize_html("<script>alert('xss')</script>hello"),
            "alert('xss')hello"
        );
        assert_eq!(sanitize_html("no tags here"), "no tags here");
    }

    #[test]
    fn test_redact_contact_info() {
        let text = "Call me at 416-555-1234 or email john@example.com";
        let redacted = redact_contact_info(text);
        assert!(!redacted.contains("416-555-1234"));
        assert!(!redacted.contains("john@example.com"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_validate_email_ok() {
        assert!(validate_email("test@example.com").is_ok());
    }

    #[test]
    fn test_validate_email_invalid_no_at() {
        assert!(validate_email("testexample.com").is_err());
    }

    #[test]
    fn test_validate_email_invalid_no_dot() {
        assert!(validate_email("test@examplecom").is_err());
    }

    #[test]
    fn test_validate_email_too_long() {
        let long_email = format!("{}@example.com", "a".repeat(250));
        assert!(validate_email(&long_email).is_err());
    }

    #[test]
    fn test_validate_uid_ok() {
        assert!(validate_uid("user_id", "user_12345").is_ok());
    }

    #[test]
    fn test_validate_uid_empty() {
        assert!(validate_uid("user_id", "").is_err());
    }

    #[test]
    fn test_validate_uid_too_long() {
        let long_uid = "u".repeat(150);
        assert!(validate_uid("user_id", &long_uid).is_err());
    }
}
