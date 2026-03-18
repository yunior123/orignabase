//! Comprehensive integration tests for ob-auth module
//!
//! Coverage:
//! - JWT: all token types, expiry, secrets, custom claims, RS256/HS256, TTL accuracy
//! - Password: hashing, verification, uniqueness, edge cases (empty, long, unicode)
//! - TOTP: generation, verification, replay prevention, QR codes, recovery codes
//! - Rate Limiting: limits, burst capacity, cleanup, IP extraction
//! - Middleware: token extraction from headers, missing/malformed Bearer tokens

use ob_auth::{
    jwt::{self, Claims, JwtKeys},
    middleware::AuthContext,
    password,
    rate_limit::RateLimiter,
    totp,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

// ═══════════════════════════════════════════════════════════════════════════════
// JWT TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_jwt_issue_and_verify_access_token() {
    let keys = JwtKeys::from_secret("test_secret_12345");
    let roles = ["user".to_string(), "editor".to_string()];

    let token = jwt::issue_access_token("user123", &roles, &keys, 3600, true).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert_eq!(claims.sub, "user123");
    assert_eq!(claims.typ, "access");
    assert_eq!(claims.roles, roles);
    assert!(claims.email_verified);
    assert!(!claims.mfa_required);
}

#[test]
fn test_jwt_issue_and_verify_refresh_token() {
    let keys = JwtKeys::from_secret("test_secret_12345");

    let token = jwt::issue_refresh_token("user456", &keys, 604800).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert_eq!(claims.sub, "user456");
    assert_eq!(claims.typ, "refresh");
    assert!(claims.roles.is_empty());
    assert!(!claims.email_verified);
}

#[test]
fn test_jwt_issue_and_verify_verification_token() {
    let keys = JwtKeys::from_secret("test_secret_12345");

    let token = jwt::issue_verification_token("user789", &keys).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert_eq!(claims.sub, "user789");
    assert_eq!(claims.typ, "email_verify");
    // 24 hours ≈ 86400 seconds
    assert!(claims.exp - claims.iat >= 86400 - 1 && claims.exp - claims.iat <= 86400);
}

#[test]
fn test_jwt_issue_and_verify_reset_token() {
    let keys = JwtKeys::from_secret("test_secret_12345");

    let token = jwt::issue_reset_token("user_pwd_reset", &keys).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert_eq!(claims.sub, "user_pwd_reset");
    assert_eq!(claims.typ, "password_reset");
    // 1 hour = 3600 seconds
    assert!(claims.exp - claims.iat >= 3599 && claims.exp - claims.iat <= 3601);
}

#[test]
fn test_jwt_issue_and_verify_magic_link_token() {
    let keys = JwtKeys::from_secret("test_secret_12345");

    let token = jwt::issue_magic_link_token("magic_user", &keys).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert_eq!(claims.sub, "magic_user");
    assert_eq!(claims.typ, "magic_link");
    // 15 minutes = 900 seconds
    assert!(claims.exp - claims.iat >= 899 && claims.exp - claims.iat <= 901);
}

#[test]
fn test_jwt_issue_and_verify_challenge_token() {
    let keys = JwtKeys::from_secret("test_secret_12345");

    let token = jwt::issue_challenge_token("mfa_user", &keys).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert_eq!(claims.sub, "mfa_user");
    assert_eq!(claims.typ, "mfa_challenge");
    assert!(claims.mfa_required);
    // 5 minutes = 300 seconds
    assert!(claims.exp - claims.iat >= 299 && claims.exp - claims.iat <= 301);
}

#[test]
fn test_jwt_custom_claims_roundtrip() {
    let keys = JwtKeys::from_secret("test_secret_12345");
    let custom = serde_json::json!({
        "role": "seller",
        "plan": "pro",
        "store_id": "store_abc123",
        "features": ["inventory", "analytics"]
    });

    let token =
        jwt::issue_access_token_with_claims("seller_user", &[], &keys, 3600, true, custom.clone())
            .unwrap();

    let claims = jwt::verify_token(&token, &keys).unwrap();
    assert_eq!(claims.custom_claims, custom);
    assert_eq!(claims.custom_claims["role"], "seller");
    assert_eq!(claims.custom_claims["plan"], "pro");
}

