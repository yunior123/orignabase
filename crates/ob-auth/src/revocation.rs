//! Token revocation system for logout and refresh token rotation.
//!
//! Implements:
//! - Token revocation with hash-based storage (don't store raw tokens)
//! - Revocation check during token verification
//! - Automatic cleanup of expired revocation entries
//! - Atomic refresh token rotation (revoke old, issue new)

use ob_core::{Error, Result};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;
use sqlx::postgres::PgTransaction;
use tracing::info;

/// Transaction type for the refresh rotation lock.
pub type RotationLockTx<'a> = PgTransaction<'a>;

/// Hash a raw token using SHA256.
/// Returns hex-encoded hash suitable for storage.
fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Derive a 64-bit advisory lock key from a refresh token.
fn rotation_lock_key(token: &str) -> i64 {
    let hash = hash_token(token);
    let bytes = hex::decode(&hash[..16]).unwrap_or_default();
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&bytes);
    i64::from_be_bytes(arr)
}

async fn ensure_revoked_tokens_table(db: &ob_database::DatabaseClient) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS _revoked_tokens (
            id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
            data JSONB NOT NULL DEFAULT '{}'::jsonb,
            created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
        )
        "#,
    )
    .execute(db.inner().pool())
    .await
    .map_err(|e| Error::Database(format!("Failed to ensure _revoked_tokens table: {e}")))?;

    Ok(())
}

/// Acquire an advisory lock for refresh token rotation.
///
/// Returns a transaction that holds the lock. The lock is released
/// when the transaction is committed or rolled back.
pub async fn acquire_refresh_rotation_lock<'a>(
    db: &'a ob_database::DatabaseClient,
    token: &str,
) -> Result<RotationLockTx<'a>> {
    let key = rotation_lock_key(token);
    let mut tx =
        db.inner().pool().begin().await.map_err(|e| {
            Error::Database(format!("Failed to begin refresh lock transaction: {e}"))
        })?;

    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(key)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| Error::Database(format!("Failed to acquire refresh lock: {e}")))?;

    if !acquired {
        return Err(Error::Auth(
            "Refresh token is already being rotated. Please retry.".into(),
        ));
    }

    Ok(tx)
}

/// Revoke a token by storing its hash in the database.
///
/// The token is stored with an expiry timestamp equal to its original TTL,
/// so revocation records can be automatically cleaned up.
///
/// # Arguments
/// * `db` - Database client
/// * `token` - Raw token to revoke
/// * `ttl_secs` - Token's time-to-live in seconds (from issuance)
pub async fn revoke_token(
    db: &ob_database::DatabaseClient,
    token: &str,
    ttl_secs: u64,
) -> Result<()> {
    let token_hash = hash_token(token);
    let expires_at = chrono::Utc::now().timestamp() + ttl_secs as i64;
    let revoked_at = chrono::Utc::now().to_rfc3339();
    ensure_revoked_tokens_table(db).await?;

    db.upsert_document(
        "_revoked_tokens",
        &token_hash,
        json!({
            "hash": token_hash,
            "expiresAt": expires_at,
            "revokedAt": revoked_at,
        }),
    )
    .await
    .map_err(|e| Error::Internal(format!("Failed to revoke token: {e}")))?;

    info!("Token revoked (hash: {})", &token_hash[..8]); // Log truncated hash for debugging
    Ok(())
}

