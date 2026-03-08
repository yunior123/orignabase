use crate::SearchConfig;
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// HTTP client wrapper for Meilisearch API.
#[derive(Clone)]
pub struct SearchClient {
    config: SearchConfig,
}

/// Search result from Meilisearch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub hits: Vec<Value>,
    pub query: String,
    pub processing_time_ms: u64,
    pub estimated_total_hits: Option<u64>,
}

impl SearchClient {
    pub fn new(config: SearchConfig) -> Self {
        Self { config }
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
        let client = reqwest_client();
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

        resp.json::<SearchResult>()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse search response: {e}")))
    }

    /// Upsert documents into a Meilisearch index.
    pub async fn upsert_documents(&self, index: &str, documents: &[Value]) -> Result<()> {
        let url = format!("{}/indexes/{}/documents", self.config.url, index);
        let client = reqwest_client();
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
        let url = format!(
            "{}/indexes/{}/documents/{}",
            self.config.url, index, document_id
        );
        let client = reqwest_client();
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

/// Create a minimal reqwest client. We avoid adding reqwest as a workspace dep —
/// this is a lazy-initialized, zero-config client.
fn reqwest_client() -> reqwest::Client {
    reqwest::Client::new()
}