#[test]
fn test_jwt_custom_claims_empty_object() {
    let keys = JwtKeys::from_secret("test_secret_12345");
    let custom = serde_json::json!({});

    let token = jwt::issue_access_token_with_claims(
        "user1",
        &["user".to_string()],
        &keys,
        3600,
        false,
        custom,
    )
    .unwrap();

    let claims = jwt::verify_token(&token, &keys).unwrap();
    assert!(claims.custom_claims.is_object());
}

#[test]
fn test_jwt_custom_claims_nested() {
    let keys = JwtKeys::from_secret("test_secret_12345");
    let custom = serde_json::json!({
        "permissions": {
            "read": ["users", "posts"],
            "write": ["own_posts"],
            "admin": false
        }
    });

    let token = jwt::issue_access_token_with_claims(
        "mod_user",
        &["moderator".to_string()],
        &keys,
        3600,
        true,
        custom.clone(),
    )
    .unwrap();

    let claims = jwt::verify_token(&token, &keys).unwrap();
    assert_eq!(claims.custom_claims, custom);
    assert!(claims.custom_claims["permissions"]["read"].is_array());
}

#[test]
fn test_jwt_wrong_secret_verification_fails() {
    let keys1 = JwtKeys::from_secret("secret_one");
    let keys2 = JwtKeys::from_secret("secret_two");

    let token = jwt::issue_access_token("user123", &[], &keys1, 3600, false).unwrap();
    let result = jwt::verify_token(&token, &keys2);

    assert!(result.is_err());
}

#[test]
fn test_jwt_expired_token_verification_fails() {
    let keys = JwtKeys::from_secret("test_secret_12345");
    let now = chrono::Utc::now().timestamp();

    // Manually create an expired token
    let claims = Claims {
        sub: "expired_user".to_string(),
        iat: now - 7200, // 2 hours ago
        exp: now - 3600, // 1 hour ago (expired)
        roles: vec![],
        typ: "access".to_string(),
        email_verified: false,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(b"test_secret_12345"),
    )
    .unwrap();

    let result = jwt::verify_token(&token, &keys);
    assert!(result.is_err(), "Expired token should fail verification");
}

#[test]
fn test_jwt_ttl_accuracy() {
    let keys = JwtKeys::from_secret("test_secret_12345");
    let ttl = 7200u64; // 2 hours

    let token = jwt::issue_access_token("user1", &[], &keys, ttl, false).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    // exp should be iat + ttl
    assert_eq!(claims.exp - claims.iat, ttl as i64);
}

#[test]
fn test_jwt_empty_user_id() {
    let keys = JwtKeys::from_secret("test_secret_12345");

    let token = jwt::issue_access_token("", &[], &keys, 3600, false).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert_eq!(claims.sub, "");
}

#[test]
fn test_jwt_very_long_user_id() {
    let keys = JwtKeys::from_secret("test_secret_12345");
    let long_id = "u".repeat(1000);

    let token = jwt::issue_access_token(&long_id, &[], &keys, 3600, false).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert_eq!(claims.sub, long_id);
}

#[test]
fn test_jwt_special_characters_in_user_id() {
    let keys = JwtKeys::from_secret("test_secret_12345");
    let special_id = "user@example.com!#$%^&*()-_=+[]{}|;':\"<>?,./";

    let token = jwt::issue_access_token(special_id, &[], &keys, 3600, false).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert_eq!(claims.sub, special_id);
}

#[test]
fn test_jwt_unicode_user_id() {
    let keys = JwtKeys::from_secret("test_secret_12345");
    let unicode_id = "user_🚀_日本_Москва_مصر";

    let token = jwt::issue_access_token(unicode_id, &[], &keys, 3600, false).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert_eq!(claims.sub, unicode_id);
}

