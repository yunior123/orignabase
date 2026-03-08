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
        let Some(rule_set) = self.rules.get(collection) else {
            // No rules defined → deny by default
            return Ok(false);
        };

        for rule in &rule_set.rules {
            if rule.operations.iter().any(|op| op == operation) {
                let allowed = self.eval_expr(&rule.condition, ctx)?;
                if !allowed {
                    return Ok(false);
                }

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
        }

        // No matching rule → deny
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
                        Ok(uid == owner_id)
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
}
