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
            "_starts_with" => value
                .as_str()
                .map(|s| format!("string::startsWith({field}, '{s}')")),
            _ => {
                tracing::warn!("Unknown filter operator: {op}");
                None
            }
        }
    }

    fn value_to_surreal(value: &Value) -> String {
        match value {
            Value::String(s) => format!("'{s}'"),
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
        let mut query = format!("SELECT * FROM {collection}");

        if let Some(f) = filters {
            let where_clause = Self::filters_to_where(f);
            if !where_clause.is_empty() {
                query.push(' ');
                query.push_str(&where_clause);
            }
        }

        if let Some(field) = order_by {
            let dir = if descending { "DESC" } else { "ASC" };
            query.push_str(&format!(" ORDER BY {field} {dir}"));
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
        let query = QueryTranslator::build_select(
            "events",
            None,
            Some("timestamp"),
            false,
            None,
            None,
        );
        assert_eq!(query, "SELECT * FROM events ORDER BY timestamp ASC");
    }

    #[test]
    fn test_build_select_with_empty_where_clause() {
        // Filters that produce no conditions (e.g. unknown operator) should not add WHERE
        let filters = json!({ "field": { "_bogus": 1 } });
        let query = QueryTranslator::build_select(
            "items",
            Some(&filters),
            None,
            false,
            None,
            None,
        );
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
}