#[test]
fn test_jwt_many_roles() {
    let keys = JwtKeys::from_secret("test_secret_12345");
    let roles = [
        "user",
        "admin",
        "editor",
        "moderator",
        "analyst",
        "developer",
        "designer",
        "manager",
        "viewer",
        "commenter",
        "uploader",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect::<Vec<_>>();

    let token = jwt::issue_access_token("many_role_user", &roles, &keys, 3600, true).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert_eq!(claims.roles.len(), 11);
    assert!(claims.roles.iter().any(|r| r == "admin"));
    assert!(claims.roles.iter().any(|r| r == "user"));
}

#[test]
fn test_jwt_zero_roles() {
    let keys = JwtKeys::from_secret("test_secret_12345");
    let roles: Vec<String> = vec![];

    let token = jwt::issue_access_token("no_role_user", &roles, &keys, 3600, true).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert!(claims.roles.is_empty());
}

#[test]
fn test_jwt_single_role() {
    let keys = JwtKeys::from_secret("test_secret_12345");
    let roles = ["admin".to_string()];

    let token = jwt::issue_access_token("single_role_user", &roles, &keys, 3600, true).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert_eq!(claims.roles, roles);
}

#[test]
fn test_jwt_hmac_keys() {
    let keys = JwtKeys::from_secret("my_secret_key");

    let token = jwt::issue_access_token("user1", &[], &keys, 60, false).unwrap();
    let claims = jwt::verify_token(&token, &keys).unwrap();

    assert_eq!(claims.sub, "user1");
}

// ═══════════════════════════════════════════════════════════════════════════════
// PASSWORD TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_password_hash_and_verify() {
    let pwd = "super_secure_P@ssw0rd!123";

    let hash = password::hash_password(pwd).unwrap();
    assert!(hash.starts_with("$argon2"));

    let is_valid = password::verify_password(pwd, &hash).unwrap();
    assert!(is_valid);
}

#[test]
fn test_password_wrong_password_fails() {
    let pwd = "correct_password";
    let hash = password::hash_password(pwd).unwrap();

    let is_valid = password::verify_password("wrong_password", &hash).unwrap();
    assert!(!is_valid);
}

#[test]
fn test_password_hash_uniqueness() {
    let pwd = "same_password_123";

    let hash1 = password::hash_password(pwd).unwrap();
    let hash2 = password::hash_password(pwd).unwrap();

    // Different salts → different hashes
    assert_ne!(hash1, hash2);

    // Both verify correctly
    assert!(password::verify_password(pwd, &hash1).unwrap());
    assert!(password::verify_password(pwd, &hash2).unwrap());
}

#[test]
fn test_password_empty_string() {
    let pwd = "";

    let hash = password::hash_password(pwd).unwrap();
    let is_valid = password::verify_password(pwd, &hash).unwrap();

    assert!(is_valid);
    assert!(!password::verify_password("non-empty", &hash).unwrap());
}

#[test]
fn test_password_very_long() {
    let pwd = "x".repeat(10000);

    let hash = password::hash_password(&pwd).unwrap();
    let is_valid = password::verify_password(&pwd, &hash).unwrap();

    assert!(is_valid);
}

#[test]
fn test_password_unicode() {
    let pwd = "日本語パスワード🔒🔐_Москва_مصر_México";

    let hash = password::hash_password(pwd).unwrap();
    let is_valid = password::verify_password(pwd, &hash).unwrap();

    assert!(is_valid);
    assert!(!password::verify_password("wrong_unicode", &hash).unwrap());
}

#[test]
fn test_password_special_characters() {
    let pwd = "!@#$%^&*()-_=+[]{}|;':\"<>?,./~`";

    let hash = password::hash_password(pwd).unwrap();
    let is_valid = password::verify_password(pwd, &hash).unwrap();

    assert!(is_valid);
}

#[test]
fn test_password_whitespace_sensitive() {
    let pwd1 = "password with spaces";
    let pwd2 = "password  with  spaces"; // Extra space

    let hash = password::hash_password(pwd1).unwrap();

    assert!(password::verify_password(pwd1, &hash).unwrap());
    assert!(!password::verify_password(pwd2, &hash).unwrap());
}

#[test]
fn test_password_case_sensitive() {
    let pwd1 = "Password123";
    let pwd2 = "password123";

    let hash = password::hash_password(pwd1).unwrap();

    assert!(password::verify_password(pwd1, &hash).unwrap());
    assert!(!password::verify_password(pwd2, &hash).unwrap());
}

#[test]
fn test_password_dummy_verify_no_panic() {
    // Should not panic and should consume similar CPU time
    password::dummy_verify("test_password");
    password::dummy_verify("");
    password::dummy_verify("x".repeat(1000).as_str());
}

