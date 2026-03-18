use ob_core::{escape_surreal_string, validate_identifier};
use serde_json::Value;

/// Translates GraphQL-style filter operators into SurrealQL WHERE clauses.
///
/// Supported operators:
/// - `_eq`: equals
/// - `_neq`: not equals
/// - `_gt`, `_gte`, `_lt`, `_lte`: comparison
/// - `_in`: value in array
/// - `_contains`: string contains
/// - `_starts_with`: string starts with
pub struct QueryTranslator;

impl QueryTranslator {
    /// Convert a filter map `{ field: { _op: value } }` to a SurrealQL WHERE clause.
    pub fn filters_to_where(filters: &Value) -> String {
        let Some(obj) = filters.as_object() else {
            return String::new();
        };

        let mut conditions = Vec::new();

        for (field, ops) in obj {
            if let Some(ops_obj) = ops.as_object() {
                for (op, value) in ops_obj {
                    if let Some(condition) = Self::translate_op(field, op, value) {
                        conditions.push(condition);
                    }
                }
            }
        }

        if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        }
    }

    fn translate_op(field: &str, op: &str, value: &Value) -> Option<String> {
        // Validate field name to prevent SurrealQL injection
        if validate_identifier(field).is_err() {
            tracing::warn!("Rejected invalid field name in filter: {field}");
            return None;
        }
        let val_str = Self::value_to_surreal(value);
        match op {
            "_eq" => Some(format!("{field} = {val_str}")),
            "_neq" => Some(format!("{field} != {val_str}")),
            "_gt" => Some(format!("{field} > {val_str}")),
            "_gte" => Some(format!("{field} >= {val_str}")),
            "_lt" => Some(format!("{field} < {val_str}")),
            "_lte" => Some(format!("{field} <= {val_str}")),
            "_in" => {
                if let Some(arr) = value.as_array() {
                    let items: Vec<String> = arr.iter().map(Self::value_to_surreal).collect();
                    Some(format!("{field} IN [{}]", items.join(", ")))
                } else {
                    None
                }
            }
            "_contains" => Some(format!("{field} CONTAINS {val_str}")),
            "_starts_with" => value.as_str().map(|s| {
                format!(
                    "string::startsWith({field}, '{}')",
                    escape_surreal_string(s)
                )
            }),
            _ => {
                tracing::warn!("Unknown filter operator: {op}");
                None
            }
        }
    }

    fn value_to_surreal(value: &Value) -> String {
        match value {
            Value::String(s) => format!("'{}'", escape_surreal_string(s)),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => "NONE".to_string(),
            _ => value.to_string(),
        }
    }

    /// Build a full SELECT query from collection, filters, order, and limit.
    pub fn build_select(
        collection: &str,
        filters: Option<&Value>,
        order_by: Option<&str>,
        descending: bool,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> String {
        Self::build_select_ext(
            collection, filters, order_by, descending, limit, offset, None, None,
        )
    }

    /// Extended SELECT builder with cursor pagination and field projection.
    ///
    /// - `fields`: optional list of field names to select (instead of `*`)
    /// - `start_after`: cursor-based pagination — document ID to start after
    #[allow(clippy::too_many_arguments)]
    pub fn build_select_ext(
        collection: &str,
        filters: Option<&Value>,
        order_by: Option<&str>,
        descending: bool,
        limit: Option<usize>,
        offset: Option<usize>,
        fields: Option<&[&str]>,
        start_after: Option<&str>,
    ) -> String {
        // Field projection
        let select_fields = match fields {
            Some(f) if !f.is_empty() => {
                // Validate field names to prevent injection
                for field in f {
                    if !field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                        return format!("SELECT * FROM {collection}");
                    }
                }
                let mut field_list = f.join(", ");
                // Always include id
                if !f.contains(&"id") {
                    field_list = format!("id, {field_list}");
                }
                field_list
            }
            _ => "*".to_string(),
        };

        let mut query = format!("SELECT {select_fields} FROM {collection}");

        // Combine filter WHERE clauses with cursor WHERE clause
        let mut where_parts = Vec::new();

        if let Some(f) = filters {
            let where_clause = Self::filters_to_where(f);
            if !where_clause.is_empty() {
                // Strip "WHERE " prefix since we'll add it ourselves
                where_parts.push(
                    where_clause
                        .strip_prefix("WHERE ")
                        .unwrap_or(&where_clause)
                        .to_string(),
                );
            }
        }

        // Cursor-based pagination: startAfter
        if let Some(cursor_id) = start_after {
            let safe_cursor = escape_surreal_string(cursor_id);
            let order_field = order_by.unwrap_or("id");
            // Validate the order field used in cursor comparison
            if validate_identifier(order_field).is_ok() {
                let op = if descending { "<" } else { ">" };
                where_parts.push(format!(
                    "{order_field} {op} type::thing('{collection}', '{safe_cursor}').{order_field}"
                ));
            } else {
                tracing::warn!("Rejected invalid cursor order field: {order_field}");
            }
        }

        if !where_parts.is_empty() {
            query.push_str(&format!(" WHERE {}", where_parts.join(" AND ")));
        }

        if let Some(field) = order_by {
            // Validate order_by field to prevent SurrealQL injection
            if validate_identifier(field).is_ok() {
                let dir = if descending { "DESC" } else { "ASC" };
                query.push_str(&format!(" ORDER BY {field} {dir}"));
            } else {
                tracing::warn!("Rejected invalid order_by field: {field}");
            }
        }

        if let Some(n) = limit {
            query.push_str(&format!(" LIMIT {n}"));
        }

        if let Some(n) = offset {
            query.push_str(&format!(" START {n}"));
        }

        query
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simple_eq_filter() {
        let filters = json!({
            "status": { "_eq": "active" }
        });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE status = 'active'");
    }

    #[test]
    fn test_multiple_filters() {
        let filters = json!({
            "price": { "_gt": 100 },
            "status": { "_eq": "active" }
        });
        let result = QueryTranslator::filters_to_where(&filters);
        assert!(result.contains("price > 100"));
        assert!(result.contains("status = 'active'"));
        assert!(result.contains(" AND "));
    }

    #[test]
    fn test_in_filter() {
        let filters = json!({
            "category": { "_in": ["electronics", "books"] }
        });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE category IN ['electronics', 'books']");
    }

    #[test]
    fn test_build_select() {
        let filters = json!({ "status": { "_eq": "active" } });
        let query = QueryTranslator::build_select(
            "products",
            Some(&filters),
            Some("created_at"),
            true,
            Some(20),
            None,
        );
        assert_eq!(
            query,
            "SELECT * FROM products WHERE status = 'active' ORDER BY created_at DESC LIMIT 20"
        );
    }

    #[test]
    fn test_empty_filters() {
        let query = QueryTranslator::build_select("users", None, None, false, Some(10), None);
        assert_eq!(query, "SELECT * FROM users LIMIT 10");
    }

    #[test]
    fn test_neq_filter() {
        let filters = json!({ "status": { "_neq": "deleted" } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE status != 'deleted'");
    }

    #[test]
    fn test_gte_filter() {
        let filters = json!({ "age": { "_gte": 18 } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE age >= 18");
    }

    #[test]
    fn test_lte_filter() {
        let filters = json!({ "price": { "_lte": 99.99 } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE price <= 99.99");
    }

    #[test]
    fn test_lt_filter() {
        let filters = json!({ "count": { "_lt": 5 } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE count < 5");
    }

    #[test]
    fn test_contains_filter() {
        let filters = json!({ "name": { "_contains": "test" } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE name CONTAINS 'test'");
    }

    #[test]
    fn test_starts_with_filter() {
        let filters = json!({ "email": { "_starts_with": "admin" } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE string::startsWith(email, 'admin')");
    }

    #[test]
    fn test_starts_with_non_string_returns_none() {
        // _starts_with with a non-string value should return None (no condition)
        let filters = json!({ "email": { "_starts_with": 123 } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "");
    }

    #[test]
    fn test_unknown_operator_skipped() {
        let filters = json!({ "field": { "_unknown_op": "value" } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "");
    }

    #[test]
    fn test_in_with_non_array_returns_none() {
        let filters = json!({ "id": { "_in": "not_an_array" } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "");
    }

    #[test]
    fn test_value_to_surreal_bool() {
        let filters = json!({ "active": { "_eq": true } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE active = true");
    }

    #[test]
    fn test_value_to_surreal_null() {
        let filters = json!({ "deleted_at": { "_eq": null } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE deleted_at = NONE");
    }

    #[test]
    fn test_value_to_surreal_non_primitive() {
        // An object value gets serialized via serde_json::to_string
        let filters = json!({ "meta": { "_eq": {"key": "val"} } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert!(result.contains("meta = "));
        assert!(result.contains("key"));
    }

    #[test]
    fn test_build_select_with_offset() {
        let query = QueryTranslator::build_select(
            "products",
            None,
            Some("created_at"),
            true,
            Some(10),
            Some(20),
        );
        assert_eq!(
            query,
            "SELECT * FROM products ORDER BY created_at DESC LIMIT 10 START 20"
        );
    }

    #[test]
    fn test_build_select_ascending_order() {
        let query =
            QueryTranslator::build_select("events", None, Some("timestamp"), false, None, None);
        assert_eq!(query, "SELECT * FROM events ORDER BY timestamp ASC");
    }

    #[test]
    fn test_build_select_with_empty_where_clause() {
        // Filters that produce no conditions (e.g. unknown operator) should not add WHERE
        let filters = json!({ "field": { "_bogus": 1 } });
        let query = QueryTranslator::build_select("items", Some(&filters), None, false, None, None);
        assert_eq!(query, "SELECT * FROM items");
    }

    #[test]
    fn test_filters_to_where_non_object_input() {
        // Non-object top-level value should return empty string
        let filters = json!("not an object");
        assert_eq!(QueryTranslator::filters_to_where(&filters), "");

        let filters = json!(42);
        assert_eq!(QueryTranslator::filters_to_where(&filters), "");

        let filters = json!([1, 2, 3]);
        assert_eq!(QueryTranslator::filters_to_where(&filters), "");

        let filters = json!(null);
        assert_eq!(QueryTranslator::filters_to_where(&filters), "");
    }

    #[test]
    fn test_build_select_no_options() {
        let query = QueryTranslator::build_select("logs", None, None, false, None, None);
        assert_eq!(query, "SELECT * FROM logs");
    }

    // ── Extended SELECT (cursor + projection) ───────────────────────

    #[test]
    fn test_build_select_ext_field_projection() {
        let query = QueryTranslator::build_select_ext(
            "users",
            None,
            None,
            false,
            Some(10),
            None,
            Some(&["name", "email"]),
            None,
        );
        assert_eq!(query, "SELECT id, name, email FROM users LIMIT 10");
    }

    #[test]
    fn test_build_select_ext_field_projection_with_id() {
        let query = QueryTranslator::build_select_ext(
            "users",
            None,
            None,
            false,
            None,
            None,
            Some(&["id", "name"]),
            None,
        );
        assert_eq!(query, "SELECT id, name FROM users");
    }

    #[test]
    fn test_build_select_ext_cursor_pagination() {
        let query = QueryTranslator::build_select_ext(
            "products",
            None,
            Some("created_at"),
            true,
            Some(20),
            None,
            None,
            Some("abc123"),
        );
        assert!(query.contains("WHERE"));
        assert!(query.contains("created_at <"));
        assert!(query.contains("ORDER BY created_at DESC"));
        assert!(query.contains("LIMIT 20"));
    }

    #[test]
    fn test_build_select_ext_cursor_with_filters() {
        let filters = json!({ "status": { "_eq": "active" } });
        let query = QueryTranslator::build_select_ext(
            "orders",
            Some(&filters),
            Some("id"),
            false,
            Some(10),
            None,
            None,
            Some("order_99"),
        );
        assert!(query.contains("status = 'active'"));
        assert!(query.contains("AND"));
        assert!(query.contains("id >"));
    }

    #[test]
    fn test_build_select_ext_empty_fields_uses_star() {
        let query = QueryTranslator::build_select_ext(
            "items",
            None,
            None,
            false,
            None,
            None,
            Some(&[]),
            None,
        );
        assert_eq!(query, "SELECT * FROM items");
    }

    #[test]
    fn test_build_select_ext_none_fields_uses_star() {
        let query =
            QueryTranslator::build_select_ext("items", None, None, false, None, None, None, None);
        assert_eq!(query, "SELECT * FROM items");
    }
}
