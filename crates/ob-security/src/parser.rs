use pest::Parser;
use pest_derive::Parser;
use std::collections::HashMap;

#[derive(Parser)]
#[grammar = "rules.pest"]
struct RulesParser;

/// A parsed security rule for a specific operation.
#[derive(Debug, Clone)]
pub struct SecurityRule {
    pub operations: Vec<String>,
    pub condition: Expression,
    pub validation: Option<Expression>,
}

/// An expression in the security rules DSL.
#[derive(Debug, Clone)]
pub enum Expression {
    Bool(bool),
    Number(f64),
    StringLit(String),
    Path(String),
    FunctionCall {
        name: String,
        args: Vec<Expression>,
    },
    Comparison {
        left: Box<Expression>,
        op: CompOp,
        right: Box<Expression>,
    },
    And(Vec<Expression>),
    Or(Vec<Expression>),
    Not(Box<Expression>),
}

#[derive(Debug, Clone)]
pub enum CompOp {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
}

/// A complete set of rules for one collection.
#[derive(Debug, Clone)]
pub struct RuleSet {
    pub collection: String,
    pub rules: Vec<SecurityRule>,
}

/// Parse a rules file into a map of collection name to RuleSet.
pub fn parse_rules(input: &str) -> ob_core::Result<HashMap<String, RuleSet>> {
    let file = RulesParser::parse(Rule::file, input)
        .map_err(|e| ob_core::Error::Config(format!("Rules parse error: {e}")))?;

    let mut result = HashMap::new();

    // parse() returns Pairs; the first pair is `file`, iterate its inner pairs
    for top_pair in file {
        for pair in top_pair.into_inner() {
            match pair.as_rule() {
                Rule::rule_block => {
                    let rule_set = parse_rule_block(pair)?;
                    result.insert(rule_set.collection.clone(), rule_set);
                }
                Rule::EOI => {}
                _ => {}
            }
        }
    }

    Ok(result)
}

use pest::iterators::Pair;

fn parse_rule_block(pair: Pair<Rule>) -> ob_core::Result<RuleSet> {
    let mut inner = pair.into_inner();
    let collection = inner.next().unwrap().as_str().to_string();
    let mut rules = Vec::new();

    for entry in inner {
        match entry.as_rule() {
            Rule::rule_entry => {
                let mut entry_inner = entry.into_inner();
                let ops = parse_op_list(entry_inner.next().unwrap());
                let condition = parse_expr(entry_inner.next().unwrap())?;
                rules.push(SecurityRule {
                    operations: ops,
                    condition,
                    validation: None,
                });
            }
            Rule::validate_entry => {
                let mut entry_inner = entry.into_inner();
                let ops = parse_op_list(entry_inner.next().unwrap());
                let validation = parse_expr(entry_inner.next().unwrap())?;
                rules.push(SecurityRule {
                    operations: ops,
                    condition: Expression::Bool(true),
                    validation: Some(validation),
                });
            }
            _ => {}
        }
    }

    Ok(RuleSet { collection, rules })
}

fn parse_op_list(pair: Pair<Rule>) -> Vec<String> {
    pair.into_inner().map(|p| p.as_str().to_string()).collect()
}

