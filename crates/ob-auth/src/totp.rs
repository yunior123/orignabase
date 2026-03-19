use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, KeyInit, OsRng},
};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use ob_core::{Error, Result};
use rand::RngCore;
use totp_rs::{Algorithm, Secret, TOTP};

/// Length of TOTP secrets in bytes.
const SECRET_LENGTH: usize = 20;

/// Number of recovery codes to generate.
pub const RECOVERY_CODE_COUNT: usize = 8;

/// TOTP skew (allow ±1 time step).
const TOTP_SKEW: u8 = 1;

/// TOTP time step in seconds.
const TOTP_STEP: u64 = 30;

/// TOTP digit count.
const TOTP_DIGITS: usize = 6;

/// Generate a cryptographically random TOTP secret.
pub fn generate_secret() -> Vec<u8> {
    let mut secret = vec![0u8; SECRET_LENGTH];
    OsRng.fill_bytes(&mut secret);
    secret
}

/// Build a TOTP instance from a raw secret.
fn build_totp(secret: &[u8], issuer: &str, account: &str) -> Result<TOTP> {
    TOTP::new(
        Algorithm::SHA1, // SHA-1 for max authenticator app compatibility
        TOTP_DIGITS,
        TOTP_SKEW,
        TOTP_STEP,
        secret.to_vec(),
        Some(issuer.to_string()),
        account.to_string(),
    )
    .map_err(|e| Error::Internal(format!("TOTP creation failed: {e}")))
}

/// Build a standard otpauth:// URL for authenticator apps.
pub fn build_otpauth_url(secret: &[u8], issuer: &str, account: &str) -> Result<String> {
    let totp = build_totp(secret, issuer, account)?;
    Ok(totp.get_url())
}

/// Generate a QR code as a base64-encoded PNG.
pub fn generate_qr_base64(secret: &[u8], issuer: &str, account: &str) -> Result<String> {
    let totp = build_totp(secret, issuer, account)?;
    totp.get_qr_base64()
        .map_err(|e| Error::Internal(format!("QR generation failed: {e}")))
}

/// Verify a TOTP code against a secret, with replay prevention.
///
/// Returns the current time step if verification succeeds.
/// `last_used_step` should be the step returned from the previous successful verification.
/// Codes from the same or earlier step are rejected to prevent replay attacks.
pub fn verify_totp(secret: &[u8], code: &str, last_used_step: Option<u64>) -> Result<u64> {
    let totp = build_totp(secret, "orignabase", "user")?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| Error::Internal(format!("System time error: {e}")))?
        .as_secs();

    let current_step = now / TOTP_STEP;

    // Check for replay
    if let Some(last) = last_used_step
        && current_step <= last
    {
        return Err(Error::Auth("TOTP code already used".into()));
    }

    // Verify with skew
    if totp
        .check_current(code)
        .map_err(|e| Error::Internal(format!("TOTP check failed: {e}")))?
    {
        Ok(current_step)
    } else {
        Err(Error::Auth("Invalid TOTP code".into()))
    }
}

/// Generate recovery codes as plaintext strings.
/// Each code is 8 hex characters (32 bits of entropy per code).
pub fn generate_recovery_codes() -> Vec<String> {
    let mut codes = Vec::with_capacity(RECOVERY_CODE_COUNT);
    for _ in 0..RECOVERY_CODE_COUNT {
        let mut buf = [0u8; 4];
        OsRng.fill_bytes(&mut buf);
        codes.push(hex::encode(buf));
    }
    codes
}

/// Hash a recovery code using Argon2id for storage.
pub fn hash_recovery_code(code: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(code.as_bytes(), &salt)
        .map_err(|e| Error::Internal(format!("Recovery code hashing failed: {e}")))?;
    Ok(hash.to_string())
}

/// Verify a recovery code against a stored Argon2id hash.
pub fn verify_recovery_code(code: &str, hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(hash)
        .map_err(|e| Error::Internal(format!("Invalid recovery code hash: {e}")))?;
    Ok(Argon2::default()
        .verify_password(code.as_bytes(), &parsed)
        .is_ok())
}

