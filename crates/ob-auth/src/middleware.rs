use axum::{extract::Request, http::header::AUTHORIZATION, middleware::Next, response::Response};
use ob_core::Error;
use std::sync::Arc;
use tracing::warn;

use crate::jwt::{Claims, JwtKeys, verify_token};

/// Panics at startup if JWT secret is the default placeholder, empty, or too short in production.
pub fn assert_jwt_secret_configured(jwt_secret: &str) {
    let environment = std::env::var("ENVIRONMENT").unwrap_or_default();
    if environment == "production" {
        if jwt_secret.is_empty() {
            panic!(
                "FATAL: JWT secret is empty in production. Set OB_AUTH__JWT_SECRET to a strong random value (at least 32 bytes)."
            );
        }
        if jwt_secret == "CHANGE_ME_IN_PRODUCTION" {
            panic!(
                "FATAL: JWT secret is still the default 'CHANGE_ME_IN_PRODUCTION' in production. Set OB_AUTH__JWT_SECRET to a strong random value."
            );
        }
        if jwt_secret.len() < 32 {
            panic!(
                "FATAL: JWT secret is only {} bytes in production. Use at least 32 bytes for adequate security.",
                jwt_secret.len()
            );
        }
    }
}

/// Panics at startup if a live Stripe key is used in non-production.
pub fn assert_no_live_stripe_in_dev(stripe_key: &str) {
    let environment = std::env::var("ENVIRONMENT").unwrap_or_default();
    if environment != "production" && stripe_key.starts_with("sk_live_") {
        panic!(
            "FATAL: Live Stripe key (sk_live_) detected in {} environment. Use sk_test_ for non-production.",
            environment
        );
    }
}

/// Panics at startup if OB_TEST_MODE=1 in anything other than development or test.
/// Call this during server initialization to prevent accidental auth bypass.
pub fn assert_test_mode_not_in_production() {
    let test_mode = std::env::var("OB_TEST_MODE").unwrap_or_default() == "1";
    let environment = std::env::var("ENVIRONMENT").unwrap_or_default();
    if test_mode && environment != "development" && environment != "test" {
        panic!(
            "FATAL: OB_TEST_MODE=1 is only allowed in development or test environments. \
             Current environment: '{environment}'. This bypasses authentication and is a critical security risk."
        );
    }
}

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
    let test_mode = std::env::var("OB_TEST_MODE").unwrap_or_default() == "1";

    let auth_context = if let Some(auth_header) = request.headers().get(AUTHORIZATION) {
        let header_str = auth_header
            .to_str()
            .map_err(|_| Error::Auth("Invalid Authorization header".into()))?;

        if let Some(token) = header_str.strip_prefix("Bearer ") {
            if let Some(keys) = &jwt_keys {
                match verify_token(token, keys) {
                    Ok(claims) if claims.typ == "access" => AuthContext::from_claims(claims),
                    Ok(_) if test_mode => {
                        warn!(
                            "OB_TEST_MODE: bypassing auth for invalid token type — falling back to anonymous"
                        );
                        AuthContext::anonymous()
                    }
                    Ok(_) => return Err(Error::Auth("Invalid token type".into())),
                    Err(e) => {
                        if test_mode {
                            warn!(
                                "OB_TEST_MODE: bypassing auth for invalid JWT ({e}) — falling back to anonymous"
                            );
                            AuthContext::anonymous()
                        } else {
                            // CRITICAL FIX: Authorization header present but JWT invalid → 401
                            // Do NOT silently become anonymous.
                            // Log the full error server-side but return a generic message
                            // to avoid leaking JWT internals (algorithm, expiry details, etc.)
                            warn!(error = %e, "JWT verification failed");
                            return Err(Error::Auth("Invalid or expired token".into()));
                        }
                    }
                }
            } else {
                // JWT keys not available, but Authorization header was present
                // Return error instead of silently becoming anonymous
                if test_mode {
                    warn!("OB_TEST_MODE: JWT keys not configured — falling back to anonymous");
                    AuthContext::anonymous()
                } else {
                    return Err(Error::Auth("JWT validation keys not configured".into()));
                }
            }
        } else {
            // Authorization header present but doesn't start with "Bearer "
            if test_mode {
                warn!(
                    "OB_TEST_MODE: invalid Authorization header format — falling back to anonymous"
                );
                AuthContext::anonymous()
            } else {
                return Err(Error::Auth("Invalid Authorization header format".into()));
            }
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
