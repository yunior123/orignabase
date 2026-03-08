use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// An analytics event ingested from clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsEvent {
    /// Unique event ID.
    pub id: String,
    /// Event name (e.g., "page_view", "button_click").
    pub event: String,
    /// Hashed visitor identifier (no PII).
    pub visitor_hash: String,
    /// Event properties (schemaless).
    #[serde(default)]
    pub properties: serde_json::Value,
    /// ISO 8601 timestamp.
    pub timestamp: String,
    /// URL path (for page views).
    pub path: Option<String>,
    /// Referrer domain (stripped to domain only).
    pub referrer: Option<String>,
    /// Country code (from IP geolocation, if available).
    pub country: Option<String>,
    /// Device type (desktop/mobile/tablet).
    pub device: Option<String>,
    /// Browser name.
    pub browser: Option<String>,
}

/// Daily aggregation rollup for fast dashboard queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyRollup {
    /// Date in YYYY-MM-DD format.
    pub date: String,
    /// Event name.
    pub event: String,
    /// Total count for the day.
    pub count: u64,
    /// Unique visitors (by visitor_hash).
    pub unique_visitors: u64,
    /// Top paths (for page_view events).
    #[serde(default)]
    pub top_paths: Vec<PathCount>,
    /// Top referrers.
    #[serde(default)]
    pub top_referrers: Vec<PathCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathCount {
    pub value: String,
    pub count: u64,
}

/// Hash an IP address for privacy (one-way, salted).
pub fn hash_ip(ip: &str, salt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(ip.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..16]) // First 16 bytes = 32 hex chars
}

/// Extract domain from a referrer URL.
pub fn extract_domain(referrer: &str) -> Option<String> {
    if referrer.is_empty() {
        return None;
    }
    // Simple extraction: strip protocol, take domain
    let stripped = referrer
        .strip_prefix("https://")
        .or_else(|| referrer.strip_prefix("http://"))
        .unwrap_or(referrer);
    stripped.split('/').next().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_ip_deterministic() {
        let hash1 = hash_ip("192.168.1.1", "salt123");
        let hash2 = hash_ip("192.168.1.1", "salt123");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_ip_different_ips() {
        let hash1 = hash_ip("192.168.1.1", "salt");
        let hash2 = hash_ip("192.168.1.2", "salt");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_ip_different_salts() {
        let hash1 = hash_ip("192.168.1.1", "salt1");
        let hash2 = hash_ip("192.168.1.1", "salt2");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_extract_domain() {
        assert_eq!(
            extract_domain("https://google.com/search?q=test"),
            Some("google.com".into())
        );
        assert_eq!(
            extract_domain("http://example.org/path"),
            Some("example.org".into())
        );
        assert_eq!(extract_domain(""), None);
    }
}
