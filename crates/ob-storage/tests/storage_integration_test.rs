use std::collections::BTreeMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, Method, Request, StatusCode},
    response::IntoResponse,
    routing::{get, put},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use ob_auth::middleware::AuthContext;
use ob_core::constants::fields;
use ob_storage::{
    LocalStorage, ResumableUploadManager, S3Config, S3Storage, SignedUrlGenerator, StorageBackend,
    routes::{StorageState, storage_router},
};
use serde::Deserialize;
use serde_json::json;
use sha2::Sha256;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tower::util::ServiceExt;

type HmacSha256 = Hmac<Sha256>;

fn test_png_bytes() -> Vec<u8> {
    vec![
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
    ]
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn test_auth() -> AuthContext {
    AuthContext {
        user_id: "seller-123".to_string(),
        roles: vec![],
        authenticated: true,
        email_verified: true,
        custom_claims: serde_json::Value::Null,
    }
}

fn storage_state(root: &FsPath) -> StorageState {
    StorageState {
        storage: LocalStorage::new(root.join("files")).unwrap(),
        url_generator: SignedUrlGenerator::new("integration-secret", "http://localhost"),
        resumable: ResumableUploadManager::new(root.join("chunks")).unwrap(),
    }
}

fn signed_uri(url: &str) -> String {
    url[url.find("/storage").unwrap()..].to_string()
}

fn sign_delete_uri(path: &str, ttl_secs: u64) -> String {
    let expires = chrono::Utc::now().timestamp() as u64 + ttl_secs;
    let mut mac = HmacSha256::new_from_slice(b"integration-secret").unwrap();
    mac.update(format!("DELETE:{path}:{expires}").as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("/storage/delete/{path}?expires={expires}&sig={sig}")
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn signed_upload_download_and_delete_roundtrip_through_router() {
    let root = unique_temp_dir("ob-storage-router-roundtrip");
    let state = storage_state(&root);
    let app = storage_router(state.clone());
    let upload_uri = signed_uri(
        &state
            .url_generator
            .sign_upload("users/seller-123/avatar.png", 3600)
            .unwrap(),
    );

    let upload_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(upload_uri)
                .header("content-type", "image/png")
                .body(Body::from(test_png_bytes()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(upload_response.status(), StatusCode::OK);

    let download_uri = signed_uri(
        &state
            .url_generator
            .sign_download("users/seller-123/avatar.png", 3600)
            .unwrap(),
    );
    let download_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(download_uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(download_response.status(), StatusCode::OK);
    assert_eq!(
        download_response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );
    let downloaded = to_bytes(download_response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(downloaded.as_ref(), test_png_bytes().as_slice());

    let delete_uri = sign_delete_uri("users/seller-123/avatar.png", 3600);
    let delete_response = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(delete_uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete_response.status(), StatusCode::OK);
    assert!(
        !state
            .storage
            .exists("users/seller-123/avatar.png")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn batch_presign_and_resumable_flow_store_bytes_for_authenticated_user() {
    let root = unique_temp_dir("ob-storage-router-auth");
    let state = storage_state(&root);
    let app = storage_router(state.clone());

    let mut presign_request = Request::builder()
        .method(Method::POST)
        .uri("/storage/presign/upload")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "paths": ["products/seller-123/catalog/item.png"],
                "ttl_secs": 120
            })
            .to_string(),
        ))
        .unwrap();
    presign_request.extensions_mut().insert(test_auth());

    let presign_response = app.clone().oneshot(presign_request).await.unwrap();
    assert_eq!(presign_response.status(), StatusCode::OK);
    let payload = response_json(presign_response).await;
    assert_eq!(
        payload["urls"][0]["path"],
        "products/seller-123/catalog/item.png"
    );

    let mut init_request = Request::builder()
        .method(Method::POST)
        .uri("/storage/upload/resumable")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "path": "products/seller-123/catalog/item.png",
                "content_type": "image/png",
                "total_size": test_png_bytes().len()
            })
            .to_string(),
        ))
        .unwrap();
    init_request.extensions_mut().insert(test_auth());

    let init_response = app.clone().oneshot(init_request).await.unwrap();
    assert_eq!(init_response.status(), StatusCode::OK);
    let session = response_json(init_response).await;
    let session_id = session[fields::ID].as_str().unwrap().to_string();

    let mut append_request = Request::builder()
        .method(Method::PATCH)
        .uri(format!("/storage/upload/resumable/{session_id}"))
        .header("upload-offset", "0")
        .body(Body::from(test_png_bytes()))
        .unwrap();
    append_request.extensions_mut().insert(test_auth());

    let append_response = app.clone().oneshot(append_request).await.unwrap();
    assert_eq!(append_response.status(), StatusCode::OK);
    let final_session = response_json(append_response).await;
    assert_eq!(final_session[fields::STATUS], "complete");

    let stored = state
        .storage
        .download("products/seller-123/catalog/item.png")
        .await
        .unwrap();
    assert_eq!(stored, test_png_bytes());

    let mut status_request = Request::builder()
        .method(Method::GET)
        .uri(format!("/storage/upload/resumable/{session_id}"))
        .body(Body::empty())
        .unwrap();
    status_request.extensions_mut().insert(test_auth());

    let status_response = app.oneshot(status_request).await.unwrap();
    assert_eq!(status_response.status(), StatusCode::NOT_FOUND);
}

#[derive(Clone, Default)]
struct FakeS3State {
    objects: Arc<RwLock<BTreeMap<String, StoredObject>>>,
}

#[derive(Clone)]
struct StoredObject {
    body: Vec<u8>,
    content_type: String,
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(rename = "list-type")]
    list_type: Option<String>,
    #[serde(rename = "encoding-type")]
    encoding_type: Option<String>,
    prefix: Option<String>,
}