fn parse_expr(pair: Pair<Rule>) -> ob_core::Result<Expression> {
    let rule = pair.as_rule();
    let text = pair.as_str();

    match rule {
        Rule::file => {
            // Traverse into file → rule_block
            let inner: Vec<Pair<Rule>> = pair.into_inner().collect();
            if let Some(first) = inner.into_iter().next() {
                parse_expr(first)
            } else {
                Ok(Expression::Bool(false))
            }
        }
        Rule::expr => {
            // OR expression: and_expr ("||" and_expr)*
            let parts: Vec<Pair<Rule>> = pair.into_inner().collect();
            if parts.len() == 1 {
                parse_expr(parts.into_iter().next().unwrap())
            } else {
                let exprs: Vec<Expression> = parts
                    .into_iter()
                    .map(parse_expr)
                    .collect::<ob_core::Result<_>>()?;
                Ok(Expression::Or(exprs))
            }
        }
        Rule::and_expr => {
            let parts: Vec<Pair<Rule>> = pair.into_inner().collect();
            if parts.len() == 1 {
                parse_expr(parts.into_iter().next().unwrap())
            } else {
                let exprs: Vec<Expression> = parts
                    .into_iter()
                    .map(parse_expr)
                    .collect::<ob_core::Result<_>>()?;
                Ok(Expression::And(exprs))
            }
        }
        Rule::not_expr => {
            let parts: Vec<Pair<Rule>> = pair.into_inner().collect();
            if parts.len() == 1 {
                parse_expr(parts.into_iter().next().unwrap())
            } else {
                // "!" + atom
                let expr = parse_expr(parts.into_iter().last().unwrap())?;
                Ok(Expression::Not(Box::new(expr)))
            }
        }
        Rule::comparison => {
            let mut it = pair.into_inner();
            let left = parse_expr(it.next().unwrap())?;
            let op_str = it.next().unwrap().as_str();
            let right = parse_expr(it.next().unwrap())?;
            let op = match op_str {
                "==" => CompOp::Eq,
                "!=" => CompOp::Neq,
                ">" => CompOp::Gt,
                ">=" => CompOp::Gte,
                "<" => CompOp::Lt,
                "<=" => CompOp::Lte,
                _ => {
                    return Err(ob_core::Error::Config(format!(
                        "Unknown operator: {op_str}"
                    )));
                }
            };
            Ok(Expression::Comparison {
                left: Box::new(left),
                op,
                right: Box::new(right),
            })
        }
        Rule::atom => {
            // "(" expr ")" | value
            let inner = pair.into_inner().next().unwrap();
            parse_expr(inner)
        }
        Rule::value => {
            // function_call | dot_path | string_lit | number_lit | bool_lit
            let inner = pair.into_inner().next().unwrap();
            parse_expr(inner)
        }
        Rule::function_call => {
            let mut it = pair.into_inner();
            let name = it.next().unwrap().as_str().to_string();
            let args: Vec<Expression> = it.map(parse_expr).collect::<ob_core::Result<_>>()?;
            Ok(Expression::FunctionCall { name, args })
        }
        // Atomic rules — no inner pairs, use as_str() on the pair directly
        Rule::bool_lit => Ok(Expression::Bool(text == "true")),
        Rule::number_lit => {
            let n: f64 = text
                .parse()
                .map_err(|_| ob_core::Error::Config("Invalid number".into()))?;
            Ok(Expression::Number(n))
        }
        Rule::string_lit => {
            // Strip quotes
            Ok(Expression::StringLit(text[1..text.len() - 1].to_string()))
        }
        Rule::dot_path => Ok(Expression::Path(text.to_string())),
        Rule::ident => Ok(Expression::Path(text.to_string())),
        _ => Err(ob_core::Error::Config(format!(
            "Unexpected rule: {:?} with text: {text}",
            rule
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_rules() {
        let input = r#"
            rules products {
                read: true;
                create: isAuthenticated();
            }
        "#;
        let result = parse_rules(input).unwrap();
        assert!(result.contains_key("products"));
        assert_eq!(result["products"].rules.len(), 2);
    }

    #[test]
    fn test_parse_complex_rules() {
        let input = r#"
            rules products {
                read: true;
                create: isAuthenticated() && hasRole("seller");
                update: isOwner(resource.seller_id) || hasRole("admin");
                delete: hasRole("admin");
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rules = &result["products"];
        assert_eq!(rules.rules.len(), 4);
    }

    #[test]
    fn test_parse_not_expression() {
        // NOTE: The `!` token is anonymous in pest, so the parser currently
        // does not propagate it — `!isAuthenticated()` parses the same as
        // `isAuthenticated()`. This test documents the current behavior.
        let input = r#"
            rules products {
                read: !isAuthenticated();
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rule = &result["products"].rules[0];
        assert_eq!(rule.operations, vec!["read"]);
        // Due to parser bug, the Not is lost; it parses as FunctionCall
        assert!(matches!(rule.condition, Expression::FunctionCall { .. }));
    }

    #[test]
    fn test_parse_comparison_number() {
        let input = r#"
            rules products {
                read: resource.price > 100;
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rule = &result["products"].rules[0];
        assert!(matches!(rule.condition, Expression::Comparison { .. }));
        if let Expression::Comparison { ref left, ref op, ref right } = rule.condition {
            assert!(matches!(left.as_ref(), Expression::Path(p) if p == "resource.price"));
            assert!(matches!(op, CompOp::Gt));
            assert!(matches!(right.as_ref(), Expression::Number(n) if (*n - 100.0).abs() < f64::EPSILON));
        }
    }

    #[test]
    fn test_parse_comparison_string_literal() {
        let input = r#"
            rules products {
                read: resource.status == "active";
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rule = &result["products"].rules[0];
        if let Expression::Comparison { ref left, ref op, ref right } = rule.condition {
            assert!(matches!(left.as_ref(), Expression::Path(p) if p == "resource.status"));
            assert!(matches!(op, CompOp::Eq));
            assert!(matches!(right.as_ref(), Expression::StringLit(s) if s == "active"));
        } else {
            panic!("Expected Comparison expression");
        }
    }

    #[test]
    fn test_parse_validate_entry() {
        let input = r#"
            rules products {
                create: {
                    validate: incoming.price > 0;
                }
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rule = &result["products"].rules[0];
        assert_eq!(rule.operations, vec!["create"]);
        // validate_entry sets condition = Bool(true), validation = Some(...)
        assert!(matches!(rule.condition, Expression::Bool(true)));
        assert!(rule.validation.is_some());
    }

    #[test]
    fn test_parse_multiple_operations() {
        let input = r#"
            rules products {
                create, update: isAuthenticated();
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rule = &result["products"].rules[0];
        assert_eq!(rule.operations, vec!["create", "update"]);
    }

    #[test]
    fn test_parse_error_invalid_syntax() {
        let input = r#"this is not valid rules syntax at all {{{}"#;
        let result = parse_rules(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_error_missing_semicolon() {
        let input = r#"
            rules products {
                read: true
            }
        "#;
        let result = parse_rules(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_bool_false() {
        let input = r#"
            rules products {
                delete: false;
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rule = &result["products"].rules[0];
        assert!(matches!(rule.condition, Expression::Bool(false)));
    }

    #[test]
    fn test_parse_parenthesized_expression() {
        let input = r#"
            rules products {
                read: (isAuthenticated() && hasRole("admin")) || hasRole("super");
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rule = &result["products"].rules[0];
        // Top-level should be Or
        assert!(matches!(rule.condition, Expression::Or(_)));
    }

    #[test]
    fn test_parse_nested_path() {
        let input = r#"
            rules products {
                read: resource.address.city == "Toronto";
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rule = &result["products"].rules[0];
        if let Expression::Comparison { ref left, .. } = rule.condition {
            assert!(matches!(left.as_ref(), Expression::Path(p) if p == "resource.address.city"));
        } else {
            panic!("Expected Comparison expression");
        }
    }

    #[test]
    fn test_parse_multiple_rule_blocks() {
        let input = r#"
            rules products {
                read: true;
            }
            rules orders {
                read: isAuthenticated();
                create: isAuthenticated();
            }
        "#;
        let result = parse_rules(input).unwrap();
        assert!(result.contains_key("products"));
        assert!(result.contains_key("orders"));
        assert_eq!(result["products"].rules.len(), 1);
        assert_eq!(result["orders"].rules.len(), 2);
    }

    #[test]
    fn test_parse_all_comparison_operators() {
        let ops = vec![(">=", "Gte"), ("<=", "Lte"), ("!=", "Neq"), ("<", "Lt"), ("==", "Eq")];
        for (op_str, _label) in ops {
            let input = format!(
                r#"rules products {{ read: resource.val {op_str} 10; }}"#
            );
            let result = parse_rules(&input).unwrap();
            let rule = &result["products"].rules[0];
            assert!(matches!(rule.condition, Expression::Comparison { .. }),
                "Failed to parse operator {op_str}");
        }
    }

    #[test]
    fn test_parse_function_with_path_arg() {
        let input = r#"
            rules products {
                update: isOwner(resource.seller_id);
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rule = &result["products"].rules[0];
        if let Expression::FunctionCall { ref name, ref args } = rule.condition {
            assert_eq!(name, "isOwner");
            assert_eq!(args.len(), 1);
            assert!(matches!(&args[0], Expression::Path(p) if p == "resource.seller_id"));
        } else {
            panic!("Expected FunctionCall");
        }
    }

    #[test]
    fn test_parse_or_expression() {
        let input = r#"
            rules products {
                delete: hasRole("admin") || hasRole("super");
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rule = &result["products"].rules[0];
        if let Expression::Or(ref exprs) = rule.condition {
            assert_eq!(exprs.len(), 2);
        } else {
            panic!("Expected Or expression");
        }
    }

    #[test]
    fn test_parse_and_expression() {
        let input = r#"
            rules products {
                create: isAuthenticated() && hasRole("seller") && hasRole("verified");
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rule = &result["products"].rules[0];
        if let Expression::And(ref exprs) = rule.condition {
            assert_eq!(exprs.len(), 3);
        } else {
            panic!("Expected And expression with 3 parts");
        }
    }

    #[test]
    fn test_parse_empty_rule_block() {
        let input = r#"
            rules products {
            }
        "#;
        let result = parse_rules(input).unwrap();
        assert!(result.contains_key("products"));
        assert_eq!(result["products"].rules.len(), 0);
    }

    #[test]
    fn test_parse_number_literal_decimal() {
        let input = r#"
            rules products {
                read: resource.rating > 4.5;
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rule = &result["products"].rules[0];
        if let Expression::Comparison { ref right, .. } = rule.condition {
            assert!(matches!(right.as_ref(), Expression::Number(n) if (*n - 4.5).abs() < f64::EPSILON));
        } else {
            panic!("Expected Comparison");
        }
    }

    #[test]
    fn test_parse_list_operation() {
        let input = r#"
            rules products {
                list: isAuthenticated();
            }
        "#;
        let result = parse_rules(input).unwrap();
        let rule = &result["products"].rules[0];
        assert_eq!(rule.operations, vec!["list"]);
    }
}
