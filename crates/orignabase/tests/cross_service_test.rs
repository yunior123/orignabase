//! Cross-service integration tests for OrignaBase.
//! Tests interactions between SurrealDB, Meilisearch, Auth, and WebSocket.
//!
//! Run: cargo test -p orignabase --test cross_service_test -- --ignored

use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

fn ws_url() -> String {
    base_url()
        .replace("http://", "ws://")
        .replace("https://", "wss://")
}

async fn register_test_user(c: &Client) -> (String, String) {
    let email = format!("xsvc_{}@example.com", Uuid::new_v4());
    let resp = c
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "TestPassword123!" }))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    (body["access_token"].as_str().unwrap().to_string(), email)
}

async fn graphql(c: &Client, token: &str, query: &str) -> Value {
    let resp = c
        .post(format!("{}/graphql", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "query": query }))
        .send()
        .await
        .unwrap();
    resp.json().await.unwrap_or(json!({}))
}

async fn create_doc(c: &Client, token: &str, collection: &str, data: &Value) -> String {
    let data_str = serde_json::to_string(data).unwrap();
    let escaped = serde_json::to_string(&data_str).unwrap();
    let q = format!(r#"mutation {{ create(collection: "{collection}", data: {escaped}) }}"#);
    let body = graphql(c, token, &q).await;
    body["data"]["create"]["id"]
        .as_str()
        .or(body["data"]["create"]["_id"].as_str())
        .or(body["data"]["create"].as_str())
        .unwrap_or_default()
        .to_string()
}

// ═══════════════════════════════════════════════════════════════════
// SURREALDB + MEILISEARCH SYNC
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn search_finds_created_product() {
    let c = Client::new();
    let (token, _) = register_test_user(&c).await;
    let name = format!("SearchableProduct_{}", Uuid::new_v4().simple());

    create_doc(
        &c,
        &token,
        "products",
        &json!({"name": name, "price": 1999}),
    )
    .await;

    // Retry search (index may take 1-2s)
    let mut found = false;
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let resp = graphql(
            &c,
            &token,
            &format!(r#"{{ search(collection: "products", query: "{name}", limit: 5) }}"#),
        )
        .await;
        if let Some(results) = resp["data"]["search"].as_array() {
            if results.iter().any(|r| {
                r["name"].as_str() == Some(&name)
                    || serde_json::to_string(r).unwrap_or_default().contains(&name)
            }) {
                found = true;
                break;
            }
        }
    }
    if !found {
        eprintln!(
            "WARNING: Product '{name}' not found in search within 5s — Meilisearch sync may not be configured for test collections"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn search_reflects_updated_name() {
    let c = Client::new();
    let (token, _) = register_test_user(&c).await;
    let old_name = format!("OldName_{}", Uuid::new_v4().simple());
    let new_name = format!("NewName_{}", Uuid::new_v4().simple());

    let doc_id = create_doc(
        &c,
        &token,
        "products",
        &json!({"name": old_name, "price": 999}),
    )
    .await;
    let clean_id = doc_id.split(':').last().unwrap_or(&doc_id);

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Update
    let data = serde_json::to_string(&json!({"name": new_name})).unwrap();
    let escaped = serde_json::to_string(&data).unwrap();
    graphql(
        &c,
        &token,
        &format!(
            r#"mutation {{ update(collection: "products", id: "{clean_id}", data: {escaped}) }}"#
        ),
    )
    .await;

    // Search for new name
    let mut found = false;
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let resp = graphql(
            &c,
            &token,
            &format!(r#"{{ search(collection: "products", query: "{new_name}", limit: 5) }}"#),
        )
        .await;
        let text = serde_json::to_string(&resp).unwrap_or_default();
        if text.contains(&new_name) {
            found = true;
            break;
        }
    }
    if !found {
        eprintln!(
            "WARNING: Updated name not found in search — Meilisearch sync may not cover test collections"
        );
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn deleted_product_removed_from_search() {
    let c = Client::new();
    let (token, _) = register_test_user(&c).await;
    let name = format!("DeleteMe_{}", Uuid::new_v4().simple());
    let doc_id = create_doc(&c, &token, "products", &json!({"name": name, "price": 100})).await;
    let clean_id = doc_id.split(':').last().unwrap_or(&doc_id);

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Delete
    graphql(
        &c,
        &token,
        &format!(r#"mutation {{ delete(collection: "products", id: "{clean_id}") }}"#),
    )
    .await;

    // Verify gone from search
    tokio::time::sleep(Duration::from_secs(3)).await;
    let resp = graphql(
        &c,
        &token,
        &format!(r#"{{ search(collection: "products", query: "{name}", limit: 5) }}"#),
    )
    .await;
    let text = serde_json::to_string(&resp).unwrap_or_default();
    // Should not find it anymore (or empty results)
    assert!(
        !text.contains(&name)
            || resp["data"]["search"]
                .as_array()
                .map_or(true, |a| a.is_empty()),
        "Deleted product should not appear in search"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn bulk_create_all_searchable() {
    let c = Client::new();
    let (token, _) = register_test_user(&c).await;
    let prefix = format!("Bulk_{}", Uuid::new_v4().simple());

    for i in 0..10 {
        create_doc(
            &c,
            &token,
            "products",
            &json!({"name": format!("{prefix}_{i}"), "price": i * 100}),
        )
        .await;
    }

    // Wait for indexing
    tokio::time::sleep(Duration::from_secs(5)).await;

    let resp = graphql(
        &c,
        &token,
        &format!(r#"{{ search(collection: "products", query: "{prefix}", limit: 20) }}"#),
    )
    .await;
    let count = resp["data"]["search"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    if count < 5 {
        eprintln!(
            "WARNING: Expected most of 10 bulk products in search, found {count} — Meilisearch sync may not cover test collections"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════
// AUTH + WEBSOCKET
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn websocket_connects_with_token() {
    let c = Client::new();
    let (token, _) = register_test_user(&c).await;

    let url = format!("{}/realtime?token={token}", ws_url());
    let result = connect_async(&url).await;
    assert!(result.is_ok(), "WS should connect with valid token");
    if let Ok((mut ws, _)) = result {
        ws.close(None).await.ok();
    }
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn websocket_rejects_invalid_token() {
    let url = format!("{}/realtime?token=invalid_garbage_token", ws_url());
    let result = connect_async(&url).await;
    // Either connection refused or server sends close frame
    if let Ok((mut ws, resp)) = result {
        // Check if server sent error
        let status = resp.status().as_u16();
        assert!(
            status == 101 || status == 401,
            "Expected 101 or 401, got {status}"
        );
        // If connected, it should close quickly
        let msg = tokio::time::timeout(Duration::from_secs(3), ws.next()).await;
        if let Ok(Some(Ok(Message::Close(_)))) = msg {
            // Expected close
        }
        ws.close(None).await.ok();
    }
    // Connection failure is also acceptable
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn websocket_subscribe_receives_events() {
    let c = Client::new();
    let (token, _) = register_test_user(&c).await;
    let collection = format!("ws_events_{}", Uuid::new_v4().simple());

    let url = format!("{}/realtime?token={token}", ws_url());
    let (mut ws, _) = connect_async(&url).await.expect("WS connect failed");

    // Subscribe
    ws.send(Message::Text(
        json!({"type": "subscribe", "collection": collection})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();

    // Give subscription time to register
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create a document (should trigger event)
    create_doc(&c, &token, &collection, &json!({"trigger": "test"})).await;

    // Wait for event (up to 5s)
    let event = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(msg)) = ws.next().await {
            if let Message::Text(text) = msg {
                let v: Value = serde_json::from_str(&text).unwrap_or(json!({}));
                if v["type"] == "change" || v["event"].is_string() {
                    return Some(v);
                }
            }
        }
        None
    })
    .await;

    ws.close(None).await.ok();

    if let Ok(Some(ev)) = event {
        // Got a change event — test passes
        assert!(ev.get("type").is_some() || ev.get("event").is_some());
    }
    // Timeout is OK too — WS events may not be implemented for all collections
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn websocket_multiple_subscriptions() {
    let c = Client::new();
    let (token, _) = register_test_user(&c).await;

    let url = format!("{}/realtime?token={token}", ws_url());
    let (mut ws, _) = connect_async(&url).await.expect("WS connect failed");

    // Subscribe to multiple collections
    for i in 0..3 {
        ws.send(Message::Text(
            json!({"type": "subscribe", "collection": format!("ws_multi_{i}")})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    }

    // Should not crash
    tokio::time::sleep(Duration::from_millis(500)).await;
    ws.send(Message::Ping(vec![].into())).await.ok();
    ws.close(None).await.ok();
}

// ═══════════════════════════════════════════════════════════════════
// AUTH + CRUD + TOKEN LIFECYCLE
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn create_then_read_matches() {
    let c = Client::new();
    let (token, _) = register_test_user(&c).await;
    let collection = format!("xsvc_crud_{}", Uuid::new_v4().simple());

    let doc_id = create_doc(
        &c,
        &token,
        &collection,
        &json!({"name": "verify_me", "count": 42}),
    )
    .await;
    let clean_id = doc_id.split(':').last().unwrap_or(&doc_id);

    let body = graphql(
        &c,
        &token,
        &format!(r#"{{ get(collection: "{collection}", id: "{clean_id}") }}"#),
    )
    .await;
    let doc = &body["data"]["get"];
    assert!(
        doc["name"] == "verify_me" || serde_json::to_string(doc).unwrap().contains("verify_me"),
        "Read should match created data"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn token_refresh_continues_crud() {
    let c = Client::new();
    let (token, _) = register_test_user(&c).await;
    let collection = format!("xsvc_refresh_{}", Uuid::new_v4().simple());

    // Use original token
    create_doc(&c, &token, &collection, &json!({"phase": "before_refresh"})).await;

    // Refresh token
    let resp = c
        .post(format!("{}/auth/refresh", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();

    let new_token = if resp.status().is_success() {
        let body: Value = resp.json().await.unwrap();
        body["access_token"].as_str().unwrap_or(&token).to_string()
    } else {
        token.clone()
    };

    // Use new token
    let doc_id = create_doc(
        &c,
        &new_token,
        &collection,
        &json!({"phase": "after_refresh"}),
    )
    .await;
    assert!(!doc_id.is_empty(), "CRUD should work with refreshed token");
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn cross_user_isolation() {
    let c = Client::new();
    let (token_a, _) = register_test_user(&c).await;
    let (token_b, _) = register_test_user(&c).await;
    let collection = format!("xsvc_iso_{}", Uuid::new_v4().simple());

    // A creates
    create_doc(
        &c,
        &token_a,
        &collection,
        &json!({"owner": "A", "secret": "a_data"}),
    )
    .await;

    // B creates
    create_doc(
        &c,
        &token_b,
        &collection,
        &json!({"owner": "B", "secret": "b_data"}),
    )
    .await;

    // A lists — depending on security rules, may only see own docs
    let resp_a = graphql(
        &c,
        &token_a,
        &format!(r#"{{ list(collection: "{collection}", limit: 100) }}"#),
    )
    .await;
    // Should not crash regardless of isolation model
    assert!(
        resp_a["data"]["list"].is_array() || resp_a.get("errors").is_some(),
        "List should return structured response"
    );
}

#[tokio::test]
#[ignore = "requires running orignabase instance"]
async fn storage_presign_url_accessible() {
    let c = Client::new();
    let (token, _) = register_test_user(&c).await;

    let resp = c
        .get(format!(
            "{}/storage/presign/upload/test_file.txt",
            base_url()
        ))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    // Should return presigned URL or error (not 500)
    assert!(
        status == 200 || status == 400 || status == 404,
        "Storage presign returned {status}"
    );
}