fn percent_encode_s3_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        let is_unreserved = matches!(
            byte,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~'
        );
        if is_unreserved {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

async fn fake_s3_put(
    State(state): State<FakeS3State>,
    Path((bucket, key)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    state.objects.write().await.insert(
        format!("{bucket}/{key}"),
        StoredObject {
            body: body.to_vec(),
            content_type: headers
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string(),
        },
    );
    StatusCode::OK
}

async fn fake_s3_get(
    State(state): State<FakeS3State>,
    Path(bucket): Path<String>,
    Query(query): Query<ListQuery>,
) -> impl IntoResponse {
    if query.list_type.as_deref() == Some("2") {
        let prefix = query.prefix.unwrap_or_default();
        let encoding_type = query.encoding_type.as_deref();
        let response_prefix = if encoding_type == Some("url") {
            percent_encode_s3_value(&prefix)
        } else {
            prefix.clone()
        };
        let objects = state.objects.read().await;
        let key_count = objects
            .keys()
            .filter(|full_key| {
                full_key
                    .strip_prefix(&format!("{bucket}/"))
                    .is_some_and(|key| key.starts_with(&prefix))
            })
            .count();
        let contents = objects
            .iter()
            .filter_map(|(full_key, object)| {
                let key = full_key.strip_prefix(&format!("{bucket}/"))?;
                if key.starts_with(&prefix) {
                    let response_key = if encoding_type == Some("url") {
                        percent_encode_s3_value(key)
                    } else {
                        key.to_string()
                    };
                    Some(format!(
                        "<Contents><Key>{key}</Key><LastModified>2024-01-01T00:00:00.000Z</LastModified><ETag>\"test-etag\"</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Contents>",
                        object.body.len(),
                        key = response_key
                    ))
                } else {
                    None
                }
            })
            .collect::<String>();
        let encoding_type_xml = if let Some(encoding_type) = encoding_type {
            format!("<EncodingType>{encoding_type}</EncodingType>")
        } else {
            String::new()
        };
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>{bucket}</Name><Prefix>{prefix}</Prefix>{encoding_type_xml}<KeyCount>{key_count}</KeyCount><MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated>{contents}</ListBucketResult>",
            prefix = response_prefix
        );
        return (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/xml")],
            body,
        )
            .into_response();
    }

    StatusCode::BAD_REQUEST.into_response()
}

async fn fake_s3_get_object(
    State(state): State<FakeS3State>,
    Path((bucket, key)): Path<(String, String)>,
) -> impl IntoResponse {
    let objects = state.objects.read().await;
    if let Some(object) = objects.get(&format!("{bucket}/{key}")) {
        return (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                object.content_type.clone(),
            )],
            object.body.clone(),
        )
            .into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

async fn fake_s3_head(
    State(state): State<FakeS3State>,
    Path((bucket, key)): Path<(String, String)>,
) -> impl IntoResponse {
    let objects = state.objects.read().await;
    if let Some(object) = objects.get(&format!("{bucket}/{key}")) {
        return (
            StatusCode::OK,
            [
                (
                    axum::http::header::CONTENT_LENGTH,
                    object.body.len().to_string(),
                ),
                (
                    axum::http::header::CONTENT_TYPE,
                    object.content_type.clone(),
                ),
                (
                    axum::http::header::LAST_MODIFIED,
                    "Wed, 21 Oct 2015 07:28:00 GMT".to_string(),
                ),
            ],
        )
            .into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

async fn fake_s3_delete(
    State(state): State<FakeS3State>,
    Path((bucket, key)): Path<(String, String)>,
) -> impl IntoResponse {
    state
        .objects
        .write()
        .await
        .remove(&format!("{bucket}/{key}"));
    StatusCode::NO_CONTENT
}

async fn spawn_fake_s3_server() -> String {
    let state = FakeS3State::default();
    let app = Router::new()
        .route("/{bucket}", get(fake_s3_get))
        .route("/{bucket}/", get(fake_s3_get))
        .route(
            "/{bucket}/{*key}",
            put(fake_s3_put)
                .get(fake_s3_get_object)
                .delete(fake_s3_delete)
                .head(fake_s3_head),
        )
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{address}")
}

#[tokio::test]
async fn s3_storage_roundtrip_hits_real_local_s3_compatible_server() {
    let endpoint = spawn_fake_s3_server().await;
    let storage = S3Storage::new(S3Config {
        bucket: "test-bucket".to_string(),
        region: "us-east-1".to_string(),
        endpoint: Some(endpoint),
        access_key: "test-access".to_string(),
        secret_key: "test-secret".to_string(),
    })
    .await
    .unwrap();

    let uploaded = storage
        .upload("catalog/item.png", &test_png_bytes(), "image/png")
        .await
        .unwrap();
    assert_eq!(uploaded.path, "catalog/item.png");

    assert!(storage.exists("catalog/item.png").await.unwrap());
    assert_eq!(
        storage.download("catalog/item.png").await.unwrap(),
        test_png_bytes()
    );

    let metadata = storage.metadata("catalog/item.png").await.unwrap();
    assert_eq!(metadata.size, test_png_bytes().len() as u64);
    assert_eq!(metadata.content_type, "image/png");

    let listed = storage.list("catalog/").await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, "catalog/item.png");

    storage.delete("catalog/item.png").await.unwrap();
    assert!(!storage.exists("catalog/item.png").await.unwrap());
}
