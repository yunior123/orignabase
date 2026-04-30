use crate::parser::{CompOp, Expression, RuleSet};
use serde_json::Value;
use std::collections::HashMap;

/// Context for evaluating security rules against a request.
#[derive(Debug, Clone)]
pub struct SecurityContext {
    pub user_id: Option<String>,
    pub roles: Vec<String>,
    pub authenticated: bool,
    /// The document being accessed (for update/delete checks)
    pub resource: Option<Value>,
    /// The incoming data (for create/update validation)
    pub incoming: Option<Value>,
}

/// Evaluates security rules against requests.
pub struct RuleEngine {
    rules: HashMap<String, RuleSet>,
}

impl RuleEngine {
    pub fn new(rules: HashMap<String, RuleSet>) -> Self {
        Self { rules }
    }

    /// Check if an operation is allowed on a collection.
    pub fn check(
        &self,
        collection: &str,
        operation: &str,
        ctx: &SecurityContext,
    ) -> ob_core::Result<bool> {
        let Some(rule_set) = self.rules.get(collection).or_else(|| self.rules.get("*")) else {
            // No rules defined (neither specific nor wildcard) → deny by default
            return Ok(false);
        };

        // OR semantics: if ANY matching rule allows the operation, it's permitted.
        // This supports patterns like: allow read if isOwner(); allow read if isAdmin();
        let mut found_matching_rule = false;
        for rule in &rule_set.rules {
            if rule.operations.iter().any(|op| op == operation) {
                found_matching_rule = true;
                let allowed = self.eval_expr(&rule.condition, ctx)?;
                if allowed {
                    // Check validation rules if present and this is a write operation
                    if let Some(ref validation) = rule.validation
                        && !self.eval_expr(validation, ctx)?
                    {
                        return Err(ob_core::Error::Validation(
                            "Validation rule failed".to_string(),
                        ));
                    }
                    return Ok(true);
                }
                // Rule didn't match — continue checking other rules
            }
        }

        // No matching rule found, or all matching rules denied → deny
        if !found_matching_rule {
            tracing::debug!("No rules defined for {collection}/{operation}");
        }
        Ok(false)
    }