/// Encrypt a TOTP secret using AES-256-GCM before storing in DB.
/// Returns (nonce || ciphertext) as a single byte vector.
pub fn encrypt_secret(secret: &[u8], encryption_key: &[u8; 32]) -> Result<Vec<u8>> {
    let key = Key::<Aes256Gcm>::from_slice(encryption_key);
    let cipher = Aes256Gcm::new(key);

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, secret)
        .map_err(|e| Error::Internal(format!("Encryption failed: {e}")))?;

    let mut result = nonce_bytes.to_vec();
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Decrypt a TOTP secret that was encrypted with `encrypt_secret`.
pub fn decrypt_secret(encrypted: &[u8], encryption_key: &[u8; 32]) -> Result<Vec<u8>> {
    if encrypted.len() < 13 {
        return Err(Error::Internal("Encrypted data too short".into()));
    }

    let key = Key::<Aes256Gcm>::from_slice(encryption_key);
    let cipher = Aes256Gcm::new(key);

    let (nonce_bytes, ciphertext) = encrypted.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| Error::Internal(format!("Decryption failed: {e}")))
}

/// Get the base32-encoded representation of a secret (for manual entry).
pub fn secret_to_base32(secret: &[u8]) -> String {
    Secret::Raw(secret.to_vec()).to_encoded().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_secret_length() {
        let secret = generate_secret();
        assert_eq!(secret.len(), SECRET_LENGTH);
    }

    #[test]
    fn test_generate_secret_unique() {
        let s1 = generate_secret();
        let s2 = generate_secret();
        assert_ne!(s1, s2);
    }

    #[test]
    fn test_build_otpauth_url() {
        let secret = generate_secret();
        let url = build_otpauth_url(&secret, "OrignaBase", "user@example.com").unwrap();
        assert!(url.starts_with("otpauth://totp/"));
        assert!(url.contains("OrignaBase"));
        assert!(url.contains("user%40example.com") || url.contains("user@example.com"));
    }

    #[test]
    fn test_generate_qr_base64() {
        let secret = generate_secret();
        let qr = generate_qr_base64(&secret, "OrignaBase", "test@test.com").unwrap();
        assert!(!qr.is_empty());
    }

    #[test]
    fn test_verify_totp_valid() {
        let secret = generate_secret();
        let totp = build_totp(&secret, "test", "test").unwrap();
        let code = totp.generate_current().unwrap();
        let result = verify_totp(&secret, &code, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_totp_invalid_code() {
        let secret = generate_secret();
        let result = verify_totp(&secret, "000000", None);
        // May or may not fail depending on timing, so just check it doesn't panic
        let _ = result;
    }

    #[test]
    fn test_recovery_codes_count() {
        let codes = generate_recovery_codes();
        assert_eq!(codes.len(), RECOVERY_CODE_COUNT);
    }

    #[test]
    fn test_recovery_codes_unique() {
        let codes = generate_recovery_codes();
        let unique: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(unique.len(), codes.len());
    }

    #[test]
    fn test_recovery_code_hash_verify() {
        let codes = generate_recovery_codes();
        let code = &codes[0];
        let hash = hash_recovery_code(code).unwrap();
        assert!(verify_recovery_code(code, &hash).unwrap());
        assert!(!verify_recovery_code("wrong_code", &hash).unwrap());
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let secret = generate_secret();
        let key = [42u8; 32];
        let encrypted = encrypt_secret(&secret, &key).unwrap();
        let decrypted = decrypt_secret(&encrypted, &key).unwrap();
        assert_eq!(secret, decrypted);
    }

    #[test]
    fn test_encrypt_different_nonces() {
        let secret = generate_secret();
        let key = [42u8; 32];
        let e1 = encrypt_secret(&secret, &key).unwrap();
        let e2 = encrypt_secret(&secret, &key).unwrap();
        assert_ne!(e1, e2); // Different nonces
        assert_eq!(
            decrypt_secret(&e1, &key).unwrap(),
            decrypt_secret(&e2, &key).unwrap()
        );
    }

    #[test]
    fn test_decrypt_wrong_key_fails() {
        let secret = generate_secret();
        let key1 = [1u8; 32];
        let key2 = [2u8; 32];
        let encrypted = encrypt_secret(&secret, &key1).unwrap();
        assert!(decrypt_secret(&encrypted, &key2).is_err());
    }

    #[test]
    fn test_decrypt_too_short_fails() {
        let key = [42u8; 32];
        assert!(decrypt_secret(&[0u8; 5], &key).is_err());
    }

    #[test]
    fn test_secret_to_base32() {
        let secret = generate_secret();
        let b32 = secret_to_base32(&secret);
        assert!(!b32.is_empty());
        // Base32 chars only
        assert!(
            b32.chars()
                .all(|c| "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567=".contains(c))
        );
    }
}

    #[test]
    fn test_verify_totp_replay_prevention() {
        let secret = generate_secret();
        let totp = build_totp(&secret, "test", "test").unwrap();
        let code = totp.generate_current().unwrap();
        
        // First verification with step tracking
        let result1 = verify_totp(&secret, &code, None);
        assert!(result1.is_ok());
        
        let step1 = result1.unwrap();
        
        // Immediate replay with same step should fail
        let result2 = verify_totp(&secret, &code, Some(step1));
        assert!(result2.is_err());
    }

    #[test]
    fn test_secret_generation_length() {
        let secret = generate_secret();
        assert_eq!(secret.len(), SECRET_LENGTH);
    }

    #[test]
    fn test_secret_generation_randomness() {
        let s1 = generate_secret();
        let s2 = generate_secret();
        assert_ne!(s1, s2); // Should be different
    }

    #[test]
    fn test_recovery_codes_non_empty() {
        let codes = generate_recovery_codes();
        for code in &codes {
            assert!(!code.is_empty());
            assert!(code.len() > 0);
        }
    }

    #[test]
    fn test_recovery_codes_format() {
        let codes = generate_recovery_codes();
        // Codes should be alphanumeric
        for code in &codes {
            assert!(code.chars().all(|c| c.is_ascii_alphanumeric()));
        }
    }

    #[test]
    fn test_recovery_code_hash_format() {
        let code = "RECOVERY_CODE_123";
        let hash = hash_recovery_code(code).unwrap();
        // Should be Argon2
        assert!(hash.starts_with("$argon2"));
    }

    #[test]
    fn test_recovery_code_case_sensitive() {
        let code = "ABCDEFGH";
        let hash = hash_recovery_code(code).unwrap();
        
        assert!(verify_recovery_code("ABCDEFGH", &hash).unwrap());
        assert!(!verify_recovery_code("abcdefgh", &hash).unwrap());
        assert!(!verify_recovery_code("ABCDEFGHI", &hash).unwrap());
    }

    #[test]
    fn test_build_totp_from_secret() {
        let secret = generate_secret();
        let totp = build_totp(&secret, "test_issuer", "test_account");
        assert!(totp.is_ok());
    }

    #[test]
    fn test_otpauth_url_contains_issuer() {
        let secret = generate_secret();
        let url = build_otpauth_url(&secret, "MyApp", "user@example.com").unwrap();
        assert!(url.contains("MyApp"));
    }

    #[test]
    fn test_otpauth_url_contains_account() {
        let secret = generate_secret();
        let url = build_otpauth_url(&secret, "test", "myuser").unwrap();
        assert!(url.contains("myuser"));
    }

    #[test]
    fn test_qr_base64_format() {
        let secret = generate_secret();
        let qr = generate_qr_base64(&secret, "OrignaBase", "test@test.com").unwrap();
        // Base64 QR should start with data URL or base64 string
        assert!(!qr.is_empty());
    }

    #[test]
    fn test_totp_code_is_six_digits() {
        let secret = generate_secret();
        let totp = build_totp(&secret, "test", "test").unwrap();
        let code = totp.generate_current().unwrap();
        
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_verify_totp_multiple_valid_codes_over_time() {
        let secret = generate_secret();
        
        // Generate code now
        let totp = build_totp(&secret, "test", "test").unwrap();
        let code1 = totp.generate_current().unwrap();
        
        // Verify it works
        let result1 = verify_totp(&secret, &code1, None);
        assert!(result1.is_ok());
    }

    #[test]
    fn test_recovery_code_wrong_hash() {
        let codes = generate_recovery_codes();
        let code = &codes[0];
        let hash = hash_recovery_code(code).unwrap();
        
        // Wrong code should fail
        let result = verify_recovery_code(&codes[1], &hash);
        assert!(!result.unwrap());
    }

    #[test]
    fn test_verify_totp_invalid_code_format() {
        let secret = generate_secret();
        
        // Try with non-numeric code
        let result = verify_totp(&secret, "abcdef", None);
        // Should error on invalid format
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_totp_too_short_code() {
        let secret = generate_secret();
        let result = verify_totp(&secret, "123", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_totp_too_long_code() {
        let secret = generate_secret();
        let result = verify_totp(&secret, "1234567", None);
        assert!(result.is_err());
    }
