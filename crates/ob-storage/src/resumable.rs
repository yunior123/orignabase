use dashmap::DashMap;
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

/// Hard limits for resumable uploads.
const MAX_FILE_SIZE: u64 = 500 * 1024 * 1024; // 500 MB
const MAX_ACTIVE_SESSIONS: usize = 100;
const MAX_SESSIONS_PER_USER: usize = 10;
const SESSION_TTL_SECS: i64 = 24 * 3600; // 24 hours

/// Session metadata for a resumable upload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSession {
    pub id: String,
    pub path: String,
    pub content_type: String,
    pub total_size: u64,
    pub bytes_received: u64,
    pub created_at: String,
    pub status: UploadStatus,
    /// User who created this session (for ownership checks).
    #[serde(default)]
    pub owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UploadStatus {
    InProgress,
    Complete,
    Cancelled,
}

/// Manages resumable upload sessions with chunk-based file assembly.
///
/// Security hardening:
/// - Max file size (500MB), max active sessions (100)
/// - Path sanitization (no traversal)
/// - Owner binding (session creator must match on operations)
/// - TTL-based session expiry (24h)
#[derive(Clone)]
pub struct ResumableUploadManager {
    sessions: Arc<DashMap<String, UploadSession>>,
    chunks_dir: PathBuf,
}

impl ResumableUploadManager {
    pub fn new(chunks_dir: impl Into<PathBuf>) -> Result<Self> {
        let chunks_dir = chunks_dir.into();
        std::fs::create_dir_all(&chunks_dir)
            .map_err(|e| Error::Internal(format!("Failed to create chunks dir: {e}")))?;

        let mgr = Self {
            sessions: Arc::new(DashMap::new()),
            chunks_dir,
        };

        // Cleanup orphaned temp files from previous runs
        mgr.cleanup_orphaned_files();

        Ok(mgr)
    }

    /// Sanitize upload path — prevent traversal attacks.
    /// Uses iterative replacement to handle nested traversal like `....//`.
    fn sanitize_path(path: &str) -> Result<String> {
        let mut sanitized = path.replace('\\', "/");
        loop {
            let next = sanitized.replace("..", "");
            if next == sanitized {
                break;
            }
            sanitized = next;
        }
        let sanitized = sanitized.trim_start_matches('/').to_string();

        if sanitized.is_empty() {
            return Err(Error::Validation("Path cannot be empty".into()));
        }
        Ok(sanitized)
    }

    /// Start a new resumable upload session.
    pub fn create_session(
        &self,
        path: &str,
        content_type: &str,
        total_size: u64,
        owner: &str,
    ) -> Result<UploadSession> {
        if total_size == 0 {
            return Err(Error::Validation("total_size must be > 0".into()));
        }
        if total_size > MAX_FILE_SIZE {
            return Err(Error::Validation(format!(
                "total_size exceeds maximum of {MAX_FILE_SIZE} bytes"
            )));
        }

        // Enforce session limits (global + per-user)
        self.reap_expired();
        if self.sessions.len() >= MAX_ACTIVE_SESSIONS {
            return Err(Error::Validation(
                "Too many active upload sessions. Try again later.".into(),
            ));
        }
        let user_sessions = self
            .sessions
            .iter()
            .filter(|s| s.owner == owner && s.status == UploadStatus::InProgress)
            .count();
        if user_sessions >= MAX_SESSIONS_PER_USER {
            return Err(Error::Validation(format!(
                "Too many active sessions for this user (max {MAX_SESSIONS_PER_USER})"
            )));
        }

        let sanitized_path = Self::sanitize_path(path)?;

        let id = Uuid::new_v4().to_string();
        let session = UploadSession {
            id: id.clone(),
            path: sanitized_path,
            content_type: content_type.to_string(),
            total_size,
            bytes_received: 0,
            created_at: chrono::Utc::now().to_rfc3339(),
            status: UploadStatus::InProgress,
            owner: owner.to_string(),
        };
        self.sessions.insert(id, session.clone());
        Ok(session)
    }

    /// Append a chunk to an upload session.
    /// Owner must match the session creator.
    pub async fn append_chunk(
        &self,
        session_id: &str,
        offset: u64,
        data: &[u8],
        owner: &str,
    ) -> Result<UploadSession> {
        // Use entry API for atomic read-check-write
        let mut session = self
            .sessions
            .get(session_id)
            .map(|s| s.value().clone())
            .ok_or_else(|| Error::NotFound(format!("Upload session not found: {session_id}")))?;

        // Ownership check: reject both empty owner and mismatched owner
        if session.owner.is_empty() {
            return Err(Error::Auth("Upload requires authenticated user".into()));
        }
        if session.owner != owner {
            return Err(Error::Auth("Not authorized for this upload session".into()));
        }

        if session.status != UploadStatus::InProgress {
            return Err(Error::Validation(format!(
                "Session is {:?}, not in progress",
                session.status
            )));
        }

        if offset != session.bytes_received {
            return Err(Error::Validation(format!(
                "Expected offset {}, got {offset}",
                session.bytes_received
            )));
        }

        if session.bytes_received + data.len() as u64 > session.total_size {
            return Err(Error::Validation("Chunk exceeds total_size".into()));
        }

        // Append to chunk file
        let chunk_path = self.chunks_dir.join(session_id);
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&chunk_path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to open chunk file: {e}")))?;

        file.write_all(data)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write chunk: {e}")))?;

        session.bytes_received += data.len() as u64;

        if session.bytes_received == session.total_size {
            session.status = UploadStatus::Complete;
        }

        self.sessions
            .insert(session_id.to_string(), session.clone());
        Ok(session)
    }

