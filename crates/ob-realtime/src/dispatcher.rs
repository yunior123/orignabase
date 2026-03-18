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
        before_data: None,
        after_data: Some(data.clone()),
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
            document_id: None,
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
            document_id: None,
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

    #[test]
    fn test_change_event_creation_and_fields() {
        let event = ChangeEvent {
            action: ChangeAction::Create,
            collection: "products".to_string(),
            document_id: "products:xyz".to_string(),
            before_data: None,
            after_data: Some(serde_json::json!({"title": "Widget", "price": 9.99})),
            data: serde_json::json!({"title": "Widget", "price": 9.99}),
            timestamp: "2026-03-08T12:00:00Z".to_string(),
        };

        assert_eq!(event.action, ChangeAction::Create);
        assert_eq!(event.collection, "products");
        assert_eq!(event.document_id, "products:xyz");
        assert_eq!(event.data["title"], "Widget");
        assert_eq!(event.timestamp, "2026-03-08T12:00:00Z");
    }

    #[test]
    fn test_change_event_serialization() {
        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "orders".to_string(),
            document_id: "orders:123".to_string(),
            before_data: Some(serde_json::json!({"status": "processing"})),
            after_data: Some(serde_json::json!({"status": "shipped"})),
            data: serde_json::json!({"status": "shipped"}),
            timestamp: "2026-03-08T14:00:00Z".to_string(),
        };

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["action"], "update");
        assert_eq!(json["collection"], "orders");
        assert_eq!(json["document_id"], "orders:123");
        assert_eq!(json["data"]["status"], "shipped");

        // Deserialize back
        let deserialized: ChangeEvent = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.action, ChangeAction::Update);
        assert_eq!(deserialized.collection, "orders");
    }

    #[test]
    fn test_change_action_serialization() {
        // ChangeAction uses rename_all = "lowercase"
        let create_json = serde_json::to_value(ChangeAction::Create).unwrap();
        let update_json = serde_json::to_value(ChangeAction::Update).unwrap();
        let delete_json = serde_json::to_value(ChangeAction::Delete).unwrap();

        assert_eq!(create_json, "create");
        assert_eq!(update_json, "update");
        assert_eq!(delete_json, "delete");

        // Round-trip
        let back: ChangeAction = serde_json::from_value(create_json).unwrap();
        assert_eq!(back, ChangeAction::Create);
    }

    #[test]
    fn test_realtime_message_serialization() {
        let event = ChangeEvent {
            action: ChangeAction::Delete,
            collection: "users".to_string(),
            document_id: "users:u1".to_string(),
            before_data: Some(serde_json::json!({"name": "User"})),
            after_data: None,
            data: serde_json::json!(null),
            timestamp: "2026-03-08T16:00:00Z".to_string(),
        };

        let msg = RealtimeMessage {
            subscription_id: "sub_abc".to_string(),
            event,
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["subscription_id"], "sub_abc");
        assert_eq!(json["event"]["action"], "delete");
        assert_eq!(json["event"]["collection"], "users");
    }

    #[tokio::test]
    async fn test_emit_change_creates_timestamp() {
        let (tx, mut rx) = mpsc::channel(16);

        emit_change(
            &tx,
            ChangeAction::Create,
            "test_col",
            "test_col:1",
            serde_json::json!({"key": "value"}),
        )
        .await;

        let event = rx.recv().await.expect("should receive event");
        assert_eq!(event.action, ChangeAction::Create);
        assert_eq!(event.collection, "test_col");
        assert_eq!(event.document_id, "test_col:1");
        assert_eq!(event.data["key"], "value");
        // timestamp should be a non-empty RFC3339 string
        assert!(!event.timestamp.is_empty());
        assert!(
            event.timestamp.contains('T'),
            "timestamp should be RFC3339 format"
        );
    }
}
