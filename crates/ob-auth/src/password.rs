use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use ob_core::{Error, Result};

/// Hash a password using Argon2id (OWASP recommended).
pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();

    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Internal(format!("Password hashing failed: {e}")))
}

/// Run a dummy argon2id hash to prevent timing-based user enumeration.
/// When a login attempt fails because the user doesn't exist, this function
/// burns the same CPU time as a real password verification would, ensuring
/// the response time is indistinguishable from a real failed login.
pub fn dummy_verify(password: &str) {
    let salt = SaltString::generate(&mut OsRng);
    let _ = Argon2::default().hash_password(password.as_bytes(), &salt);
}

/// Verify a password against its Argon2id hash.
pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    let parsed =
        PasswordHash::new(hash).map_err(|e| Error::Internal(format!("Invalid hash: {e}")))?;

    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_and_verify() {
        let password = "super_secure_p@ssw0rd!";
        let hash = hash_password(password).unwrap();

        assert!(hash.starts_with("$argon2"));
        assert!(verify_password(password, &hash).unwrap());
        assert!(!verify_password("wrong_password", &hash).unwrap());
    }

    #[test]
    fn test_dummy_verify_does_not_panic() {
        // dummy_verify should complete without panicking and burn CPU time
        dummy_verify("any_password_here");
    }

    #[test]
    fn test_different_passwords_different_hashes() {
        let h1 = hash_password("password1").unwrap();
        let h2 = hash_password("password1").unwrap();
        // Same password, different salts → different hashes
        assert_ne!(h1, h2);
    }
}

#[test]
fn test_hash_format_is_argon2() {
    let password = "test_password";
    let hash = hash_password(password).unwrap();
    assert!(hash.starts_with("$argon2id$") || hash.starts_with("$argon2i$"));
}

#[test]
fn test_verify_password_correct() {
    let password = "correct_password_123!@#";
    let hash = hash_password(password).unwrap();
    assert!(verify_password(password, &hash).unwrap());
}

#[test]
fn test_verify_password_incorrect() {
    let password = "correct_password";
    let wrong = "wrong_password";
    let hash = hash_password(password).unwrap();
    assert!(!verify_password(wrong, &hash).unwrap());
}

#[test]
fn test_verify_password_empty_password() {
    let password = "not_empty";
    let hash = hash_password(password).unwrap();
    assert!(!verify_password("", &hash).unwrap());
}

#[test]
fn test_verify_password_case_sensitive() {
    let password = "MyPassword";
    let hash = hash_password(password).unwrap();
    assert!(!verify_password("mypassword", &hash).unwrap());
    assert!(!verify_password("MYPASSWORD", &hash).unwrap());
}

#[test]
fn test_unicode_password() {
    let password = "pässwörd_with_üñícödé_🔐";
    let hash = hash_password(password).unwrap();
    assert!(verify_password(password, &hash).unwrap());
    assert!(!verify_password("pässwörd_with_üñícödé", &hash).unwrap());
}

#[test]
fn test_very_long_password() {
    let password = "a".repeat(1000);
    let hash = hash_password(&password).unwrap();
    assert!(verify_password(&password, &hash).unwrap());
}

#[test]
fn test_special_characters_password() {
    let password = "!@#$%^&*()_+-=[]{}|;':\",./<>?";
    let hash = hash_password(password).unwrap();
    assert!(verify_password(password, &hash).unwrap());
}

#[test]
fn test_whitespace_password() {
    let password = "  spaces  everywhere  ";
    let hash = hash_password(password).unwrap();
    assert!(verify_password(password, &hash).unwrap());
    assert!(!verify_password("spacesevrywhere", &hash).unwrap());
}

#[test]
fn test_same_password_different_hashes() {
    let password = "same_password";
    let hash1 = hash_password(password).unwrap();
    let hash2 = hash_password(password).unwrap();

    // Different salts mean different hashes
    assert_ne!(hash1, hash2);

    // But both should verify
    assert!(verify_password(password, &hash1).unwrap());
    assert!(verify_password(password, &hash2).unwrap());
}

#[test]
fn test_invalid_hash_format() {
    let bad_hash = "not_a_valid_argon2_hash";
    let result = verify_password("password", bad_hash);
    assert!(result.is_err());
}

#[test]
fn test_dummy_verify_completes() {
    // Should not panic, should complete execution
    dummy_verify("test_password");
    dummy_verify("");
    dummy_verify("very_long_password_to_test_cpu_burn");
}

#[test]
fn test_timing_dummy_verify_similar_to_real() {
    use std::time::Instant;

    let password = "test_password_12345";
    let hash = hash_password(password).unwrap();

    let start_dummy = Instant::now();
    dummy_verify(password);
    let dummy_time = start_dummy.elapsed();

    let start_real = Instant::now();
    let _ = verify_password(password, &hash);
    let real_time = start_real.elapsed();

    // Dummy and real should be in similar ballpark (within 2x)
    // Note: This is a rough heuristic, timing can vary
    let ratio = dummy_time.as_millis() as f64 / real_time.as_millis().max(1) as f64;
    assert!(
        ratio > 0.5 && ratio < 2.0,
        "Timing mismatch: dummy {}, real {}",
        dummy_time.as_millis(),
        real_time.as_millis()
    );
}

#[test]
fn test_numeric_only_password() {
    let password = "1234567890";
    let hash = hash_password(password).unwrap();
    assert!(verify_password(password, &hash).unwrap());
    assert!(!verify_password("0987654321", &hash).unwrap());
}

#[test]
fn test_single_character_password() {
    let password = "a";
    let hash = hash_password(password).unwrap();
    assert!(verify_password(password, &hash).unwrap());
    assert!(!verify_password("b", &hash).unwrap());
}
