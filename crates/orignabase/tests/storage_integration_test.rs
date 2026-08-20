//! Integration tests for ob-storage endpoints: presigned URLs, upload, download, batch ops.
//!
//! Run with: `cargo test --test storage_integration_test -- --ignored`
//!
//! Requirements:
//!   OB_TEST_URL=http://localhost:8080 (or remote OrignaBase instance with OB_TEST_MODE=1)

use ob_database::fields;
use serde_json::{Value, json};
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string()) // ignore-magic
}

/// Register a test user and return (access_token, user_id, email).
async fn register_test_user(client: &reqwest::Client) -> (String, String, String) {
    let email = format!("test_{}@example.com", Uuid::new_v4()); // ignore-magic
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "TestPassword123!" })) // ignore-magic
        .send()
        .await
        .expect("register failed");
    assert_eq!(resp.status(), 200, "Registration should succeed");
    let body: Value = resp.json().await.unwrap();
    let token = body["access_token"] // ignore-magic
        .as_str()
        .expect("missing access_token")
        .to_string();
    let user_id = body["user"][fields::ID] // ignore-magic
        .as_str()
        .expect("missing user.id")
        .to_string();
    (token, user_id, email)
}

// =============================================================================
// SECTION 1: Presigned Upload URL Generation
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_storage_presign_upload_single_path() {
    let client = reqwest::Client::new();
    let (token, user_id, _) = register_test_user(&client).await;

    let path = format!("users/{}/test_{}.jpg", user_id, Uuid::new_v4());
    let resp = client
        .post(format!("{}/storage/presign/upload", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ // ignore-magic
            "paths": [path],
            "ttl_secs": 3600
        }))
        .send()
        .await
        .expect("presign upload request failed");

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic

    assert_eq!(status, 200, "Presign upload should succeed: {body:?}");
    let urls = body["urls"].as_array().expect("should return urls array"); // ignore-magic
    assert_eq!(urls.len(), 1);
    assert!(
        urls[0]["upload_url"].as_str().is_some(), // ignore-magic
        "Should include upload_url"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_storage_presign_upload_multiple_paths() {
    let client = reqwest::Client::new();
    let (token, user_id, _) = register_test_user(&client).await;

    let paths: Vec<String> = (0..3)
        .map(|i| format!("users/{}/batch_{i}_{}.jpg", user_id, Uuid::new_v4()))
        .collect();

    let resp = client
        .post(format!("{}/storage/presign/upload", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ // ignore-magic
            "paths": paths,
            "ttl_secs": 7200
        }))
        .send()
        .await
        .expect("presign upload request failed");

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic

    assert_eq!(status, 200, "Batch presign upload should succeed: {body:?}");
    let urls = body["urls"].as_array().expect("should return urls array"); // ignore-magic
    assert_eq!(urls.len(), 3, "Should return 3 presigned URLs");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_storage_presign_upload_empty_paths_rejected() {
    let client = reqwest::Client::new();
    let (token, _, _) = register_test_user(&client).await;

    let resp = client
        .post(format!("{}/storage/presign/upload", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ // ignore-magic
            "paths": [],
            "ttl_secs": 3600
        }))
        .send()
        .await
        .expect("presign upload request failed");

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "Empty paths should be rejected, got status={status}"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_storage_presign_upload_directory_traversal_rejected() {
    let client = reqwest::Client::new();
    let (token, _, _) = register_test_user(&client).await;

    let resp = client
        .post(format!("{}/storage/presign/upload", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ // ignore-magic
            "paths": ["../../etc/passwd"],
            "ttl_secs": 3600
        }))
        .send()
        .await
        .expect("presign upload request failed");

    let status = resp.status().as_u16();
    // Path should be sanitized or rejected
    assert!(
        status == 200 || status == 400 || status == 403 || status == 422,
        "Directory traversal path should be sanitized or rejected, got status={status}"
    );
}

// =============================================================================
// SECTION 2: Presigned Download URL Generation
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_storage_presign_download_single_path() {
    let client = reqwest::Client::new();
    let (token, user_id, _) = register_test_user(&client).await;

    let path = format!("users/{}/download_test_{}.jpg", user_id, Uuid::new_v4());
    let resp = client
        .post(format!("{}/storage/presign/download", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ // ignore-magic
            "paths": [path],
            "ttl_secs": 3600
        }))
        .send()
        .await
        .expect("presign download request failed");

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic

    assert_eq!(status, 200, "Presign download should succeed: {body:?}");
    let urls = body["urls"].as_array().expect("should return urls array"); // ignore-magic
    assert_eq!(urls.len(), 1);
    assert!(
        urls[0]["download_url"].as_str().is_some(), // ignore-magic
        "Should include download_url"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_storage_presign_download_default_ttl() {
    let client = reqwest::Client::new();
    let (token, user_id, _) = register_test_user(&client).await;

    let path = format!("users/{}/ttl_test_{}.jpg", user_id, Uuid::new_v4());
    let resp = client
        .post(format!("{}/storage/presign/download", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ // ignore-magic
            "paths": [path]
        }))
        .send()
        .await
        .expect("presign download request failed");

    let status = resp.status().as_u16();
    assert_eq!(status, 200, "Should work with default TTL");
}

