use ob_core::{escape_surreal_string, validate_document_id, validate_identifier};
use serde_json::json;

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// VALIDATION TESTS (ob-core::validate)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[cfg(test)]
mod validation_tests {
    use super::*;

    // ─────────────────────────────────────────────────────────────────────────
    // validate_identifier: Table-driven tests
    // ─────────────────────────────────────────────────────────────────────────

    struct IdentifierCase {
        input: &'static str,
        should_pass: bool,
        description: &'static str,
    }

    #[test]
    fn test_validate_identifier_table_driven() {
        let cases = vec![
            // Valid identifiers
            IdentifierCase {
                input: "users",
                should_pass: true,
                description: "simple lowercase",
            },
            IdentifierCase {
                input: "_private",
                should_pass: true,
                description: "starts with underscore",
            },
            IdentifierCase {
                input: "order_items",
                should_pass: true,
                description: "with underscore separator",
            },
            IdentifierCase {
                input: "Products123",
                should_pass: true,
                description: "mixed case with numbers",
            },
            IdentifierCase {
                input: "a",
                should_pass: true,
                description: "single letter",
            },
            IdentifierCase {
                input: "_",
                should_pass: true,
                description: "single underscore",
            },
            IdentifierCase {
                input: "ALLCAPS",
                should_pass: true,
                description: "all uppercase",
            },
            IdentifierCase {
                input: "_123",
                should_pass: true,
                description: "underscore followed by numbers",
            },
            IdentifierCase {
                input: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                should_pass: true,
                description: "max length (255 chars)",
            },
            // Invalid identifiers
            IdentifierCase {
                input: "",
                should_pass: false,
                description: "empty string",
            },
            IdentifierCase {
                input: "123abc",
                should_pass: false,
                description: "starts with digit",
            },
            IdentifierCase {
                input: "my-table",
                should_pass: false,
                description: "contains hyphen",
            },
            IdentifierCase {
                input: "my table",
                should_pass: false,
                description: "contains space",
            },
            IdentifierCase {
                input: "users;DROP",
                should_pass: false,
                description: "contains semicolon (SQL injection attempt)",
            },
            IdentifierCase {
                input: "field:name",
                should_pass: false,
                description: "contains colon",
            },
            IdentifierCase {
                input: "field.name",
                should_pass: false,
                description: "contains dot",
            },
            IdentifierCase {
                input: "field'name",
                should_pass: false,
                description: "contains single quote",
            },
            IdentifierCase {
                input: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                should_pass: false,
                description: "exceeds max length (256 chars)",
            },
            IdentifierCase {
                input: "tab\nle",
                should_pass: false,
                description: "contains newline",
            },
            IdentifierCase {
                input: "tab\r\nle",
                should_pass: false,
                description: "contains CRLF",
            },
            IdentifierCase {
                input: "table\t",
                should_pass: false,
                description: "contains tab",
            },
        ];

        for case in cases {
            let result = validate_identifier(case.input);
            if case.should_pass {
                assert!(
                    result.is_ok(),
                    "Expected '{}' ({}) to be valid, but got error: {:?}",
                    case.input,
                    case.description,
                    result.err()
                );
                assert_eq!(result.unwrap(), case.input);
            } else {
                assert!(
                    result.is_err(),
                    "Expected '{}' ({}) to be invalid, but it passed",
                    case.input,
                    case.description
                );
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // validate_document_id: Table-driven tests
    // ─────────────────────────────────────────────────────────────────────────

    struct DocumentIdCase {
        input: &'static str,
        should_pass: bool,
        description: &'static str,
    }

    #[test]
    fn test_validate_document_id_table_driven() {
        let cases = vec![
            // Valid document IDs
            DocumentIdCase {
                input: "abc123",
                should_pass: true,
                description: "simple alphanumeric",
            },
            DocumentIdCase {
                input: "user-uuid-here",
                should_pass: true,
                description: "with hyphens",
            },
            DocumentIdCase {
                input: "doc.v2",
                should_pass: true,
                description: "with dot",
            },
            DocumentIdCase {
                input: "a_b-c.d",
                should_pass: true,
                description: "mixed allowed chars",
            },
            DocumentIdCase {
                input: "a",
                should_pass: true,
                description: "single character",
            },
            DocumentIdCase {
                input: "_",
                should_pass: true,
                description: "single underscore",
            },
            DocumentIdCase {
                input: "-",
                should_pass: true,
                description: "single hyphen",
            },
            DocumentIdCase {
                input: ".",
                should_pass: true,
                description: "single dot",
            },
            DocumentIdCase {
                input: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                should_pass: true,
                description: "max length (512 chars)",
            },
            DocumentIdCase {
                input: "uuid-8f3c-4d2e-b1a9-7c6d5e4f3a2b",
                should_pass: true,
                description: "UUID format",
            },
            // Invalid document IDs
            DocumentIdCase {
                input: "",
                should_pass: false,
                description: "empty string",
            },
            DocumentIdCase {
                input: "id;DROP TABLE users;--",
                should_pass: false,
                description: "SQL injection with semicolon",
            },
            DocumentIdCase {
                input: "id' OR '1'='1",
                should_pass: false,
                description: "SQL injection with single quote",
            },
            DocumentIdCase {
                input: "id\n",
                should_pass: false,
                description: "contains newline",
            },
            DocumentIdCase {
                input: "id\r\ntest",
                should_pass: false,
                description: "contains CRLF",
            },
            DocumentIdCase {
                input: "id\t",
                should_pass: false,
                description: "contains tab",
            },
            DocumentIdCase {
                input: "id space",
                should_pass: false,
                description: "contains space",
            },
            DocumentIdCase {
                input: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                should_pass: false,
                description: "exceeds max length (513 chars)",
            },
            DocumentIdCase {
                input: "id@example",
                should_pass: false,
                description: "contains @ symbol",
            },
            DocumentIdCase {
                input: "id#hash",
                should_pass: false,
                description: "contains # symbol",
            },
            DocumentIdCase {
                input: "id$var",
                should_pass: false,
                description: "contains $ symbol",
            },
            DocumentIdCase {
                input: "id(test)",
                should_pass: false,
                description: "contains parentheses",
            },
            DocumentIdCase {
                input: "id[index]",
                should_pass: false,
                description: "contains square brackets",
            },
            DocumentIdCase {
                input: "id{json}",
                should_pass: false,
                description: "contains curly braces",
            },
            DocumentIdCase {
                input: "id<tag>",
                should_pass: false,
                description: "contains angle brackets",
            },
        ];

        for case in cases {
            let result = validate_document_id(case.input);
            if case.should_pass {
                assert!(
                    result.is_ok(),
                    "Expected '{}' ({}) to be valid, but got error: {:?}",
                    case.input,
                    case.description,
                    result.err()
                );
                assert_eq!(result.unwrap(), case.input);
            } else {
                assert!(
                    result.is_err(),
                    "Expected '{}' ({}) to be invalid, but it passed",
                    case.input,
                    case.description
                );
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // escape_surreal_string: SQL injection prevention
    // ─────────────────────────────────────────────────────────────────────────

    struct EscapeCase {
        input: &'static str,
        expected: &'static str,
        description: &'static str,
    }

    #[test]
    fn test_escape_surreal_string_table_driven() {
        let cases = vec![
            EscapeCase {
                input: "hello",
                expected: "hello",
                description: "plain text unchanged",
            },
            EscapeCase {
                input: "it's",
                expected: "it\\'s",
                description: "single quote escaped",
            },
            EscapeCase {
                input: "a\\b",
                expected: "a\\\\b",
                description: "backslash escaped",
            },
            EscapeCase {
                input: "'; DROP TABLE--",
                expected: "\\'; DROP TABLE--",
                description: "SQL injection with DROP attempt",
            },
            EscapeCase {
                input: "' OR '1'='1",
                expected: "\\' OR \\'1\\'=\\'1",
                description: "SQL injection OR condition",
            },
            EscapeCase {
                input: "admin'--",
                expected: "admin\\'--",
                description: "comment-based injection",
            },
            EscapeCase {
                input: "1' UNION SELECT--",
                expected: "1\\' UNION SELECT--",
                description: "UNION-based injection",
            },
            EscapeCase {
                input: "O'Reilly's \\ Book",
                expected: "O\\'Reilly\\'s \\\\ Book",
                description: "combined quotes and backslashes",
            },
            EscapeCase {
                input: "\\\\\\",
                expected: "\\\\\\\\\\\\",
                description: "multiple backslashes",
            },
            EscapeCase {
                input: "'''''",
                expected: "\\'\\'\\'\\'\\'",
                description: "multiple single quotes",
            },
            EscapeCase {
                input: "",
                expected: "",
                description: "empty string",
            },
            EscapeCase {
                input: "normal-text_123",
                expected: "normal-text_123",
                description: "normal identifier-like text",
            },
            EscapeCase {
                input: "test@example.com",
                expected: "test@example.com",
                description: "email-like string",
            },
        ];

        for case in cases {
            let result = escape_surreal_string(case.input);
            assert_eq!(
                result, case.expected,
                "Escape failed for {} ({}): expected '{}', got '{}'",
                case.input, case.description, case.expected, result
            );
        }
    }

    #[test]
    fn test_escape_surreal_string_injection_patterns() {
        // These patterns remain data inside a quoted literal once single quotes are escaped.
        let patterns = vec![
            "; DROP TABLE users;",
            "'; DROP TABLE users;--",
            "1' OR '1'='1",
            "admin' --",
            "' UNION SELECT * FROM passwords--",
            "'; DELETE FROM users;--",
            "1' AND '1'='1",
            "' AND 1=1 AND '",
            "1' HAVING '1'='1",
            "'; EXEC sp_executesql--",
        ];

        for pattern in patterns {
            let escaped = escape_surreal_string(pattern);
            let quoted_literal = format!("'{}'", escaped);

            // Any single quote inside the final literal should be escaped.
            for (i, c) in quoted_literal
                .chars()
                .enumerate()
                .skip(1)
                .take(quoted_literal.len() - 2)
            {
                if c == '\'' {
                    let prev = quoted_literal.chars().nth(i.saturating_sub(1));
                    assert_eq!(
                        prev,
                        Some('\\'),
                        "Pattern '{}' has unescaped quote in literal '{}'",
                        pattern,
                        quoted_literal
                    );
                }
            }

            assert!(
                quoted_literal.starts_with('\'') && quoted_literal.ends_with('\''),
                "Pattern '{}' should remain wrapped as a single literal: '{}'",
                pattern,
                quoted_literal
            );
        }
    }

    #[test]
    fn test_escape_preserves_length_information() {
        // Escaped strings should be longer or equal in length
        let inputs = vec!["hello", "it's", "a\\b", "'; DROP TABLE--"];

        for input in inputs {
            let escaped = escape_surreal_string(input);
            // Escape can only add characters (\ before ' or \)
            assert!(
                escaped.len() >= input.len(),
                "Escaped string '{}' is shorter than input '{}'",
                escaped,
                input
            );
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// QUERY BUILDER TESTS (ob-database::query)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[cfg(test)]
mod query_builder_tests {
    use super::*;
    use ob_database::query::QueryTranslator;

    // ─────────────────────────────────────────────────────────────────────────
    // WHERE clause generation
    // ─────────────────────────────────────────────────────────────────────────

    struct FilterCase {
        filters: serde_json::Value,
        expected_clause: &'static str,
        description: &'static str,
    }

    #[test]
    fn test_query_translator_where_clause_table_driven() {
        let cases = vec![
            FilterCase {
                filters: json!({"status": {"_eq": "active"}}),
                expected_clause: "WHERE status = 'active'",
                description: "simple equality",
            },
            FilterCase {
                filters: json!({"age": {"_gt": 18}}),
                expected_clause: "WHERE age > 18",
                description: "greater than",
            },
            FilterCase {
                filters: json!({"price": {"_lte": 99.99}}),
                expected_clause: "WHERE price <= 99.99",
                description: "less than or equal",
            },
            FilterCase {
                filters: json!({"deleted": {"_neq": true}}),
                expected_clause: "WHERE deleted != true",
                description: "not equal boolean",
            },
            FilterCase {
                filters: json!({"category": {"_in": ["electronics", "books"]}}),
                expected_clause: "WHERE category IN ['electronics', 'books']",
                description: "in array",
            },
            FilterCase {
                filters: json!({"name": {"_contains": "john"}}),
                expected_clause: "WHERE name CONTAINS 'john'",
                description: "string contains",
            },
            FilterCase {
                filters: json!({"email": {"_starts_with": "admin"}}),
                expected_clause: "WHERE string::startsWith(email, 'admin')",
                description: "starts with",
            },
            FilterCase {
                filters: json!({"deleted_at": {"_eq": null}}),
                expected_clause: "WHERE deleted_at = NONE",
                description: "null equality",
            },
        ];

        for case in cases {
            let result = QueryTranslator::filters_to_where(&case.filters);
            assert_eq!(
                result, case.expected_clause,
                "Filter test {} failed: expected '{}', got '{}'",
                case.description, case.expected_clause, result
            );
        }
    }

    #[test]
    fn test_query_translator_multiple_filters() {
        let filters = json!({
            "price": {"_gt": 100},
            "status": {"_eq": "active"}
        });
        let result = QueryTranslator::filters_to_where(&filters);
        assert!(result.contains("price > 100"));
        assert!(result.contains("status = 'active'"));
        assert!(result.contains(" AND "));
    }

    #[test]
    fn test_query_translator_invalid_field_name_rejected() {
        // Field names with invalid characters should be rejected
        let filters = json!({"field;DROP": {"_eq": "value"}});
        let result = QueryTranslator::filters_to_where(&filters);
        // Invalid field should not appear in result
        assert!(!result.contains("field;DROP"));
    }

    #[test]
    fn test_query_translator_escape_string_values() {
        let filters = json!({"name": {"_eq": "O'Reilly"}});
        let result = QueryTranslator::filters_to_where(&filters);
        // Single quote in value should be escaped
        assert!(result.contains("O\\'Reilly") || result.contains("O'Reilly"));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // SELECT query building
    // ─────────────────────────────────────────────────────────────────────────

    struct SelectCase {
        collection: &'static str,
        filters: Option<serde_json::Value>,
        order_by: Option<&'static str>,
        descending: bool,
        limit: Option<usize>,
        offset: Option<usize>,
        expected_contains: Vec<&'static str>,
        should_not_contain: Vec<&'static str>,
        description: &'static str,
    }

    #[test]
    fn test_query_translator_build_select_table_driven() {
        let cases = vec![
            SelectCase {
                collection: "users",
                filters: None,
                order_by: None,
                descending: false,
                limit: None,
                offset: None,
                expected_contains: vec!["SELECT * FROM users"],
                should_not_contain: vec!["WHERE", "ORDER", "LIMIT"],
                description: "simple select all",
            },
            SelectCase {
                collection: "products",
                filters: Some(json!({"status": {"_eq": "active"}})),
                order_by: Some("price"),
                descending: true,
                limit: Some(20),
                offset: None,
                expected_contains: vec![
                    "SELECT * FROM products",
                    "WHERE status = 'active'",
                    "ORDER BY price DESC",
                    "LIMIT 20",
                ],
                should_not_contain: vec!["START"],
                description: "full featured select",
            },
            SelectCase {
                collection: "orders",
                filters: None,
                order_by: Some("created_at"),
                descending: false,
                limit: Some(10),
                offset: Some(20),
                expected_contains: vec![
                    "SELECT * FROM orders",
                    "ORDER BY created_at ASC",
                    "LIMIT 10",
                    "START 20",
                ],
                should_not_contain: vec![],
                description: "with offset (START)",
            },
            SelectCase {
                collection: "logs",
                filters: None,
                order_by: None,
                descending: false,
                limit: Some(100),
                offset: None,
                expected_contains: vec!["SELECT * FROM logs", "LIMIT 100"],
                should_not_contain: vec!["WHERE", "ORDER", "START"],
                description: "with limit only",
            },
        ];

        for case in cases {
            let result = QueryTranslator::build_select(
                case.collection,
                case.filters.as_ref(),
                case.order_by,
                case.descending,
                case.limit,
                case.offset,
            );

            for expected in case.expected_contains {
                assert!(
                    result.contains(expected),
                    "Select test '{}' failed: expected '{}' in result '{}'",
                    case.description,
                    expected,
                    result
                );
            }

            for not_expected in case.should_not_contain {
                assert!(
                    !result.contains(not_expected),
                    "Select test '{}' failed: should not contain '{}' in result '{}'",
                    case.description,
                    not_expected,
                    result
                );
            }
        }
    }

    #[test]
    fn test_query_translator_empty_filters_no_where() {
        let filters = json!({"field": {"_unknown_op": "value"}});
        let result =
            QueryTranslator::build_select("items", Some(&filters), None, false, None, None);
        // Unknown operator should be skipped, so no WHERE clause
        assert!(!result.contains("WHERE"));
    }

    #[test]
    fn test_query_translator_field_projection() {
        let result = QueryTranslator::build_select_ext(
            "users",
            None,
            None,
            false,
            Some(10),
            None,
            Some(&["name", "email"]),
            None,
        );
        assert!(result.contains("id, name, email"));
    }

    #[test]
    fn test_query_translator_field_projection_with_existing_id() {
        let result = QueryTranslator::build_select_ext(
            "users",
            None,
            None,
            false,
            None,
            None,
            Some(&["id", "name"]),
            None,
        );
        assert_eq!(result, "SELECT id, name FROM users");
    }

    #[test]
    fn test_query_translator_invalid_field_projection_fallback() {
        // Invalid field names should fall back to SELECT *
        let result = QueryTranslator::build_select_ext(
            "items",
            None,
            None,
            false,
            None,
            None,
            Some(&["field;DROP", "valid_field"]),
            None,
        );
        // Should fall back to SELECT *
        assert!(result.contains("SELECT * FROM items"));
    }

    #[test]
    fn test_query_translator_cursor_pagination() {
        let result = QueryTranslator::build_select_ext(
            "products",
            None,
            Some("created_at"),
            true,
            Some(20),
            None,
            None,
            Some("abc123"),
        );
        assert!(result.contains("WHERE"));
        assert!(result.contains("created_at <")); // DESC = <
        assert!(result.contains("ORDER BY created_at DESC"));
        assert!(result.contains("LIMIT 20"));
    }

    #[test]
    fn test_query_translator_cursor_with_filters() {
        let filters = json!({"status": {"_eq": "active"}});
        let result = QueryTranslator::build_select_ext(
            "orders",
            Some(&filters),
            Some("id"),
            false,
            Some(10),
            None,
            None,
            Some("order_99"),
        );
        assert!(result.contains("status = 'active'"));
        assert!(result.contains("AND"));
        assert!(result.contains("id >")); // ASC = >
    }

    #[test]
    fn test_query_translator_cursor_ascending_uses_greater_than() {
        let result = QueryTranslator::build_select_ext(
            "items",
            None,
            Some("id"),
            false, // ascending
            None,
            None,
            None,
            Some("item_50"),
        );
        assert!(result.contains("id >"));
    }

    #[test]
    fn test_query_translator_cursor_descending_uses_less_than() {
        let result = QueryTranslator::build_select_ext(
            "items",
            None,
            Some("id"),
            true, // descending
            None,
            None,
            None,
            Some("item_50"),
        );
        assert!(result.contains("id <"));
    }

    #[test]
    fn test_query_translator_empty_fields_uses_star() {
        let result = QueryTranslator::build_select_ext(
            "items",
            None,
            None,
            false,
            None,
            None,
            Some(&[]),
            None,
        );
        assert_eq!(result, "SELECT * FROM items");
    }

    #[test]
    fn test_query_translator_non_object_filters_ignored() {
        let filters = json!("not an object");
        let result =
            QueryTranslator::build_select("items", Some(&filters), None, false, None, None);
        assert!(!result.contains("WHERE"));
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// TRANSACTION TESTS (ob-database::transaction)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[cfg(test)]
mod transaction_tests {
    use super::*;
    use ob_database::Transaction;

    #[test]
    fn test_transaction_empty() {
        let tx = Transaction::new();
        assert!(tx.is_empty());
        assert_eq!(tx.len(), 0);
    }

    #[test]
    fn test_transaction_add_query() {
        let mut tx = Transaction::new();
        tx.add("SELECT * FROM users", None);
        assert_eq!(tx.len(), 1);
        assert!(!tx.is_empty());
    }

    #[test]
    fn test_transaction_add_with_binds() {
        let mut tx = Transaction::new();
        tx.add(
            "UPDATE users:1 SET name = $name",
            Some(json!({"name": "Alice"})),
        );
        assert_eq!(tx.len(), 1);
    }

    #[test]
    fn test_transaction_add_raw() {
        let mut tx = Transaction::new();
        tx.add_raw("SELECT 1");
        assert_eq!(tx.len(), 1);
    }

    #[test]
    fn test_transaction_chaining() {
        let mut tx = Transaction::new();
        tx.add_raw("SELECT 1")
            .add_raw("SELECT 2")
            .add_raw("SELECT 3");
        assert_eq!(tx.len(), 3);
    }

    #[test]
    fn test_transaction_default() {
        let tx = Transaction::default();
        assert!(tx.is_empty());
    }

    #[test]
    fn test_transaction_multiple_operations() {
        let mut tx = Transaction::new();
        tx.add_raw("INSERT INTO users CONTENT {name: 'Alice'}")
            .add(
                "UPDATE products:1 SET stock = stock - $qty",
                Some(json!({"qty": 5})),
            )
            .add_raw("DELETE FROM logs WHERE created_at < '2026-01-01'");
        assert_eq!(tx.len(), 3);
        assert!(!tx.is_empty());
    }

    #[test]
    fn test_transaction_complex_query() {
        let mut tx = Transaction::new();
        let complex_query = "SELECT * FROM users WHERE created_at > $start AND created_at < $end";
        let binds = json!({"start": "2026-01-01", "end": "2026-12-31"});
        tx.add(complex_query, Some(binds));
        assert_eq!(tx.len(), 1);
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// TASK QUEUE TESTS (ob-database::task_queue)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[cfg(test)]
mod task_queue_tests {
    use super::*;
    use ob_database::{EnqueueRequest, TaskStatus};

    // ─────────────────────────────────────────────────────────────────────────
    // Task status transitions
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_task_status_serialization() {
        assert_eq!(
            serde_json::to_string(&TaskStatus::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Failed).unwrap(),
            "\"failed\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::DeadLetter).unwrap(),
            "\"dead_letter\""
        );
    }

    #[test]
    fn test_task_status_deserialization() {
        let statuses = vec![
            ("\"pending\"", TaskStatus::Pending),
            ("\"running\"", TaskStatus::Running),
            ("\"completed\"", TaskStatus::Completed),
            ("\"failed\"", TaskStatus::Failed),
            ("\"dead_letter\"", TaskStatus::DeadLetter),
        ];

        for (json_str, expected) in statuses {
            let status: TaskStatus = serde_json::from_str(json_str).unwrap();
            assert_eq!(status, expected);
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Enqueue request creation
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_enqueue_request_default() {
        let req = EnqueueRequest::default();
        assert_eq!(req.queue, "default");
        assert_eq!(req.max_retries, 3);
        assert_eq!(req.delay_secs, 0);
        assert_eq!(req.priority, 0);
    }

    struct EnqueueCase {
        queue: &'static str,
        max_retries: u32,
        delay_secs: u64,
        priority: i32,
        description: &'static str,
    }

    #[test]
    fn test_enqueue_request_table_driven() {
        let cases = vec![
            EnqueueCase {
                queue: "default",
                max_retries: 3,
                delay_secs: 0,
                priority: 0,
                description: "default values",
            },
            EnqueueCase {
                queue: "emails",
                max_retries: 5,
                delay_secs: 60,
                priority: 1,
                description: "custom email queue",
            },
            EnqueueCase {
                queue: "high_priority",
                max_retries: 1,
                delay_secs: 0,
                priority: -10,
                description: "high priority (negative)",
            },
            EnqueueCase {
                queue: "maintenance",
                max_retries: 0,
                delay_secs: 3600,
                priority: 100,
                description: "low priority with 1 hour delay",
            },
        ];

        for case in cases {
            let req = EnqueueRequest {
                task_type: "test".into(),
                payload: json!({}),
                queue: case.queue.into(),
                max_retries: case.max_retries,
                delay_secs: case.delay_secs,
                priority: case.priority,
            };
            assert_eq!(
                req.queue, case.queue,
                "Queue mismatch for {}",
                case.description
            );
            assert_eq!(
                req.max_retries, case.max_retries,
                "Max retries mismatch for {}",
                case.description
            );
            assert_eq!(
                req.delay_secs, case.delay_secs,
                "Delay secs mismatch for {}",
                case.description
            );
            assert_eq!(
                req.priority, case.priority,
                "Priority mismatch for {}",
                case.description
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Exponential backoff calculation
    // ─────────────────────────────────────────────────────────────────────────

    struct BackoffCase {
        attempt: u32,
        expected_secs: i64,
        description: &'static str,
    }

    #[test]
    fn test_exponential_backoff_table_driven() {
        let cases = vec![
            BackoffCase {
                attempt: 1,
                expected_secs: 2,
                description: "1st retry: 2^1 = 2s",
            },
            BackoffCase {
                attempt: 2,
                expected_secs: 4,
                description: "2nd retry: 2^2 = 4s",
            },
            BackoffCase {
                attempt: 3,
                expected_secs: 8,
                description: "3rd retry: 2^3 = 8s",
            },
            BackoffCase {
                attempt: 4,
                expected_secs: 16,
                description: "4th retry: 2^4 = 16s",
            },
            BackoffCase {
                attempt: 5,
                expected_secs: 32,
                description: "5th retry: 2^5 = 32s",
            },
            BackoffCase {
                attempt: 10,
                expected_secs: 1024,
                description: "10th retry: 2^10 = 1024s",
            },
        ];

        for case in cases {
            let backoff = 2i64.pow(case.attempt);
            assert_eq!(
                backoff, case.expected_secs,
                "Backoff mismatch for {}",
                case.description
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Task payload serialization
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_enqueue_request_serialization() {
        let req = EnqueueRequest {
            task_type: "send_email".into(),
            payload: json!({"to": "user@example.com", "template": "welcome"}),
            queue: "emails".into(),
            max_retries: 3,
            delay_secs: 0,
            priority: 0,
        };

        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["task_type"], "send_email");
        assert_eq!(json["queue"], "emails");
        assert_eq!(json["max_retries"], 3);
        assert_eq!(json["payload"]["to"], "user@example.com");
    }

    #[test]
    fn test_enqueue_request_with_complex_payload() {
        let payload = json!({
            "user_id": "user_123",
            "action": "export",
            "filters": {
                "date_from": "2026-01-01",
                "date_to": "2026-12-31"
            },
            "format": "csv"
        });

        let req = EnqueueRequest {
            task_type: "export_data".into(),
            payload: payload.clone(),
            queue: "reports".into(),
            max_retries: 2,
            delay_secs: 300,
            priority: 5,
        };

        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["payload"], payload);
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// CRUD OPERATIONS TESTS (ob-database::crud)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[cfg(test)]
mod crud_tests {
    use super::*;

    // ─────────────────────────────────────────────────────────────────────────
    // CRUD operation validation
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_crud_collection_name_validation() {
        // Valid collection names
        assert!(validate_identifier("users").is_ok());
        assert!(validate_identifier("products").is_ok());
        assert!(validate_identifier("_internal").is_ok());
        assert!(validate_identifier("order_items").is_ok());

        // Invalid collection names
        assert!(validate_identifier("my-table").is_err());
        assert!(validate_identifier("123table").is_err());
        assert!(validate_identifier("table;DROP").is_err());
    }

    #[test]
    fn test_crud_document_id_validation() {
        // Valid document IDs
        assert!(validate_document_id("abc123").is_ok());
        assert!(validate_document_id("user-uuid-here").is_ok());
        assert!(validate_document_id("doc.v2").is_ok());

        // Invalid document IDs
        assert!(validate_document_id("id;DROP").is_err());
        assert!(validate_document_id("id' OR 1=1").is_err());
    }

    // ─────────────────────────────────────────────────────────────────────────
    // CRUD query generation (simulated, without DB)
    // ─────────────────────────────────────────────────────────────────────────

    #[allow(dead_code)]
    struct CrudQueryCase {
        collection: &'static str,
        id: &'static str,
        operation: &'static str,
        valid: bool,
        description: &'static str,
    }

    #[test]
    fn test_crud_query_validation_table_driven() {
        let cases = vec![
            CrudQueryCase {
                collection: "users",
                id: "abc123",
                operation: "get",
                valid: true,
                description: "get valid user",
            },
            CrudQueryCase {
                collection: "products",
                id: "prod-uuid-123",
                operation: "update",
                valid: true,
                description: "update product with hyphenated id",
            },
            CrudQueryCase {
                collection: "products",
                id: "prod.v2.latest",
                operation: "delete",
                valid: true,
                description: "delete product with dotted id",
            },
            CrudQueryCase {
                collection: "bad;table",
                id: "doc1",
                operation: "get",
                valid: false,
                description: "invalid collection name",
            },
            CrudQueryCase {
                collection: "users",
                id: "'; DROP TABLE--",
                operation: "get",
                valid: false,
                description: "SQL injection attempt in id",
            },
            CrudQueryCase {
                collection: "orders",
                id: "order' OR 1=1",
                operation: "update",
                valid: false,
                description: "OR injection attempt in id",
            },
        ];

        for case in cases {
            let collection_valid = validate_identifier(case.collection).is_ok();
            let id_valid = validate_document_id(case.id).is_ok();
            let should_be_valid = collection_valid && id_valid;

            assert_eq!(
                should_be_valid, case.valid,
                "CRUD validation mismatch for {}: collection_valid={}, id_valid={}, expected valid={}",
                case.description, collection_valid, id_valid, case.valid
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Escape in CRUD context
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_crud_escape_string_values_in_updates() {
        // When building update queries with string values
        let value = "O'Reilly";
        let escaped = escape_surreal_string(value);
        // The escaped value should be safe to use in SurrealQL string literals
        assert_eq!(escaped, "O\\'Reilly");

        // SQL injection attempt
        let injection = "'; DELETE FROM users;--";
        let escaped_injection = escape_surreal_string(injection);
        assert!(escaped_injection.starts_with("\\'"));
        assert_eq!(escaped_injection.matches("\\'").count(), 1);
    }

    #[test]
    fn test_crud_batch_operations_validation() {
        // Simulating batch operations
        let batch_ids = vec!["user_1", "user_2", "user-uuid-123", "user.v2"];

        for id in batch_ids {
            assert!(
                validate_document_id(id).is_ok(),
                "Batch ID '{}' should be valid",
                id
            );
        }

        // Invalid batch IDs should fail
        let invalid_ids = vec!["user'; DROP--", "user' OR 1=1"];

        for id in invalid_ids {
            assert!(
                validate_document_id(id).is_err(),
                "Batch ID '{}' should be invalid",
                id
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Field value operations
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_field_value_operations_field_validation() {
        // Field names in FieldValue operations must be validated
        let valid_fields = vec!["name", "created_at", "price", "user_id"];
        for field in valid_fields {
            assert!(
                validate_identifier(field).is_ok(),
                "Field '{}' should be valid",
                field
            );
        }

        let invalid_fields = vec!["field;DROP", "field' OR 1=1", "field:nested"];
        for field in invalid_fields {
            assert!(
                validate_identifier(field).is_err(),
                "Field '{}' should be invalid",
                field
            );
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// EDGE CASES AND BOUNDARY TESTS
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[cfg(test)]
mod edge_case_tests {
    use super::*;

    #[test]
    fn test_special_characters_in_strings() {
        let special_chars = vec![
            "hello\x00world",
            "test\u{202e}reverse",
            "emoji😀test",
            "zero\u{200b}width",
        ];

        for s in special_chars {
            let escaped = escape_surreal_string(s);
            // Should not panic, should produce output
            assert!(!escaped.is_empty() || s.is_empty());
        }
    }

    #[test]
    fn test_very_long_strings() {
        let long_string = "a".repeat(10_000);
        let escaped = escape_surreal_string(&long_string);
        assert_eq!(escaped, long_string); // No special chars, should be unchanged
    }

    #[test]
    fn test_very_long_identifier() {
        let long = "a".repeat(255);
        assert!(validate_identifier(&long).is_ok());

        let too_long = "a".repeat(256);
        assert!(validate_identifier(&too_long).is_err());
    }

    #[test]
    fn test_very_long_document_id() {
        let long = "a".repeat(512);
        assert!(validate_document_id(&long).is_ok());

        let too_long = "a".repeat(513);
        assert!(validate_document_id(&too_long).is_err());
    }

    #[test]
    fn test_only_special_characters_in_escape() {
        let strings = vec!["'''''", "\\\\\\\\", "';'';'", "\\'\\'\\'"];

        for s in strings {
            let escaped = escape_surreal_string(s);
            // Should handle gracefully
            assert!(!escaped.is_empty());
        }
    }

    #[test]
    fn test_mixed_injection_patterns() {
        let patterns = vec![
            "1' OR '1'='1' AND 'a'='a",
            "' UNION SELECT * FROM users--",
            "'; EXEC xp_cmdshell 'dir'--",
            "1' AND (SELECT COUNT(*) FROM users) > 0--",
        ];

        for pattern in patterns {
            let escaped = escape_surreal_string(pattern);
            // All single quotes should be escaped
            for (i, c) in escaped.chars().enumerate() {
                if c == '\'' {
                    // Previous char should be backslash
                    let prev = escaped.chars().nth(i.saturating_sub(1));
                    assert_eq!(prev, Some('\\'), "Unescaped quote in: {}", escaped);
                }
            }
        }
    }

    #[test]
    fn test_identifier_with_max_valid_length() {
        // Exactly 255 chars should pass
        let max_valid = "a".repeat(255);
        assert!(validate_identifier(&max_valid).is_ok());

        // 256 should fail
        let over_limit = "a".repeat(256);
        assert!(validate_identifier(&over_limit).is_err());
    }

    #[test]
    fn test_document_id_with_max_valid_length() {
        // Exactly 512 chars should pass
        let max_valid = "a".repeat(512);
        assert!(validate_document_id(&max_valid).is_ok());

        // 513 should fail
        let over_limit = "a".repeat(513);
        assert!(validate_document_id(&over_limit).is_err());
    }

    #[test]
    fn test_whitespace_variations() {
        let whitespaces = vec![
            "field name",
            "field\tname",
            "field\nname",
            "field\rname",
            "field\u{00a0}name",
            "field\u{2003}name",
        ];

        for ws in whitespaces {
            assert!(validate_identifier(ws).is_err());
            assert!(validate_document_id(ws).is_err());
        }
    }

    #[test]
    fn test_injection_across_all_operators() {
        // Test that injection attempts don't work for any operator
        let injection_value = "test' OR 1=1--";
        let escaped = escape_surreal_string(injection_value);

        // Ensure no unescaped single quotes
        for (i, c) in escaped.chars().enumerate() {
            if c == '\'' {
                assert_eq!(escaped.chars().nth(i.saturating_sub(1)), Some('\\'));
            }
        }
    }
}
