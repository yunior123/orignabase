use ob_auth::middleware::AuthContext;

pub fn require_authenticated(auth: &AuthContext) -> ob_core::Result<&str> {
    if !auth.authenticated || auth.user_id.is_empty() {
        return Err(ob_core::Error::Auth("Authentication required".into()));
    }
    Ok(auth.user_id.as_str())
}

pub fn require_admin(auth: &AuthContext) -> ob_core::Result<&str> {
    let user_id = require_authenticated(auth)?;
    if !auth.has_role("admin") {
        return Err(ob_core::Error::Forbidden("Admin role required".into()));
    }
    Ok(user_id)
}

pub fn resolve_self_user_id(
    auth: &AuthContext,
    provided: Option<&str>,
    field_name: &str,
) -> ob_core::Result<String> {
    let user_id = require_authenticated(auth)?;
    if let Some(provided) = provided
        && !provided.is_empty()
        && provided != user_id
    {
        return Err(ob_core::Error::Forbidden(format!(
            "{field_name} must match the authenticated user"
        )));
    }
    Ok(user_id.to_string())
}

pub fn resolve_admin_user_id(
    auth: &AuthContext,
    provided: Option<&str>,
    field_name: &str,
) -> ob_core::Result<String> {
    let admin_id = require_admin(auth)?;
    if let Some(provided) = provided
        && !provided.is_empty()
        && provided != admin_id
    {
        return Err(ob_core::Error::Forbidden(format!(
            "{field_name} must match the authenticated admin"
        )));
    }
    Ok(admin_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ob_auth::middleware::AuthContext;

    fn auth_ctx(user_id: &str, roles: Vec<&str>, authenticated: bool) -> AuthContext {
        AuthContext {
            user_id: user_id.into(),
            roles: roles.into_iter().map(String::from).collect(),
            authenticated,
            email_verified: false,
            custom_claims: serde_json::Value::Null,
        }
    }

    // ── require_authenticated ──────────────────────────────────────

    #[test]
    fn test_require_authenticated_valid() {
        let auth = auth_ctx("user_42", vec!["user"], true);
        assert_eq!(require_authenticated(&auth).unwrap(), "user_42");
    }

    #[test]
    fn test_require_authenticated_not_flagged() {
        let auth = auth_ctx("user_42", vec!["user"], false);
        assert!(require_authenticated(&auth).is_err());
    }

    #[test]
    fn test_require_authenticated_empty_user_id() {
        let auth = auth_ctx("", vec![], true);
        assert!(require_authenticated(&auth).is_err());
    }

    #[test]
    fn test_require_authenticated_anonymous() {
        let auth = AuthContext::anonymous();
        assert!(require_authenticated(&auth).is_err());
    }

    // ── require_admin ──────────────────────────────────────────────

    #[test]
    fn test_require_admin_valid() {
        let auth = auth_ctx("admin_1", vec!["admin", "user"], true);
        assert_eq!(require_admin(&auth).unwrap(), "admin_1");
    }

    #[test]
    fn test_require_admin_not_admin_role() {
        let auth = auth_ctx("user_1", vec!["user"], true);
        assert!(require_admin(&auth).is_err());
    }

    #[test]
    fn test_require_admin_not_authenticated() {
        let auth = auth_ctx("admin_1", vec!["admin"], false);
        assert!(require_admin(&auth).is_err());
    }

    #[test]
    fn test_require_admin_empty_roles() {
        let auth = auth_ctx("u1", vec![], true);
        let err = require_admin(&auth).unwrap_err();
        assert!(matches!(err, ob_core::Error::Forbidden(_)));
    }

    // ── resolve_self_user_id ───────────────────────────────────────

    #[test]
    fn test_resolve_self_no_provided() {
        let auth = auth_ctx("u1", vec!["user"], true);
        assert_eq!(resolve_self_user_id(&auth, None, "buyerId").unwrap(), "u1");
    }

    #[test]
    fn test_resolve_self_matching_provided() {
        let auth = auth_ctx("u1", vec!["user"], true);
        assert_eq!(
            resolve_self_user_id(&auth, Some("u1"), "buyerId").unwrap(),
            "u1"
        );
    }

    #[test]
    fn test_resolve_self_empty_provided() {
        let auth = auth_ctx("u1", vec!["user"], true);
        assert_eq!(
            resolve_self_user_id(&auth, Some(""), "buyerId").unwrap(),
            "u1"
        );
    }

    #[test]
    fn test_resolve_self_mismatched_provided() {
        let auth = auth_ctx("u1", vec!["user"], true);
        let err = resolve_self_user_id(&auth, Some("u2"), "buyerId").unwrap_err();
        match err {
            ob_core::Error::Forbidden(msg) => {
                assert!(msg.contains("buyerId"));
            }
            _ => panic!("Expected Forbidden error"),
        }
    }

    #[test]
    fn test_resolve_self_not_authenticated() {
        let auth = AuthContext::anonymous();
        assert!(resolve_self_user_id(&auth, None, "field").is_err());
    }

    // ── resolve_admin_user_id ──────────────────────────────────────

    #[test]
    fn test_resolve_admin_no_provided() {
        let auth = auth_ctx("admin_1", vec!["admin"], true);
        assert_eq!(
            resolve_admin_user_id(&auth, None, "sellerId").unwrap(),
            "admin_1"
        );
    }

    #[test]
    fn test_resolve_admin_matching_provided() {
        let auth = auth_ctx("admin_1", vec!["admin"], true);
        assert_eq!(
            resolve_admin_user_id(&auth, Some("admin_1"), "sellerId").unwrap(),
            "admin_1"
        );
    }

    #[test]
    fn test_resolve_admin_mismatched_provided() {
        let auth = auth_ctx("admin_1", vec!["admin"], true);
        let err = resolve_admin_user_id(&auth, Some("other_admin"), "sellerId").unwrap_err();
        match err {
            ob_core::Error::Forbidden(msg) => {
                assert!(msg.contains("sellerId"));
            }
            _ => panic!("Expected Forbidden error"),
        }
    }

    #[test]
    fn test_resolve_admin_empty_provided() {
        let auth = auth_ctx("admin_1", vec!["admin"], true);
        assert_eq!(
            resolve_admin_user_id(&auth, Some(""), "sellerId").unwrap(),
            "admin_1"
        );
    }

    #[test]
    fn test_resolve_admin_not_admin_role() {
        let auth = auth_ctx("u1", vec!["user"], true);
        assert!(resolve_admin_user_id(&auth, None, "sellerId").is_err());
    }

    #[test]
    fn test_resolve_admin_not_authenticated() {
        let auth = AuthContext::anonymous();
        assert!(resolve_admin_user_id(&auth, None, "sellerId").is_err());
    }
}