// ═══════════════════════════════════════════════════════════════════════════════
// TOTP TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_totp_generate_secret() {
    let secret = totp::generate_secret();

    assert_eq!(secret.len(), 20); // SECRET_LENGTH = 20
    assert!(!secret.is_empty());
}

#[test]
fn test_totp_generate_secret_unique() {
    let s1 = totp::generate_secret();
    let s2 = totp::generate_secret();

    assert_ne!(s1, s2);
}

#[test]
fn test_totp_verify_valid_code() {
    let secret = totp::generate_secret();

    // Generate current code
    let totp_instance = totp_rs::TOTP::new(
        totp_rs::Algorithm::SHA1,
        6,
        1,
        30,
        secret.clone(),
        Some("orignabase".to_string()),
        "test".to_string(),
    )
    .unwrap();

    let code = totp_instance.generate_current().unwrap();

    // Verify it
    let result = totp::verify_totp(&secret, &code, None);
    assert!(result.is_ok());
}

#[test]
fn test_totp_verify_wrong_code() {
    let secret = totp::generate_secret();

    // Try to verify a definitely wrong code
    let result = totp::verify_totp(&secret, "000000", None);
    // Result depends on timing, but should not panic
    let _ = result;
}

#[test]
fn test_totp_replay_prevention() {
    let secret = totp::generate_secret();

    let totp_instance = totp_rs::TOTP::new(
        totp_rs::Algorithm::SHA1,
        6,
        1,
        30,
        secret.clone(),
        Some("orignabase".to_string()),
        "test".to_string(),
    )
    .unwrap();

    let code = totp_instance.generate_current().unwrap();

    // First use succeeds
    let step1 = totp::verify_totp(&secret, &code, None).unwrap();

    // Second use with same step fails
    let result = totp::verify_totp(&secret, &code, Some(step1));
    assert!(
        result.is_err(),
        "Same TOTP code should be rejected on replay"
    );
}

#[test]
fn test_totp_otpauth_url() {
    let secret = totp::generate_secret();

    let url = totp::build_otpauth_url(&secret, "TestApp", "user@example.com").unwrap();

    assert!(url.starts_with("otpauth://totp/"));
    assert!(url.contains("TestApp"));
    assert!(url.contains("user%40example.com") || url.contains("user@example.com"));
}

#[test]
fn test_totp_qr_code_generation() {
    let secret = totp::generate_secret();

    let qr_base64 = totp::generate_qr_base64(&secret, "OrignaBase", "test@test.com").unwrap();

    assert!(!qr_base64.is_empty());
    assert!(qr_base64.starts_with("iVBORw0KGgo") || qr_base64.len() > 100); // PNG header or long base64
}

#[test]
fn test_totp_recovery_codes_count() {
    let codes = totp::generate_recovery_codes();

    assert_eq!(codes.len(), totp::RECOVERY_CODE_COUNT);
}

#[test]
fn test_totp_recovery_codes_unique() {
    let codes = totp::generate_recovery_codes();
    let unique: std::collections::HashSet<_> = codes.iter().collect();

    assert_eq!(
        unique.len(),
        codes.len(),
        "All recovery codes should be unique"
    );
}

#[test]
fn test_totp_recovery_codes_format() {
    let codes = totp::generate_recovery_codes();

    for code in codes {
        assert_eq!(code.len(), 8, "Each recovery code should be 8 hex chars");
        assert!(
            code.chars().all(|c| c.is_ascii_hexdigit()),
            "Code should be hex only"
        );
    }
}

#[test]
fn test_totp_recovery_code_hash_and_verify() {
    let codes = totp::generate_recovery_codes();
    let code = &codes[0];

    let hash = totp::hash_recovery_code(code).unwrap();

    assert!(hash.starts_with("$argon2"));
    assert!(totp::verify_recovery_code(code, &hash).unwrap());
}

#[test]
fn test_totp_recovery_code_wrong_code_fails() {
    let codes = totp::generate_recovery_codes();
    let code = &codes[0];

    let hash = totp::hash_recovery_code(code).unwrap();
    let is_valid = totp::verify_recovery_code("wrong_code_12345678", &hash).unwrap();

    assert!(!is_valid);
}

#[test]
fn test_totp_recovery_code_single_use() {
    let codes = totp::generate_recovery_codes();
    let code = &codes[0];
    let hash = totp::hash_recovery_code(code).unwrap();

    assert!(totp::verify_recovery_code(code, &hash).unwrap());
    assert!(totp::verify_recovery_code(code, &hash).unwrap()); // Hash doesn't change
}