    fn eval_expr(&self, expr: &Expression, ctx: &SecurityContext) -> ob_core::Result<bool> {
        match expr {
            Expression::Bool(b) => Ok(*b),
            Expression::FunctionCall { name, args } => self.eval_function(name, args, ctx),
            Expression::And(exprs) => {
                for e in exprs {
                    if !self.eval_expr(e, ctx)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Expression::Or(exprs) => {
                for e in exprs {
                    if self.eval_expr(e, ctx)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Expression::Not(e) => Ok(!self.eval_expr(e, ctx)?),
            Expression::Comparison { left, op, right } => {
                let l = self.eval_value(left, ctx);
                let r = self.eval_value(right, ctx);
                Ok(compare_values(&l, op, &r))
            }
            Expression::Path(path) => {
                // A bare path evaluates to truthy/falsy
                let val = self.resolve_path(path, ctx);
                Ok(is_truthy(&val))
            }
            _ => Ok(false),
        }
    }

    fn eval_function(
        &self,
        name: &str,
        args: &[Expression],
        ctx: &SecurityContext,
    ) -> ob_core::Result<bool> {
        match name {
            "isAuthenticated" => Ok(ctx.authenticated),
            "hasRole" => {
                if let Some(Expression::StringLit(role)) = args.first() {
                    Ok(ctx.roles.iter().any(|r| r == role))
                } else {
                    Ok(false)
                }
            }
            "isOwner" => {
                if let Some(Expression::Path(field)) = args.first() {
                    let resource_val = self.resolve_path(field, ctx);
                    if let (Some(uid), Value::String(owner_id)) = (&ctx.user_id, &resource_val) {
                        Ok(owner_matches_uid(owner_id, uid))
                    } else {
                        Ok(false)
                    }
                } else {
                    Ok(false)
                }
            }
            _ => {
                tracing::warn!("Unknown security function: {name}");
                Ok(false)
            }
        }
    }

    fn eval_value(&self, expr: &Expression, ctx: &SecurityContext) -> Value {
        match expr {
            Expression::Bool(b) => Value::Bool(*b),
            Expression::Number(n) => serde_json::Number::from_f64(*n)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            Expression::StringLit(s) => Value::String(s.clone()),
            Expression::Path(p) => self.resolve_path(p, ctx),
            _ => Value::Null,
        }
    }

    fn resolve_path(&self, path: &str, ctx: &SecurityContext) -> Value {
        let parts: Vec<&str> = path.split('.').collect();

        match parts[0] {
            "resource" => {
                if let Some(ref resource) = ctx.resource {
                    resolve_json_path(resource, &parts[1..])
                } else {
                    Value::Null
                }
            }
            "incoming" => {
                if let Some(ref incoming) = ctx.incoming {
                    resolve_json_path(incoming, &parts[1..])
                } else {
                    Value::Null
                }
            }
            "auth" => match parts.get(1) {
                Some(&"uid") => ctx
                    .user_id
                    .as_ref()
                    .map(|s| Value::String(s.clone()))
                    .unwrap_or(Value::Null),
                _ => Value::Null,
            },
            _ => Value::Null,
        }
    }

    /// Check if a specific field is accessible for an operation.
    /// Returns true if the field is allowed, false if denied.
    /// Fields not explicitly restricted are allowed by default.
    pub fn check_field(
        &self,
        collection: &str,
        field: &str,
        operation: &str,
        ctx: &SecurityContext,
    ) -> ob_core::Result<bool> {
        let field_key = format!("{}.{}", collection, field);
        if let Some(rule_set) = self.rules.get(&field_key) {
            for rule in &rule_set.rules {
                if rule.operations.iter().any(|op| op == operation) {
                    let allowed = self.eval_expr(&rule.condition, ctx)?;
                    if allowed {
                        // Also check validation rules on write operations
                        if let Some(ref validation) = rule.validation
                            && !self.eval_expr(validation, ctx)?
                        {
                            return Err(ob_core::Error::Validation(format!(
                                "Field validation failed for {field}"
                            )));
                        }
                        return Ok(true);
                    }
                    // Continue checking other rules (OR semantics)
                }
            }
            // Field rule exists but no matching rule allowed → deny
            Ok(false)
        } else {
            // No field-level rule → allow (collection-level rule already checked)
            Ok(true)
        }
    }

    /// Filter fields from a document based on field-level rules.
    /// Returns a new document with restricted fields removed.
    pub fn filter_fields(&self, collection: &str, doc: &Value, ctx: &SecurityContext) -> Value {
        if let Value::Object(map) = doc {
            let mut filtered = serde_json::Map::new();
            for (key, val) in map {
                if let Ok(true) = self.check_field(collection, key, "read", ctx) {
                    filtered.insert(key.clone(), val.clone());
                }
            }
            Value::Object(filtered)
        } else {
            doc.clone()
        }
    }
}

fn resolve_json_path(value: &Value, path: &[&str]) -> Value {
    let mut current = value;
    for &segment in path {
        match current.get(segment) {
            Some(v) => current = v,
            None => return Value::Null,
        }
    }
    current.clone()
}

fn owner_matches_uid(owner_id: &str, uid: &str) -> bool {
    owner_id == uid || owner_id.rsplit_once(':').is_some_and(|(_, id)| id == uid)
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(_) => true,
    }
}

fn compare_values(left: &Value, op: &CompOp, right: &Value) -> bool {
    match op {
        CompOp::Eq => left == right,
        CompOp::Neq => left != right,
        CompOp::Gt | CompOp::Gte | CompOp::Lt | CompOp::Lte => {
            if let (Some(l), Some(r)) = (left.as_f64(), right.as_f64()) {
                match op {
                    CompOp::Gt => l > r,
                    CompOp::Gte => l >= r,
                    CompOp::Lt => l < r,
                    CompOp::Lte => l <= r,
                    _ => false,
                }
            } else if let (Some(l), Some(r)) = (left.as_str(), right.as_str()) {
                match op {
                    CompOp::Gt => l > r,
                    CompOp::Gte => l >= r,
                    CompOp::Lt => l < r,
                    CompOp::Lte => l <= r,
                    _ => false,
                }
            } else {
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_rules;

    fn test_ctx(authenticated: bool, roles: Vec<&str>) -> SecurityContext {
        SecurityContext {
            user_id: if authenticated {
                Some("user123".into())
            } else {
                None
            },
            roles: roles.into_iter().map(String::from).collect(),
            authenticated,
            resource: None,
            incoming: None,
        }
    }

    #[test]
    fn test_public_read() {
        let rules = parse_rules(
            r#"
            rules products {
                read: true;
                create: isAuthenticated();
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);

        let anon = test_ctx(false, vec![]);
        assert!(engine.check("products", "read", &anon).unwrap());
        assert!(!engine.check("products", "create", &anon).unwrap());
    }

    #[test]
    fn test_authenticated_create() {
        let rules = parse_rules(
            r#"
            rules products {
                create: isAuthenticated();
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);

        let user = test_ctx(true, vec!["user"]);
        assert!(engine.check("products", "create", &user).unwrap());
    }

    #[test]
    fn test_role_based_access() {
        let rules = parse_rules(
            r#"
            rules products {
                create: isAuthenticated() && hasRole("seller");
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);

        let regular = test_ctx(true, vec!["user"]);
        assert!(!engine.check("products", "create", &regular).unwrap());

        let seller = test_ctx(true, vec!["user", "seller"]);
        assert!(engine.check("products", "create", &seller).unwrap());
    }

    #[test]
    fn test_undefined_collection_denied() {
        let rules = parse_rules(
            r#"
            rules products {
                read: true;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);

        let anon = test_ctx(false, vec![]);
        assert!(!engine.check("unknown_collection", "read", &anon).unwrap());
    }

    // Helper: context with resource data
    fn ctx_with_resource(user_id: &str, resource: Value) -> SecurityContext {
        SecurityContext {
            user_id: Some(user_id.to_string()),
            roles: vec![],
            authenticated: true,
            resource: Some(resource),
            incoming: None,
        }
    }

    // Helper: context with incoming data
    fn ctx_with_incoming(user_id: &str, incoming: Value) -> SecurityContext {
        SecurityContext {
            user_id: Some(user_id.to_string()),
            roles: vec![],
            authenticated: true,
            resource: None,
            incoming: Some(incoming),
        }
    }

    // Helper: context with both resource and incoming
    fn ctx_full(
        user_id: &str,
        resource: Value,
        incoming: Value,
        roles: Vec<&str>,
    ) -> SecurityContext {
        SecurityContext {
            user_id: Some(user_id.to_string()),
            roles: roles.into_iter().map(String::from).collect(),
            authenticated: true,
            resource: Some(resource),
            incoming: Some(incoming),
        }
    }

    // ---- isOwner tests ----

    #[test]
    fn test_is_owner_matches() {
        let rules = parse_rules(
            r#"
            rules products {
                update: isOwner(resource.seller_id);
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("user123", serde_json::json!({"seller_id": "user123"}));
        assert!(engine.check("products", "update", &ctx).unwrap());
    }

    #[test]
    fn test_is_owner_matches_record_reference() {
        let rules = parse_rules(
            r#"
            rules cart {
                create: isOwner(incoming.userId) || isOwner(incoming.parent_id);
                read: isOwner(resource.userId) || isOwner(resource.parent_id);
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let create_ctx = ctx_with_incoming(
            "user123",
            serde_json::json!({"userId": "users:user123", "parent_id": "users:user123"}),
        );
        assert!(engine.check("cart", "create", &create_ctx).unwrap());

        let read_ctx = ctx_with_resource(
            "user123",
            serde_json::json!({"userId": "users:user123", "parent_id": "users:user123"}),
        );
        assert!(engine.check("cart", "read", &read_ctx).unwrap());
    }

    #[test]
    fn test_is_owner_no_match() {
        let rules = parse_rules(
            r#"
            rules products {
                update: isOwner(resource.seller_id);
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("user123", serde_json::json!({"seller_id": "other_user"}));
        assert!(!engine.check("products", "update", &ctx).unwrap());
    }

    #[test]
    fn test_is_owner_missing_field() {
        let rules = parse_rules(
            r#"
            rules products {
                update: isOwner(resource.seller_id);
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("user123", serde_json::json!({"name": "Widget"}));
        assert!(!engine.check("products", "update", &ctx).unwrap());
    }

    #[test]
    fn test_is_owner_no_resource() {
        let rules = parse_rules(
            r#"
            rules products {
                update: isOwner(resource.seller_id);
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = test_ctx(true, vec![]);
        assert!(!engine.check("products", "update", &ctx).unwrap());
    }

    // ---- Or expression ----

    #[test]
    fn test_or_expression_first_true() {
        let rules = parse_rules(
            r#"
            rules products {
                delete: isOwner(resource.seller_id) || hasRole("admin");
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("user123", serde_json::json!({"seller_id": "user123"}));
        assert!(engine.check("products", "delete", &ctx).unwrap());
    }

    #[test]
    fn test_or_expression_second_true() {
        let rules = parse_rules(
            r#"
            rules products {
                delete: isOwner(resource.seller_id) || hasRole("admin");
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_full(
            "user123",
            serde_json::json!({"seller_id": "someone_else"}),
            serde_json::json!({}),
            vec!["admin"],
        );
        assert!(engine.check("products", "delete", &ctx).unwrap());
    }

    #[test]
    fn test_or_expression_both_false() {
        let rules = parse_rules(
            r#"
            rules products {
                delete: isOwner(resource.seller_id) || hasRole("admin");
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_full(
            "user123",
            serde_json::json!({"seller_id": "someone_else"}),
            serde_json::json!({}),
            vec!["user"],
        );
        assert!(!engine.check("products", "delete", &ctx).unwrap());
    }

    // ---- Not expression ----
    // NOTE: The `!` operator in the grammar is currently not propagated by the
    // parser (the "!" token is anonymous in pest and not counted in children).
    // These tests document the current behavior. When the parser is fixed,
    // update these assertions.

    #[test]
    fn test_not_expression_currently_ignored() {
        // Due to parser bug, `!isAuthenticated()` parses as `isAuthenticated()`
        let rules = parse_rules(
            r#"
            rules products {
                read: !isAuthenticated();
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        // Authenticated → the `!` is lost, so result is true (not negated)
        let ctx = test_ctx(true, vec![]);
        assert!(engine.check("products", "read", &ctx).unwrap());
        // Unauthenticated → false (not negated)
        let ctx2 = test_ctx(false, vec![]);
        assert!(!engine.check("products", "read", &ctx2).unwrap());
    }

    #[test]
    fn test_not_expression_via_eval_directly() {
        // Test the Not arm of eval_expr by constructing rules manually
        let mut rules_map = HashMap::new();
        rules_map.insert(
            "test".to_string(),
            crate::parser::RuleSet {
                collection: "test".to_string(),
                rules: vec![crate::parser::SecurityRule {
                    operations: vec!["read".to_string()],
                    condition: Expression::Not(Box::new(Expression::Bool(true))),
                    validation: None,
                }],
            },
        );
        let engine = RuleEngine::new(rules_map);
        let ctx = test_ctx(false, vec![]);
        assert!(!engine.check("test", "read", &ctx).unwrap());

        // Not(false) → true
        let mut rules_map2 = HashMap::new();
        rules_map2.insert(
            "test".to_string(),
            crate::parser::RuleSet {
                collection: "test".to_string(),
                rules: vec![crate::parser::SecurityRule {
                    operations: vec!["read".to_string()],
                    condition: Expression::Not(Box::new(Expression::Bool(false))),
                    validation: None,
                }],
            },
        );
        let engine2 = RuleEngine::new(rules_map2);
        assert!(engine2.check("test", "read", &ctx).unwrap());
    }

    // ---- Comparison expressions ----

    #[test]
    fn test_comparison_gt_number_true() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.price > 100;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"price": 200}));
        assert!(engine.check("products", "read", &ctx).unwrap());
    }

    #[test]
    fn test_comparison_gt_number_false() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.price > 100;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"price": 50}));
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    #[test]
    fn test_comparison_gte() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.price >= 100;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx_eq = ctx_with_resource("u1", serde_json::json!({"price": 100}));
        assert!(engine.check("products", "read", &ctx_eq).unwrap());
        let ctx_less = ctx_with_resource("u1", serde_json::json!({"price": 99}));
        assert!(!engine.check("products", "read", &ctx_less).unwrap());
    }

    #[test]
    fn test_comparison_lt() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.price < 100;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"price": 50}));
        assert!(engine.check("products", "read", &ctx).unwrap());
        let ctx2 = ctx_with_resource("u1", serde_json::json!({"price": 100}));
        assert!(!engine.check("products", "read", &ctx2).unwrap());
    }

    #[test]
    fn test_comparison_lte() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.price <= 100;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"price": 100}));
        assert!(engine.check("products", "read", &ctx).unwrap());
        let ctx2 = ctx_with_resource("u1", serde_json::json!({"price": 101}));
        assert!(!engine.check("products", "read", &ctx2).unwrap());
    }

    #[test]
    fn test_comparison_eq_string() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.status == "active";
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"status": "active"}));
        assert!(engine.check("products", "read", &ctx).unwrap());
        let ctx2 = ctx_with_resource("u1", serde_json::json!({"status": "inactive"}));
        assert!(!engine.check("products", "read", &ctx2).unwrap());
    }

    #[test]
    fn test_comparison_neq() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.status != "deleted";
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"status": "active"}));
        assert!(engine.check("products", "read", &ctx).unwrap());
        let ctx2 = ctx_with_resource("u1", serde_json::json!({"status": "deleted"}));
        assert!(!engine.check("products", "read", &ctx2).unwrap());
    }

    #[test]
    fn test_comparison_string_ordering() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.name > "apple";
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"name": "banana"}));
        assert!(engine.check("products", "read", &ctx).unwrap());
        let ctx2 = ctx_with_resource("u1", serde_json::json!({"name": "aardvark"}));
        assert!(!engine.check("products", "read", &ctx2).unwrap());
    }

