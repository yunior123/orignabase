//! Per-endpoint rate limiting using governor.

use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::shared::schema::collections;
use ob_database::DatabaseClient;

const TRUSTED_PROXY_IP: &str = "127.0.0.1";

/// A simple rate limiter for a specific endpoint.
pub struct EndpointRateLimiter {
    limiter: Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>,
}

impl EndpointRateLimiter {
    /// Create a rate limiter that allows `max_per_minute` requests per minute.
    pub fn new(max_per_minute: u32) -> Self {
        let quota = Quota::per_minute(NonZeroU32::new(max_per_minute).unwrap_or(NonZeroU32::MIN));
        Self {
            limiter: Arc::new(RateLimiter::direct(quota)),
        }
    }

    /// Check if a request is allowed. Returns `true` if allowed.
    pub fn check(&self) -> bool {
        self.limiter.check().is_ok()
    }
}

/// Extract client IP from request, respecting X-Forwarded-For only from trusted proxy.
/// Only trusts X-Forwarded-For from Caddy reverse proxy at 127.0.0.1.
/// For other sources, uses the peer/connection IP.
pub fn extract_client_ip(
    headers: &axum::http::HeaderMap,
    socket_addr: std::net::SocketAddr,
) -> String {
    let peer_ip = socket_addr.ip();

    // Only trust X-Forwarded-For from Caddy reverse proxy (127.0.0.1)
    if peer_ip.to_string() == TRUSTED_PROXY_IP
        && let Some(xff) = headers.get("x-forwarded-for")
        && let Ok(xff_str) = xff.to_str()
    {
        // Take first IP if multiple (CSV format)
        if let Some(first_ip) = xff_str.split(',').next() {
            let ip = first_ip.trim();
            // Validate it's a valid IP address
            if ip.parse::<std::net::IpAddr>().is_ok() {
                return ip.to_string();
            }
        }
    }

    // Not from trusted proxy or invalid header → use peer IP
    peer_ip.to_string()
}

