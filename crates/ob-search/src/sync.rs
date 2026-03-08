use crate::SearchClient;
use serde_json::Value;
use tokio::sync::mpsc;

/// Change event received from the realtime dispatcher for search sync.
#[derive(Debug, Clone)]
pub struct SearchSyncEvent {
    pub action: SearchAction,
    pub index: String,
    pub document_id: String,
    pub data: Value,
}

#[derive(Debug, Clone)]
pub enum SearchAction {
    Upsert,
    Delete,
}

/// Background task that syncs database changes to Meilisearch.
pub struct SearchSyncer {
    client: SearchClient,
    receiver: mpsc::Receiver<SearchSyncEvent>,
}

impl SearchSyncer {
    pub fn new(client: SearchClient) -> (Self, mpsc::Sender<SearchSyncEvent>) {
        let (tx, rx) = mpsc::channel(1024);
        (
            Self {
                client,
                receiver: rx,
            },
            tx,
        )
    }

    /// Run the sync loop — batches events and flushes to Meilisearch.
    pub async fn run(mut self) {
        tracing::info!("Search syncer started");

        while let Some(event) = self.receiver.recv().await {
            match event.action {
                SearchAction::Upsert => {
                    if let Err(e) = self
                        .client
                        .upsert_documents(&event.index, &[event.data])
                        .await
                    {
                        tracing::error!(
                            index = %event.index,
                            doc_id = %event.document_id,
                            "Search sync upsert failed: {e}"
                        );
                    }
                }
                SearchAction::Delete => {
                    if let Err(e) = self
                        .client
                        .delete_document(&event.index, &event.document_id)
                        .await
                    {
                        tracing::error!(
                            index = %event.index,
                            doc_id = %event.document_id,
                            "Search sync delete failed: {e}"
                        );
                    }
                }
            }
        }

        tracing::info!("Search syncer stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_action_debug_and_clone() {
        let upsert = SearchAction::Upsert;
        let delete = SearchAction::Delete;

        // Debug
        assert_eq!(format!("{:?}", upsert), "Upsert");
        assert_eq!(format!("{:?}", delete), "Delete");

        // Clone
        let cloned = upsert.clone();
        assert!(matches!(cloned, SearchAction::Upsert));
    }

    #[test]
    fn test_search_sync_event_construction() {
        let event = SearchSyncEvent {
            action: SearchAction::Upsert,
            index: "products".to_string(),
            document_id: "prod_123".to_string(),
            data: serde_json::json!({"id": "prod_123", "title": "Widget"}),
        };

        assert!(matches!(event.action, SearchAction::Upsert));
        assert_eq!(event.index, "products");
        assert_eq!(event.document_id, "prod_123");
        assert_eq!(event.data["title"], "Widget");
    }

    #[test]
    fn test_search_sync_event_clone() {
        let event = SearchSyncEvent {
            action: SearchAction::Delete,
            index: "users".to_string(),
            document_id: "u_1".to_string(),
            data: Value::Null,
        };
        let cloned = event.clone();
        assert_eq!(cloned.index, "users");
        assert_eq!(cloned.document_id, "u_1");
        assert!(matches!(cloned.action, SearchAction::Delete));
    }

    #[test]
    fn test_search_syncer_channel_capacity() {
        let config = crate::SearchConfig::default();
        let client = SearchClient::new(config);
        let (_syncer, tx) = SearchSyncer::new(client);

        // Channel should have capacity (1024 as defined)
        assert_eq!(tx.capacity(), 1024);
    }
}
