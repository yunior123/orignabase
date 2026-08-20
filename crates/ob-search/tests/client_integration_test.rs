use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode},
    response::IntoResponse,
    routing::{delete, post, put},
};
use ob_core::constants::fields as f;
use ob_search::{SearchClient, SearchConfig, config::IndexConfig};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[derive(Clone, Debug)]
struct RecordedRequest {
    method: Method,
    path: String,
    auth_header: Option<String>,
    body: Value,
}

#[derive(Clone, Default)]
struct TestServerState {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

async fn spawn_search_server() -> (SocketAddr, TestServerState) {
    let state = TestServerState::default();
    let app = Router::new()
        .route("/indexes/{index}/search", post(handle_search))
        .route("/indexes/{index}/documents", post(handle_upsert))
        .route("/indexes/{index}/documents/{id}", delete(handle_delete))
        .route(
            "/indexes/{index}/settings/{setting}",
            put(handle_settings_update),
        )
        .with_state(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (addr, state)
}

async fn record_request(
    state: &TestServerState,
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Bytes,
) -> Value {
    let parsed_body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };

    state.requests.lock().await.push(RecordedRequest {
        method,
        path,
        auth_header: headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned),
        body: parsed_body.clone(),
    });

    parsed_body
}

async fn handle_search(
    State(state): State<TestServerState>,
    headers: HeaderMap,
    axum::extract::Path(index): axum::extract::Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    let request_body = record_request(
        &state,
        Method::POST,
        format!("/indexes/{index}/search"),
        headers,
        body,
    )
    .await;

    let query = request_body
        .get("q")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if query == "trigger-server-error" {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "meilisearch upstream exploded".to_string(),
        )
            .into_response();
    }

    axum::Json(json!({
        "hits": [
            {
                "id": "products_widget_1",
                "record_id": "products:widget-1",
                "title": "Widget"
            }
        ],
        "query": query,
        "processingTimeMs": 7,
        "estimatedTotalHits": 1
    }))
    .into_response()
}

async fn handle_upsert(
    State(state): State<TestServerState>,
    headers: HeaderMap,
    axum::extract::Path(index): axum::extract::Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    record_request(
        &state,
        Method::POST,
        format!("/indexes/{index}/documents"),
        headers,
        body,
    )
    .await;
    StatusCode::ACCEPTED
}

async fn handle_delete(
    State(state): State<TestServerState>,
    headers: HeaderMap,
    axum::extract::Path((index, id)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    record_request(
        &state,
        Method::DELETE,
        format!("/indexes/{index}/documents/{id}"),
        headers,
        Bytes::new(),
    )
    .await;
    StatusCode::NO_CONTENT
}

async fn handle_settings_update(
    State(state): State<TestServerState>,
    headers: HeaderMap,
    axum::extract::Path((index, setting)): axum::extract::Path<(String, String)>,
    body: Bytes,
) -> impl IntoResponse {
    record_request(
        &state,
        Method::PUT,
        format!("/indexes/{index}/settings/{setting}"),
        headers,
        body,
    )
    .await;
    StatusCode::ACCEPTED
}

fn enabled_config(url: &str) -> SearchConfig {
    SearchConfig {
        enabled: true,
        url: url.to_string(),
        api_key: Some("integration-test-key".to_string()),
        indexes: HashMap::new(),
    }
}

#[tokio::test]
async fn search_hits_real_local_server_and_restores_record_id() {
    let (addr, state) = spawn_search_server().await;
    let client = SearchClient::new(
        enabled_config(&format!("http://{addr}")),
        reqwest::Client::new(),
    );

    let result = client
        .search(
            "products",
            "widget",
            Some(5),
            Some(10),
            Some("category = 'electronics'"),
        )
        .await
        .unwrap();

    assert_eq!(result.hits[0][f::ID], "products:widget-1");
    assert_eq!(result.query, "widget");
    assert_eq!(result.processing_time_ms, 7);

    let requests = state.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, Method::POST);
    assert_eq!(request.path, "/indexes/products/search");
    assert_eq!(
        request.auth_header.as_deref(),
        Some("Bearer integration-test-key")
    );
    assert_eq!(request.body["q"], "widget");
    assert_eq!(request.body["limit"], 5);
    assert_eq!(request.body["offset"], 10);
    assert_eq!(request.body["filter"], "category = 'electronics'");
}

#[tokio::test]
async fn ensure_indexes_updates_all_setting_endpoints() {
    let (addr, state) = spawn_search_server().await;
    let mut config = enabled_config(&format!("http://{addr}"));
    config.indexes.insert(
        "products".to_string(),
        IndexConfig {
            searchable: vec!["title".to_string(), "description".to_string()],
            filterable: vec!["category".to_string()],
            sortable: vec!["price_cents".to_string()],
            primary_key: "id".to_string(),
        },
    );

    let client = SearchClient::new(config, reqwest::Client::new());
    client.ensure_indexes().await.unwrap();

    let requests = state.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "/indexes/products/settings/searchable-attributes",
            "/indexes/products/settings/filterable-attributes",
            "/indexes/products/settings/sortable-attributes",
        ]
    );
    assert_eq!(requests[0].body, json!(["title", "description"]));
    assert_eq!(requests[1].body, json!(["category"]));
    assert_eq!(requests[2].body, json!(["price_cents"]));
}

#[tokio::test]
async fn upsert_and_delete_documents_hit_real_http_endpoints() {
    let (addr, state) = spawn_search_server().await;
    let client = SearchClient::new(
        enabled_config(&format!("http://{addr}")),
        reqwest::Client::new(),
    );

    client
        .upsert_documents(
            "products",
            &[json!({"id": "products:1", "title": "Widget"})],
        )
        .await
        .unwrap();
    client
        .delete_document("products", "products:1")
        .await
        .unwrap();

    let requests = state.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/indexes/products/documents");
    assert_eq!(
        requests[0].body,
        json!([{ "id": "products:1", "title": "Widget" }])
    );
    assert_eq!(requests[1].path, "/indexes/products/documents/products:1");
    assert!(requests[1].body.is_null());
}

#[tokio::test]
async fn search_surfaces_real_server_error_response_preview() {
    let (addr, _state) = spawn_search_server().await;
    let client = SearchClient::new(
        enabled_config(&format!("http://{addr}")),
        reqwest::Client::new(),
    );

    let error = client
        .search("products", "trigger-server-error", None, None, None)
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("Meilisearch error"));
    assert!(error.contains("meilisearch upstream exploded"));
}
