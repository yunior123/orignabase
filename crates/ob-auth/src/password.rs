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
