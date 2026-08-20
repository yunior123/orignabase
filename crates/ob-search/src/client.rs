use crate::SearchConfig;
use crate::config::IndexConfig;
use ob_core::constants::fields as f;
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_QUERY_LENGTH: usize = 500;

fn sanitize_search_query(input: &str) -> String {
    let mut result = String::with_capacity(input.len().min(MAX_QUERY_LENGTH));

    for c in input.chars() {
        if c.is_control() {
            continue;
        }
        if c.is_whitespace() {
            if !result.ends_with(' ') {
                result.push(' ');
            }
        } else {
            result.push(c);
        }
    }

    result.trim().chars().take(MAX_QUERY_LENGTH).collect()
}

/// HTTP client wrapper for Meilisearch API.
#[derive(Clone)]
pub struct SearchClient {
    config: SearchConfig,
    http: reqwest::Client,
}

/// Search result from Meilisearch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub hits: Vec<Value>,
    pub query: String,
    #[serde(rename = "processingTimeMs", alias = "processing_time_ms")]
    pub processing_time_ms: u64,
    #[serde(rename = "estimatedTotalHits", alias = "estimated_total_hits")]
    pub estimated_total_hits: Option<u64>,
}

impl SearchClient {
    pub fn new(config: SearchConfig, http_client: reqwest::Client) -> Self {
        Self {
            config,
            http: http_client,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Apply configured index settings to Meilisearch.
    pub async fn ensure_indexes(&self) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        for (index, settings) in &self.config.indexes {
            self.apply_index_settings(index, settings).await?;
        }

        Ok(())
    }

    /// Search an index. Makes an HTTP request to Meilisearch.
    pub async fn search(
        &self,
        index: &str,
        query: &str,
        limit: Option<usize>,
        offset: Option<usize>,
        filter: Option<&str>,
    ) -> Result<SearchResult> {
        let sanitized_query = sanitize_search_query(query);

        if !self.config.enabled {
            return Ok(SearchResult {
                hits: vec![],
                query: sanitized_query,
                processing_time_ms: 0,
                estimated_total_hits: Some(0),
            });
        }

        let url = format!("{}/indexes/{}/search", self.config.url, index);

        let mut body = serde_json::json!({
            "q": sanitized_query,
        });

        if let Some(n) = limit {
            body["limit"] = serde_json::json!(n);
        }
        if let Some(n) = offset {
            body["offset"] = serde_json::json!(n);
        }
        if let Some(f) = filter {
            body["filter"] = serde_json::json!(f);
        }

        // Build request
        let client = &self.http;
        let mut req = client.post(&url).json(&body);

        if let Some(ref key) = self.config.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Meilisearch request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            let error_preview = if body.len() > 200 {
                &body[..200]
            } else {
                &body
            };
            return Err(Error::Internal(format!(
                "Meilisearch error ({status}): {error_preview}"
            )));
        }

        let mut result = resp
            .json::<SearchResult>()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse search response: {e}")))?;

        for hit in &mut result.hits {
            let record_id = hit
                .get("record_id")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned);
            if let Some(record_id) = record_id
                && let Some(obj) = hit.as_object_mut()
            {
                obj.insert(f::ID.to_string(), Value::String(record_id));
            }
        }

