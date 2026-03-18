use axum::{extract::Request, http::header::AUTHORIZATION, middleware::Next, response::Response};
use ob_core::Error;
use std::sync::Arc;

use crate::jwt::{Claims, JwtKeys, verify_token};

/// Extracted auth context available to handlers.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user_id: String,
    pub roles: Vec<String>,
    pub authenticated: bool,
    pub email_verified: bool,
    /// Custom claims set by admin (e.g. `{"role": "seller", "store_id": "abc"}`).
    /// Available in security rules and handler logic.
    pub custom_claims: serde_json::Value,
}

impl AuthContext {
    pub fn anonymous() -> Self {
        Self {
            user_id: String::new(),
            roles: vec![],
            authenticated: false,
            email_verified: false,
            custom_claims: serde_json::Value::Null,
        }
    }

    pub fn from_claims(claims: Claims) -> Self {
        Self {
            user_id: claims.sub,
            roles: claims.roles,
            authenticated: true,
            email_verified: claims.email_verified,
            custom_claims: claims.custom_claims,
        }
    }

    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }
}

/// Middleware that extracts JWT from the Authorization header.
/// CRITICAL FIX: If Authorization header present but JWT invalid, return 401.
/// Anonymous requests (no Authorization header) are allowed — security rules enforce access control.
pub async fn auth_extractor(mut request: Request, next: Next) -> Result<Response, Error> {
    let jwt_keys = request.extensions().get::<Arc<JwtKeys>>().cloned();

    let auth_context = if let Some(auth_header) = request.headers().get(AUTHORIZATION) {
        let header_str = auth_header
            .to_str()
            .map_err(|_| Error::Auth("Invalid Authorization header".into()))?;

        if let Some(token) = header_str.strip_prefix("Bearer ") {
            if let Some(keys) = &jwt_keys {
                match verify_token(token, keys) {
                    Ok(claims) if claims.typ == "access" => AuthContext::from_claims(claims),
                    Ok(_) => return Err(Error::Auth("Invalid token type".into())),
                    Err(e) => {
                        // CRITICAL FIX: Authorization header present but JWT invalid → 401
                        // Do NOT silently become anonymous
                        return Err(Error::Auth(format!("Invalid or expired token: {e}")));
                    }
                }
            } else {
                // JWT keys not available, but Authorization header was present
                // Return error instead of silently becoming anonymous
                return Err(Error::Auth("JWT validation keys not configured".into()));
            }
        } else {
            // Authorization header present but doesn't start with "Bearer "
            return Err(Error::Auth("Invalid Authorization header format".into()));
        }
    } else {
        // No Authorization header → anonymous is OK
        AuthContext::anonymous()
    };

    request.extensions_mut().insert(auth_context);
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── AuthContext::anonymous ───────────────────────────────────────

    #[test]
    fn test_anonymous_context() {
        let ctx = AuthContext::anonymous();
        assert_eq!(ctx.user_id, "");
        assert!(ctx.roles.is_empty());
        assert!(!ctx.authenticated);
    }

    // ── AuthContext::from_claims ─────────────────────────────────────

    #[test]
    fn test_from_claims_basic() {
        let claims = Claims {
            sub: "user42".into(),
            iat: 0,
            exp: 9999999999,
            roles: vec!["admin".into(), "user".into()],
            typ: "access".into(),
            email_verified: true,
            mfa_required: false,
            custom_claims: serde_json::Value::Null,
        };
        let ctx = AuthContext::from_claims(claims);
        assert_eq!(ctx.user_id, "user42");
        assert!(ctx.authenticated);
        assert_eq!(ctx.roles, vec!["admin", "user"]);
    }

    #[test]
    fn test_from_claims_empty_roles() {
        let claims = Claims {
            sub: "u1".into(),
            iat: 0,
            exp: 0,
            roles: vec![],
            typ: "access".into(),
            email_verified: false,
            mfa_required: false,
            custom_claims: serde_json::Value::Null,
        };
        let ctx = AuthContext::from_claims(claims);
        assert!(ctx.roles.is_empty());
        assert!(ctx.authenticated);
    }

    // ── AuthContext::has_role ────────────────────────────────────────

    #[test]
    fn test_has_role_present() {
        let ctx = AuthContext {
            user_id: "u".into(),
            roles: vec!["admin".into(), "editor".into()],
            authenticated: true,
            email_verified: false,
            custom_claims: serde_json::Value::Null,
        };
        assert!(ctx.has_role("admin"));
        assert!(ctx.has_role("editor"));
    }

    #[test]
    fn test_has_role_absent() {
        let ctx = AuthContext {
            user_id: "u".into(),
            roles: vec!["user".into()],
            authenticated: true,
            email_verified: false,
            custom_claims: serde_json::Value::Null,
        };
        assert!(!ctx.has_role("admin"));
    }

    #[test]
    fn test_has_role_empty_roles() {
        let ctx = AuthContext::anonymous();
        assert!(!ctx.has_role("anything"));
    }

    // ── Clone + Debug ───────────────────────────────────────────────

    #[test]
    fn test_auth_context_clone() {
        let ctx = AuthContext {
            user_id: "u1".into(),
            roles: vec!["user".into()],
            authenticated: true,
            email_verified: true,
            custom_claims: serde_json::Value::Null,
        };
        let cloned = ctx.clone();
        assert_eq!(cloned.user_id, "u1");
        assert_eq!(cloned.roles, ctx.roles);
        assert_eq!(cloned.authenticated, ctx.authenticated);
        assert_eq!(cloned.email_verified, ctx.email_verified);
    }

    #[test]
    fn test_auth_context_debug() {
        let ctx = AuthContext::anonymous();
        let debug = format!("{:?}", ctx);
        assert!(debug.contains("AuthContext"));
    }
}
