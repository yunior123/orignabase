use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
use dashmap::DashMap;
use ob_core::Error;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Sliding-window rate limiter state shared across requests.
#[derive(Clone)]
pub struct RateLimiter {
    /// Map of IP → (request_count, window_start)
    state: Arc<DashMap<IpAddr, (u64, Instant)>>,
    /// Maximum requests per window.
    max_requests: u64,
    /// Window duration.
    window: Duration,
}

impl RateLimiter {
    pub fn new(max_requests: u64, window: Duration) -> Self {
        Self {
            state: Arc::new(DashMap::new()),
            max_requests,
            window,
        }
    }

    /// Check if a request from the given IP is allowed.
    /// Returns `true` if allowed, `false` if rate-limited.
    pub fn check(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut entry = self.state.entry(ip).or_insert((0, now));
        let (count, window_start) = entry.value_mut();

        // Reset window if expired
        if now.duration_since(*window_start) >= self.window {
            *count = 0;
            *window_start = now;
        }

        *count += 1;
        *count <= self.max_requests
    }

    /// Periodically clean up expired entries to prevent memory leaks.
    pub fn cleanup(&self) {
        let now = Instant::now();
        self.state
            .retain(|_, (_, start)| now.duration_since(*start) < self.window);
    }
}

/// Extract client IP from request (checks X-Forwarded-For, then peer addr).
fn extract_ip(request: &Request) -> IpAddr {
    // Check X-Forwarded-For header first (for reverse proxies)
    if let Some(forwarded) = request.headers().get("x-forwarded-for")
        && let Ok(val) = forwarded.to_str()
        && let Some(first_ip) = val.split(',').next()
        && let Ok(ip) = first_ip.trim().parse::<IpAddr>()
    {
        return ip;
    }

    // Check X-Real-IP header
    if let Some(real_ip) = request.headers().get("x-real-ip")
        && let Ok(val) = real_ip.to_str()
        && let Ok(ip) = val.trim().parse::<IpAddr>()
    {
        return ip;
    }

    // Fallback to loopback
    IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
}

/// Axum middleware for rate limiting auth routes.
pub async fn rate_limit_middleware(
    request: Request,
    next: Next,
) -> std::result::Result<Response, StatusCode> {
    let limiter = request.extensions().get::<RateLimiter>().cloned();

    if let Some(limiter) = limiter {
        let ip = extract_ip(&request);
        if !limiter.check(ip) {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
    }

    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_rate_limiter_allows_within_limit() {
        let limiter = RateLimiter::new(5, Duration::from_secs(60));
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));

        for _ in 0..5 {
            assert!(limiter.check(ip));
        }
    }

    #[test]
    fn test_rate_limiter_blocks_over_limit() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        assert!(limiter.check(ip)); // 1
        assert!(limiter.check(ip)); // 2
        assert!(limiter.check(ip)); // 3
        assert!(!limiter.check(ip)); // 4 → blocked
    }

    #[test]
    fn test_rate_limiter_different_ips_independent() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        let ip1 = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(2, 2, 2, 2));

        assert!(limiter.check(ip1)); // ip1: 1
        assert!(limiter.check(ip1)); // ip1: 2
        assert!(!limiter.check(ip1)); // ip1: blocked

        // ip2 should still be allowed
        assert!(limiter.check(ip2)); // ip2: 1
        assert!(limiter.check(ip2)); // ip2: 2
    }

    #[test]
    fn test_rate_limiter_window_reset() {
        let limiter = RateLimiter::new(2, Duration::from_millis(1));
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        assert!(limiter.check(ip));
        assert!(limiter.check(ip));
        assert!(!limiter.check(ip)); // blocked

        // Wait for window to expire
        std::thread::sleep(Duration::from_millis(5));

        // Should be allowed again
        assert!(limiter.check(ip));
    }

    #[test]
    fn test_rate_limiter_cleanup() {
        let limiter = RateLimiter::new(10, Duration::from_millis(1));
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        limiter.check(ip);

        std::thread::sleep(Duration::from_millis(5));
        limiter.cleanup();

        // Entry should be cleaned up, state map empty
        assert_eq!(limiter.state.len(), 0);
    }

    #[test]
    fn test_rate_limiter_ipv6() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        let ip = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));

        assert!(limiter.check(ip));
        assert!(limiter.check(ip));
        assert!(!limiter.check(ip));
    }

    #[test]
    fn test_extract_ip_fallback() {
        let req = Request::builder().body(axum::body::Body::empty()).unwrap();
        let ip = extract_ip(&req);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn test_extract_ip_from_x_forwarded_for() {
        let req = Request::builder()
            .header("x-forwarded-for", "203.0.113.50, 70.41.3.18")
            .body(axum::body::Body::empty())
            .unwrap();
        let ip = extract_ip(&req);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 50)));
    }

    #[test]
    fn test_extract_ip_from_x_real_ip() {
        let req = Request::builder()
            .header("x-real-ip", "198.51.100.7")
            .body(axum::body::Body::empty())
            .unwrap();
        let ip = extract_ip(&req);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7)));
    }

    #[test]
    fn test_extract_ip_x_forwarded_for_takes_priority() {
        let req = Request::builder()
            .header("x-forwarded-for", "1.2.3.4")
            .header("x-real-ip", "5.6.7.8")
            .body(axum::body::Body::empty())
            .unwrap();
        let ip = extract_ip(&req);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)));
    }
}

/// Database-backed rate limiter for user actions (e.g., TOTP attempts).
/// Used for per-user rate limiting (not IP-based).
pub async fn check_rate_limit(
    db: &ob_database::DatabaseClient,
    user_id: &str,
    action: &str,
    max_attempts: i64,
    window_seconds: i64,
) -> std::result::Result<(), Error> {
    use chrono::Utc;

    let now = Utc::now().timestamp();
    let window_start = now - window_seconds;

    // Count recent attempts
    let query = format!(
        "SELECT count() FROM mfa_attempts WHERE user_id = '{}' AND action = '{}' AND timestamp >= {} GROUP ALL",
        user_id, action, window_start
    );

    let results = db
        .query_raw(&query)
        .await
        .map_err(|e| Error::Internal(format!("Rate limit check failed: {}", e)))?;

    let count = if let Some(row) = results.first() {
        row.get("count").and_then(|v| v.as_i64()).unwrap_or(0)
    } else {
        0
    };

    if count >= max_attempts {
        return Err(Error::Auth(
            "Too many failed attempts. Please try again later.".into(),
        ));
    }

    // Log this attempt
    let _ = db
        .create_document(
            "mfa_attempts",
            serde_json::json!({
                "user_id": user_id,
                "action": action,
                "timestamp": now,
            }),
        )
        .await;

    Ok(())
}