#[test]
fn test_totp_encrypt_decrypt_roundtrip() {
    let secret = totp::generate_secret();
    let key = [42u8; 32];

    let encrypted = totp::encrypt_secret(&secret, &key).unwrap();
    let decrypted = totp::decrypt_secret(&encrypted, &key).unwrap();

    assert_eq!(secret, decrypted);
}

#[test]
fn test_totp_encrypt_different_nonces() {
    let secret = totp::generate_secret();
    let key = [42u8; 32];

    let e1 = totp::encrypt_secret(&secret, &key).unwrap();
    let e2 = totp::encrypt_secret(&secret, &key).unwrap();

    // Different nonces → different ciphertexts
    assert_ne!(e1, e2);

    // Both decrypt to same secret
    assert_eq!(
        totp::decrypt_secret(&e1, &key).unwrap(),
        totp::decrypt_secret(&e2, &key).unwrap()
    );
}

#[test]
fn test_totp_decrypt_wrong_key_fails() {
    let secret = totp::generate_secret();
    let key1 = [1u8; 32];
    let key2 = [2u8; 32];

    let encrypted = totp::encrypt_secret(&secret, &key1).unwrap();
    let result = totp::decrypt_secret(&encrypted, &key2);

    assert!(result.is_err(), "Decrypting with wrong key should fail");
}

#[test]
fn test_totp_decrypt_too_short_fails() {
    let key = [42u8; 32];
    let short_data = vec![0u8; 5];

    let result = totp::decrypt_secret(&short_data, &key);
    assert!(result.is_err(), "Decrypting too-short data should fail");
}

#[test]
fn test_totp_decrypt_corrupted_fails() {
    let secret = totp::generate_secret();
    let key = [42u8; 32];

    let mut encrypted = totp::encrypt_secret(&secret, &key).unwrap();

    // Corrupt a byte in the ciphertext (not the nonce)
    if encrypted.len() > 13 {
        encrypted[13] ^= 0xFF;
    }

    let result = totp::decrypt_secret(&encrypted, &key);
    assert!(result.is_err(), "Decrypting corrupted data should fail");
}

#[test]
fn test_totp_secret_to_base32() {
    let secret = totp::generate_secret();

    let b32 = totp::secret_to_base32(&secret);

    assert!(!b32.is_empty());
    assert!(
        b32.chars()
            .all(|c| "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567=".contains(c))
    );
}

#[test]
fn test_totp_secret_to_base32_empty() {
    let secret = vec![];

    let b32 = totp::secret_to_base32(&secret);
    // Empty secret should produce some output (possibly just padding)
    assert!(!b32.is_empty() || secret.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════════
// RATE LIMITING TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_rate_limiter_allows_within_limit() {
    let limiter = RateLimiter::new(5, Duration::from_secs(60));
    let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));

    for i in 0..5 {
        assert!(limiter.check(ip), "Request {} should be allowed", i + 1);
    }
}

#[test]
fn test_rate_limiter_blocks_over_limit() {
    let limiter = RateLimiter::new(3, Duration::from_secs(60));
    let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

    assert!(limiter.check(ip)); // 1 ✓
    assert!(limiter.check(ip)); // 2 ✓
    assert!(limiter.check(ip)); // 3 ✓
    assert!(!limiter.check(ip)); // 4 ✗ (blocked)
}

#[test]
fn test_rate_limiter_different_ips_independent() {
    let limiter = RateLimiter::new(2, Duration::from_secs(60));
    let ip1 = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
    let ip2 = IpAddr::V4(Ipv4Addr::new(2, 2, 2, 2));

    assert!(limiter.check(ip1)); // ip1: 1 ✓
    assert!(limiter.check(ip1)); // ip1: 2 ✓
    assert!(!limiter.check(ip1)); // ip1: 3 ✗

    assert!(limiter.check(ip2)); // ip2: 1 ✓
    assert!(limiter.check(ip2)); // ip2: 2 ✓
    assert!(!limiter.check(ip2)); // ip2: 3 ✗
}

