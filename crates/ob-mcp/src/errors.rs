//! MCP error handling — JSON-RPC 2.0 compatible error responses

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Method not found: {0}")]
    MethodNotFound(String),

    #[error("Invalid params: {0}")]
    InvalidParams(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Rate limited")]
    RateLimited,

    #[error("Idempotency key mismatch: {0}")]
    IdempotencyMismatch(String),

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Validation error: {0}")]
    ValidationError(String),
}

impl McpError {
    /// Convert to JSON-RPC 2.0 error code
    pub fn code(&self) -> i32 {
        match self {
            McpError::InvalidRequest(_) => -32600,
            McpError::MethodNotFound(_) => -32601,
            McpError::InvalidParams(_) => -32602,
            McpError::Internal(_) => -32603,
            McpError::Unauthorized => 401,
            McpError::Forbidden(_) => 403,
            McpError::NotFound(_) => 404,
            McpError::RateLimited => 429,
            McpError::IdempotencyMismatch(_) => 409,
            McpError::DatabaseError(_) => 500,
            McpError::ValidationError(_) => 422,
        }
    }

    /// Sanitize error message (no stack traces, no DB details, no resource IDs)
    pub fn message(&self) -> String {
        match self {
            McpError::Internal(_) => "Internal server error".to_string(),
            McpError::DatabaseError(_) => "Database operation failed".to_string(),
            McpError::NotFound(id) => {
                tracing::debug!("Resource not found: {}", id);
                "Resource not found".to_string()
            }
            McpError::Forbidden(_) => "Forbidden".to_string(),
            _ => self.to_string(),
        }
    }
}

/// JSON-RPC 2.0 error response
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl From<McpError> for JsonRpcError {
    fn from(err: McpError) -> Self {
        JsonRpcError {
            code: err.code(),
            message: err.message(),
            data: None,
        }
    }
}

pub type McpResult<T> = Result<T, McpError>;

#[cfg(test)]
mod tests {
    use super::*;

    // ── McpError::code() ──

    #[test]
    fn test_error_code_invalid_request() {
        assert_eq!(McpError::InvalidRequest("bad".into()).code(), -32600);
    }

    #[test]
    fn test_error_code_method_not_found() {
        assert_eq!(McpError::MethodNotFound("foo".into()).code(), -32601);
    }

    #[test]
    fn test_error_code_invalid_params() {
        assert_eq!(McpError::InvalidParams("x".into()).code(), -32602);
    }

    #[test]
    fn test_error_code_internal() {
        assert_eq!(McpError::Internal("oops".into()).code(), -32603);
    }

    #[test]
    fn test_error_code_unauthorized() {
        assert_eq!(McpError::Unauthorized.code(), 401);
    }

    #[test]
    fn test_error_code_forbidden() {
        assert_eq!(McpError::Forbidden("test".into()).code(), 403);
    }

    #[test]
    fn test_error_code_not_found() {
        assert_eq!(McpError::NotFound("x".into()).code(), 404);
    }

    #[test]
    fn test_error_code_rate_limited() {
        assert_eq!(McpError::RateLimited.code(), 429);
    }

    #[test]
    fn test_error_code_idempotency_mismatch() {
        assert_eq!(McpError::IdempotencyMismatch("key".into()).code(), 409);
    }

    #[test]
    fn test_error_code_database_error() {
        assert_eq!(McpError::DatabaseError("db".into()).code(), 500);
    }

    #[test]
    fn test_error_code_validation_error() {
        assert_eq!(McpError::ValidationError("bad".into()).code(), 422);
    }

    // ── McpError::message() ──

    #[test]
    fn test_message_internal_sanitized() {
        let err = McpError::Internal("secret stack trace details".into());
        assert_eq!(err.message(), "Internal server error");
    }

    #[test]
    fn test_message_database_sanitized() {
        let err = McpError::DatabaseError("SELECT * FROM secrets".into());
        assert_eq!(err.message(), "Database operation failed");
    }

    #[test]
    fn test_message_unauthorized() {
        let err = McpError::Unauthorized;
        assert_eq!(err.message(), "Unauthorized");
    }

    #[test]
    fn test_message_forbidden() {
        assert_eq!(
            McpError::Forbidden("secret reason".into()).message(),
            "Forbidden"
        );
    }

    #[test]
    fn test_message_not_found() {
        // NotFound message is sanitized — no resource ID leaked
        assert_eq!(
            McpError::NotFound("order:123".into()).message(),
            "Resource not found"
        );
    }

    #[test]
    fn test_message_rate_limited() {
        assert_eq!(McpError::RateLimited.message(), "Rate limited");
    }

    #[test]
    fn test_message_validation_error() {
        assert_eq!(
            McpError::ValidationError("Rating must be 1-5".into()).message(),
            "Validation error: Rating must be 1-5"
        );
    }

    #[test]
    fn test_message_invalid_params() {
        assert_eq!(
            McpError::InvalidParams("Missing 'x'".into()).message(),
            "Invalid params: Missing 'x'"
        );
    }

    #[test]
    fn test_message_idempotency_mismatch() {
        assert_eq!(
            McpError::IdempotencyMismatch("key1".into()).message(),
            "Idempotency key mismatch: key1"
        );
    }

    // ── JsonRpcError conversion ──

    #[test]
    fn test_jsonrpc_error_from_mcp_error() {
        let mcp_err = McpError::NotFound("product:42".into());
        let rpc_err: JsonRpcError = mcp_err.into();
        assert_eq!(rpc_err.code, 404);
        assert_eq!(rpc_err.message, "Resource not found");
        assert!(rpc_err.data.is_none());
    }

    #[test]
    fn test_jsonrpc_error_from_internal_sanitized() {
        let mcp_err = McpError::Internal("DB connection pool exhausted at line 42".into());
        let rpc_err: JsonRpcError = mcp_err.into();
        assert_eq!(rpc_err.code, -32603);
        assert_eq!(rpc_err.message, "Internal server error");
    }

    #[test]
    fn test_jsonrpc_error_from_method_not_found() {
        let mcp_err = McpError::MethodNotFound("unknown_method".into());
        let rpc_err: JsonRpcError = mcp_err.into();
        assert_eq!(rpc_err.code, -32601);
        assert_eq!(rpc_err.message, "Method not found: unknown_method");
    }

    // ── Display / Error trait ──

    #[test]
    fn test_error_display() {
        assert_eq!(McpError::Unauthorized.to_string(), "Unauthorized");
        assert_eq!(
            McpError::Forbidden("access denied".into()).to_string(),
            "Forbidden: access denied"
        );
        assert_eq!(
            McpError::InvalidRequest("no jsonrpc field".into()).to_string(),
            "Invalid request: no jsonrpc field"
        );
    }

    #[test]
    fn test_all_variants_have_unique_codes() {
        let codes = vec![
            McpError::InvalidRequest("".into()).code(),
            McpError::MethodNotFound("".into()).code(),
            McpError::InvalidParams("".into()).code(),
            McpError::Internal("".into()).code(),
            McpError::Unauthorized.code(),
            McpError::Forbidden("".into()).code(),
            McpError::NotFound("".into()).code(),
            McpError::RateLimited.code(),
            McpError::IdempotencyMismatch("".into()).code(),
            McpError::DatabaseError("".into()).code(),
            McpError::ValidationError("".into()).code(),
        ];
        let unique: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(
            unique.len(),
            codes.len(),
            "All error codes should be unique"
        );
    }
}
