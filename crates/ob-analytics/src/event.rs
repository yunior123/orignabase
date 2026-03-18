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

    #[test]
    fn test_extract_domain_no_protocol() {
        assert_eq!(
            extract_domain("example.com/path"),
            Some("example.com".into())
        );
    }

    #[test]
    fn test_analytics_event_has_required_fields() {
        let event = AnalyticsEvent {
            id: "evt_001".to_string(),
            event: "page_view".to_string(),
            visitor_hash: "abc123def456".to_string(),
            properties: serde_json::json!({"utm_source": "google"}),
            timestamp: "2026-03-08T12:00:00Z".to_string(),
            path: Some("/products".to_string()),
            referrer: Some("google.com".to_string()),
            country: Some("CA".to_string()),
            device: Some("desktop".to_string()),
            browser: Some("chrome".to_string()),
        };

        assert_eq!(event.id, "evt_001");
        assert_eq!(event.event, "page_view");
        assert!(!event.visitor_hash.is_empty());
        assert!(!event.timestamp.is_empty());
        assert_eq!(event.path.as_deref(), Some("/products"));
    }

    #[test]
    fn test_analytics_event_serialization_roundtrip() {
        let event = AnalyticsEvent {
            id: "evt_002".to_string(),
            event: "button_click".to_string(),
            visitor_hash: hash_ip("10.0.0.1", "testsalt"),
            properties: serde_json::json!({"button": "buy_now"}),
            timestamp: "2026-03-08T14:30:00Z".to_string(),
            path: None,
            referrer: None,
            country: None,
            device: None,
            browser: None,
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: AnalyticsEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, event.id);
        assert_eq!(deserialized.event, event.event);
        assert_eq!(deserialized.visitor_hash, event.visitor_hash);
        assert!(deserialized.path.is_none());
        assert!(deserialized.referrer.is_none());
    }

    #[test]
    fn test_analytics_event_optional_fields_default() {
        // properties should default to null/empty when missing
        let json_str = r#"{
            "id": "evt_003",
            "event": "signup",
            "visitor_hash": "hash123",
            "timestamp": "2026-03-08T10:00:00Z"
        }"#;

        let event: AnalyticsEvent = serde_json::from_str(json_str).unwrap();
        assert_eq!(event.id, "evt_003");
        assert_eq!(event.event, "signup");
        assert!(event.path.is_none());
        assert!(event.device.is_none());
        assert!(event.browser.is_none());
    }

    #[test]
    fn test_hash_ip_consistency_across_calls() {
        // Same input always produces the same hash (deterministic)
        let salt = "prod_salt_2026";
        let ip = "203.0.113.42";

        let hash_a = hash_ip(ip, salt);
        let hash_b = hash_ip(ip, salt);
        let hash_c = hash_ip(ip, salt);

        assert_eq!(hash_a, hash_b);
        assert_eq!(hash_b, hash_c);
    }

    #[test]
    fn test_hash_ip_output_length() {
        // Should be 32 hex chars (16 bytes encoded)
        let hash = hash_ip("127.0.0.1", "salt");
        assert_eq!(hash.len(), 32);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hash_ip_is_one_way() {
        // Different IPs with same salt should produce different hashes
        let salt = "same_salt";
        let hashes: Vec<String> = (1..=5)
            .map(|i| hash_ip(&format!("10.0.0.{i}"), salt))
            .collect();

        // All should be unique
        let unique: std::collections::HashSet<&String> = hashes.iter().collect();
        assert_eq!(unique.len(), hashes.len(), "All hashes should be unique");
    }

    #[test]
    fn test_daily_rollup_serialization() {
        let rollup = DailyRollup {
            date: "2026-03-08".to_string(),
            event: "page_view".to_string(),
            count: 1500,
            unique_visitors: 423,
            top_paths: vec![
                PathCount {
                    value: "/".to_string(),
                    count: 800,
                },
                PathCount {
                    value: "/products".to_string(),
                    count: 500,
                },
            ],
            top_referrers: vec![PathCount {
                value: "google.com".to_string(),
                count: 300,
            }],
        };

        let json = serde_json::to_value(&rollup).unwrap();
        assert_eq!(json["date"], "2026-03-08");
        assert_eq!(json["count"], 1500);
        assert_eq!(json["unique_visitors"], 423);
        assert_eq!(json["top_paths"].as_array().unwrap().len(), 2);
    }
}
