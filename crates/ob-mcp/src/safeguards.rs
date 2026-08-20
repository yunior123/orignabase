//! Safety mechanisms for MCP tools — idempotency, spend limits, confirmations

use crate::errors::{McpError, McpResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Idempotency key tracking — prevents duplicate operations
#[derive(Clone)]
pub struct IdempotencyTracker {
    // Map of idempotency_key -> (response, timestamp)
    cache: Arc<RwLock<HashMap<String, (serde_json::Value, i64)>>>,
    /// Number of check() calls between automatic cleanup runs
    cleanup_interval: u64,
    /// Counter for check() calls since last cleanup
    check_count: Arc<RwLock<u64>>,
}

/// TTL for idempotency entries: 24 hours in seconds
const IDEMPOTENCY_TTL_SECS: i64 = 24 * 60 * 60;
/// Maximum number of entries before forced eviction
const MAX_ENTRIES: usize = 10_000;

impl IdempotencyTracker {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            cleanup_interval: 100,
            check_count: Arc::new(RwLock::new(0)),
        }
    }

    /// Check if operation was already processed.
    /// Runs periodic cleanup to evict expired entries.
    pub async fn check(&self, key: &str) -> Option<serde_json::Value> {
        // Periodic cleanup every N calls
        {
            let mut count = self.check_count.write().await;
            *count += 1;
            if *count >= self.cleanup_interval {
                *count = 0;
                drop(count);
                self.cleanup().await;
            }
        }

        let cache = self.cache.read().await;
        cache.get(key).and_then(|(value, ts)| {
            let now = chrono::Utc::now().timestamp();
            if now - ts < IDEMPOTENCY_TTL_SECS {
                Some(value.clone())
            } else {
                None // Expired — treat as not found
            }
        })
    }

    /// Mark operation as processed with result
    pub async fn mark(&self, key: String, result: serde_json::Value) {
        let now = chrono::Utc::now().timestamp();
        let mut cache = self.cache.write().await;

        // If exceeding max entries, evict oldest 50%
        if cache.len() >= MAX_ENTRIES {
            let mut entries: Vec<(String, i64)> =
                cache.iter().map(|(k, (_, ts))| (k.clone(), *ts)).collect();
            entries.sort_by_key(|(_, ts)| *ts);
            let evict_count = entries.len() / 2;
            for (k, _) in entries.into_iter().take(evict_count) {
                cache.remove(&k);
            }
            tracing::debug!(
                "IdempotencyTracker: evicted {} entries (capacity limit)",
                evict_count
            );
        }

        cache.insert(key, (result, now));
    }

    /// Remove entries older than 24 hours
    pub async fn cleanup(&self) {
        let now = chrono::Utc::now().timestamp();
        let mut cache = self.cache.write().await;
        let before = cache.len();
        cache.retain(|_, (_, ts)| now - *ts < IDEMPOTENCY_TTL_SECS);
        let removed = before - cache.len();
        if removed > 0 {
            tracing::debug!("IdempotencyTracker: cleaned up {} expired entries", removed);
        }
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

    /// Track spend per user: Vec of (amount_cents, timestamp_secs) entries
    #[allow(clippy::type_complexity)]
    user_spend: Arc<RwLock<HashMap<String, Vec<(u64, i64)>>>>,

    /// Counter for periodic full cleanup (same pattern as IdempotencyTracker)
    check_count: Arc<RwLock<u64>>,
}

/// TTL for spend entries: 24 hours in seconds
const SPEND_TTL_SECS: i64 = 86_400;
/// Maximum users tracked before forced eviction
const SPEND_MAX_USERS: usize = 10_000;
/// Run full cleanup every N check() calls
const SPEND_CLEANUP_INTERVAL: u64 = 100;

impl SpendLimit {
    pub fn new(max_amount_cents: u64, max_per_24h_cents: u64) -> Self {
        Self {
            max_amount_cents,
            max_per_24h_cents,
            user_spend: Arc::new(RwLock::new(HashMap::new())),
            check_count: Arc::new(RwLock::new(0)),
        }
    }

    /// Prune entries older than the 24h window and return current total
    fn prune_and_sum(entries: &mut Vec<(u64, i64)>, now: i64) -> u64 {
        entries.retain(|(_, ts)| now - *ts < SPEND_TTL_SECS);
        entries.iter().map(|(amt, _)| amt).sum()
    }

