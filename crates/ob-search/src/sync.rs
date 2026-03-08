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
