use crate::{ObjectMeta, StorageBackend};
use ob_core::{Error, Result};
use std::path::{Path, PathBuf};

/// Local filesystem storage backend.
#[derive(Clone)]
pub struct LocalStorage {
    root: PathBuf,
}

impl LocalStorage {
    /// Create a new local storage backend rooted at the given directory.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)
            .map_err(|e| Error::Internal(format!("Failed to create storage dir: {e}")))?;
        Ok(Self { root })
    }

    fn full_path(&self, path: &str) -> PathBuf {
        // Sanitize: prevent path traversal
        let sanitized = path.replace("..", "").trim_start_matches('/').to_string();
        self.root.join(sanitized)
    }
}

impl StorageBackend for LocalStorage {
    async fn upload(&self, path: &str, data: &[u8], content_type: &str) -> Result<ObjectMeta> {
        let full = self.full_path(path);

        // Create parent directories
        if let Some(parent) = full.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| Error::Internal(format!("Failed to create dirs: {e}")))?;
        }

        tokio::fs::write(&full, data)
            .await
            .map_err(|e| Error::Internal(format!("Write failed: {e}")))?;

        let now = chrono::Utc::now().to_rfc3339();
        Ok(ObjectMeta {
            path: path.to_string(),
            size: data.len() as u64,
            content_type: content_type.to_string(),
            created_at: now.clone(),
            updated_at: now,
        })
    }

    async fn download(&self, path: &str) -> Result<Vec<u8>> {
        let full = self.full_path(path);
        tokio::fs::read(&full)
            .await
            .map_err(|e| Error::NotFound(format!("File not found: {e}")))
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let full = self.full_path(path);
        tokio::fs::remove_file(&full)
            .await
            .map_err(|e| Error::NotFound(format!("File not found: {e}")))
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        let full = self.full_path(path);
        Ok(full.exists())
    }

    async fn metadata(&self, path: &str) -> Result<ObjectMeta> {
        let full = self.full_path(path);
        let meta = tokio::fs::metadata(&full)
            .await
            .map_err(|e| Error::NotFound(format!("File not found: {e}")))?;

        let now = chrono::Utc::now().to_rfc3339();
        Ok(ObjectMeta {
            path: path.to_string(),
            size: meta.len(),
            content_type: guess_content_type(path),
            created_at: now.clone(),
            updated_at: now,
        })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<ObjectMeta>> {
        let dir = self.full_path(prefix);
        if !dir.exists() {
            return Ok(vec![]);
        }

        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| Error::Internal(format!("Read dir failed: {e}")))?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| Error::Internal(format!("Read entry failed: {e}")))?
        {
            let meta = entry
                .metadata()
                .await
                .map_err(|e| Error::Internal(format!("Metadata failed: {e}")))?;

            if meta.is_file() {
                let file_path = entry.path();
                let relative = file_path
                    .strip_prefix(&self.root)
                    .unwrap_or(&file_path)
                    .to_string_lossy()
                    .to_string();

                let now = chrono::Utc::now().to_rfc3339();
                entries.push(ObjectMeta {
                    path: relative.clone(),
                    size: meta.len(),
                    content_type: guess_content_type(&relative),
                    created_at: now.clone(),
                    updated_at: now,
                });
            }
        }

        Ok(entries)
    }
}

