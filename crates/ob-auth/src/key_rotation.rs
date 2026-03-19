use chrono::{DateTime, Utc};
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::Path;

/// Metadata for a rotated key
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMetadata {
    /// When this key was created/rotated in
    pub created_at: DateTime<Utc>,
    /// Public key fingerprint (SHA256 first 16 chars)
    pub fingerprint: String,
    /// Whether this is the current signing key
    pub is_current: bool,
}

/// Key rotation manager: tracks current and historical keys
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyRotationManager {
    /// Current key for signing (index 0 in keys)
    pub current_key_metadata: KeyMetadata,
    /// Previous keys for verification fallback (max 2 old keys)
    pub previous_keys_metadata: VecDeque<KeyMetadata>,
}

impl KeyRotationManager {
    /// Create a new rotation manager with an initial key
    pub fn new(fingerprint: String) -> Self {
        Self {
            current_key_metadata: KeyMetadata {
                created_at: Utc::now(),
                fingerprint,
                is_current: true,
            },
            previous_keys_metadata: VecDeque::new(),
        }
    }

    /// Rotate to a new key. Moves current to previous, makes new key current.
    /// Keeps max 2 previous keys (drops oldest if > 2).
    pub fn rotate(&mut self, new_fingerprint: String) {
        let mut old_key = self.current_key_metadata.clone();
        old_key.is_current = false;

        self.previous_keys_metadata.push_front(old_key);

        // Keep only 2 previous keys (total: 1 current + 2 previous = 3 keys)
        if self.previous_keys_metadata.len() > 2 {
            self.previous_keys_metadata.pop_back();
        }

        self.current_key_metadata = KeyMetadata {
            created_at: Utc::now(),
            fingerprint: new_fingerprint,
            is_current: true,
        };
    }

    /// Load rotation metadata from a JSON file
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("Failed to read rotation metadata: {e}")))?;
        serde_json::from_str(&contents)
            .map_err(|e| Error::Config(format!("Failed to parse rotation metadata: {e}")))
    }

    /// Save rotation metadata to a JSON file
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        std::fs::create_dir_all(
            path.parent()
                .ok_or_else(|| Error::Config("Invalid rotation metadata path".into()))?,
        )
        .map_err(|e| Error::Config(format!("Failed to create directory: {e}")))?;

        let contents = serde_json::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("Failed to serialize metadata: {e}")))?;

        std::fs::write(path, &contents)
            .map_err(|e| Error::Config(format!("Failed to write rotation metadata: {e}")))
    }

    /// Get all key fingerprints (current + previous) in order
    pub fn all_fingerprints(&self) -> Vec<&str> {
        let mut fps = vec![self.current_key_metadata.fingerprint.as_str()];
        for metadata in &self.previous_keys_metadata {
            fps.push(metadata.fingerprint.as_str());
        }
        fps
    }
}

/// Generate SHA256 fingerprint of a public key (first 16 chars hex)
pub fn fingerprint_public_key(public_pem: &[u8]) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(public_pem);
    let result = hasher.finalize();

    Ok(format!("{:x}", result)[0..16].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_rotation_manager_new() {
        let mgr = KeyRotationManager::new("abc123".to_string());
        assert_eq!(mgr.current_key_metadata.fingerprint, "abc123");
        assert!(mgr.current_key_metadata.is_current);
        assert_eq!(mgr.previous_keys_metadata.len(), 0);
    }

    #[test]
    fn test_key_rotation() {
        let mut mgr = KeyRotationManager::new("key1".to_string());
        mgr.rotate("key2".to_string());

        assert_eq!(mgr.current_key_metadata.fingerprint, "key2");
        assert!(mgr.current_key_metadata.is_current);
        assert_eq!(mgr.previous_keys_metadata.len(), 1);
        assert_eq!(mgr.previous_keys_metadata[0].fingerprint, "key1");
        assert!(!mgr.previous_keys_metadata[0].is_current);
    }

    #[test]
    fn test_max_previous_keys() {
        let mut mgr = KeyRotationManager::new("key1".to_string());
        mgr.rotate("key2".to_string());
        mgr.rotate("key3".to_string());
        mgr.rotate("key4".to_string());

        assert_eq!(mgr.current_key_metadata.fingerprint, "key4");
        assert_eq!(mgr.previous_keys_metadata.len(), 2);
        assert_eq!(mgr.previous_keys_metadata[0].fingerprint, "key3");
        assert_eq!(mgr.previous_keys_metadata[1].fingerprint, "key2");
        // key1 should be dropped
    }

    #[test]
    fn test_all_fingerprints() {
        let mut mgr = KeyRotationManager::new("key1".to_string());
        mgr.rotate("key2".to_string());
        mgr.rotate("key3".to_string());

        let fps = mgr.all_fingerprints();
        assert_eq!(fps, vec!["key3", "key2", "key1"]);
    }
}