    #[test]
    fn test_comparison_mixed_types_returns_false() {
        // Comparing string to number should return false for ordering ops
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.name > 100;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"name": "banana"}));
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    // ---- Path expression (bare path truthy/falsy) ----

    #[test]
    fn test_path_truthy_nonempty_string() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.name;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"name": "Widget"}));
        assert!(engine.check("products", "read", &ctx).unwrap());
    }

    #[test]
    fn test_path_falsy_empty_string() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.name;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"name": ""}));
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    #[test]
    fn test_path_falsy_null() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.missing_field;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"name": "Widget"}));
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    #[test]
    fn test_path_truthy_nonzero_number() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.count;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"count": 42}));
        assert!(engine.check("products", "read", &ctx).unwrap());
    }

    #[test]
    fn test_path_falsy_zero_number() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.count;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"count": 0}));
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    #[test]
    fn test_path_truthy_nonempty_array() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.tags;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"tags": ["a", "b"]}));
        assert!(engine.check("products", "read", &ctx).unwrap());
    }

    #[test]
    fn test_path_falsy_empty_array() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.tags;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"tags": []}));
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    #[test]
    fn test_path_truthy_object() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.meta;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"meta": {"key": "val"}}));
        assert!(engine.check("products", "read", &ctx).unwrap());
    }

    #[test]
    fn test_path_truthy_bool_true() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.active;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"active": true}));
        assert!(engine.check("products", "read", &ctx).unwrap());
    }

    #[test]
    fn test_path_falsy_bool_false() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.active;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"active": false}));
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    // ---- resolve_path: auth.uid ----

    #[test]
    fn test_auth_uid_path() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.owner == auth.uid;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let mut ctx = ctx_with_resource("user123", serde_json::json!({"owner": "user123"}));
        assert!(engine.check("products", "read", &ctx).unwrap());
        ctx.resource = Some(serde_json::json!({"owner": "other"}));
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    #[test]
    fn test_auth_uid_unauthenticated() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.owner == auth.uid;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = SecurityContext {
            user_id: None,
            roles: vec![],
            authenticated: false,
            resource: Some(serde_json::json!({"owner": "user123"})),
            incoming: None,
        };
        // auth.uid resolves to Null, string != Null → false
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    // ---- resolve_path: unknown root ----

    #[test]
    fn test_unknown_path_root() {
        let rules = parse_rules(
            r#"
            rules products {
                read: unknown.field;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = test_ctx(true, vec![]);
        // unknown root → Null → falsy
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    // ---- resolve_path: auth with unknown subpath ----

    #[test]
    fn test_auth_unknown_subpath() {
        let rules = parse_rules(
            r#"
            rules products {
                read: auth.email;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = test_ctx(true, vec![]);
        // auth.email → Null → falsy
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    // ---- incoming path ----

    #[test]
    fn test_incoming_path() {
        let rules = parse_rules(
            r#"
            rules products {
                create: incoming.price > 0;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_incoming("u1", serde_json::json!({"price": 10}));
        assert!(engine.check("products", "create", &ctx).unwrap());
        let ctx2 = ctx_with_incoming("u1", serde_json::json!({"price": -5}));
        assert!(!engine.check("products", "create", &ctx2).unwrap());
    }

    #[test]
    fn test_incoming_path_no_incoming() {
        let rules = parse_rules(
            r#"
            rules products {
                create: incoming.price > 0;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = test_ctx(true, vec![]); // no incoming
        // incoming.price → Null, Null > 0 → false (mixed types)
        assert!(!engine.check("products", "create", &ctx).unwrap());
    }

    // ---- Validation rules ----

    #[test]
    fn test_validation_passes() {
        let rules = parse_rules(
            r#"
            rules products {
                create: isAuthenticated();
                create: {
                    validate: incoming.price > 0;
                }
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_incoming("u1", serde_json::json!({"price": 25}));
        // First rule matches create → isAuthenticated() → true, no validation → allowed
        assert!(engine.check("products", "create", &ctx).unwrap());
    }

    #[test]
    fn test_validation_fails() {
        let rules = parse_rules(
            r#"
            rules products {
                create: {
                    validate: incoming.price > 0;
                }
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_incoming("u1", serde_json::json!({"price": -5}));
        // condition is true (default), but validation fails
        let result = engine.check("products", "create", &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_succeeds() {
        let rules = parse_rules(
            r#"
            rules products {
                create: {
                    validate: incoming.price > 0;
                }
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_incoming("u1", serde_json::json!({"price": 50}));
        assert!(engine.check("products", "create", &ctx).unwrap());
    }

    // ---- Unknown function ----

    #[test]
    fn test_unknown_function_returns_false() {
        let rules = parse_rules(
            r#"
            rules products {
                read: someCustomFunction();
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = test_ctx(true, vec![]);
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    // ---- Multiple operations in single rule entry ----

    #[test]
    fn test_multiple_operations_single_rule() {
        let rules = parse_rules(
            r#"
            rules products {
                create, update: isAuthenticated();
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let auth = test_ctx(true, vec![]);
        let anon = test_ctx(false, vec![]);
        assert!(engine.check("products", "create", &auth).unwrap());
        assert!(engine.check("products", "update", &auth).unwrap());
        assert!(!engine.check("products", "create", &anon).unwrap());
        assert!(!engine.check("products", "update", &anon).unwrap());
    }

    // ---- Bool literal false ----

    #[test]
    fn test_bool_false_denies() {
        let rules = parse_rules(
            r#"
            rules products {
                delete: false;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = test_ctx(true, vec!["admin"]);
        assert!(!engine.check("products", "delete", &ctx).unwrap());
    }

    // ---- No matching operation ----

    #[test]
    fn test_no_matching_operation_denied() {
        let rules = parse_rules(
            r#"
            rules products {
                read: true;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = test_ctx(true, vec![]);
        assert!(!engine.check("products", "delete", &ctx).unwrap());
    }

    // ---- Nested path resolution ----

    #[test]
    fn test_nested_resource_path() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.address.city == "Toronto";
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"address": {"city": "Toronto"}}));
        assert!(engine.check("products", "read", &ctx).unwrap());
        let ctx2 = ctx_with_resource("u1", serde_json::json!({"address": {"city": "Montreal"}}));
        assert!(!engine.check("products", "read", &ctx2).unwrap());
    }

    // ---- hasRole with no string arg ----

    #[test]
    fn test_has_role_non_string_arg() {
        let rules = parse_rules(
            r#"
            rules products {
                read: hasRole(resource.role);
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"role": "admin"}));
        // hasRole expects StringLit arg, gets Path → false
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    // ---- isOwner with non-path arg ----

    #[test]
    fn test_is_owner_non_path_arg() {
        let rules = parse_rules(
            r#"
            rules products {
                update: isOwner("literal_string");
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("user123", serde_json::json!({"seller_id": "user123"}));
        // isOwner expects Path arg, gets StringLit → false
        assert!(!engine.check("products", "update", &ctx).unwrap());
    }

    // ---- eval_value for non-matching expression ----

    #[test]
    fn test_comparison_with_bool_literal() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.active == true;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"active": true}));
        assert!(engine.check("products", "read", &ctx).unwrap());
        let ctx2 = ctx_with_resource("u1", serde_json::json!({"active": false}));
        assert!(!engine.check("products", "read", &ctx2).unwrap());
    }

    // ---- Parenthesized expression ----

    #[test]
    fn test_parenthesized_expression() {
        let rules = parse_rules(
            r#"
            rules products {
                read: (isAuthenticated() && hasRole("admin")) || hasRole("super");
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        // admin + authenticated → true
        let ctx = test_ctx(true, vec!["admin"]);
        assert!(engine.check("products", "read", &ctx).unwrap());
        // super alone → true
        let ctx2 = test_ctx(false, vec!["super"]);
        assert!(engine.check("products", "read", &ctx2).unwrap());
        // neither admin nor super → false
        let ctx3 = test_ctx(true, vec!["user"]);
        assert!(!engine.check("products", "read", &ctx3).unwrap());
    }

    // ---- Eq/Neq with numbers ----

    #[test]
    fn test_eq_numbers() {
        // NOTE: Number equality uses Value::eq which distinguishes i64 vs f64.
        // Rule literals are parsed as f64 (5.0), so resource values must also
        // be f64 for Eq to match. json!(5) creates i64, json!(5.0) creates f64.
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.quantity == 5;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        // f64 resource value matches f64 rule literal
        let ctx = ctx_with_resource("u1", serde_json::json!({"quantity": 5.0}));
        assert!(engine.check("products", "read", &ctx).unwrap());
        let ctx2 = ctx_with_resource("u1", serde_json::json!({"quantity": 10.0}));
        assert!(!engine.check("products", "read", &ctx2).unwrap());
    }

    #[test]
    fn test_eq_numbers_int_vs_float_mismatch() {
        // Documents that json integer (i64) != rule float (f64) in Eq comparison
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.quantity == 5;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        // json!(5) is i64, rule 5 is f64 → Eq comparison fails
        let ctx = ctx_with_resource("u1", serde_json::json!({"quantity": 5}));
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- Wildcard rules (`rules *`) ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_wildcard_allows_any_collection() {
        let rules = parse_rules(
            r#"
            rules * {
                read: isAuthenticated();
                create: isAuthenticated();
                update: isAuthenticated();
                delete: isAuthenticated();
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let auth = test_ctx(true, vec![]);
        let anon = test_ctx(false, vec![]);
        // Wildcard applies to any collection name
        assert!(engine.check("anything", "read", &auth).unwrap());
        assert!(engine.check("products", "create", &auth).unwrap());
        assert!(engine.check("orders", "update", &auth).unwrap());
        assert!(engine.check("xyz_123", "delete", &auth).unwrap());
        // Anon still denied
        assert!(!engine.check("anything", "read", &anon).unwrap());
    }

    #[test]
    fn test_specific_rules_override_wildcard() {
        let rules = parse_rules(
            r#"
            rules products {
                read: true;
                create: hasRole("seller");
                delete: false;
            }
            rules * {
                read: isAuthenticated();
                create: isAuthenticated();
                update: isAuthenticated();
                delete: isAuthenticated();
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let auth = test_ctx(true, vec![]);
        let seller = test_ctx(true, vec!["seller"]);
        let anon = test_ctx(false, vec![]);

        // products uses specific rules, not wildcard
        assert!(engine.check("products", "read", &anon).unwrap()); // true (public)
        assert!(!engine.check("products", "create", &auth).unwrap()); // requires seller
        assert!(engine.check("products", "create", &seller).unwrap());
        assert!(!engine.check("products", "delete", &auth).unwrap()); // false always

        // other collections fall through to wildcard
        assert!(engine.check("orders", "read", &auth).unwrap());
        assert!(!engine.check("orders", "read", &anon).unwrap());
        assert!(engine.check("orders", "delete", &auth).unwrap());
    }

    #[test]
    fn test_wildcard_deny_by_default_no_rules() {
        let rules = parse_rules("").unwrap();
        let engine = RuleEngine::new(rules);
        let auth = test_ctx(true, vec!["admin"]);
        // No rules at all → deny everything
        assert!(!engine.check("products", "read", &auth).unwrap());
        assert!(!engine.check("anything", "create", &auth).unwrap());
    }

    #[test]
    fn test_wildcard_with_role_based_access() {
        let rules = parse_rules(
            r#"
            rules * {
                read: true;
                create: isAuthenticated();
                update: isAuthenticated() && isOwner(resource.user_id);
                delete: hasRole("admin");
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);

        let anon = test_ctx(false, vec![]);
        let auth = test_ctx(true, vec![]);
        let admin = test_ctx(true, vec!["admin"]);

        // Public read for everything
        assert!(engine.check("users", "read", &anon).unwrap());
        assert!(engine.check("posts", "read", &anon).unwrap());

        // Auth required for create
        assert!(!engine.check("posts", "create", &anon).unwrap());
        assert!(engine.check("posts", "create", &auth).unwrap());

        // Owner check for update
        let owner_ctx = ctx_with_resource("user123", serde_json::json!({"user_id": "user123"}));
        assert!(engine.check("posts", "update", &owner_ctx).unwrap());
        let non_owner = ctx_with_resource("user123", serde_json::json!({"user_id": "other"}));
        assert!(!engine.check("posts", "update", &non_owner).unwrap());

        // Admin only for delete
        assert!(!engine.check("posts", "delete", &auth).unwrap());
        assert!(engine.check("posts", "delete", &admin).unwrap());
    }

    #[test]
    fn test_wildcard_operation_not_in_wildcard() {
        let rules = parse_rules(
            r#"
            rules * {
                read: true;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let auth = test_ctx(true, vec!["admin"]);
        // Only read is defined in wildcard → other ops denied
        assert!(engine.check("anything", "read", &auth).unwrap());
        assert!(!engine.check("anything", "create", &auth).unwrap());
        assert!(!engine.check("anything", "update", &auth).unwrap());
        assert!(!engine.check("anything", "delete", &auth).unwrap());
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- Field-level rules (check_field + filter_fields) ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_check_field_no_field_rule_allows() {
        let rules = parse_rules(
            r#"
            rules products {
                read: true;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = test_ctx(false, vec![]);
        // No field-level rule → allowed by default
        assert!(
            engine
                .check_field("products", "price", "read", &ctx)
                .unwrap()
        );
        assert!(
            engine
                .check_field("products", "name", "read", &ctx)
                .unwrap()
        );
    }

    #[test]
    fn test_check_field_with_field_rule() {
        let mut rules = parse_rules(
            r#"
            rules products {
                read: true;
            }
        "#,
        )
        .unwrap();
        // Add field-level rule: products.secret_field requires admin
        rules.insert(
            "products.secret_field".to_string(),
            RuleSet {
                collection: "products.secret_field".to_string(),
                rules: vec![crate::parser::SecurityRule {
                    operations: vec!["read".to_string()],
                    condition: Expression::FunctionCall {
                        name: "hasRole".to_string(),
                        args: vec![Expression::StringLit("admin".to_string())],
                    },
                    validation: None,
                }],
            },
        );
        let engine = RuleEngine::new(rules);
        let user = test_ctx(true, vec!["user"]);
        let admin = test_ctx(true, vec!["admin"]);

        // Normal fields → allowed
        assert!(
            engine
                .check_field("products", "name", "read", &user)
                .unwrap()
        );
        // Secret field → user denied, admin allowed
        assert!(
            !engine
                .check_field("products", "secret_field", "read", &user)
                .unwrap()
        );
        assert!(
            engine
                .check_field("products", "secret_field", "read", &admin)
                .unwrap()
        );
    }

    #[test]
    fn test_check_field_operation_mismatch() {
        let mut rules = parse_rules("").unwrap();
        rules.insert(
            "products.price".to_string(),
            RuleSet {
                collection: "products.price".to_string(),
                rules: vec![crate::parser::SecurityRule {
                    operations: vec!["read".to_string()],
                    condition: Expression::Bool(true),
                    validation: None,
                }],
            },
        );
        let engine = RuleEngine::new(rules);
        let ctx = test_ctx(true, vec![]);
        // Field rule exists for read but not update → deny
        assert!(
            engine
                .check_field("products", "price", "read", &ctx)
                .unwrap()
        );
        assert!(
            !engine
                .check_field("products", "price", "update", &ctx)
                .unwrap()
        );
    }

    #[test]
    fn test_filter_fields_removes_restricted() {
        let mut rules = parse_rules("").unwrap();
        // Restrict "ssn" field to admin only
        rules.insert(
            "users.ssn".to_string(),
            RuleSet {
                collection: "users.ssn".to_string(),
                rules: vec![crate::parser::SecurityRule {
                    operations: vec!["read".to_string()],
                    condition: Expression::FunctionCall {
                        name: "hasRole".to_string(),
                        args: vec![Expression::StringLit("admin".to_string())],
                    },
                    validation: None,
                }],
            },
        );
        let engine = RuleEngine::new(rules);
        let user = test_ctx(true, vec!["user"]);
        let admin = test_ctx(true, vec!["admin"]);

        let doc = serde_json::json!({
            "name": "John",
            "email": "john@example.com",
            "ssn": "123-45-6789"
        });

        // User → ssn filtered out
        let filtered = engine.filter_fields("users", &doc, &user);
        assert!(filtered.get("name").is_some());
        assert!(filtered.get("email").is_some());
        assert!(filtered.get("ssn").is_none());

        // Admin → ssn kept
        let filtered_admin = engine.filter_fields("users", &doc, &admin);
        assert!(filtered_admin.get("ssn").is_some());
    }

    #[test]
    fn test_filter_fields_non_object() {
        let rules = parse_rules("").unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = test_ctx(true, vec![]);
        let val = serde_json::json!("just a string");
        let result = engine.filter_fields("users", &val, &ctx);
        assert_eq!(result, val); // Returned unchanged
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- Complex real-world rule scenarios ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_ecommerce_rules_complete() {
        let rules = parse_rules(
            r#"
            rules users {
                read: isAuthenticated();
                create: true;
                update: isAuthenticated() && isOwner(resource.id);
                delete: hasRole("admin");
            }
            rules products {
                read: true;
                create: isAuthenticated() && hasRole("seller");
                update: isOwner(resource.seller_id) || hasRole("admin");
                delete: hasRole("admin");
            }
            rules orders {
                read: isAuthenticated() && isOwner(resource.customer_id);
                create: isAuthenticated();
                update: isOwner(resource.customer_id) || hasRole("admin");
                delete: hasRole("admin");
            }
            rules reviews {
                read: true;
                create: isAuthenticated();
                update: isOwner(resource.author_id);
                delete: isOwner(resource.author_id) || hasRole("admin");
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);

        let anon = test_ctx(false, vec![]);
        let buyer = test_ctx(true, vec!["user"]);
        let seller = test_ctx(true, vec!["user", "seller"]);
        let admin = test_ctx(true, vec!["admin"]);

        // Users
        assert!(!engine.check("users", "read", &anon).unwrap());
        assert!(engine.check("users", "read", &buyer).unwrap());
        assert!(engine.check("users", "create", &anon).unwrap()); // signup

        // Products — public read
        assert!(engine.check("products", "read", &anon).unwrap());
        assert!(!engine.check("products", "create", &buyer).unwrap());
        assert!(engine.check("products", "create", &seller).unwrap());

        // Orders — owner only
        let buyer_order =
            ctx_with_resource("user123", serde_json::json!({"customer_id": "user123"}));
        assert!(engine.check("orders", "read", &buyer_order).unwrap());
        let other_order = ctx_with_resource("user123", serde_json::json!({"customer_id": "other"}));
        assert!(!engine.check("orders", "read", &other_order).unwrap());

        // Reviews
        assert!(engine.check("reviews", "read", &anon).unwrap());
        assert!(engine.check("reviews", "create", &buyer).unwrap());
        let own_review = ctx_with_resource("user123", serde_json::json!({"author_id": "user123"}));
        assert!(engine.check("reviews", "delete", &own_review).unwrap());
        let other_review = ctx_with_resource("user123", serde_json::json!({"author_id": "other"}));
        assert!(!engine.check("reviews", "delete", &other_review).unwrap());
        assert!(engine.check("reviews", "delete", &admin).unwrap());
    }

    #[test]
    fn test_multi_collection_with_wildcard_fallback() {
        let rules = parse_rules(
            r#"
            rules users {
                read: isAuthenticated();
                create: true;
                update: isAuthenticated() && isOwner(resource.id);
                delete: hasRole("admin");
            }
            rules * {
                read: isAuthenticated();
                create: isAuthenticated();
                update: isAuthenticated();
                delete: hasRole("admin");
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);

        let anon = test_ctx(false, vec![]);
        let auth = test_ctx(true, vec![]);
        let admin = test_ctx(true, vec!["admin"]);

        // Users uses specific rules
        assert!(engine.check("users", "create", &anon).unwrap()); // public signup
        assert!(!engine.check("users", "read", &anon).unwrap());

        // Unknown collections use wildcard
        assert!(!engine.check("logs", "read", &anon).unwrap());
        assert!(engine.check("logs", "read", &auth).unwrap());
        assert!(!engine.check("logs", "delete", &auth).unwrap());
        assert!(engine.check("logs", "delete", &admin).unwrap());

        // Many different collection names all work
        for col in &[
            "settings",
            "notifications",
            "payments",
            "coupons",
            "analytics",
        ] {
            assert!(engine.check(col, "read", &auth).unwrap());
            assert!(!engine.check(col, "read", &anon).unwrap());
        }
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- Validation rules — comprehensive ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_validation_with_string_comparison() {
        let rules = parse_rules(
            r#"
            rules products {
                create: {
                    validate: incoming.status == "draft";
                }
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx_ok = ctx_with_incoming("u1", serde_json::json!({"status": "draft"}));
        assert!(engine.check("products", "create", &ctx_ok).unwrap());
        let ctx_bad = ctx_with_incoming("u1", serde_json::json!({"status": "published"}));
        assert!(engine.check("products", "create", &ctx_bad).is_err());
    }

    #[test]
    fn test_validation_with_multiple_conditions() {
        let rules = parse_rules(
            r#"
            rules products {
                create: {
                    validate: incoming.price > 0;
                }
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        // Zero price should fail validation
        let ctx = ctx_with_incoming("u1", serde_json::json!({"price": 0}));
        assert!(engine.check("products", "create", &ctx).is_err());
        // Negative price should fail
        let ctx2 = ctx_with_incoming("u1", serde_json::json!({"price": -10}));
        assert!(engine.check("products", "create", &ctx2).is_err());
        // Valid price
        let ctx3 = ctx_with_incoming("u1", serde_json::json!({"price": 0.01}));
        assert!(engine.check("products", "create", &ctx3).unwrap());
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- Complex boolean logic ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_triple_and() {
        let rules = parse_rules(
            r#"
            rules products {
                create: isAuthenticated() && hasRole("seller") && hasRole("verified");
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let verified_seller = test_ctx(true, vec!["seller", "verified"]);
        assert!(
            engine
                .check("products", "create", &verified_seller)
                .unwrap()
        );
        let unverified_seller = test_ctx(true, vec!["seller"]);
        assert!(
            !engine
                .check("products", "create", &unverified_seller)
                .unwrap()
        );
        let verified_buyer = test_ctx(true, vec!["verified"]);
        assert!(!engine.check("products", "create", &verified_buyer).unwrap());
    }

    #[test]
    fn test_triple_or() {
        let rules = parse_rules(
            r#"
            rules products {
                delete: hasRole("admin") || hasRole("moderator") || hasRole("superuser");
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        assert!(
            engine
                .check("products", "delete", &test_ctx(true, vec!["admin"]))
                .unwrap()
        );
        assert!(
            engine
                .check("products", "delete", &test_ctx(true, vec!["moderator"]))
                .unwrap()
        );
        assert!(
            engine
                .check("products", "delete", &test_ctx(true, vec!["superuser"]))
                .unwrap()
        );
        assert!(
            !engine
                .check("products", "delete", &test_ctx(true, vec!["user"]))
                .unwrap()
        );
    }

    #[test]
    fn test_mixed_and_or_precedence() {
        // AND has higher precedence than OR
        // a || b && c means a || (b && c)
        let rules = parse_rules(
            r#"
            rules products {
                read: hasRole("vip") || isAuthenticated() && hasRole("premium");
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        // VIP alone → allowed (first OR branch)
        assert!(
            engine
                .check("products", "read", &test_ctx(true, vec!["vip"]))
                .unwrap()
        );
        // Auth + premium → allowed (second branch)
        assert!(
            engine
                .check("products", "read", &test_ctx(true, vec!["premium"]))
                .unwrap()
        );
        // Auth alone → denied
        assert!(
            !engine
                .check("products", "read", &test_ctx(true, vec!["user"]))
                .unwrap()
        );
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- Deeply nested resource paths ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_deeply_nested_path_3_levels() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.seller.address.country == "CA";
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource(
            "u1",
            serde_json::json!({
                "seller": {"address": {"country": "CA"}}
            }),
        );
        assert!(engine.check("products", "read", &ctx).unwrap());
        let ctx2 = ctx_with_resource(
            "u1",
            serde_json::json!({
                "seller": {"address": {"country": "US"}}
            }),
        );
        assert!(!engine.check("products", "read", &ctx2).unwrap());
    }

    #[test]
    fn test_nested_path_missing_intermediate() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.seller.address.country == "CA";
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        // Missing "address" in path
        let ctx = ctx_with_resource("u1", serde_json::json!({"seller": {"name": "Joe"}}));
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- Edge cases: empty rules, comments, whitespace ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_comments_in_rules() {
        let rules = parse_rules(
            r#"
            // This is a comment
            rules products {
                // Public read
                read: true;
                // Only authenticated users can create
                create: isAuthenticated();
            }
            // Another comment
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let anon = test_ctx(false, vec![]);
        assert!(engine.check("products", "read", &anon).unwrap());
    }

    #[test]
    fn test_empty_file() {
        let rules = parse_rules("").unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn test_only_comments() {
        let rules = parse_rules(
            r#"
            // Just comments
            // Nothing else
        "#,
        )
        .unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn test_whitespace_heavy() {
        let rules = parse_rules(
            r#"


            rules    products    {
                read   :   true   ;
                create  :  isAuthenticated()  ;
            }


        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let anon = test_ctx(false, vec![]);
        assert!(engine.check("products", "read", &anon).unwrap());
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- Negative number in rule ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_negative_number_comparison() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.temperature > -10;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"temperature": 5.0}));
        assert!(engine.check("products", "read", &ctx).unwrap());
        let ctx2 = ctx_with_resource("u1", serde_json::json!({"temperature": -20.0}));
        assert!(!engine.check("products", "read", &ctx2).unwrap());
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- Multiple rule blocks with same collection (last wins) ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_duplicate_collection_last_wins() {
        let rules = parse_rules(
            r#"
            rules products {
                read: false;
            }
            rules products {
                read: true;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let anon = test_ctx(false, vec![]);
        // HashMap insert overwrites → last definition wins
        assert!(engine.check("products", "read", &anon).unwrap());
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- All five operations explicitly tested ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_all_five_operations() {
        let rules = parse_rules(
            r#"
            rules products {
                read: true;
                create: isAuthenticated();
                update: hasRole("editor");
                delete: hasRole("admin");
                list: true;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let anon = test_ctx(false, vec![]);
        let auth = test_ctx(true, vec![]);
        let editor = test_ctx(true, vec!["editor"]);
        let admin = test_ctx(true, vec!["admin"]);

        assert!(engine.check("products", "read", &anon).unwrap());
        assert!(engine.check("products", "list", &anon).unwrap());
        assert!(!engine.check("products", "create", &anon).unwrap());
        assert!(engine.check("products", "create", &auth).unwrap());
        assert!(!engine.check("products", "update", &auth).unwrap());
        assert!(engine.check("products", "update", &editor).unwrap());
        assert!(!engine.check("products", "delete", &editor).unwrap());
        assert!(engine.check("products", "delete", &admin).unwrap());
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- Function with multiple args ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_function_with_multiple_args() {
        let rules = parse_rules(
            r#"
            rules products {
                read: customFunc("arg1", "arg2", 42);
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = test_ctx(true, vec![]);
        // Unknown function → false, but it parsed correctly (no error)
        assert!(!engine.check("products", "read", &ctx).unwrap());
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- origna_gta-style 826-line security rules simulation ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_origna_gta_style_rules() {
        let rules = parse_rules(
            r#"
            rules users {
                read: isAuthenticated();
                create: true;
                update: isAuthenticated() && isOwner(resource.id);
                delete: hasRole("admin");
                list: isAuthenticated();
            }
            rules products {
                read: true;
                create: isAuthenticated() && hasRole("seller");
                update: isAuthenticated() && (isOwner(resource.seller_id) || hasRole("admin"));
                delete: hasRole("admin");
                list: true;
            }
            rules orders {
                read: isAuthenticated() && (isOwner(resource.customer_id) || hasRole("admin"));
                create: isAuthenticated();
                update: isAuthenticated() && (isOwner(resource.customer_id) || hasRole("admin"));
                delete: hasRole("admin");
                list: isAuthenticated();
            }
            rules reviews {
                read: true;
                create: isAuthenticated();
                update: isAuthenticated() && isOwner(resource.author_id);
                delete: isAuthenticated() && (isOwner(resource.author_id) || hasRole("admin"));
                list: true;
            }
            rules coupons {
                read: isAuthenticated();
                create: hasRole("admin");
                update: hasRole("admin");
                delete: hasRole("admin");
                list: isAuthenticated();
            }
            rules notifications {
                read: isAuthenticated() && isOwner(resource.user_id);
                create: isAuthenticated();
                update: isAuthenticated() && isOwner(resource.user_id);
                delete: isAuthenticated() && isOwner(resource.user_id);
                list: isAuthenticated();
            }
            rules * {
                read: isAuthenticated();
                create: isAuthenticated();
                update: isAuthenticated();
                delete: hasRole("admin");
                list: isAuthenticated();
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);

        let anon = test_ctx(false, vec![]);
        let buyer = test_ctx(true, vec!["user"]);
        let seller = test_ctx(true, vec!["user", "seller"]);
        let admin = test_ctx(true, vec!["admin"]);

        // -- Users --
        assert!(engine.check("users", "create", &anon).unwrap()); // signup
        assert!(!engine.check("users", "delete", &buyer).unwrap());
        assert!(engine.check("users", "delete", &admin).unwrap());

        // -- Products --
        assert!(engine.check("products", "read", &anon).unwrap());
        assert!(engine.check("products", "list", &anon).unwrap());
        assert!(!engine.check("products", "create", &buyer).unwrap());
        assert!(engine.check("products", "create", &seller).unwrap());

        // Seller updates own product
        let own_product = ctx_full(
            "user123",
            serde_json::json!({"seller_id": "user123"}),
            serde_json::json!({}),
            vec!["user", "seller"],
        );
        assert!(engine.check("products", "update", &own_product).unwrap());

        // Admin can update any product
        let admin_update = ctx_full(
            "admin1",
            serde_json::json!({"seller_id": "other_seller"}),
            serde_json::json!({}),
            vec!["admin"],
        );
        assert!(engine.check("products", "update", &admin_update).unwrap());

        // -- Orders --
        let own_order = ctx_with_resource("user123", serde_json::json!({"customer_id": "user123"}));
        assert!(engine.check("orders", "read", &own_order).unwrap());
        let other_order = ctx_with_resource("user123", serde_json::json!({"customer_id": "other"}));
        assert!(!engine.check("orders", "read", &other_order).unwrap());

        // -- Coupons: admin only for writes --
        assert!(!engine.check("coupons", "create", &buyer).unwrap());
        assert!(engine.check("coupons", "create", &admin).unwrap());

        // -- Wildcard fallback for unknown collections --
        assert!(engine.check("analytics", "read", &buyer).unwrap());
        assert!(!engine.check("analytics", "delete", &buyer).unwrap());
        assert!(engine.check("analytics", "delete", &admin).unwrap());
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- isOwner with different field names ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_is_owner_various_fields() {
        let rules = parse_rules(
            r#"
            rules posts {
                update: isOwner(resource.author_id);
                delete: isOwner(resource.created_by);
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);

        let ctx = ctx_with_resource(
            "user123",
            serde_json::json!({
                "author_id": "user123",
                "created_by": "someone_else"
            }),
        );
        assert!(engine.check("posts", "update", &ctx).unwrap()); // author matches
        assert!(!engine.check("posts", "delete", &ctx).unwrap()); // created_by doesn't match
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- Parser error cases ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_parse_error_unclosed_brace() {
        let result = parse_rules(r#"rules products { read: true;"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_error_no_collection_name() {
        let result = parse_rules(r#"rules { read: true; }"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_error_invalid_operation() {
        // "readall" is not a valid operation keyword
        let result = parse_rules(r#"rules products { readall: true; }"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_error_missing_condition() {
        let result = parse_rules(r#"rules products { read: ; }"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_error_double_colon() {
        let result = parse_rules(r#"rules products { read:: true; }"#);
        assert!(result.is_err());
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- Comparison with incoming data ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_incoming_vs_resource_comparison() {
        let rules = parse_rules(
            r#"
            rules products {
                update: incoming.price >= resource.min_price;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_full(
            "u1",
            serde_json::json!({"min_price": 10.0}),
            serde_json::json!({"price": 15.0}),
            vec![],
        );
        assert!(engine.check("products", "update", &ctx).unwrap());
        let ctx2 = ctx_full(
            "u1",
            serde_json::json!({"min_price": 10.0}),
            serde_json::json!({"price": 5.0}),
            vec![],
        );
        assert!(!engine.check("products", "update", &ctx2).unwrap());
    }

    // ══════════════════════════════════════════════════════════════════
    // ---- Decimal number edge cases ----
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_decimal_boundary() {
        let rules = parse_rules(
            r#"
            rules products {
                read: resource.price >= 9.99;
            }
        "#,
        )
        .unwrap();
        let engine = RuleEngine::new(rules);
        let ctx = ctx_with_resource("u1", serde_json::json!({"price": 9.99}));
        assert!(engine.check("products", "read", &ctx).unwrap());
        let ctx2 = ctx_with_resource("u1", serde_json::json!({"price": 9.98}));
        assert!(!engine.check("products", "read", &ctx2).unwrap());
    }
}