fn guess_content_type(path: &str) -> String {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "html" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "txt" => "text/plain",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[tokio::test]
    async fn test_upload_download_delete() {
        let dir = env::temp_dir().join("ob_storage_test_crud");
        let _ = std::fs::remove_dir_all(&dir);
        let storage = LocalStorage::new(&dir).unwrap();

        // Upload
        let meta = storage
            .upload("test/hello.txt", b"Hello World", "text/plain")
            .await
            .unwrap();
        assert_eq!(meta.path, "test/hello.txt");
        assert_eq!(meta.size, 11);
        assert_eq!(meta.content_type, "text/plain");

        // Download
        let data = storage.download("test/hello.txt").await.unwrap();
        assert_eq!(data, b"Hello World");

        // Exists
        assert!(storage.exists("test/hello.txt").await.unwrap());
        assert!(!storage.exists("test/nope.txt").await.unwrap());

        // Delete
        storage.delete("test/hello.txt").await.unwrap();
        assert!(!storage.exists("test/hello.txt").await.unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_list() {
        let dir = env::temp_dir().join("ob_storage_test_list");
        let _ = std::fs::remove_dir_all(&dir);
        let storage = LocalStorage::new(&dir).unwrap();

        storage
            .upload("docs/a.txt", b"a", "text/plain")
            .await
            .unwrap();
        storage
            .upload("docs/b.txt", b"bb", "text/plain")
            .await
            .unwrap();

        let entries = storage.list("docs").await.unwrap();
        assert_eq!(entries.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_path_traversal_prevention() {
        let dir = env::temp_dir().join("ob_storage_test_traversal");
        let _ = std::fs::remove_dir_all(&dir);
        let storage = LocalStorage::new(&dir).unwrap();

        // ".." should be stripped
        storage
            .upload("../../etc/passwd", b"nope", "text/plain")
            .await
            .unwrap();

        // File should end up safely inside the storage root
        assert!(!Path::new("/etc/passwd_ob").exists());
        assert!(storage.exists("etc/passwd").await.unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_content_type_guessing() {
        assert_eq!(guess_content_type("photo.jpg"), "image/jpeg");
        assert_eq!(guess_content_type("doc.pdf"), "application/pdf");
        assert_eq!(
            guess_content_type("unknown.xyz"),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_content_type_all_variants() {
        assert_eq!(guess_content_type("a.jpeg"), "image/jpeg");
        assert_eq!(guess_content_type("a.png"), "image/png");
        assert_eq!(guess_content_type("a.gif"), "image/gif");
        assert_eq!(guess_content_type("a.webp"), "image/webp");
        assert_eq!(guess_content_type("a.svg"), "image/svg+xml");
        assert_eq!(guess_content_type("a.json"), "application/json");
        assert_eq!(guess_content_type("a.html"), "text/html");
        assert_eq!(guess_content_type("a.css"), "text/css");
        assert_eq!(guess_content_type("a.js"), "application/javascript");
        assert_eq!(guess_content_type("a.txt"), "text/plain");
        assert_eq!(guess_content_type("a.mp4"), "video/mp4");
        assert_eq!(guess_content_type("a.mp3"), "audio/mpeg");
        assert_eq!(guess_content_type("a.zip"), "application/zip");
    }

    #[test]
    fn test_content_type_no_extension() {
        assert_eq!(guess_content_type("Makefile"), "application/octet-stream");
        assert_eq!(guess_content_type(""), "application/octet-stream");
    }

    #[test]
    fn test_content_type_nested_path() {
        assert_eq!(guess_content_type("a/b/c/photo.png"), "image/png");
        assert_eq!(guess_content_type("/deep/path/file.json"), "application/json");
    }

    #[test]
    fn test_full_path_sanitizes_traversal() {
        let dir = env::temp_dir().join("ob_storage_test_fullpath");
        let storage = LocalStorage { root: dir.clone() };

        // Double dots stripped
        let p = storage.full_path("../../etc/passwd");
        assert!(p.starts_with(&dir));
        assert!(!p.to_string_lossy().contains(".."));

        // Leading slashes stripped
        let p = storage.full_path("/absolute/path.txt");
        assert!(p.starts_with(&dir));
        assert_eq!(p, dir.join("absolute/path.txt"));
    }

    #[test]
    fn test_full_path_normal_paths() {
        let dir = env::temp_dir().join("ob_storage_test_fullpath2");
        let storage = LocalStorage { root: dir.clone() };

        assert_eq!(storage.full_path("a/b/c.txt"), dir.join("a/b/c.txt"));
        assert_eq!(storage.full_path("file.txt"), dir.join("file.txt"));
    }

    #[test]
    fn test_full_path_empty_input() {
        let dir = env::temp_dir().join("ob_storage_test_fullpath_empty");
        let storage = LocalStorage { root: dir.clone() };

        let p = storage.full_path("");
        assert_eq!(p, dir.join(""));
    }

    #[test]
    fn test_object_meta_serde_roundtrip() {
        let meta = super::ObjectMeta {
            path: "test/file.txt".to_string(),
            size: 42,
            content_type: "text/plain".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: super::ObjectMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.path, "test/file.txt");
        assert_eq!(parsed.size, 42);
        assert_eq!(parsed.content_type, "text/plain");
    }
}
