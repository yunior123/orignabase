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

    #[error("Forbidden")]
    Forbidden,

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
            McpError::Forbidden => 403,
            McpError::NotFound(_) => 404,
            McpError::RateLimited => 429,
            McpError::IdempotencyMismatch(_) => 409,
            McpError::DatabaseError(_) => 500,
            McpError::ValidationError(_) => 422,
        }
    }

    /// Sanitize error message (no stack traces, no DB details)
    pub fn message(&self) -> String {
        match self {
            McpError::Internal(_) => "Internal server error".to_string(),
            McpError::DatabaseError(_) => "Database operation failed".to_string(),
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
