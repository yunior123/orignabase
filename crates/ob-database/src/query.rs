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
}
