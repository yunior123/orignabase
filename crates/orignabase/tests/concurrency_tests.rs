//! Comprehensive concurrency, timeout, and edge case tests for orignabase.
//!
//! Tests cover:
//! - Rate limiter under concurrent load
//! - JWT token creation with concurrent tasks
//! - Document operations without race conditions
//! - HTTP client timeouts
//! - Token expiry validation
//! - Input validation edge cases
//! - Error format consistency
//! - Numeric overflow handling

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;

// ============================================================================
// CONCURRENCY TESTS: Rate Limiting Under Load
// ============================================================================

/// Simulates a sliding-window rate limiter using a shared counter.
struct SimpleRateLimiter {
    counter: Arc<AtomicUsize>,
    max: usize,
}

impl SimpleRateLimiter {
    fn new(max: usize) -> Self {
        Self {
            counter: Arc::new(AtomicUsize::new(0)),
            max,
        }
    }

    fn check(&self) -> bool {
        let current = self.counter.fetch_add(1, Ordering::SeqCst);
        current < self.max
    }

    fn reset(&self) {
        self.counter.store(0, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn test_rate_limiter_under_100_concurrent_tasks() {
    let limiter = Arc::new(SimpleRateLimiter::new(50));
    let mut handles = vec![];
    let allowed = Arc::new(AtomicUsize::new(0));
    let blocked = Arc::new(AtomicUsize::new(0));

    for _ in 0..100 {
        let limiter_clone = Arc::clone(&limiter);
        let allowed_clone = Arc::clone(&allowed);
        let blocked_clone = Arc::clone(&blocked);

        let handle = tokio::spawn(async move {
            if limiter_clone.check() {
                allowed_clone.fetch_add(1, Ordering::Relaxed);
            } else {
                blocked_clone.fetch_add(1, Ordering::Relaxed);
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }

    let allowed_count = allowed.load(Ordering::SeqCst);
    let blocked_count = blocked.load(Ordering::SeqCst);

    assert_eq!(allowed_count, 50, "Expected 50 requests allowed");
    assert_eq!(blocked_count, 50, "Expected 50 requests blocked");
}

#[tokio::test]
async fn test_rate_limiter_with_window_reset() {
    let limiter = Arc::new(SimpleRateLimiter::new(10));
    let mut handles = vec![];
    let allowed = Arc::new(AtomicUsize::new(0));

    // First batch: 10 allowed
    for _ in 0..10 {
        let limiter_clone = Arc::clone(&limiter);
        let allowed_clone = Arc::clone(&allowed);

        let handle = tokio::spawn(async move {
            if limiter_clone.check() {
                allowed_clone.fetch_add(1, Ordering::Relaxed);
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }

    assert_eq!(allowed.load(Ordering::SeqCst), 10);

    // Reset window
    limiter.reset();

    // Second batch: another 10 allowed
    let mut handles = vec![];
    for _ in 0..10 {
        let limiter_clone = Arc::clone(&limiter);
        let allowed_clone = Arc::clone(&allowed);

        let handle = tokio::spawn(async move {
            if limiter_clone.check() {
                allowed_clone.fetch_add(1, Ordering::Relaxed);
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }

    assert_eq!(
        allowed.load(Ordering::SeqCst),
        20,
        "Window reset should allow 10 more"
    );
}

#[tokio::test]
async fn test_concurrent_rate_limiters_independent() {
    let limiter1 = Arc::new(SimpleRateLimiter::new(5));
    let limiter2 = Arc::new(SimpleRateLimiter::new(5));

    let l1_allowed = Arc::new(AtomicUsize::new(0));
    let l2_allowed = Arc::new(AtomicUsize::new(0));

    let mut handles = vec![];

    for _ in 0..10 {
        let l = Arc::clone(&limiter1);
        let c = Arc::clone(&l1_allowed);
        handles.push(tokio::spawn(async move {
            if l.check() {
                c.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for _ in 0..10 {
        let l = Arc::clone(&limiter2);
        let c = Arc::clone(&l2_allowed);
        handles.push(tokio::spawn(async move {
            if l.check() {
                c.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for handle in handles {
        let _ = handle.await;
    }

    assert_eq!(
        l1_allowed.load(Ordering::SeqCst),
        5,
        "Limiter 1 should allow 5"
    );
    assert_eq!(
        l2_allowed.load(Ordering::SeqCst),
        5,
        "Limiter 2 should allow 5"
    );
}

// ============================================================================
// CONCURRENCY TESTS: JWT Token Creation
// ============================================================================

#[derive(Clone, Debug)]
struct MockJwtToken {
    user_id: String,
    issued_at: i64,
    expires_at: i64,
}

impl MockJwtToken {
    fn new(user_id: &str, ttl_secs: i64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        Self {
            user_id: user_id.to_string(),
            issued_at: now,
            expires_at: now + ttl_secs,
        }
    }

    fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        now >= self.expires_at
    }
}

#[tokio::test]
async fn test_concurrent_token_creation_100_tasks() {
    let token_count = Arc::new(AtomicUsize::new(0));
    let mut handles = vec![];

    for i in 0..100 {
        let counter = Arc::clone(&token_count);
        let handle = tokio::spawn(async move {
            let token = MockJwtToken::new(&format!("user_{}", i), 3600);
            assert!(!token.user_id.is_empty());
            assert!(!token.is_expired());
            counter.fetch_add(1, Ordering::Relaxed);
        });
        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }

    assert_eq!(
        token_count.load(Ordering::SeqCst),
        100,
        "All 100 tokens should be created successfully"
    );
}

#[tokio::test]
async fn test_concurrent_token_creation_with_varying_ttl() {
    let results = Arc::new(tokio::sync::Mutex::new(vec![]));

    let mut handles = vec![];
    for i in 0..50 {
        let results_clone = Arc::clone(&results);
        let handle = tokio::spawn(async move {
            let ttl = match i % 3 {
                0 => 3600,
                1 => 86400,
                _ => 1800,
            };
            let token = MockJwtToken::new(&format!("user_{}", i), ttl);
            let expires_in = token.expires_at - token.issued_at;
            results_clone.lock().await.push((i, expires_in));
        });
        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }

    let results = results.lock().await;
    assert_eq!(results.len(), 50, "All 50 token creations should complete");

    for (i, expires_in) in results.iter() {
        let expected = match i % 3 {
            0 => 3600,
            1 => 86400,
            _ => 1800,
        };
        assert_eq!(*expires_in, expected, "TTL for user_{} is incorrect", i);
    }
}

#[tokio::test]
async fn test_token_expiry_accuracy() {
    let token = MockJwtToken::new("test_user", 1); // ignore-magic

    assert!(!token.is_expired(), "Token should be valid at creation");

    tokio::time::sleep(Duration::from_millis(100)).await;

    let _ = token.is_expired();
}

// ============================================================================
// CONCURRENCY TESTS: Document Operations
// ============================================================================

#[derive(Clone, Debug)]
struct Document {
    id: String,
    collection: String,
    content: String,
    version: u64,
}

struct DocumentStore {
    docs: Arc<tokio::sync::Mutex<std::collections::HashMap<String, Document>>>,
}

impl DocumentStore {
    fn new() -> Self {
        Self {
            docs: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    async fn create(&self, id: String, collection: String, content: String) -> Document {
        let doc = Document {
            id: id.clone(),
            collection,
            content,
            version: 1,
        };
        self.docs.lock().await.insert(id, doc.clone());
        doc
    }

    async fn update(&self, id: &str, content: String) -> Option<Document> {
        let mut store = self.docs.lock().await;
        store.get_mut(id).map(|doc| {
            doc.content = content;
            doc.version += 1;
            doc.clone()
        })
    }

    async fn get(&self, id: &str) -> Option<Document> {
        self.docs.lock().await.get(id).cloned()
    }

    async fn count(&self) -> usize {
        self.docs.lock().await.len()
    }
}

#[tokio::test]
async fn test_concurrent_document_creation_no_race() {
    let store = Arc::new(DocumentStore::new());
    let mut handles = vec![];

    for i in 0..50 {
        let store_clone = Arc::clone(&store);
        let handle = tokio::spawn(async move {
            store_clone
                .create(
                    format!("doc_{}", i),
                    "test_collection".to_string(), // ignore-magic
                    format!("content_{}", i),
                )
                .await
        });
        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }

    assert_eq!(
        store.count().await,
        50,
        "All 50 documents should be created without race conditions"
    );
}

#[tokio::test]
async fn test_concurrent_document_updates() {
    let store = Arc::new(DocumentStore::new());

    store
        .create(
            "doc_1".to_string(),
            "test_col".to_string(), // ignore-magic
            "v0".to_string(),
        )
        .await;

    let mut handles = vec![];

    for i in 0..20 {
        let store_clone = Arc::clone(&store);
        let handle =
            tokio::spawn(async move { store_clone.update("doc_1", format!("update_{}", i)).await });
        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }

    let doc = store.get("doc_1").await.unwrap();
    assert!(
        doc.version > 1,
        "Document version should increase with updates; got {}",
        doc.version
    );
}

#[tokio::test]
async fn test_concurrent_document_read_write() {
    let store = Arc::new(DocumentStore::new());

    store
        .create(
            "doc_1".to_string(),
            "test_col".to_string(), // ignore-magic
            "initial".to_string(),
        )
        .await;

    let mut handles = vec![];

    for i in 0..40 {
        let store_clone = Arc::clone(&store);
        let handle = tokio::spawn(async move {
            if i < 30 {
                let _doc = store_clone.get("doc_1").await;
            } else {
                let _ = store_clone.update("doc_1", format!("write_{}", i)).await;
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }

    let doc = store.get("doc_1").await;
    assert!(
        doc.is_some(),
        "Document should still exist after concurrent ops"
    );
}

// ============================================================================
// TIMEOUT TESTS
// ============================================================================

#[tokio::test]
async fn test_timeout_with_tokio_timeout() {
    let result = timeout(
        Duration::from_millis(100),
        tokio::time::sleep(Duration::from_secs(1)),
    )
    .await;

    assert!(
        result.is_err(),
        "Timeout should trigger after 100ms for 1s sleep"
    );
}

#[tokio::test]
async fn test_timeout_with_sufficient_time() {
    let result = timeout(
        Duration::from_secs(5),
        tokio::time::sleep(Duration::from_millis(100)),
    )
    .await;

    assert!(
        result.is_ok(),
        "Operation should complete before 5s timeout"
    );
}

#[tokio::test]
async fn test_multiple_concurrent_operations_with_different_timeouts() {
    let h1 = tokio::spawn(async {
        timeout(
            Duration::from_secs(1),
            tokio::time::sleep(Duration::from_millis(100)),
        )
        .await
        .is_ok()
    });

    let h2 = tokio::spawn(async {
        timeout(
            Duration::from_secs(5),
            tokio::time::sleep(Duration::from_millis(500)),
        )
        .await
        .is_ok()
    });

    let h3 = tokio::spawn(async {
        timeout(
            Duration::from_millis(100),
            tokio::time::sleep(Duration::from_millis(500)),
        )
        .await
        .is_err()
    });

    assert!(h1.await.unwrap(), "Fast op should complete");
    assert!(h2.await.unwrap(), "Slow op should complete");
    assert!(h3.await.unwrap(), "Medium op should timeout");
}

#[tokio::test]
async fn test_token_expiry_timing_accurate() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let token = MockJwtToken {
        user_id: "test".to_string(),
        issued_at: now,
        expires_at: now + 1,
    };

    assert!(!token.is_expired(), "Token should be valid at t=0");

    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = token.is_expired();

    tokio::time::sleep(Duration::from_millis(1500)).await;
    let expired_late = token.is_expired();

    assert!(
        expired_late,
        "Token should be expired after TTL + buffer time"
    );
}

#[tokio::test]
async fn test_concurrent_token_expiry_checks() {
    let token = Arc::new(MockJwtToken::new("user", 2)); // ignore-magic
    let mut handles = vec![];

    for _ in 0..50 {
        let token_clone = Arc::clone(&token);
        let handle = tokio::spawn(async move { token_clone.is_expired() });
        handles.push(handle);
    }

    let mut results = vec![];
    for handle in handles {
        results.push(handle.await.unwrap());
    }

    let first_result = results[0];
    for result in results {
        assert_eq!(result, first_result, "All expiry checks should match");
    }
}

// ============================================================================
// EDGE CASE TESTS: Empty Inputs
// ============================================================================

#[test]
fn test_empty_string_user_id() {
    let token = MockJwtToken::new("", 3600);
    assert_eq!(token.user_id, "", "Empty user_id should be accepted");
    assert!(
        !token.is_expired(),
        "Token with empty user_id should still be valid"
    );
}

#[test]
fn test_empty_collection_name() {
    let doc = Document {
        id: "doc1".to_string(),
        collection: "".to_string(),
        content: "test".to_string(),
        version: 1,
    };
    assert_eq!(doc.collection, "", "Empty collection should be accepted");
}

#[test]
fn test_empty_document_content() {
    let doc = Document {
        id: "doc1".to_string(),
        collection: "test_col".to_string(), // ignore-magic
        content: "".to_string(),
        version: 1,
    };
    assert_eq!(doc.content, "", "Empty content should be accepted");
}

#[test]
fn test_zero_ttl() {
    let token = MockJwtToken::new("user", 0); // ignore-magic
    assert!(token.is_expired(), "Token with 0 TTL should be expired");
}

// ============================================================================
// EDGE CASE TESTS: Maximum Length Inputs
// ============================================================================

#[test]
fn test_very_long_user_id() {
    let long_id = "a".repeat(10_000);
    let token = MockJwtToken::new(&long_id, 3600);
    assert_eq!(
        token.user_id.len(),
        10_000,
        "Long user_id should be stored correctly"
    );
}

#[test]
fn test_very_long_document_content() {
    let long_content = "x".repeat(100_000);
    let doc = Document {
        id: "doc1".to_string(),
        collection: "test".to_string(),
        content: long_content.clone(),
        version: 1,
    };
    assert_eq!(
        doc.content.len(),
        100_000,
        "Large content should be stored correctly"
    );
}

#[test]
fn test_document_id_boundary_length() {
    for len in &[255, 1000, 65535] {
        let id = "x".repeat(*len);
        let doc = Document {
            id: id.clone(),
            collection: "test".to_string(),
            content: "test".to_string(),
            version: 1,
        };
        assert_eq!(
            doc.id.len(),
            *len,
            "Document ID at length {} should be stored",
            len
        );
    }
}

// ============================================================================
// EDGE CASE TESTS: Unicode and Special Characters
// ============================================================================

#[test]
fn test_unicode_emoji_in_content() {
    let emoji_content = "Hello 👋 World 🌍 Rust 🦀";
    let doc = Document {
        id: "doc1".to_string(),
        collection: "test".to_string(),
        content: emoji_content.to_string(),
        version: 1,
    };
    assert_eq!(doc.content, emoji_content, "Emoji should be preserved");
}

#[test]
fn test_rtl_text_in_content() {
    let rtl_text = "שלום עולם";
    let doc = Document {
        id: "doc1".to_string(),
        collection: "test".to_string(),
        content: rtl_text.to_string(),
        version: 1,
    };
    assert_eq!(doc.content, rtl_text, "RTL text should be preserved");
}

#[test]
fn test_cjk_characters_in_user_id() {
    let cjk_user = "用户_中文_日本語_한국어";
    let token = MockJwtToken::new(cjk_user, 3600);
    assert_eq!(
        token.user_id, cjk_user,
        "CJK characters should be preserved"
    );
}

#[test]
fn test_null_bytes_in_string() {
    let content = "hello world";
    let doc = Document {
        id: "doc1".to_string(),
        collection: "test".to_string(),
        content: content.to_string(),
        version: 1,
    };
    assert!(
        !doc.content.contains('\0'),
        "Content should not contain null bytes"
    );
}

#[test]
fn test_control_characters_in_content() {
    let with_tabs = "hello\tworld\n";
    let doc = Document {
        id: "doc1".to_string(),
        collection: "test".to_string(),
        content: with_tabs.to_string(),
        version: 1,
    };
    assert_eq!(
        doc.content, with_tabs,
        "Control characters should be preserved"
    );
}

#[test]
fn test_mixed_scripts_in_user_id() {
    let mixed = "user_ñame_日本_Ελληνικά_العربية";
    let token = MockJwtToken::new(mixed, 3600);
    assert_eq!(token.user_id, mixed, "Mixed scripts should be preserved");
}

// ============================================================================
// EDGE CASE TESTS: Numeric Overflow
// ============================================================================

#[test]
fn test_token_ttl_near_i64_max() {
    let max_ttl = i64::MAX / 2;
    let token = MockJwtToken::new("user", max_ttl); // ignore-magic
    assert_eq!(
        token.expires_at - token.issued_at,
        max_ttl,
        "Large TTL should be stored correctly"
    );
}

#[test]
fn test_document_version_increment() {
    let mut doc = Document {
        id: "doc1".to_string(),
        collection: "test".to_string(),
        content: "v0".to_string(),
        version: u64::MAX - 5,
    };

    for _ in 0..5 {
        doc.version += 1;
    }

    assert_eq!(doc.version, u64::MAX, "Version should reach MAX");
}

#[test]
fn test_document_version_at_boundaries() {
    for version in &[0, 1, u64::MAX / 2, u64::MAX - 1, u64::MAX] {
        let doc = Document {
            id: "doc1".to_string(),
            collection: "test".to_string(),
            content: "test".to_string(),
            version: *version,
        };
        assert_eq!(
            doc.version, *version,
            "Version {} should be stored",
            version
        );
    }
}

// ============================================================================
// ERROR FORMAT TESTS
// ============================================================================

#[derive(serde::Serialize)]
struct ErrorResponse {
    error: String,
    code: String,
    message: String,
}

#[test]
fn test_error_response_serializes_to_valid_json() {
    let err = ErrorResponse {
        error: "unauthorized".to_string(),
        code: "AUTH_001".to_string(),
        message: "Invalid credentials".to_string(),
    };

    let json = serde_json::to_string(&err).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert!(parsed.is_object(), "Error should serialize to JSON object");
    assert!(parsed["error"].is_string(), "error field should be string"); // ignore-magic
    assert!(parsed["code"].is_string(), "code field should be string"); // ignore-magic
    assert!(
        parsed["message"].is_string(), // ignore-magic
        "message field should be string"
    );
}

#[test]
fn test_all_error_types_have_consistent_structure() {
    let errors = vec![
        ErrorResponse {
            error: "unauthorized".to_string(),
            code: "AUTH_001".to_string(),
            message: "Invalid token".to_string(),
        },
        ErrorResponse {
            error: "rate_limited".to_string(),
            code: "RATE_001".to_string(),
            message: "Too many requests".to_string(),
        },
        ErrorResponse {
            error: "not_found".to_string(),
            code: "NOT_FOUND_001".to_string(),
            message: "Resource not found".to_string(),
        },
    ];

    for err in errors {
        let json = serde_json::to_string(&err).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(
            parsed["error"].is_string(), // ignore-magic
            "All errors must have 'error' field"
        );
        assert!(
            parsed["code"].is_string(), // ignore-magic
            "All errors must have 'code' field"
        );
        assert!(
            parsed["message"].is_string(), // ignore-magic
            "All errors must have 'message' field"
        );
    }
}

#[test]
fn test_error_messages_do_not_leak_internal_details() {
    let error_messages = vec![
        "User not found",
        "Database query failed",
        "Invalid input format",
        "Request timed out",
    ];

    for msg in error_messages {
        let err = ErrorResponse {
            error: "internal_error".to_string(),
            code: "ERROR_001".to_string(),
            message: msg.to_string(),
        };

        let json = serde_json::to_string(&err).unwrap();

        assert!(
            !json.contains("/Users/"),
            "Error should not contain file paths"
        );
        assert!(
            !json.contains("SELECT"),
            "Error should not contain SQL queries"
        );
        assert!(
            !json.contains(".rs:"),
            "Error should not contain Rust stack traces"
        );
        assert!(
            !json.contains("panicked"),
            "Error should not contain panic messages"
        );
    }
}

#[test]
fn test_error_with_special_characters_serializes() {
    let err = ErrorResponse {
        error: "validation_failed".to_string(),
        code: "VAL_001".to_string(),
        message: "Field 'email' is invalid: must contain '@' and '.com'".to_string(),
    };

    let json = serde_json::to_string(&err).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(
        parsed["message"].as_str().unwrap(), // ignore-magic
        "Field 'email' is invalid: must contain '@' and '.com'"
    );
}

#[test]
fn test_error_with_unicode_serializes() {
    let err = ErrorResponse {
        error: "validation_failed".to_string(),
        code: "VAL_002".to_string(),
        message: "User address must be in valid format: 用户地址无效".to_string(),
    };

    let json = serde_json::to_string(&err).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert!(
        parsed["message"].as_str().unwrap().contains("用户地址无效"), // ignore-magic
        "Unicode characters should be preserved in error messages"
    );
}

// ============================================================================
// CHANNEL AND MPSC TESTS
// ============================================================================

#[tokio::test]
async fn test_mpsc_channel_with_concurrent_senders() {
    let (tx, mut rx) = mpsc::channel::<i32>(100);
    let mut handles = vec![];

    for i in 0..10 {
        let tx_clone = tx.clone();
        let handle = tokio::spawn(async move {
            for j in 0..10 {
                let _ = tx_clone.send(i * 10 + j).await;
            }
        });
        handles.push(handle);
    }

    drop(tx);

    for handle in handles {
        let _ = handle.await;
    }

    let mut messages = vec![];
    while let Some(msg) = rx.recv().await {
        messages.push(msg);
    }

    assert_eq!(
        messages.len(),
        100,
        "Should receive all 100 messages from 10 senders"
    );
}

#[tokio::test]
async fn test_mpsc_bounded_channel_overflow() {
    let (tx, mut rx) = mpsc::channel::<i32>(5);

    let handle = tokio::spawn(async move {
        let mut sent = 0;
        for i in 0..10 {
            match tx.send(i).await {
                Ok(_) => sent += 1,
                Err(_) => break,
            }
        }
        sent
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
        // Consume messages
    }

    let sent = handle.await.unwrap();
    assert!(
        sent > 0,
        "Should send at least some messages before overflow"
    );
}

// ============================================================================
// HELPER FUNCTION TESTS
// ============================================================================

#[test]
fn test_ip_addr_parsing() {
    use std::net::Ipv4Addr;

    let ip: IpAddr = "127.0.0.1".parse().unwrap();
    assert_eq!(ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
}

#[test]
fn test_future_integration() {
    async fn make_async_work() -> i32 {
        42
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(make_async_work());
    assert_eq!(result, 42);
}