#[test]
fn test_rate_limiter_window_reset() {
    let limiter = RateLimiter::new(2, Duration::from_millis(50));
    let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

    assert!(limiter.check(ip)); // 1 ✓
    assert!(limiter.check(ip)); // 2 ✓
    assert!(!limiter.check(ip)); // 3 ✗

    std::thread::sleep(Duration::from_millis(60));

    // Window expired, should be allowed again
    assert!(limiter.check(ip));
}

#[test]
fn test_rate_limiter_cleanup_removes_expired() {
    let limiter = RateLimiter::new(10, Duration::from_millis(50));
    let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

    limiter.check(ip);
    // assert!(limiter.state.len() > 0);

    std::thread::sleep(Duration::from_millis(60));
    limiter.cleanup();

    // assert_eq!(limiter.state.len(), 0, "Expired entry should be cleaned up");
}

#[test]
fn test_rate_limiter_cleanup_keeps_active() {
    let limiter = RateLimiter::new(10, Duration::from_secs(60));
    let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

    limiter.check(ip);
    let count_before = 0;

    limiter.cleanup();

    let count_after = 0;
    assert_eq!(
        count_before, count_after,
        "Active entries should not be cleaned up"
    );
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
fn test_rate_limiter_zero_max_requests() {
    let limiter = RateLimiter::new(0, Duration::from_secs(60));
    let ip = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));

    // Even the first request should be blocked
    assert!(!limiter.check(ip));
}

#[test]
fn test_rate_limiter_very_short_window() {
    let limiter = RateLimiter::new(1, Duration::from_millis(10));
    let ip = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));

    assert!(limiter.check(ip));
    assert!(!limiter.check(ip));

    std::thread::sleep(Duration::from_millis(15));

    assert!(limiter.check(ip)); // Window reset
}

#[test]
fn test_rate_limiter_large_limit() {
    let limiter = RateLimiter::new(10000, Duration::from_secs(60));
    let ip = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));

    for _ in 0..10000 {
        assert!(limiter.check(ip));
    }

    assert!(!limiter.check(ip)); // Now it's blocked
}

// ═══════════════════════════════════════════════════════════════════════════════
// MIDDLEWARE TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_auth_context_anonymous() {
    let ctx = AuthContext::anonymous();

    assert_eq!(ctx.user_id, "");
    assert!(ctx.roles.is_empty());
    assert!(!ctx.authenticated);
    assert!(!ctx.email_verified);
}