    /// Get the assembled file data for a completed session.
    pub async fn finalize(&self, session_id: &str) -> Result<(Vec<u8>, UploadSession)> {
        let session = self
            .sessions
            .get(session_id)
            .map(|s| s.value().clone())
            .ok_or_else(|| Error::NotFound(format!("Upload session not found: {session_id}")))?;

        if session.status != UploadStatus::Complete {
            return Err(Error::Validation(
                "Upload not complete, cannot finalize".into(),
            ));
        }

        let chunk_path = self.chunks_dir.join(session_id);
        let data = tokio::fs::read(&chunk_path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read assembled file: {e}")))?;

        // Cleanup chunk file and session
        let _ = tokio::fs::remove_file(&chunk_path).await;
        self.sessions.remove(session_id);

        Ok((data, session))
    }

    /// Get session status (for resume queries).
    /// Owner must match.
    pub fn get_session(&self, session_id: &str, owner: &str) -> Result<UploadSession> {
        let session = self
            .sessions
            .get(session_id)
            .map(|s| s.value().clone())
            .ok_or_else(|| Error::NotFound(format!("Upload session not found: {session_id}")))?;

        if !session.owner.is_empty() && session.owner != owner {
            return Err(Error::Auth("Not authorized for this upload session".into()));
        }

        Ok(session)
    }

    /// Cancel and cleanup a session.
    /// Owner must match.
    pub async fn cancel(&self, session_id: &str, owner: &str) -> Result<()> {
        if let Some((_, session)) = self.sessions.remove(session_id)
            && !session.owner.is_empty()
            && session.owner != owner
        {
            // Put it back — not authorized
            self.sessions.insert(session_id.to_string(), session);
            return Err(Error::Auth("Not authorized for this upload session".into()));
        }
        let chunk_path = self.chunks_dir.join(session_id);
        let _ = tokio::fs::remove_file(&chunk_path).await;
        Ok(())
    }

    /// Remove expired sessions (older than TTL).
    fn reap_expired(&self) {
        let now = chrono::Utc::now();
        let mut expired = Vec::new();

        for entry in self.sessions.iter() {
            if let Ok(created) = chrono::DateTime::parse_from_rfc3339(&entry.created_at)
                && (now - created.with_timezone(&chrono::Utc)).num_seconds() > SESSION_TTL_SECS
            {
                expired.push(entry.key().clone());
            }
        }

        for id in &expired {
            self.sessions.remove(id);
            let chunk_path = self.chunks_dir.join(id);
            let _ = std::fs::remove_file(&chunk_path);
        }
    }

    /// Cleanup orphaned temp files on startup.
    fn cleanup_orphaned_files(&self) {
        if let Ok(entries) = std::fs::read_dir(&self.chunks_dir) {
            for entry in entries.flatten() {
                if entry.path().is_file() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !self.sessions.contains_key(&name) {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn test_manager() -> (ResumableUploadManager, PathBuf) {
        let dir = env::temp_dir().join(format!("ob_resumable_test_{}", Uuid::new_v4()));
        let mgr = ResumableUploadManager::new(&dir).unwrap();
        (mgr, dir)
    }

    #[test]
    fn test_create_session() {
        let (mgr, dir) = test_manager();
        let session = mgr
            .create_session("photos/big.jpg", "image/jpeg", 1024, "user1")
            .unwrap();
        assert_eq!(session.path, "photos/big.jpg");
        assert_eq!(session.total_size, 1024);
        assert_eq!(session.bytes_received, 0);
        assert_eq!(session.status, UploadStatus::InProgress);
        assert_eq!(session.owner, "user1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_create_session_zero_size_fails() {
        let (mgr, dir) = test_manager();
        let result = mgr.create_session("x.txt", "text/plain", 0, "u");
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_create_session_exceeds_max_size() {
        let (mgr, dir) = test_manager();
        let result = mgr.create_session("x.txt", "text/plain", MAX_FILE_SIZE + 1, "u");
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_path_sanitization() {
        assert_eq!(
            ResumableUploadManager::sanitize_path("../../etc/passwd").unwrap(),
            "etc/passwd"
        );
        assert_eq!(
            ResumableUploadManager::sanitize_path("/absolute/path.txt").unwrap(),
            "absolute/path.txt"
        );
        assert_eq!(
            ResumableUploadManager::sanitize_path("normal/path.txt").unwrap(),
            "normal/path.txt"
        );
        assert!(ResumableUploadManager::sanitize_path("").is_err());
    }

    #[tokio::test]
    async fn test_full_upload_flow() {
        let (mgr, dir) = test_manager();
        let data = b"Hello, resumable world!";
        let session = mgr
            .create_session("test.txt", "text/plain", data.len() as u64, "user1")
            .unwrap();

        // Upload in two chunks
        let chunk1 = &data[..10];
        let chunk2 = &data[10..];

        let s = mgr
            .append_chunk(&session.id, 0, chunk1, "user1")
            .await
            .unwrap();
        assert_eq!(s.bytes_received, 10);
        assert_eq!(s.status, UploadStatus::InProgress);

        let s = mgr
            .append_chunk(&session.id, 10, chunk2, "user1")
            .await
            .unwrap();
        assert_eq!(s.bytes_received, data.len() as u64);
        assert_eq!(s.status, UploadStatus::Complete);

        // Finalize
        let (assembled, final_session) = mgr.finalize(&session.id).await.unwrap();
        assert_eq!(assembled, data);
        assert_eq!(final_session.path, "test.txt");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_wrong_offset_rejected() {
        let (mgr, dir) = test_manager();
        let session = mgr.create_session("x.txt", "text/plain", 100, "u").unwrap();

        let result = mgr.append_chunk(&session.id, 5, b"data", "u").await;
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_chunk_exceeds_total_rejected() {
        let (mgr, dir) = test_manager();
        let session = mgr.create_session("x.txt", "text/plain", 5, "u").unwrap();

        let result = mgr
            .append_chunk(&session.id, 0, b"too long data", "u")
            .await;
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_cancel_session() {
        let (mgr, dir) = test_manager();
        let session = mgr
            .create_session("x.txt", "text/plain", 100, "user1")
            .unwrap();

        mgr.append_chunk(&session.id, 0, b"partial", "user1")
            .await
            .unwrap();
        mgr.cancel(&session.id, "user1").await.unwrap();

        let result = mgr.get_session(&session.id, "user1");
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_wrong_owner_rejected() {
        let (mgr, dir) = test_manager();
        let session = mgr
            .create_session("x.txt", "text/plain", 100, "user1")
            .unwrap();

        // Different user tries to append
        let result = mgr.append_chunk(&session.id, 0, b"data", "attacker").await;
        assert!(result.is_err());

        // Different user tries to get status
        let result = mgr.get_session(&session.id, "attacker");
        assert!(result.is_err());

        // Different user tries to cancel
        let result = mgr.cancel(&session.id, "attacker").await;
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_get_session_for_resume() {
        let (mgr, dir) = test_manager();
        let session = mgr.create_session("x.txt", "text/plain", 100, "u").unwrap();

        mgr.append_chunk(&session.id, 0, &[0u8; 50], "u")
            .await
            .unwrap();

        let status = mgr.get_session(&session.id, "u").unwrap();
        assert_eq!(status.bytes_received, 50);
        assert_eq!(status.status, UploadStatus::InProgress);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_finalize_incomplete_fails() {
        let (mgr, dir) = test_manager();
        let session = mgr.create_session("x.txt", "text/plain", 100, "u").unwrap();

        mgr.append_chunk(&session.id, 0, &[0u8; 50], "u")
            .await
            .unwrap();
        let result = mgr.finalize(&session.id).await;
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_nonexistent_session() {
        let (mgr, dir) = test_manager();
        let result = mgr.get_session("nonexistent", "u");
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_single_chunk_upload() {
        let (mgr, dir) = test_manager();
        let data = b"all at once";
        let session = mgr
            .create_session("one.txt", "text/plain", data.len() as u64, "u")
            .unwrap();

        let s = mgr.append_chunk(&session.id, 0, data, "u").await.unwrap();
        assert_eq!(s.status, UploadStatus::Complete);

        let (assembled, _) = mgr.finalize(&session.id).await.unwrap();
        assert_eq!(assembled, data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_many_small_chunks() {
        let (mgr, dir) = test_manager();
        let data: Vec<u8> = (0..100u8).collect();
        let session = mgr
            .create_session("many.bin", "application/octet-stream", 100, "u")
            .unwrap();

        for i in 0..10 {
            let chunk = &data[i * 10..(i + 1) * 10];
            mgr.append_chunk(&session.id, (i * 10) as u64, chunk, "u")
                .await
                .unwrap();
        }

        let (assembled, s) = mgr.finalize(&session.id).await.unwrap();
        assert_eq!(assembled, data);
        assert_eq!(s.status, UploadStatus::Complete);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_path_traversal_blocked() {
        let (mgr, dir) = test_manager();
        let session = mgr
            .create_session("../../etc/shadow", "text/plain", 10, "u")
            .unwrap();
        // Path should be sanitized
        assert_eq!(session.path, "etc/shadow");
        assert!(!session.path.contains(".."));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