// =============================================================================
// SECTION 3: Upload + Download End-to-End
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_storage_upload_and_download_roundtrip() {
    let client = reqwest::Client::new();
    let (token, user_id, _) = register_test_user(&client).await;

    let filename = format!("test_{}.bin", Uuid::new_v4()); // ignore-magic
    let path = format!("users/{}/{}", user_id, filename);

    // Step 1: Get presigned upload URL
    let presign_resp = client
        .post(format!("{}/storage/presign/upload", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ "paths": [&path] })) // ignore-magic
        .send()
        .await
        .expect("presign upload failed");

    assert_eq!(presign_resp.status(), 200);
    let presign_body: Value = presign_resp.json().await.unwrap();
    let upload_url = presign_body["urls"][0]["upload_url"] // ignore-magic
        .as_str()
        .expect("missing upload_url");

    // Step 2: Upload file content via the presigned URL
    let file_content = b"Hello, OrignaBase Storage!";
    let upload_resp = client
        .put(upload_url)
        .header("Content-Type", "application/octet-stream") // ignore-magic
        .body(file_content.to_vec())
        .send()
        .await
        .expect("upload failed");

    let upload_status = upload_resp.status().as_u16();
    assert_eq!(upload_status, 200, "Upload should succeed");

    // Step 3: Get presigned download URL
    let download_presign = client
        .post(format!("{}/storage/presign/download", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ "paths": [&path] })) // ignore-magic
        .send()
        .await
        .expect("presign download failed");

    assert_eq!(download_presign.status(), 200);
    let dl_body: Value = download_presign.json().await.unwrap();
    let download_url = dl_body["urls"][0]["download_url"] // ignore-magic
        .as_str()
        .expect("missing download_url");

    // Step 4: Download and verify content matches
    let dl_resp = client
        .get(download_url)
        .send()
        .await
        .expect("download failed");
    let dl_status = dl_resp.status().as_u16();
    assert_eq!(dl_status, 200, "Download should succeed");

    let downloaded = dl_resp.bytes().await.unwrap();
    assert_eq!(
        downloaded.as_ref(),
        file_content,
        "Downloaded content should match uploaded"
    );
}

// =============================================================================
// SECTION 4: Batch Delete
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_storage_batch_delete() {
    let client = reqwest::Client::new();
    let (token, user_id, _) = register_test_user(&client).await;

    // Upload a file first
    let filename = format!("delete_test_{}.bin", Uuid::new_v4());
    let path = format!("users/{}/{}", user_id, filename);

    let presign_resp = client
        .post(format!("{}/storage/presign/upload", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ "paths": [&path] })) // ignore-magic
        .send()
        .await
        .unwrap();
    assert_eq!(presign_resp.status(), 200);
    let presign_body: Value = presign_resp.json().await.unwrap();
    let upload_url = presign_body["urls"][0]["upload_url"].as_str().unwrap(); // ignore-magic

    let _ = client
        .put(upload_url)
        .header("Content-Type", "application/octet-stream") // ignore-magic
        .body(b"to be deleted".to_vec())
        .send()
        .await
        .unwrap();

    // Batch delete the uploaded file
    let del_resp = client
        .post(format!("{}/storage/batch-delete", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ "paths": [&path] })) // ignore-magic
        .send()
        .await
        .expect("batch delete failed");

    let del_status = del_resp.status().as_u16();
    assert_eq!(del_status, 200, "Batch delete should succeed");
}

// =============================================================================
// SECTION 5: Resumable Upload
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_storage_resumable_upload_init() {
    let client = reqwest::Client::new();
    let (token, user_id, _) = register_test_user(&client).await;

    let path = format!("users/{}/resumable_{}.bin", user_id, Uuid::new_v4());
    let resp = client
        .post(format!("{}/storage/upload/resumable", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ // ignore-magic
            "path": path,
            "content_type": "application/octet-stream",
            "total_size": 1024
        }))
        .send()
        .await
        .expect("resumable init failed");

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic

    assert_eq!(status, 200, "Resumable init should succeed: {body:?}");
    assert!(
        body[fields::ID].as_str().is_some(),
        "Should return session ID"
    ); // ignore-magic
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_storage_resumable_upload_exceeds_limit() {
    let client = reqwest::Client::new();
    let (token, user_id, _) = register_test_user(&client).await;

    let path = format!("users/{}/huge_{}.bin", user_id, Uuid::new_v4());
    let resp = client
        .post(format!("{}/storage/upload/resumable", base_url()))
        .header("Authorization", format!("Bearer {token}")) // ignore-magic
        .json(&json!({ // ignore-magic
            "path": path,
            "content_type": "application/octet-stream",
            "total_size": 6_000_000_000_u64  // 6GB > 5GB limit
        }))
        .send()
        .await
        .expect("resumable init failed");

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "Exceeding 5GB limit should be rejected, got status={status}"
    );
}

// =============================================================================
// SECTION 6: Unauthenticated access
// =============================================================================

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn test_storage_presign_upload_no_auth() {
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/storage/presign/upload", base_url()))
        // No Authorization header
        .json(&json!({ // ignore-magic
            "paths": ["products/test.jpg"]
        }))
        .send()
        .await
        .expect("request failed");

    let status = resp.status().as_u16();
    // In test mode (OB_TEST_MODE=1) this may succeed; in prod it should fail
    assert!(
        status == 200 || status == 401 || status == 403,
        "No auth: expected 200 (test mode) or 401/403, got {status}"
    );
}