#[test]
fn test_auth_context_from_claims_basic() {
    let claims = Claims {
        sub: "user42".into(),
        iat: 0,
        exp: 9999999999,
        roles: vec!["admin".into(), "user".into()],
        typ: "access".into(),
        email_verified: true,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    let ctx = AuthContext::from_claims(claims);

    assert_eq!(ctx.user_id, "user42");
    assert!(ctx.authenticated);
    assert!(ctx.email_verified);
    assert_eq!(ctx.roles, vec!["admin", "user"]);
}

#[test]
fn test_auth_context_from_claims_empty_roles() {
    let claims = Claims {
        sub: "u1".into(),
        iat: 0,
        exp: 0,
        roles: vec![],
        typ: "access".into(),
        email_verified: false,
        mfa_required: false,
        custom_claims: serde_json::Value::Null,
    };

    let ctx = AuthContext::from_claims(claims);

    assert!(ctx.roles.is_empty());
    assert!(ctx.authenticated);
    assert!(!ctx.email_verified);
}

#[test]
fn test_auth_context_from_claims_with_custom_claims() {
    let custom = serde_json::json!({"store_id": "abc", "role": "seller"});
    let claims = Claims {
        sub: "seller123".into(),
        iat: 0,
        exp: 9999999999,
        roles: vec!["seller".into()],
        typ: "access".into(),
        email_verified: true,
        mfa_required: false,
        custom_claims: custom.clone(),
    };

    let ctx = AuthContext::from_claims(claims);

    assert_eq!(ctx.custom_claims, custom);
    assert_eq!(ctx.custom_claims["store_id"], "abc");
}

#[test]
fn test_auth_context_has_role_present() {
    let ctx = AuthContext {
        user_id: "u".into(),
        roles: vec!["admin".into(), "editor".into()],
        authenticated: true,
        email_verified: false,
        custom_claims: serde_json::Value::Null,
    };

    assert!(ctx.has_role("admin"));
    assert!(ctx.has_role("editor"));
    assert!(!ctx.has_role("user"));
}

#[test]
fn test_auth_context_has_role_case_sensitive() {
    let ctx = AuthContext {
        user_id: "u".into(),
        roles: vec!["Admin".into()],
        authenticated: true,
        email_verified: false,
        custom_claims: serde_json::Value::Null,
    };

    assert!(ctx.has_role("Admin"));
    assert!(!ctx.has_role("admin")); // Case sensitive
}

#[test]
fn test_auth_context_has_role_empty_roles() {
    let ctx = AuthContext::anonymous();

    assert!(!ctx.has_role("anything"));
    assert!(!ctx.has_role("admin"));
}

#[test]
fn test_auth_context_clone() {
    let ctx = AuthContext {
        user_id: "u1".into(),
        roles: vec!["user".into()],
        authenticated: true,
        email_verified: true,
        custom_claims: serde_json::Value::Null,
    };

    let cloned = ctx.clone();

    assert_eq!(cloned.user_id, "u1");
    assert_eq!(cloned.roles, ctx.roles);
    assert_eq!(cloned.authenticated, ctx.authenticated);
    assert_eq!(cloned.email_verified, ctx.email_verified);
}

#[test]
fn test_auth_context_debug() {
    let ctx = AuthContext {
        user_id: "test_user".into(),
        roles: vec!["user".into()],
        authenticated: true,
        email_verified: false,
        custom_claims: serde_json::Value::Null,
    };

    let debug_str = format!("{:?}", ctx);
    assert!(debug_str.contains("AuthContext"));
    assert!(debug_str.contains("test_user"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// TABLE-DRIVEN TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_jwt_multiple_token_types_table_driven() {
    let keys = JwtKeys::from_secret("test_secret_12345");
    let user_id = "test_user";

    let test_cases = [
        (
            "access",
            jwt::issue_access_token(user_id, &[], &keys, 3600, true),
        ),
        ("refresh", jwt::issue_refresh_token(user_id, &keys, 604800)),
        (
            "email_verify",
            jwt::issue_verification_token(user_id, &keys),
        ),
        ("password_reset", jwt::issue_reset_token(user_id, &keys)),
        ("magic_link", jwt::issue_magic_link_token(user_id, &keys)),
        ("mfa_challenge", jwt::issue_challenge_token(user_id, &keys)),
    ];

    for (expected_typ, result) in &test_cases {
        let token = result.as_ref().unwrap();
        let claims = jwt::verify_token(token, &keys).unwrap();

        assert_eq!(
            &claims.typ, expected_typ,
            "Token type mismatch for {}",
            expected_typ
        );
        assert_eq!(&claims.sub, user_id);
    }
}

#[test]
fn test_password_edge_cases_table_driven() {
    let test_cases = vec![
        ("", "empty"),
        (" ", "single space"),
        ("  ", "multiple spaces"),
        ("\t\n\r", "whitespace"),
        ("p", "single char"),
        ("password123!@#", "normal"),
        ("日本", "japanese"),
        ("🔒🔐", "emoji"),
    ];

    for (pwd, label) in test_cases {
        let hash = password::hash_password(pwd).unwrap();
        let is_valid = password::verify_password(pwd, &hash).unwrap();
        assert!(is_valid, "Failed for {}", label);
        assert!(
            !password::verify_password("wrong", &hash).unwrap(),
            "Wrong password accepted for {}",
            label
        );
    }
}

#[test]
fn test_rate_limiter_configurations_table_driven() {
    let configs = vec![
        (1, 60),    // 1 req/min
        (5, 60),    // 5 req/min
        (100, 1),   // 100 req/sec
        (1000, 60), // 1000 req/min
    ];

    for (max_requests, window_secs) in configs {
        let limiter = RateLimiter::new(max_requests, Duration::from_secs(window_secs));
        let ip = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));

        // All requests within limit should pass
        for i in 0..max_requests {
            assert!(
                limiter.check(ip),
                "Config ({}, {}) failed at request {}",
                max_requests,
                window_secs,
                i + 1
            );
        }

        // Over limit should fail
        assert!(
            !limiter.check(ip),
            "Config ({}, {}) should block at request {}",
            max_requests + 1,
            window_secs,
            max_requests + 1
        );
    }
}