/// Check if a token has been revoked.
///
/// Returns `true` if the token is in the revocation list and hasn't expired yet.
/// Returns `false` if the token is not revoked or if the revocation entry has expired.
///
/// # Arguments
/// * `db` - Database client
/// * `token` - Raw token to check
pub async fn is_token_revoked(db: &ob_database::DatabaseClient, token: &str) -> Result<bool> {
    let token_hash = hash_token(token);
    let now = chrono::Utc::now().timestamp();
    ensure_revoked_tokens_table(db).await?;

    let row = sqlx::query(
        r#"
        SELECT data->>'expiresAt' AS expires_at
        FROM _revoked_tokens
        WHERE data->>'hash' = $1
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(db.inner().pool())
    .await
    .map_err(|e| Error::Database(format!("Failed to load revoked token: {e}")))?;

    let expires_at = row
        .as_ref()
        .and_then(|value| value.try_get::<String, _>("expires_at").ok())
        .and_then(|value| value.parse::<i64>().ok());

    Ok(expires_at.is_some_and(|expires_at| expires_at > now))
}

/// Clean up expired revocation entries from the database.
///
/// This should be called periodically (e.g., once per day) to avoid
/// unbounded growth of the `_revoked_tokens` table.
///
/// Returns the count of deleted entries.
pub async fn cleanup_revoked_tokens(db: &ob_database::DatabaseClient) -> Result<usize> {
    let now = chrono::Utc::now().timestamp();
    ensure_revoked_tokens_table(db).await?;

    let result = sqlx::query(
        r#"
        DELETE FROM _revoked_tokens
        WHERE COALESCE(NULLIF(data->>'expiresAt', ''), '0')::bigint < $1
        "#,
    )
    .bind(now)
    .execute(db.inner().pool())
    .await
    .map_err(|e| Error::Database(format!("Failed to cleanup revoked tokens: {e}")))?;

    let count = result.rows_affected() as usize;

    info!("Cleaned up {} expired revocation entries", count);
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_token_deterministic() {
        let token = "REDACTED_SECRET";
        let hash1 = hash_token(token);
        let hash2 = hash_token(token);
        assert_eq!(hash1, hash2, "Hash should be deterministic");
    }

    #[test]
    fn test_hash_token_different_tokens() {
        let token1 = "token1";
        let token2 = "token2";
        let hash1 = hash_token(token1);
        let hash2 = hash_token(token2);
        assert_ne!(
            hash1, hash2,
            "Different tokens should have different hashes"
        );
    }

    #[test]
    fn test_hash_token_is_hex() {
        let token = "test_token";
        let hash = hash_token(token);
        // SHA256 produces 64 hex characters
        assert_eq!(hash.len(), 64, "SHA256 hex hash should be 64 characters");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "Hash should be valid hex"
        );
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    // Note: These are unit tests for the hash function.
    // Full integration tests (with database) would be in tests/ directory.

    #[tokio::test]
    async fn test_hash_consistency() {
        let token = "REDACTED_SECRET";
        let hash1 = hash_token(token);
        let hash2 = hash_token(token);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_format() {
        let token = "test.jwt.token";
        let hash = hash_token(token);

        // Verify it's valid hex
        for c in hash.chars() {
            assert!(
                c.is_ascii_hexdigit(),
                "Hash contains invalid hex character: {}",
                c
            );
        }

        // Verify length is 64 (SHA256 produces 256 bits = 64 hex chars)
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_different_tokens_different_hashes() {
        let token1 = "access_token_abc123";
        let token2 = "refresh_token_xyz789";

        let hash1 = hash_token(token1);
        let hash2 = hash_token(token2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_token_empty() {
        let hash = hash_token("");
        // Even empty string produces valid hash
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hash_token_long() {
        let long_token = "x".repeat(10000);
        let hash = hash_token(&long_token);
        assert_eq!(hash.len(), 64);
    }

    #[tokio::test]
    async fn test_refresh_rotation_lock_is_single_holder() {
        let unique_token = format!("test-refresh-rotation-{}", uuid::Uuid::new_v4());
        let db = ob_database::DatabaseClient::new_mem().await;
        let tx = acquire_refresh_rotation_lock(&db, &unique_token).await;
        assert!(tx.is_ok());

        let second = acquire_refresh_rotation_lock(&db, &unique_token).await;
        assert!(second.is_err());
    }

    #[tokio::test]
    async fn test_revoke_token_round_trip_and_cleanup() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let token = format!("refresh-token-{}", uuid::Uuid::new_v4());

        revoke_token(&db, &token, 60).await.unwrap();
        assert!(is_token_revoked(&db, &token).await.unwrap());

        let token_hash = hash_token(&token);
        db.upsert_document(
            "_revoked_tokens",
            &token_hash,
            json!({
                "hash": token_hash,
                "expiresAt": chrono::Utc::now().timestamp() - 1,
                "revokedAt": chrono::Utc::now().to_rfc3339(),
            }),
        )
        .await
        .unwrap();

        let cleaned = cleanup_revoked_tokens(&db).await.unwrap();
        assert_eq!(cleaned, 1);
        assert!(!is_token_revoked(&db, &token).await.unwrap());
    }
}
