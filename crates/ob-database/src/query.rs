use ob_core::{escape_sql_string, validate_identifier};
use serde_json::Value;

/// Translates GraphQL-style filter operators into SQL WHERE clauses.
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
    fn is_probably_numeric_field(field: &str) -> bool {
        matches!(
            field,
            "age"
                | "price"
                | "priceCents"
                | "categoryId"
                | "rating"
                | "fraudScore"
                | "avgResponseTimeHours"
                | "avgShipDays"
                | "positiveRatePct"
                | "totalReviews"
        ) || field.ends_with("Cents")
            || field.ends_with("Count")
            || field.ends_with("Quantity")
            || field.ends_with("Pct")
            || field.ends_with("Hours")
            || field.ends_with("Days")
    }

    fn sql_field_expr(field: &str) -> Option<String> {
        if validate_identifier(field).is_err() {
            tracing::warn!("Rejected invalid field name in query builder: {field}");
            return None;
        }

        Some(match field {
            "id" => "id".to_string(),
            "createdAt" | "created_at" => "created_at".to_string(),
            "updatedAt" | "updated_at" => "updated_at".to_string(),
            _ if Self::is_probably_numeric_field(field) => {
                format!("NULLIF(data->>'{field}', '')::numeric")
            }
            _ => format!("data->>'{field}'"),
        })
    }

    fn sql_json_value_expr(field: &str) -> Option<String> {
        if validate_identifier(field).is_err() {
            tracing::warn!("Rejected invalid field name in query builder: {field}");
            return None;
        }

        Some(match field {
            "id" => "to_jsonb(id)".to_string(),
            "createdAt" | "created_at" => "to_jsonb(created_at)".to_string(),
            "updatedAt" | "updated_at" => "to_jsonb(updated_at)".to_string(),
            _ => format!("data->'{field}'"),
        })
    }

    fn sql_typed_field_expr(field: &str, value: &Value) -> Option<String> {
        if validate_identifier(field).is_err() {
            tracing::warn!("Rejected invalid field name in query builder: {field}");
            return None;
        }

        match value {
            Value::Number(_) => Some(match field {
                "createdAt" | "created_at" => "EXTRACT(EPOCH FROM created_at)".to_string(),
                "updatedAt" | "updated_at" => "EXTRACT(EPOCH FROM updated_at)".to_string(),
                "id" => "id".to_string(),
                _ => format!("NULLIF(data->>'{field}', '')::numeric"),
            }),
            Value::Bool(_) => Some(match field {
                "id" => "id".to_string(),
                _ => format!("NULLIF(data->>'{field}', '')::boolean"),
            }),
            _ => Self::sql_field_expr(field),
        }
    }

    /// Convert a filter map to a SQL WHERE clause.
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
        // Validate field name to prevent SQL injection
        if validate_identifier(field).is_err() {
            tracing::warn!("Rejected invalid field name in filter: {field}");
            return None;
        }
        match op {
            "_eq" => {
                let field_expr = Self::sql_typed_field_expr(field, value)?;
                let val_str = Self::format_sql_literal(value);
                if value.is_null() {
                    Some(format!("{field_expr} IS NULL"))
                } else {
                    Some(format!("{field_expr} = {val_str}"))
                }
            }
            "_neq" => {
                let field_expr = Self::sql_typed_field_expr(field, value)?;
                let val_str = Self::format_sql_literal(value);
                if value.is_null() {
                    Some(format!("{field_expr} IS NOT NULL"))
                } else {
                    Some(format!("{field_expr} != {val_str}"))
                }
            }
            "_gt" => {
                let field_expr = Self::sql_typed_field_expr(field, value)?;
                let val_str = Self::format_sql_literal(value);
                Some(format!("{field_expr} > {val_str}"))
            }
            "_gte" => {
                let field_expr = Self::sql_typed_field_expr(field, value)?;
                let val_str = Self::format_sql_literal(value);
                Some(format!("{field_expr} >= {val_str}"))
            }
            "_lt" => {
                let field_expr = Self::sql_typed_field_expr(field, value)?;
                let val_str = Self::format_sql_literal(value);
                Some(format!("{field_expr} < {val_str}"))
            }
            "_lte" => {
                let field_expr = Self::sql_typed_field_expr(field, value)?;
                let val_str = Self::format_sql_literal(value);
                Some(format!("{field_expr} <= {val_str}"))
            }
            "_in" => {
                if let Some(arr) = value.as_array() {
                    let expr_hint = arr.first().unwrap_or(&Value::Null);
                    let field_expr = Self::sql_typed_field_expr(field, expr_hint)?;
                    let items: Vec<String> = arr.iter().map(Self::format_sql_literal).collect();
                    Some(format!("{field_expr} IN ({})", items.join(", ")))
                } else {
                    None
                }
            }
            "_contains" => value.as_str().map(|s| {
                let escaped = escape_sql_string(s);
                let json_expr = Self::sql_json_value_expr(field)
                    .unwrap_or_else(|| format!("data->'{field}'"));
                let text_expr = Self::sql_field_expr(field)
                    .unwrap_or_else(|| format!("data->>'{field}'"));
                format!(
                    "((jsonb_typeof({json_expr}) = 'array' AND {json_expr} ? '{escaped}') OR COALESCE({text_expr}, '') ILIKE '%{escaped}%')"
                )
            }),
            "_starts_with" => {
                let field_expr = Self::sql_field_expr(field)?;
                value.as_str().map(|s| {
                    format!(
                        "COALESCE({field_expr}, '') ILIKE '{}%'",
                        escape_sql_string(s)
                    )
                })
            }
            _ => {
                tracing::warn!("Unknown filter operator: {op}");
                None
            }
        }
    }

    fn format_sql_literal(value: &Value) -> String {
        match value {
            Value::String(s) => format!("'{}'", escape_sql_string(s)),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => {
                if *b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            Value::Null => "NULL".to_string(),
            _ => format!("'{}'", escape_sql_string(&value.to_string())),
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
        if validate_identifier(collection).is_err() {
            tracing::warn!("Rejected invalid collection name in query builder: {collection}");
            return "SELECT * FROM _invalid_collection WHERE false".to_string();
        }

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
            let safe_cursor = escape_sql_string(cursor_id);
            let order_field = order_by.unwrap_or("id");
            if let Some(order_expr) = Self::sql_field_expr(order_field) {
                let op = if descending { "<" } else { ">" };
                if order_field == "id" {
                    where_parts.push(format!("id {op} '{safe_cursor}'"));
                } else {
                    where_parts.push(format!(
                        "{order_expr} {op} (SELECT {order_expr} FROM {collection} WHERE id = '{safe_cursor}')"
                    ));
                }
            } else {
                tracing::warn!("Rejected invalid cursor order field: {order_field}");
            }
        }

        if !where_parts.is_empty() {
            query.push_str(&format!(" WHERE {}", where_parts.join(" AND ")));
        }

        if let Some(field) = order_by {
            if let Some(order_expr) = Self::sql_field_expr(field) {
                let dir = if descending { "DESC" } else { "ASC" };
                query.push_str(&format!(" ORDER BY {order_expr} {dir}"));
            } else {
                tracing::warn!("Rejected invalid order_by field: {field}");
            }
        }

        if let Some(n) = limit {
            query.push_str(&format!(" LIMIT {n}"));
        }

        if let Some(n) = offset {
            query.push_str(&format!(" OFFSET {n}"));
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
        assert_eq!(result, "WHERE data->>'status' = 'active'");
    }

    #[test]
    fn test_multiple_filters() {
        let filters = json!({
            "price": { "_gt": 100 },
            "status": { "_eq": "active" }
        });
        let result = QueryTranslator::filters_to_where(&filters);
        assert!(result.contains("NULLIF(data->>'price', '')::numeric > 100"));
        assert!(result.contains("data->>'status' = 'active'"));
        assert!(result.contains(" AND "));
    }

    #[test]
    fn test_in_filter() {
        let filters = json!({
            "category": { "_in": ["electronics", "books"] }
        });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(
            result,
            "WHERE data->>'category' IN ('electronics', 'books')"
        );
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
            "SELECT * FROM products WHERE data->>'status' = 'active' ORDER BY created_at DESC LIMIT 20"
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
        assert_eq!(result, "WHERE data->>'status' != 'deleted'");
    }

    #[test]
    fn test_gte_filter() {
        let filters = json!({ "age": { "_gte": 18 } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE NULLIF(data->>'age', '')::numeric >= 18");
    }

    #[test]
    fn test_lte_filter() {
        let filters = json!({ "price": { "_lte": 99.99 } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE NULLIF(data->>'price', '')::numeric <= 99.99");
    }

    #[test]
    fn test_lt_filter() {
        let filters = json!({ "count": { "_lt": 5 } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE NULLIF(data->>'count', '')::numeric < 5");
    }

    #[test]
    fn test_contains_filter() {
        let filters = json!({ "name": { "_contains": "test" } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(
            result,
            "WHERE ((jsonb_typeof(data->'name') = 'array' AND data->'name' ? 'test') OR COALESCE(data->>'name', '') ILIKE '%test%')"
        );
    }

    #[test]
    fn test_starts_with_filter() {
        let filters = json!({ "email": { "_starts_with": "admin" } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE COALESCE(data->>'email', '') ILIKE 'admin%'");
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
    fn test_format_sql_literal_bool() {
        let filters = json!({ "active": { "_eq": true } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE NULLIF(data->>'active', '')::boolean = true");
    }

    #[test]
    fn test_bool_false_filter_does_not_match_missing_values() {
        let filters = json!({ "active": { "_eq": false } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE NULLIF(data->>'active', '')::boolean = false");
    }

    #[test]
    fn test_bool_not_true_filter_does_not_treat_missing_as_false() {
        let filters = json!({ "deleted": { "_neq": true } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(
            result,
            "WHERE NULLIF(data->>'deleted', '')::boolean != true"
        );
    }

    #[test]
    fn test_numeric_eq_filter_uses_numeric_cast() {
        let filters = json!({ "categoryId": { "_eq": 1 } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE NULLIF(data->>'categoryId', '')::numeric = 1");
    }

    #[test]
    fn test_format_sql_literal_null() {
        let filters = json!({ "deleted_at": { "_eq": null } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE data->>'deleted_at' IS NULL");
    }

    #[test]
    fn test_format_sql_literal_non_primitive() {
        // An object value gets serialized via serde_json::to_string
        let filters = json!({ "meta": { "_eq": {"key": "val"} } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert!(result.contains("data->>'meta' = "));
        assert!(result.contains("key"));
    }

    #[test]
    fn test_filters_use_id_column_directly() {
        let filters = json!({ "id": { "_eq": "abc123" } });
        let result = QueryTranslator::filters_to_where(&filters);
        assert_eq!(result, "WHERE id = 'abc123'");
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
            "SELECT * FROM products ORDER BY created_at DESC LIMIT 10 OFFSET 20"
        );
    }

    #[test]
    fn test_build_select_ascending_order() {
        let query =
            QueryTranslator::build_select("events", None, Some("timestamp"), false, None, None);
        assert_eq!(
            query,
            "SELECT * FROM events ORDER BY data->>'timestamp' ASC"
        );
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
            Some("createdAt"),
            true,
            Some(20),
            None,
            None,
            Some("abc123"),
        );
        assert!(query.contains("WHERE"));
        assert!(query.contains("created_at <"));
        assert!(query.contains("ORDER BY created_at DESC"));
        assert!(query.contains("SELECT created_at FROM products WHERE id = 'abc123'"));
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
        assert!(query.contains("data->>'status' = 'active'"));
        assert!(query.contains("AND"));
        assert!(query.contains("id >"));
    }

    #[test]
    fn test_build_select_ext_orders_json_field_via_data_extraction() {
        let query = QueryTranslator::build_select_ext(
            "products",
            None,
            Some("priceCents"),
            true,
            Some(5),
            None,
            None,
            None,
        );
        assert!(query.contains("ORDER BY NULLIF(data->>'priceCents', '')::numeric DESC"));
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
