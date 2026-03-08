use crate::registry::{ChangeAction, ChangeEvent, RealtimeMessage, SubscriptionRegistry};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Dispatches database change events to affected WebSocket subscriptions.
pub struct ChangeDispatcher {
    registry: Arc<SubscriptionRegistry>,
    receiver: mpsc::Receiver<ChangeEvent>,
}

impl ChangeDispatcher {
    /// Create a new dispatcher with a channel for receiving change events.
    pub fn new(registry: Arc<SubscriptionRegistry>) -> (Self, mpsc::Sender<ChangeEvent>) {
        let (tx, rx) = mpsc::channel(1024);
        (
            Self {
                registry,
                receiver: rx,
            },
            tx,
        )
    }

    /// Run the dispatcher loop. Listens for change events and pushes to affected subscribers.
    pub async fn run(mut self) {
        tracing::info!("Realtime change dispatcher started");

        while let Some(event) = self.receiver.recv().await {
            let collection = &event.collection;
            let subscribers = self.registry.find_all_for_collection(collection);

            if subscribers.is_empty() {
                continue;
            }

            tracing::debug!(
                collection = %collection,
                action = ?event.action,
                doc_id = %event.document_id,
                subscribers = subscribers.len(),
                "Dispatching change event"
            );

            for sub in subscribers {
                let msg = RealtimeMessage {
                    subscription_id: sub.id.clone(),
                    event: event.clone(),
                };

                if sub.sender.try_send(msg).is_err() {
                    tracing::warn!(
                        sub_id = %sub.id,
                        "Failed to send to subscriber (channel full or closed)"
                    );
                }
            }
        }

        tracing::info!("Realtime change dispatcher stopped");
    }
}

/// Helper to emit a change event to the dispatcher.
pub async fn emit_change(
    sender: &mpsc::Sender<ChangeEvent>,
    action: ChangeAction,
    collection: &str,
    document_id: &str,
    data: serde_json::Value,
) {
    let event = ChangeEvent {
        action,
        collection: collection.to_string(),
        document_id: document_id.to_string(),
        data,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    if let Err(e) = sender.send(event).await {
        tracing::error!("Failed to emit change event: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Subscription;

    #[tokio::test]
    async fn test_dispatcher_sends_to_subscribers() {
        let registry = SubscriptionRegistry::new();

        // Create a subscriber
        let (sub_tx, mut sub_rx) = mpsc::channel(16);
        let sub = Subscription {
            id: "sub1".to_string(),
            collection: "products".to_string(),
            filter_hash: 0,
            user_id: None,
            sender: sub_tx,
        };
        registry.subscribe("conn1", sub);

        let (dispatcher, event_tx) = ChangeDispatcher::new(registry);

        // Spawn dispatcher
        let handle = tokio::spawn(dispatcher.run());

        // Emit a change
        emit_change(
            &event_tx,
            ChangeAction::Create,
            "products",
            "products:abc",
            serde_json::json!({"title": "Widget"}),
        )
        .await;

        // Subscriber should receive it
        let msg = tokio::time::timeout(std::time::Duration::from_secs(1), sub_rx.recv())
            .await
            .expect("timeout")
            .expect("channel closed");

        assert_eq!(msg.subscription_id, "sub1");
        assert_eq!(msg.event.action, ChangeAction::Create);
        assert_eq!(msg.event.collection, "products");

        // Cleanup
        drop(event_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_dispatcher_ignores_unrelated_collections() {
        let registry = SubscriptionRegistry::new();

        let (sub_tx, mut sub_rx) = mpsc::channel(16);
        let sub = Subscription {
            id: "sub1".to_string(),
            collection: "products".to_string(),
            filter_hash: 0,
            user_id: None,
            sender: sub_tx,
        };
        registry.subscribe("conn1", sub);

        let (dispatcher, event_tx) = ChangeDispatcher::new(registry);
        let handle = tokio::spawn(dispatcher.run());

        // Emit a change to a DIFFERENT collection
        emit_change(
            &event_tx,
            ChangeAction::Update,
            "orders",
            "orders:123",
            serde_json::json!({}),
        )
        .await;

        // Subscriber should NOT receive it
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(100), sub_rx.recv()).await;
        assert!(
            result.is_err(),
            "Should have timed out — no message expected"
        );

        drop(event_tx);
        let _ = handle.await;
    }
}