        Ok(result)
    }

    /// Upsert documents into a Meilisearch index.
    pub async fn upsert_documents(&self, index: &str, documents: &[Value]) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let url = format!(
            "{}/indexes/{}/documents?primaryKey=id",
            self.config.url, index
        );
        let client = &self.http;
        let mut req = client.post(&url).json(documents);

        if let Some(ref key) = self.config.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Meilisearch upsert failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(Error::Internal("Meilisearch upsert failed".into()));
        }

        Ok(())
    }

    /// Delete a document from a Meilisearch index.
    pub async fn delete_document(&self, index: &str, document_id: &str) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let url = format!(
            "{}/indexes/{}/documents/{}",
            self.config.url, index, document_id
        );
        let client = &self.http;
        let mut req = client.delete(&url);

        if let Some(ref key) = self.config.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Meilisearch delete failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(Error::Internal("Meilisearch delete failed".into()));
        }

        Ok(())
    }

    async fn apply_index_settings(&self, index: &str, settings: &IndexConfig) -> Result<()> {
        self.put_index_setting(index, "searchable-attributes", &settings.searchable)
            .await?;
        self.put_index_setting(index, "filterable-attributes", &settings.filterable)
            .await?;
        self.put_index_setting(index, "sortable-attributes", &settings.sortable)
            .await?;
        Ok(())
    }

    async fn put_index_setting<T: Serialize + ?Sized>(
        &self,
        index: &str,
        setting: &str,
        value: &T,
    ) -> Result<()> {
        let url = format!(
            "{}/indexes/{}/settings/{}",
            self.config.url.trim_end_matches('/'),
            index,
            setting
        );

        let client = &self.http;
        let mut req = client.put(&url).json(value);
        if let Some(ref key) = self.config.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Meilisearch settings update failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(Error::Internal("Meilisearch settings update failed".into()));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_search_query_basic() {
        assert_eq!(sanitize_search_query("hello world"), "hello world");
    }

    #[test]
    fn test_sanitize_search_query_strips_control_chars() {
        assert_eq!(sanitize_search_query("hello\x00world"), "helloworld");
        assert_eq!(sanitize_search_query("test\n\r\t"), "test");
    }

    #[test]
    fn test_sanitize_search_query_normalizes_whitespace() {
        assert_eq!(sanitize_search_query("hello   world"), "hello world");
        assert_eq!(sanitize_search_query("a  b   c"), "a b c");
    }

    #[test]
    fn test_sanitize_search_query_trims() {
        assert_eq!(sanitize_search_query("  hello  "), "hello");
    }

    #[test]
    fn test_sanitize_search_query_length_limit() {
        let long_query = "a".repeat(600);
        let sanitized = sanitize_search_query(&long_query);
        assert_eq!(sanitized.len(), 500);
    }

    #[test]
    fn test_sanitize_search_query_preserves_normal_chars() {
        assert_eq!(sanitize_search_query("test&query!@#$"), "test&query!@#$");
    }

    #[test]
    fn test_sanitize_search_query_empty() {
        assert_eq!(sanitize_search_query(""), "");
        assert_eq!(sanitize_search_query("   "), "");
    }

    #[test]
    fn test_sanitize_search_query_special_chars() {
        assert_eq!(
            sanitize_search_query("<script>alert(1)</script>"),
            "<script>alert(1)</script>"
        );
    }

    #[test]
    fn test_sanitize_search_query_unicode() {
        assert_eq!(sanitize_search_query("café"), "café");
        assert_eq!(sanitize_search_query("日本語"), "日本語");
    }

    #[test]
    fn test_search_result_serde_roundtrip() {
        let result = SearchResult {
            hits: vec![
                serde_json::json!({"id": "1", "title": "Hello"}),
                serde_json::json!({"id": "2", "title": "World"}),
            ],
            query: "hello".to_string(),
            processing_time_ms: 42,
            estimated_total_hits: Some(100),
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: SearchResult = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.hits.len(), 2);
        assert_eq!(parsed.query, "hello");
        assert_eq!(parsed.processing_time_ms, 42);
        assert_eq!(parsed.estimated_total_hits, Some(100));
    }

    #[test]
    fn test_search_result_optional_total_hits() {
        let json = r#"{
            "hits": [],
            "query": "test",
            "processing_time_ms": 0,
            "estimated_total_hits": null
        }"#;
        let result: SearchResult = serde_json::from_str(json).unwrap();
        assert!(result.estimated_total_hits.is_none());
        assert!(result.hits.is_empty());
    }

    #[test]
    fn test_search_result_meilisearch_camel_case_fields() {
        let json = r#"{
            "hits": [{"id": "1", "title": "Widget"}],
            "query": "widget",
            "processingTimeMs": 3,
            "estimatedTotalHits": 1
        }"#;
        let result: SearchResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.processing_time_ms, 3);
        assert_eq!(result.estimated_total_hits, Some(1));
        assert_eq!(result.hits.len(), 1);
    }

    #[test]
    fn test_search_result_missing_optional_field() {
        let json = r#"{
            "hits": [{"id": "1"}],
            "query": "q",
            "processing_time_ms": 5
        }"#;
        let result: SearchResult = serde_json::from_str(json).unwrap();
        assert!(result.estimated_total_hits.is_none());
        assert_eq!(result.hits.len(), 1);
    }

    #[test]
    fn test_search_client_new() {
        let config = SearchConfig {
            enabled: true,
            url: "http://meili:7700".to_string(),
            api_key: Some("key123".to_string()),
            indexes: Default::default(),
        };
        let client = SearchClient::new(config.clone(), reqwest::Client::new());
        assert!(client.config.enabled);
        assert_eq!(client.config.url, "http://meili:7700");
        assert_eq!(client.config.api_key, Some("key123".to_string()));
    }

    #[test]
    fn test_search_client_clone() {
        fn assert_clone<T: Clone>() {}
        assert_clone::<SearchClient>();
        assert_clone::<SearchResult>();
    }

    #[test]
    fn test_search_result_hits_can_restore_record_id() {
        let mut result = SearchResult {
            hits: vec![serde_json::json!({
                "id": "products_abc123",
                "record_id": "products:abc123",
                "title": "Widget",
            })],
            query: "widget".to_string(),
            processing_time_ms: 1,
            estimated_total_hits: Some(1),
        };

        for hit in &mut result.hits {
            let record_id = hit
                .get("record_id")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned);
            if let Some(record_id) = record_id
                && let Some(obj) = hit.as_object_mut()
            {
                obj.insert(f::ID.to_string(), Value::String(record_id));
            }
        }

        assert_eq!(result.hits[0][f::ID], "products:abc123");
    }

    // ── SearchClient method tests ──

    #[test]
    fn test_is_enabled_true() {
        let config = SearchConfig {
            enabled: true,
            url: "http://meili:7700".to_string(),
            api_key: None,
            indexes: Default::default(),
        };
        let client = SearchClient::new(config, reqwest::Client::new());
        assert!(client.is_enabled());
    }

    #[test]
    fn test_is_enabled_false() {
        let config = SearchConfig {
            enabled: false,
            url: "http://meili:7700".to_string(),
            api_key: None,
            indexes: Default::default(),
        };
        let client = SearchClient::new(config, reqwest::Client::new());
        assert!(!client.is_enabled());
    }

    #[test]
    fn test_search_disabled_returns_empty_result() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = SearchConfig {
                enabled: false,
                url: "http://meili:7700".to_string(),
                api_key: None,
                indexes: Default::default(),
            };
            let client = SearchClient::new(config, reqwest::Client::new());
            let result = client
                .search("products", "widget", None, None, None)
                .await
                .unwrap();
            assert!(result.hits.is_empty());
            assert_eq!(result.query, "widget");
            assert_eq!(result.processing_time_ms, 0);
            assert_eq!(result.estimated_total_hits, Some(0));
        });
    }

    #[test]
    fn test_upsert_documents_disabled_skips() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = SearchConfig {
                enabled: false,
                url: "http://meili:7700".to_string(),
                api_key: None,
                indexes: Default::default(),
            };
            let client = SearchClient::new(config, reqwest::Client::new());
            let docs = vec![serde_json::json!({"id": "1", "title": "Test"})];
            let result = client.upsert_documents("products", &docs).await;
            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_delete_document_disabled_skips() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = SearchConfig {
                enabled: false,
                url: "http://meili:7700".to_string(),
                api_key: None,
                indexes: Default::default(),
            };
            let client = SearchClient::new(config, reqwest::Client::new());
            let result = client.delete_document("products", "doc_1").await;
            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_ensure_indexes_disabled_skips() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = SearchConfig {
                enabled: false,
                url: "http://meili:7700".to_string(),
                api_key: None,
                indexes: Default::default(),
            };
            let client = SearchClient::new(config, reqwest::Client::new());
            let result = client.ensure_indexes().await;
            assert!(result.is_ok());
        });
    }

    // ── SearchResult edge cases ──

    #[test]
    fn test_search_result_empty_hits() {
        let result = SearchResult {
            hits: vec![],
            query: "nonexistent".to_string(),
            processing_time_ms: 0,
            estimated_total_hits: Some(0),
        };
        assert!(result.hits.is_empty());
        assert_eq!(result.processing_time_ms, 0);
    }

    #[test]
    fn test_search_result_many_hits() {
        let hits: Vec<Value> = (0..100)
            .map(|i| serde_json::json!({"id": format!("doc_{i}"), "title": format!("Item {i}")}))
            .collect();
        let result = SearchResult {
            hits,
            query: "item".to_string(),
            processing_time_ms: 50,
            estimated_total_hits: Some(100),
        };
        assert_eq!(result.hits.len(), 100);
        assert_eq!(result.estimated_total_hits, Some(100));
    }

    #[test]
    fn test_search_result_serializes_camel_case() {
        let result = SearchResult {
            hits: vec![serde_json::json!({"id": "1"})],
            query: "test".to_string(),
            processing_time_ms: 10,
            estimated_total_hits: Some(1),
        };
        let json_str = serde_json::to_string(&result).unwrap();
        assert!(json_str.contains("processingTimeMs"));
        assert!(json_str.contains("estimatedTotalHits"));
    }

    #[test]
    fn test_search_result_deserialize_snake_case_alias() {
        let json = r#"{
            "hits": [],
            "query": "test",
            "processing_time_ms": 5,
            "estimated_total_hits": 0
        }"#;
        let result: SearchResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.processing_time_ms, 5);
        assert_eq!(result.estimated_total_hits, Some(0));
    }

    #[test]
    fn test_search_result_hit_with_nested_objects() {
        let json = r#"{
            "hits": [{"id": "1", "metadata": {"color": "red", "size": 10}}],
            "query": "widget",
            "processingTimeMs": 2,
            "estimatedTotalHits": 1
        }"#;
        let result: SearchResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.hits[0]["metadata"]["color"], "red");
        assert_eq!(result.hits[0]["metadata"]["size"], 10);
    }

    #[test]
    fn test_search_result_hit_without_record_id_unchanged() {
        let mut hit = serde_json::json!({
            "id": "products_1",
            "title": "Widget"
        });
        // Simulate the record_id restoration logic
        let record_id = hit
            .get("record_id")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned);
        if let Some(record_id) = record_id
            && let Some(obj) = hit.as_object_mut()
        {
            obj.insert(f::ID.to_string(), Value::String(record_id));
        }
        // ID should remain unchanged since there's no record_id
        assert_eq!(hit[f::ID], "products_1");
    }

    #[test]
    fn test_search_result_query_with_special_chars() {
        let result = SearchResult {
            hits: vec![],
            query: "test & query with <special> \"chars\"".to_string(),
            processing_time_ms: 0,
            estimated_total_hits: Some(0),
        };
        assert_eq!(result.query, "test & query with <special> \"chars\"");
    }

    #[test]
    fn test_search_config_with_api_key() {
        let config = SearchConfig {
            enabled: true,
            url: "https://meili.example.com".to_string(),
            api_key: Some("master_key_abc".to_string()),
            indexes: Default::default(),
        };
        let client = SearchClient::new(config, reqwest::Client::new());
        assert_eq!(client.config.api_key, Some("master_key_abc".to_string()));
    }

    #[test]
    fn test_search_config_url_with_trailing_slash() {
        let config = SearchConfig {
            enabled: true,
            url: "http://localhost:7700/".to_string(),
            api_key: None,
            indexes: Default::default(),
        };
        // URL should be used as-is in format!, trailing slash would produce //
        // but the client doesn't normalize it
        let client = SearchClient::new(config, reqwest::Client::new());
        assert_eq!(client.config.url, "http://localhost:7700/");
    }

    #[test]
    fn test_search_config_no_api_key() {
        let config = SearchConfig {
            enabled: true,
            url: "http://meili:7700".to_string(),
            api_key: None,
            indexes: Default::default(),
        };
        let client = SearchClient::new(config, reqwest::Client::new());
        assert!(client.config.api_key.is_none());
    }

    #[test]
    fn test_search_result_debug_format() {
        let result = SearchResult {
            hits: vec![],
            query: "test".to_string(),
            processing_time_ms: 0,
            estimated_total_hits: None,
        };
        let debug = format!("{:?}", result);
        assert!(debug.contains("SearchResult"));
        assert!(debug.contains("test"));
    }

    #[test]
    fn test_search_result_with_zero_processing_time() {
        let result = SearchResult {
            hits: vec![serde_json::json!({"id": "1"})],
            query: "".to_string(),
            processing_time_ms: 0,
            estimated_total_hits: Some(1),
        };
        assert_eq!(result.processing_time_ms, 0);
        assert_eq!(result.estimated_total_hits, Some(1));
    }

    #[test]
    fn test_search_result_hits_with_arrays() {
        let json = r#"{
            "hits": [{"id": "1", "tags": ["a", "b", "c"]}],
            "query": "q",
            "processingTimeMs": 1,
            "estimatedTotalHits": 1
        }"#;
        let result: SearchResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.hits[0]["tags"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_search_result_hit_with_null_values() {
        let json = r#"{
            "hits": [{"id": "1", "description": null}],
            "query": "q",
            "processingTimeMs": 1,
            "estimatedTotalHits": 1
        }"#;
        let result: SearchResult = serde_json::from_str(json).unwrap();
        assert!(result.hits[0]["description"].is_null());
    }

    #[test]
    fn test_search_result_large_processing_time() {
        let result = SearchResult {
            hits: vec![],
            query: "complex".to_string(),
            processing_time_ms: u64::MAX,
            estimated_total_hits: Some(0),
        };
        assert_eq!(result.processing_time_ms, u64::MAX);
    }

    #[test]
    fn test_search_result_estimated_hits_large_value() {
        let result = SearchResult {
            hits: vec![],
            query: "popular".to_string(),
            processing_time_ms: 100,
            estimated_total_hits: Some(1_000_000),
        };
        assert_eq!(result.estimated_total_hits, Some(1_000_000));
    }

    // ── Wiremock-based tests for enabled client paths ──

    fn enabled_config(url: &str) -> SearchConfig {
        SearchConfig {
            enabled: true,
            url: url.to_string(),
            api_key: Some("test-api-key".to_string()),
            indexes: Default::default(),
        }
    }

    #[tokio::test]
    async fn test_search_enabled_success() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let mock_server = MockServer::start().await;
        let response_body = serde_json::json!({
            "hits": [{"id": "1", "title": "Widget"}],
            "query": "widget",
            "processingTimeMs": 5,
            "estimatedTotalHits": 1
        });

        Mock::given(matchers::method("POST"))
            .and(matchers::path("/indexes/products/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SearchClient::new(enabled_config(&mock_server.uri()), reqwest::Client::new());
        let result = client
            .search("products", "widget", Some(10), None, None)
            .await
            .unwrap();
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.query, "widget");
        assert_eq!(result.processing_time_ms, 5);
    }

    #[tokio::test]
    async fn test_search_enabled_with_offset_and_filter() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let mock_server = MockServer::start().await;
        let response_body = serde_json::json!({
            "hits": [],
            "query": "phone",
            "processingTimeMs": 2,
            "estimatedTotalHits": 0
        });

        Mock::given(matchers::method("POST"))
            .and(matchers::path("/indexes/products/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SearchClient::new(enabled_config(&mock_server.uri()), reqwest::Client::new());
        let result = client
            .search(
                "products",
                "phone",
                Some(5),
                Some(10),
                Some("category = 'electronics'"),
            )
            .await
            .unwrap();
        assert!(result.hits.is_empty());
    }

    #[tokio::test]
    async fn test_search_enabled_server_error() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let mock_server = MockServer::start().await;

        Mock::given(matchers::method("POST"))
            .and(matchers::path("/indexes/products/search"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SearchClient::new(enabled_config(&mock_server.uri()), reqwest::Client::new());
        let result = client.search("products", "test", None, None, None).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Meilisearch error"));
    }

    #[tokio::test]
    async fn test_search_enabled_record_id_restoration() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let mock_server = MockServer::start().await;
        let response_body = serde_json::json!({
            "hits": [
                {"id": "products_abc", "record_id": "products:abc", "title": "Widget"},
                {"id": "products_xyz", "title": "No record_id"}
            ],
            "query": "w",
            "processingTimeMs": 1,
            "estimatedTotalHits": 2
        });

        Mock::given(matchers::method("POST"))
            .and(matchers::path("/indexes/products/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SearchClient::new(enabled_config(&mock_server.uri()), reqwest::Client::new());
        let result = client
            .search("products", "w", None, None, None)
            .await
            .unwrap();
        // First hit should have record_id restored
        assert_eq!(result.hits[0][f::ID], "products:abc");
        // Second hit without record_id should keep original id
        assert_eq!(result.hits[1][f::ID], "products_xyz");
    }

    #[tokio::test]
    async fn test_search_enabled_no_api_key() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let mock_server = MockServer::start().await;
        let response_body = serde_json::json!({
            "hits": [],
            "query": "test",
            "processingTimeMs": 0,
            "estimatedTotalHits": 0
        });

        Mock::given(matchers::method("POST"))
            .and(matchers::path("/indexes/products/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = SearchConfig {
            enabled: true,
            url: mock_server.uri(),
            api_key: None,
            indexes: Default::default(),
        };
        let client = SearchClient::new(config, reqwest::Client::new());
        let result = client.search("products", "test", None, None, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_upsert_documents_enabled_success() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let mock_server = MockServer::start().await;

        Mock::given(matchers::method("POST"))
            .and(matchers::path("/indexes/products/documents"))
            .respond_with(
                ResponseTemplate::new(202).set_body_json(serde_json::json!({"taskUid": 1})),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SearchClient::new(enabled_config(&mock_server.uri()), reqwest::Client::new());
        let docs = vec![serde_json::json!({"id": "1", "title": "Widget"})];
        let result = client.upsert_documents("products", &docs).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_upsert_documents_enabled_server_error() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let mock_server = MockServer::start().await;

        Mock::given(matchers::method("POST"))
            .and(matchers::path("/indexes/products/documents"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SearchClient::new(enabled_config(&mock_server.uri()), reqwest::Client::new());
        let docs = vec![serde_json::json!({"id": "1"})];
        let result = client.upsert_documents("products", &docs).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_delete_document_enabled_success() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let mock_server = MockServer::start().await;

        Mock::given(matchers::method("DELETE"))
            .and(matchers::path("/indexes/products/documents/doc_1"))
            .respond_with(
                ResponseTemplate::new(202).set_body_json(serde_json::json!({"taskUid": 2})),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SearchClient::new(enabled_config(&mock_server.uri()), reqwest::Client::new());
        let result = client.delete_document("products", "doc_1").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_delete_document_enabled_server_error() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let mock_server = MockServer::start().await;

        Mock::given(matchers::method("DELETE"))
            .and(matchers::path("/indexes/products/documents/doc_1"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SearchClient::new(enabled_config(&mock_server.uri()), reqwest::Client::new());
        let result = client.delete_document("products", "doc_1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ensure_indexes_with_settings() {
        use crate::config::IndexConfig;
        use std::collections::HashMap;
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let mock_server = MockServer::start().await;

        // Mock all 3 settings endpoints
        Mock::given(matchers::method("PUT"))
            .respond_with(
                ResponseTemplate::new(202).set_body_json(serde_json::json!({"taskUid": 1})),
            )
            .expect(3) // searchable, filterable, sortable
            .mount(&mock_server)
            .await;

        let mut indexes = HashMap::new();
        indexes.insert(
            "products".to_string(),
            IndexConfig {
                searchable: vec!["title".into(), "description".into()],
                filterable: vec!["category".into()],
                sortable: vec!["price".into()],
                primary_key: "id".into(),
            },
        );

        let config = SearchConfig {
            enabled: true,
            url: mock_server.uri(),
            api_key: Some("key".into()),
            indexes,
        };
        let client = SearchClient::new(config, reqwest::Client::new());
        let result = client.ensure_indexes().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_ensure_indexes_settings_error() {
        use crate::config::IndexConfig;
        use std::collections::HashMap;
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let mock_server = MockServer::start().await;

        Mock::given(matchers::method("PUT"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1..)
            .mount(&mock_server)
            .await;

        let mut indexes = HashMap::new();
        indexes.insert(
            "products".to_string(),
            IndexConfig {
                searchable: vec!["title".into()],
                filterable: vec![],
                sortable: vec![],
                primary_key: "id".into(),
            },
        );

        let config = SearchConfig {
            enabled: true,
            url: mock_server.uri(),
            api_key: None,
            indexes,
        };
        let client = SearchClient::new(config, reqwest::Client::new());
        let result = client.ensure_indexes().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_search_enabled_invalid_json_response() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let mock_server = MockServer::start().await;

        Mock::given(matchers::method("POST"))
            .and(matchers::path("/indexes/products/search"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SearchClient::new(enabled_config(&mock_server.uri()), reqwest::Client::new());
        let result = client.search("products", "test", None, None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("parse"));
    }

    #[tokio::test]
    async fn test_search_enabled_connection_refused() {
        let config = SearchConfig {
            enabled: true,
            url: "http://127.0.0.1:19999".to_string(),
            api_key: None,
            indexes: Default::default(),
        };
        let client = SearchClient::new(config, reqwest::Client::new());
        let result = client.search("products", "test", None, None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("request failed"));
    }

    #[tokio::test]
    async fn test_upsert_documents_connection_refused() {
        let config = SearchConfig {
            enabled: true,
            url: "http://127.0.0.1:19999".to_string(),
            api_key: None,
            indexes: Default::default(),
        };
        let client = SearchClient::new(config, reqwest::Client::new());
        let docs = vec![serde_json::json!({"id": "1"})];
        let result = client.upsert_documents("products", &docs).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_delete_document_connection_refused() {
        let config = SearchConfig {
            enabled: true,
            url: "http://127.0.0.1:19999".to_string(),
            api_key: None,
            indexes: Default::default(),
        };
        let client = SearchClient::new(config, reqwest::Client::new());
        let result = client.delete_document("products", "doc_1").await;
        assert!(result.is_err());
    }
}
