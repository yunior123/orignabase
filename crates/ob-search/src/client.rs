use crate::SearchConfig;
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
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
        if !self.config.enabled {
            return Ok(SearchResult {
                hits: vec![],
                query: query.to_string(),
                processing_time_ms: 0,
                estimated_total_hits: Some(0),
            });
        }

        let url = format!("{}/indexes/{}/search", self.config.url, index);

        let mut body = serde_json::json!({
            "q": query,
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
            return Err(Error::Internal(format!(
                "Meilisearch error ({status}): {body}"
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
                obj.insert("id".to_string(), Value::String(record_id));
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
            let body = resp.text().await.unwrap_or_else(|_| "unknown".to_string());
            return Err(Error::Internal(format!("Meilisearch upsert error: {body}")));
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
            let body = resp.text().await.unwrap_or_else(|_| "unknown".to_string());
            return Err(Error::Internal(format!("Meilisearch delete error: {body}")));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let client = SearchClient::new(config.clone());
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
                obj.insert("id".to_string(), Value::String(record_id));
            }
        }

        assert_eq!(result.hits[0]["id"], "products:abc123");
    }
}
