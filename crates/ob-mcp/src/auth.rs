//! MCP authentication middleware — reuses ob-auth JWT verification

use crate::errors::{McpError, McpResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Extracted JWT claims from Authorization header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpClaims {
    pub sub: String,           // user ID in format "users:xxx"
    pub uid: String,           // short user ID "xxx"
    pub role: Option<String>,  // "admin", "seller", "buyer"
    pub iat: i64,              // issued at
    pub exp: i64,              // expiration
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

/// Extract JWT claims from Authorization header
pub fn extract_claims(auth_header: Option<&str>) -> McpResult<McpClaims> {
    let header = auth_header.ok_or(McpError::Unauthorized)?;

    let bearer = header
        .strip_prefix("Bearer ")
        .ok_or(McpError::Unauthorized)?;

    // NOTE: In actual implementation, this would call ob-auth::jwt::verify_token()
    // For now, we stub the parsing. Full JWT verification happens in the transport layer.
    parse_jwt_claims(bearer)
}

/// Stub JWT parsing (actual verification done by ob-auth middleware)
fn parse_jwt_claims(token: &str) -> McpResult<McpClaims> {
    // This is a placeholder. Real implementation would:
    // 1. Split token into header.payload.signature
    // 2. Decode payload as base64-json
    // 3. Verify signature against pub key from ob-auth
    //
    // For now, reject if token is empty
    if token.is_empty() {
        return Err(McpError::Unauthorized);
    }

    // Placeholder: assume valid JWT, extract claims
    // In production, use jsonwebtoken crate + ob-auth public key
    Ok(McpClaims {
        sub: "users:placeholder".to_string(),
        uid: "placeholder".to_string(),
        role: Some("buyer".to_string()),
        iat: 0,
        exp: i64::MAX,
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
            return Err(McpError::Forbidden);
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
