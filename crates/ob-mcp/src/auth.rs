//! MCP authentication middleware — reuses ob-auth JWT verification

use crate::errors::{McpError, McpResult};
use ob_auth::jwt::{JwtKeys, verify_token};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Extracted JWT claims from Authorization header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpClaims {
    pub sub: String,          // user ID in format "users:xxx"
    pub uid: String,          // short user ID "xxx"
    pub role: Option<String>, // "admin", "seller", "buyer"
    pub iat: i64,             // issued at
    pub exp: i64,             // expiration
}

impl McpClaims {
    /// Check if user has required role
    pub fn has_role(&self, required_role: &str) -> bool {
        self.role.as_deref() == Some(required_role)
    }

    /// Check if user is an admin
    pub fn is_admin(&self) -> bool {
        self.has_role("admin")
    }

    /// Check if user is a seller
    pub fn is_seller(&self) -> bool {
        self.has_role("seller")
    }

    /// Verify user owns a resource (used for order/profile access control)
    pub fn owns_resource(&self, owner_id: &str) -> bool {
        // owner_id might be full "users:xxx" or short "xxx"
        self.sub == owner_id || self.uid == owner_id
    }
}

/// Extract JWT claims from Authorization header, verifying via ob-auth
pub fn extract_claims(auth_header: Option<&str>, jwt_keys: &JwtKeys) -> McpResult<McpClaims> {
    let header = auth_header.ok_or(McpError::Unauthorized)?;

    let bearer = header
        .strip_prefix("Bearer ")
        .ok_or(McpError::Unauthorized)?;

    if bearer.is_empty() {
        return Err(McpError::Unauthorized);
    }

    parse_jwt_claims(bearer, jwt_keys)
}

/// Verify JWT and map ob-auth Claims to McpClaims
fn parse_jwt_claims(token: &str, jwt_keys: &JwtKeys) -> McpResult<McpClaims> {
    let claims = verify_token(token, jwt_keys).map_err(|_| McpError::Unauthorized)?;

    // Reject non-access tokens (refresh, email_verify, password_reset, etc.)
    if claims.typ != "access" {
        return Err(McpError::Unauthorized);
    }

    // Strip "users:" prefix for short uid
    let uid = claims
        .sub
        .strip_prefix("users:")
        .unwrap_or(&claims.sub)
        .to_string();

    // Use highest-privilege role rather than just the first one.
    // Priority: admin > seller > buyer > anything else.
    let role = {
        let roles = &claims.roles;
        if roles.iter().any(|r| r == "admin") {
            Some("admin".to_string())
        } else if roles.iter().any(|r| r == "seller") {
            Some("seller".to_string())
        } else if roles.iter().any(|r| r == "buyer") {
            Some("buyer".to_string())
        } else {
            roles.first().cloned()
        }
    };

    Ok(McpClaims {
        sub: claims.sub,
        uid,
        role,
        iat: claims.iat,
        exp: claims.exp,
    })
}

/// Request context with optional authenticated user
#[derive(Debug, Clone)]
pub struct McpContext {
    pub claims: Option<McpClaims>,
    pub request_id: String,
    pub metadata: HashMap<String, String>,
}

impl McpContext {
    /// Create new unauthenticated context
    pub fn new() -> Self {
        Self {
            claims: None,
            request_id: uuid::Uuid::new_v4().to_string(),
            metadata: HashMap::new(),
        }
    }

    /// Create context with claims
    pub fn with_claims(claims: McpClaims) -> Self {
        Self {
            claims: Some(claims),
            request_id: uuid::Uuid::new_v4().to_string(),
            metadata: HashMap::new(),
        }
    }

    /// Get user ID if authenticated
    pub fn user_id(&self) -> McpResult<String> {
        self.claims
            .as_ref()
            .map(|c| c.sub.clone())
            .ok_or(McpError::Unauthorized)
    }

    /// Require specific role
    pub fn require_role(&self, role: &str) -> McpResult<()> {
        let claims = self.claims.as_ref().ok_or(McpError::Unauthorized)?;
        if !claims.has_role(role) {
            return Err(McpError::Forbidden(format!("Required role: {}", role)));
        }
        Ok(())
    }

    /// Require admin role
    pub fn require_admin(&self) -> McpResult<()> {
        self.require_role("admin")
    }

    /// Require seller role
    pub fn require_seller(&self) -> McpResult<()> {
        self.require_role("seller")
    }
}

