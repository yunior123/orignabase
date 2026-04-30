//! Safety mechanisms for MCP tools — idempotency, spend limits, confirmations

use crate::errors::{McpError, McpResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Idempotency key tracking — prevents duplicate operations
#[derive(Clone)]
pub struct IdempotencyTracker {
    // Map of idempotency_key -> last_response
    cache: Arc<RwLock<HashMap<String, serde_json::Value>>>,
}

impl IdempotencyTracker {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Check if operation was already processed
    pub async fn check(&self, key: &str) -> Option<serde_json::Value> {
        self.cache.read().await.get(key).cloned()
    }

    /// Mark operation as processed with result
    pub async fn mark(&self, key: String, result: serde_json::Value) {
        self.cache.write().await.insert(key, result);
    }

    /// Clear stale entries (in production, use TTL-based eviction)
    pub async fn clear_old(&self) {
        // Placeholder: actual implementation would check entry timestamps
        // and evict entries older than 24 hours
    }
}

impl Default for IdempotencyTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Spend limit — prevents runaway checkouts via MCP
#[derive(Debug, Clone)]
pub struct SpendLimit {
    /// Maximum amount in integer cents per request
    pub max_amount_cents: u64,

    /// Maximum total in integer cents per user per 24h
    pub max_per_24h_cents: u64,

    /// Track spend per user
    user_spend: Arc<RwLock<HashMap<String, u64>>>,
}

impl SpendLimit {
    pub fn new(max_amount_cents: u64, max_per_24h_cents: u64) -> Self {
        Self {
            max_amount_cents,
            max_per_24h_cents,
            user_spend: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Check if user can spend amount_cents
    pub async fn check(&self, user_id: &str, amount_cents: u64) -> McpResult<()> {
        if amount_cents > self.max_amount_cents {
            return Err(McpError::ValidationError(format!(
                "Amount exceeds per-request limit of ${}",
                self.max_amount_cents / 100
            )));
        }

        let spend = self.user_spend.read().await;
        let current = spend.get(user_id).copied().unwrap_or(0);
        if current + amount_cents > self.max_per_24h_cents {
            return Err(McpError::ValidationError(
                "Amount exceeds 24h spend limit".to_string(),
            ));
        }

        Ok(())
    }

    /// Record spend
    pub async fn record(&self, user_id: String, amount_cents: u64) {
        let mut spend = self.user_spend.write().await;
        *spend.entry(user_id).or_insert(0) += amount_cents;
    }
}

/// Confirmation token — for sensitive operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmationToken {
    pub token: String,
    pub operation: String,
    pub expires_at: i64,
}

impl ConfirmationToken {
    pub fn new(operation: String) -> Self {
        Self {
            token: uuid::Uuid::new_v4().to_string(),
            operation,
            expires_at: chrono::Utc::now().timestamp() + 3600, // 1 hour
        }
    }

    pub fn is_valid(&self) -> bool {
        chrono::Utc::now().timestamp() < self.expires_at
    }

    pub fn verify(&self, provided_token: &str) -> McpResult<()> {
        if !self.is_valid() {
            return Err(McpError::ValidationError("Token expired".to_string()));
        }
        if self.token != provided_token {
            return Err(McpError::ValidationError(
                "Invalid confirmation token".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_idempotency() {
        let tracker = IdempotencyTracker::new();
        let key = "test-key";
        let result = serde_json::json!({"ok": true});

        // First call should be None
        assert!(tracker.check(key).await.is_none());

        // Mark as processed
        tracker.mark(key.to_string(), result.clone()).await;

        // Second call should return same result
        assert_eq!(tracker.check(key).await, Some(result));
    }

    #[tokio::test]
    async fn test_spend_limit() {
        let limit = SpendLimit::new(100_000, 1_000_000); // $1000 max, $10000 per day
        let user = "user:test";

        // Within limit
        assert!(limit.check(user, 50_000).await.is_ok());
        limit.record(user.to_string(), 50_000).await;

        // Exceed single request limit
        assert!(limit.check(user, 150_000).await.is_err());

        // Within limit still
        assert!(limit.check(user, 90_000).await.is_ok());
    }

    #[test]
    fn test_confirmation_token() {
        let token = ConfirmationToken::new("delete_account".to_string());
        assert!(token.is_valid());
        assert!(token.verify(&token.token).is_ok());
        assert!(token.verify("wrong").is_err());
    }
}