/// Checks a database-backed rate limit by User ID and Action.
/// If the rate limit is exceeded, returns a Validation error.
/// Otherwise, records the request and returns Ok.
pub async fn check_user_rate_limit(
    db: &DatabaseClient,
    user_id: &str,
    action: &str,
    max_requests: u64,
    window_minutes: i64,
) -> Result<(), ob_core::Error> {
    // Use Unix timestamps (i64) instead of RFC3339 strings for reliable comparisons
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let window_start = now_secs - (window_minutes * 60);

    let query = format!(
        "SELECT count() FROM {} WHERE userId = $user_id AND action = $action AND createdAt >= $window_start GROUP ALL",
        collections::RATE_LIMITS
    );

    let results = db
        .query_bind_value(
            &query,
            serde_json::json!({
                "user_id": user_id,
                "action": action,
                "window_start": window_start,
            }),
        )
        .await?;

    let count = if let Some(first) = results.first() {
        first.get("count").and_then(|v| v.as_u64()).unwrap_or(0)
    } else {
        0
    };

    if count >= max_requests {
        return Err(ob_core::Error::Validation(
            "Rate limit exceeded. Please try again later.".into(),
        ));
    }

    let _ = db
        .create_document(
            collections::RATE_LIMITS,
            serde_json::json!({
                "userId": user_id,
                "action": action,
                "createdAt": now_secs,
            }),
        )
        .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_allows_within_limit() {
        let limiter = EndpointRateLimiter::new(10);
        assert!(limiter.check());
    }

    #[test]
    fn test_rate_limiter_exhaustion() {
        let limiter = EndpointRateLimiter::new(1);
        assert!(limiter.check()); // First request allowed
        assert!(!limiter.check()); // Second request denied immediately
    }

    #[test]
    fn test_rate_limiter_zero_limit() {
        let limiter = EndpointRateLimiter::new(0); // Should fallback to NonZeroU32::MIN (1)
        assert!(limiter.check());
        assert!(!limiter.check());
    }

    #[test]
    fn test_extract_client_ip_from_trusted_proxy() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            axum::http::HeaderValue::from_static("203.0.113.42, 198.51.100.5"),
        );
        let socket_addr = "127.0.0.1:8080".parse().unwrap();

        let ip = extract_client_ip(&headers, socket_addr);
        assert_eq!(ip, "203.0.113.42");
    }

    #[test]
    fn test_extract_client_ip_rejects_spoofed_header() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            axum::http::HeaderValue::from_static("203.0.113.42"),
        );
        // Request from non-trusted IP (not 127.0.0.1)
        let socket_addr = "203.0.113.99:54321".parse().unwrap();

        let ip = extract_client_ip(&headers, socket_addr);
        // Should use peer IP, not the spoofed header
        assert_eq!(ip, "203.0.113.99");
    }

    #[test]
    fn test_extract_client_ip_invalid_header() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            axum::http::HeaderValue::from_static("not-an-ip"),
        );
        let socket_addr = "127.0.0.1:8080".parse().unwrap();

        let ip = extract_client_ip(&headers, socket_addr);
        // Should fall back to peer IP when header is invalid
        assert_eq!(ip, "127.0.0.1");
    }

    #[test]
    fn test_extract_client_ip_missing_header() {
        let headers = axum::http::HeaderMap::new();
        let socket_addr = "127.0.0.1:8080".parse().unwrap();

        let ip = extract_client_ip(&headers, socket_addr);
        assert_eq!(ip, "127.0.0.1");
    }

    #[tokio::test]
    async fn test_database_rate_limiter() {
        let db = DatabaseClient::new_mem().await;
        let user_id = "user_123";
        let action = "test_action";

        // First 2 requests should be allowed
        assert!(
            check_user_rate_limit(&db, user_id, action, 2, 1)
                .await
                .is_ok()
        );
        assert!(
            check_user_rate_limit(&db, user_id, action, 2, 1)
                .await
                .is_ok()
        );

        // Third request should be blocked
        let result = check_user_rate_limit(&db, user_id, action, 2, 1).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Rate limit exceeded")
        );

        // Different user should be allowed
        assert!(
            check_user_rate_limit(&db, "user_456", action, 2, 1)
                .await
                .is_ok()
        );

        // Different action should be allowed
        assert!(
            check_user_rate_limit(&db, user_id, "other_action", 2, 1)
                .await
                .is_ok()
        );
    }

    // --- Ported from Python test_services_rate_limiter_deep.py ---

    #[test]
    fn test_rate_limiter_high_limit() {
        // High limit: first request always passes
        let limiter = EndpointRateLimiter::new(1000);
        assert!(limiter.check());
    }

    #[test]
    fn test_rate_limiter_exactly_at_limit() {
        // Limit of 2: first two pass, third blocked
        let limiter = EndpointRateLimiter::new(2);
        assert!(limiter.check()); // 1st
        assert!(limiter.check()); // 2nd
        assert!(!limiter.check()); // 3rd — blocked
    }

    #[test]
    fn test_rate_limiter_multiple_instances_are_independent() {
        let limiter_a = EndpointRateLimiter::new(1);
        let limiter_b = EndpointRateLimiter::new(1);
        assert!(limiter_a.check());
        assert!(limiter_b.check()); // Different limiter, should still pass
        assert!(!limiter_a.check()); // Same limiter, should block
        assert!(!limiter_b.check()); // Same limiter, should block
    }

    #[tokio::test]
    async fn test_rate_limit_single_request_allowed() {
        let db = DatabaseClient::new_mem().await;
        // Single request with limit of 1 should pass
        let result = check_user_rate_limit(&db, "user_single", "checkout", 1, 1).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_rate_limit_error_message_content() {
        let db = DatabaseClient::new_mem().await;
        // Exhaust the limit
        let _ = check_user_rate_limit(&db, "user_msg", "webhook", 1, 1).await;
        let result = check_user_rate_limit(&db, "user_msg", "webhook", 1, 1).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Rate limit exceeded"));
        assert!(err_msg.contains("try again later"));
    }

    #[tokio::test]
    async fn test_rate_limit_zero_max_requests_blocks_immediately() {
        let db = DatabaseClient::new_mem().await;
        // max_requests=0 means no requests allowed
        let result = check_user_rate_limit(&db, "user_zero", "action", 0, 1).await;
        assert!(result.is_err());
    }
}