impl Default for McpContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ob_auth::jwt::{JwtKeys, issue_access_token, issue_refresh_token};

    const TEST_SECRET: &str = "test-secret-for-mcp-unit-tests";

    fn test_keys() -> JwtKeys {
        JwtKeys::from_secret(TEST_SECRET)
    }

    // ── McpClaims::has_role ──

    fn make_claims(role: Option<&str>) -> McpClaims {
        McpClaims {
            sub: "users:u1".into(),
            uid: "u1".into(),
            role: role.map(String::from),
            iat: 0,
            exp: i64::MAX,
        }
    }

    #[test]
    fn test_has_role_admin() {
        let c = make_claims(Some("admin"));
        assert!(c.has_role("admin"));
        assert!(!c.has_role("seller"));
        assert!(!c.has_role("buyer"));
    }

    #[test]
    fn test_has_role_none() {
        let c = make_claims(None);
        assert!(!c.has_role("admin"));
        assert!(!c.has_role("seller"));
        assert!(!c.has_role("buyer"));
    }

    #[test]
    fn test_has_role_empty_string() {
        let c = make_claims(Some(""));
        assert!(!c.has_role("admin"));
        assert!(c.has_role(""));
    }

    // ── McpClaims::is_admin / is_seller ──

    #[test]
    fn test_is_admin_true() {
        assert!(make_claims(Some("admin")).is_admin());
    }

    #[test]
    fn test_is_admin_false() {
        assert!(!make_claims(Some("buyer")).is_admin());
        assert!(!make_claims(None).is_admin());
    }

    #[test]
    fn test_is_seller_true() {
        assert!(make_claims(Some("seller")).is_seller());
    }

    #[test]
    fn test_is_seller_false() {
        assert!(!make_claims(Some("admin")).is_seller());
        assert!(!make_claims(None).is_seller());
    }

    // ── McpClaims::owns_resource ──

    #[test]
    fn test_owns_resource_full_sub() {
        let c = make_claims(None);
        assert!(c.owns_resource("users:u1"));
    }

    #[test]
    fn test_owns_resource_short_uid() {
        let c = make_claims(None);
        assert!(c.owns_resource("u1"));
    }

    #[test]
    fn test_owns_resource_wrong_id() {
        let c = make_claims(None);
        assert!(!c.owns_resource("users:u2"));
        assert!(!c.owns_resource("u2"));
    }

    #[test]
    fn test_owns_resource_empty() {
        let c = make_claims(None);
        assert!(!c.owns_resource(""));
    }

    // ── extract_claims (real JWT verification) ──

    #[test]
    fn test_extract_claims_none_header() {
        let keys = test_keys();
        let result = extract_claims(None, &keys);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::Unauthorized));
    }

    #[test]
    fn test_extract_claims_no_bearer_prefix() {
        let keys = test_keys();
        let result = extract_claims(Some("Basic abc123"), &keys);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::Unauthorized));
    }

    #[test]
    fn test_extract_claims_empty_bearer() {
        let keys = test_keys();
        let result = extract_claims(Some("Bearer "), &keys);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_claims_valid() {
        let keys = test_keys();
        let token =
            issue_access_token("users:u1", &["buyer".to_string()], &keys, 3600, true).unwrap();

        let header = format!("Bearer {}", token);
        let result = extract_claims(Some(&header), &keys);
        assert!(result.is_ok());
        let claims = result.unwrap();
        assert_eq!(claims.sub, "users:u1");
        assert_eq!(claims.uid, "u1");
        assert_eq!(claims.role, Some("buyer".into()));
    }

    #[test]
    fn test_extract_claims_admin_role() {
        let keys = test_keys();
        let token =
            issue_access_token("users:admin1", &["admin".to_string()], &keys, 3600, true).unwrap();

        let header = format!("Bearer {}", token);
        let claims = extract_claims(Some(&header), &keys).unwrap();
        assert_eq!(claims.uid, "admin1");
        assert_eq!(claims.role, Some("admin".into()));
        assert!(claims.is_admin());
    }

    #[test]
    fn test_extract_claims_rejects_refresh_token() {
        let keys = test_keys();
        let token = issue_refresh_token("users:u1", &keys, 86400).unwrap();

        let header = format!("Bearer {}", token);
        let result = extract_claims(Some(&header), &keys);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::Unauthorized));
    }

    #[test]
    fn test_extract_claims_invalid_token() {
        let keys = test_keys();
        let result = extract_claims(Some("Bearer not.a.valid.jwt"), &keys);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::Unauthorized));
    }

    #[test]
    fn test_extract_claims_wrong_secret() {
        let sign_keys = JwtKeys::from_secret("signing-secret");
        let verify_keys = JwtKeys::from_secret("different-secret");

        let token =
            issue_access_token("users:u1", &["buyer".to_string()], &sign_keys, 3600, true).unwrap();

        let header = format!("Bearer {}", token);
        let result = extract_claims(Some(&header), &verify_keys);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::Unauthorized));
    }

    #[test]
    fn test_extract_claims_wrong_prefix_lowercase() {
        let keys = test_keys();
        let result = extract_claims(Some("bearer abc"), &keys);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_claims_empty_string_header() {
        let keys = test_keys();
        let result = extract_claims(Some(""), &keys);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_claims_uid_without_users_prefix() {
        let keys = test_keys();
        // sub without "users:" prefix — uid should equal sub
        let token = issue_access_token("custom_id_123", &["seller".to_string()], &keys, 3600, true)
            .unwrap();

        let header = format!("Bearer {}", token);
        let claims = extract_claims(Some(&header), &keys).unwrap();
        assert_eq!(claims.sub, "custom_id_123");
        assert_eq!(claims.uid, "custom_id_123");
        assert_eq!(claims.role, Some("seller".into()));
    }

    #[test]
    fn test_extract_claims_multiple_roles_takes_first() {
        let keys = test_keys();
        let token = issue_access_token(
            "users:u1",
            &["admin".to_string(), "seller".to_string()],
            &keys,
            3600,
            true,
        )
        .unwrap();

        let header = format!("Bearer {}", token);
        let claims = extract_claims(Some(&header), &keys).unwrap();
        assert_eq!(claims.role, Some("admin".into()));
    }

    #[test]
    fn test_extract_claims_no_roles() {
        let keys = test_keys();
        let token = issue_access_token("users:u1", &[], &keys, 3600, true).unwrap();

        let header = format!("Bearer {}", token);
        let claims = extract_claims(Some(&header), &keys).unwrap();
        assert_eq!(claims.role, None);
    }

    // ── McpContext::new ──

    #[test]
    fn test_context_new_unauthenticated() {
        let ctx = McpContext::new();
        assert!(ctx.claims.is_none());
        assert!(!ctx.request_id.is_empty());
        assert!(ctx.metadata.is_empty());
    }

    #[test]
    fn test_context_default() {
        let ctx = McpContext::default();
        assert!(ctx.claims.is_none());
    }

    #[test]
    fn test_context_with_claims() {
        let claims = make_claims(Some("admin"));
        let ctx = McpContext::with_claims(claims);
        assert!(ctx.claims.is_some());
        assert!(ctx.claims.unwrap().is_admin());
    }

    #[test]
    fn test_context_unique_request_ids() {
        let ctx1 = McpContext::new();
        let ctx2 = McpContext::new();
        assert_ne!(ctx1.request_id, ctx2.request_id);
    }

    // ── McpContext::user_id ──

    #[test]
    fn test_user_id_authenticated() {
        let ctx = McpContext::with_claims(make_claims(Some("buyer")));
        assert_eq!(ctx.user_id().unwrap(), "users:u1");
    }

    #[test]
    fn test_user_id_unauthenticated() {
        let ctx = McpContext::new();
        let result = ctx.user_id();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::Unauthorized));
    }

    // ── McpContext::require_role ──

    #[test]
    fn test_require_role_ok() {
        let ctx = McpContext::with_claims(make_claims(Some("admin")));
        assert!(ctx.require_role("admin").is_ok());
    }

    #[test]
    fn test_require_role_wrong_role() {
        let ctx = McpContext::with_claims(make_claims(Some("buyer")));
        let result = ctx.require_role("admin");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::Forbidden(_)));
    }

    #[test]
    fn test_require_role_no_claims() {
        let ctx = McpContext::new();
        let result = ctx.require_role("admin");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::Unauthorized));
    }

    // ── McpContext::require_admin / require_seller ──

    #[test]
    fn test_require_admin_ok() {
        let ctx = McpContext::with_claims(make_claims(Some("admin")));
        assert!(ctx.require_admin().is_ok());
    }

    #[test]
    fn test_require_admin_forbidden() {
        let ctx = McpContext::with_claims(make_claims(Some("seller")));
        assert!(matches!(ctx.require_admin(), Err(McpError::Forbidden(_))));
    }

    #[test]
    fn test_require_admin_no_claims() {
        let ctx = McpContext::new();
        assert!(matches!(ctx.require_admin(), Err(McpError::Unauthorized)));
    }

    #[test]
    fn test_require_seller_ok() {
        let ctx = McpContext::with_claims(make_claims(Some("seller")));
        assert!(ctx.require_seller().is_ok());
    }

    #[test]
    fn test_require_seller_forbidden() {
        let ctx = McpContext::with_claims(make_claims(Some("buyer")));
        assert!(matches!(ctx.require_seller(), Err(McpError::Forbidden(_))));
    }

    #[test]
    fn test_require_seller_no_claims() {
        let ctx = McpContext::new();
        assert!(matches!(ctx.require_seller(), Err(McpError::Unauthorized)));
    }

    // ── Serialization ──

    #[test]
    fn test_claims_serialization() {
        let claims = make_claims(Some("admin"));
        let json = serde_json::to_value(&claims).unwrap();
        assert_eq!(json["sub"], "users:u1");
        assert_eq!(json["uid"], "u1");
        assert_eq!(json["role"], "admin");
    }

    #[test]
    fn test_claims_deserialization() {
        let json = serde_json::json!({
            "sub": "users:u2",
            "uid": "u2",
            "role": "seller",
            "iat": 1000,
            "exp": 9999
        });
        let claims: McpClaims = serde_json::from_value(json).unwrap();
        assert_eq!(claims.sub, "users:u2");
        assert_eq!(claims.role, Some("seller".into()));
    }

    // ── McpContext metadata ──

    #[test]
    fn test_context_metadata() {
        let mut ctx = McpContext::new();
        ctx.metadata.insert("trace_id".into(), "abc123".into());
        assert_eq!(ctx.metadata.get("trace_id").unwrap(), "abc123");
    }
}