    /// Remove all expired entries and users with no remaining entries.
    /// Called periodically to prevent unbounded memory growth from idle users.
    pub async fn cleanup(&self) {
        let now = chrono::Utc::now().timestamp();
        let mut spend = self.user_spend.write().await;
        let before = spend.len();

        // Prune expired entries per user, then remove users with empty vecs
        spend.retain(|_, entries| {
            entries.retain(|(_, ts)| now - *ts < SPEND_TTL_SECS);
            !entries.is_empty()
        });

        // If still over capacity, evict users with oldest last-activity
        if spend.len() > SPEND_MAX_USERS {
            let mut users_by_age: Vec<(String, i64)> = spend
                .iter()
                .map(|(uid, entries)| {
                    let newest = entries.iter().map(|(_, ts)| *ts).max().unwrap_or(0);
                    (uid.clone(), newest)
                })
                .collect();
            users_by_age.sort_by_key(|(_, ts)| *ts);
            let evict_count = spend.len() - SPEND_MAX_USERS / 2;
            for (uid, _) in users_by_age.into_iter().take(evict_count) {
                spend.remove(&uid);
            }
            tracing::debug!("SpendLimit: evicted {} users (capacity limit)", evict_count);
        }

        let removed = before - spend.len();
        if removed > 0 {
            tracing::debug!("SpendLimit: cleaned up {} idle user entries", removed);
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

        // Periodic cleanup every N calls
        {
            let mut count = self.check_count.write().await;
            *count += 1;
            if *count >= SPEND_CLEANUP_INTERVAL {
                *count = 0;
                drop(count);
                self.cleanup().await;
            }
        }

        let now = chrono::Utc::now().timestamp();
        let mut spend = self.user_spend.write().await;
        let entries = spend.entry(user_id.to_string()).or_default();
        let current = Self::prune_and_sum(entries, now);

        if current + amount_cents > self.max_per_24h_cents {
            return Err(McpError::ValidationError(
                "Amount exceeds 24h spend limit".to_string(),
            ));
        }

        Ok(())
    }

    /// Record spend
    pub async fn record(&self, user_id: String, amount_cents: u64) {
        let now = chrono::Utc::now().timestamp();
        let mut spend = self.user_spend.write().await;
        let entries = spend.entry(user_id).or_default();
        // Prune stale entries on every write to bound memory
        Self::prune_and_sum(entries, now);
        entries.push((amount_cents, now));
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
    async fn test_idempotency_cleanup_removes_expired() {
        let tracker = IdempotencyTracker::new();

        // Insert an entry with a fake old timestamp
        {
            let mut cache = tracker.cache.write().await;
            let old_ts = chrono::Utc::now().timestamp() - IDEMPOTENCY_TTL_SECS - 1;
            cache.insert(
                "old-key".to_string(),
                (serde_json::json!({"old": true}), old_ts),
            );
        }

        // Cleanup should remove expired entry
        tracker.cleanup().await;
        assert!(tracker.check("old-key").await.is_none());
    }

    #[tokio::test]
    async fn test_idempotency_expired_entry_returns_none() {
        let tracker = IdempotencyTracker::new();

        // Insert an entry with expired timestamp
        {
            let mut cache = tracker.cache.write().await;
            let old_ts = chrono::Utc::now().timestamp() - IDEMPOTENCY_TTL_SECS - 1;
            cache.insert(
                "expired".to_string(),
                (serde_json::json!({"expired": true}), old_ts),
            );
        }

        // Should return None for expired entry
        assert!(tracker.check("expired").await.is_none());
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

    #[tokio::test]
    async fn test_spend_limit_cleanup_removes_expired_users() {
        let limit = SpendLimit::new(100_000, 1_000_000);
        let old_ts = chrono::Utc::now().timestamp() - SPEND_TTL_SECS - 1;

        // Insert an expired entry directly
        {
            let mut spend = limit.user_spend.write().await;
            spend.insert("stale_user".to_string(), vec![(5000, old_ts)]);
            spend.insert(
                "active_user".to_string(),
                vec![(5000, chrono::Utc::now().timestamp())],
            );
        }

        limit.cleanup().await;

        let spend = limit.user_spend.read().await;
        assert!(
            !spend.contains_key("stale_user"),
            "expired user should be evicted"
        );
        assert!(
            spend.contains_key("active_user"),
            "active user should remain"
        );
    }

    #[tokio::test]
    async fn test_spend_limit_periodic_cleanup_triggers() {
        let limit = SpendLimit::new(100_000, 1_000_000);
        let old_ts = chrono::Utc::now().timestamp() - SPEND_TTL_SECS - 1;

        // Insert expired entry
        {
            let mut spend = limit.user_spend.write().await;
            spend.insert("old_user".to_string(), vec![(1000, old_ts)]);
        }

        // Call check() SPEND_CLEANUP_INTERVAL times to trigger cleanup
        for i in 0..SPEND_CLEANUP_INTERVAL {
            let _ = limit.check(&format!("user_{i}"), 100).await;
        }

        let spend = limit.user_spend.read().await;
        assert!(
            !spend.contains_key("old_user"),
            "cleanup should have evicted expired user"
        );
    }

    #[test]
    fn test_confirmation_token() {
        let token = ConfirmationToken::new("delete_account".to_string());
        assert!(token.is_valid());
        assert!(token.verify(&token.token).is_ok());
        assert!(token.verify("wrong").is_err());
    }
}
