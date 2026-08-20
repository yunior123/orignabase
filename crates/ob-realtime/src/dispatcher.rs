use crate::registry::{
    ChangeAction, ChangeEvent, RealtimeMessage, Subscription, SubscriptionRegistry,
};
use ob_core::constants::fields as f;
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
            let mut subscribers = self.registry.find_all_for_collection(collection);

            if subscribers.is_empty() {
                continue;
            }

            // Apply ownership-aware filtering to private collections
            let initial_count = subscribers.len();
            filter_by_ownership(&mut subscribers, collection, &event.data);
            if subscribers.len() < initial_count {
                tracing::debug!(
                    collection = %collection,
                    filtered_out = initial_count - subscribers.len(),
                    "Subscribers filtered by ownership check"
                );
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

/// Filter subscribers to only those who own/are authorized for the document.
/// Public collections (products, reviews, categories) are not filtered.
/// Private collections (orders, cart, notifications) are filtered to the owning user(s).
fn filter_by_ownership(
    subscribers: &mut Vec<Subscription>,
    collection: &str,
    data: &serde_json::Value,
) {
    match collection {
        "orders" => {
            // Orders visible to both buyer and seller
            let buyer_id = data.get(f::BUYER_ID).and_then(|v| v.as_str());
            let seller_id = data.get(f::SELLER_ID).and_then(|v| v.as_str());
            subscribers.retain(|sub| {
                match sub.user_id.as_deref() {
                    None => true, // Anonymous/system subscriptions pass through
                    Some(uid) => Some(uid) == buyer_id || Some(uid) == seller_id,
                }
            });
        }
        "cart" | "notifications" | "subscriptions" => {
            // Single-owner collections
            let owner_id = data.get(f::USER_ID).and_then(|v| v.as_str());
            subscribers.retain(|sub| match sub.user_id.as_deref() {
                None => true,
                Some(uid) => Some(uid) == owner_id,
            });
        }
        "return_requests" => {
            // Visible to buyer and seller
            let buyer_id = data.get(f::BUYER_ID).and_then(|v| v.as_str());
            let seller_id = data.get(f::SELLER_ID).and_then(|v| v.as_str());
            subscribers.retain(|sub| match sub.user_id.as_deref() {
                None => true,
                Some(uid) => Some(uid) == buyer_id || Some(uid) == seller_id,
            });
        }
        "chat_messages" | "chat_threads" => {
            // Chat visible to both participants
            let buyer_id = data.get(f::BUYER_ID).and_then(|v| v.as_str());
            let seller_id = data.get(f::SELLER_ID).and_then(|v| v.as_str());
            subscribers.retain(|sub| match sub.user_id.as_deref() {
                None => true,
                Some(uid) => Some(uid) == buyer_id || Some(uid) == seller_id,
            });
        }
        "seller_profiles" | "warehouses" => {
            // Seller-owned — check sellerId or parent_id for subcollections
            let seller_id = data
                .get(f::SELLER_ID)
                .and_then(|v| v.as_str())
                .or_else(|| data.get(f::PARENT_ID).and_then(|v| v.as_str()));
            subscribers.retain(|sub| match sub.user_id.as_deref() {
                None => true,
                Some(uid) => Some(uid) == seller_id,
            });
        }
        // Public collections: products, reviews, users, categories, etc. — no filtering
        _ => {}
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
    use ob_database::fields;

    #[tokio::test]
    async fn test_dispatcher_sends_to_subscribers() {
        let registry = SubscriptionRegistry::new();

        // Create a subscriber
        let (sub_tx, mut sub_rx) = mpsc::channel(16);
        let sub = Subscription {
            id: "sub1".to_string(),
            collection: Arc::from("products"),
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
            collection: Arc::from("products"),
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

    #[tokio::test]
    async fn test_ownership_filtering_orders_buyer_sees_own() {
        let registry = SubscriptionRegistry::new();

        // Buyer sub
        let (sub_tx, mut sub_rx) = mpsc::channel(16);
        let sub = Subscription {
            id: "sub_buyer".to_string(),
            collection: Arc::from("orders"),
            filter_hash: 0,
            document_id: None,
            user_id: Some("buyer_123".to_string()),
            sender: sub_tx,
        };
        registry.subscribe("conn_buyer", sub);

        let (dispatcher, event_tx) = ChangeDispatcher::new(registry);
        let handle = tokio::spawn(dispatcher.run());

        // Emit an order change for this buyer
        emit_change(
            &event_tx,
            ChangeAction::Create,
            "orders",
            "orders:ord_1",
            serde_json::json!({
                "buyerId": "buyer_123",
                "sellerId": "seller_456",
                "status": "pending"
            }),
        )
        .await;

        // Buyer should receive it
        let msg = tokio::time::timeout(std::time::Duration::from_secs(1), sub_rx.recv())
            .await
            .expect("timeout")
            .expect("channel closed");

        assert_eq!(msg.subscription_id, "sub_buyer");

        drop(event_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_ownership_filtering_orders_buyer_blocked_from_other() {
        let registry = SubscriptionRegistry::new();

        // Buyer A sub
        let (sub_tx_a, mut sub_rx_a) = mpsc::channel(16);
        let sub_a = Subscription {
            id: "sub_buyer_a".to_string(),
            collection: Arc::from("orders"),
            filter_hash: 0,
            document_id: None,
            user_id: Some("buyer_a".to_string()),
            sender: sub_tx_a,
        };
        registry.subscribe("conn_buyer_a", sub_a);

        let (dispatcher, event_tx) = ChangeDispatcher::new(registry);
        let handle = tokio::spawn(dispatcher.run());

        // Emit an order change for BUYER B (not A)
        emit_change(
            &event_tx,
            ChangeAction::Create,
            "orders",
            "orders:ord_1",
            serde_json::json!({
                "buyerId": "buyer_b",
                "sellerId": "seller_456",
                "status": "pending"
            }),
        )
        .await;

        // Buyer A should NOT receive it
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(100), sub_rx_a.recv()).await;
        assert!(
            result.is_err(),
            "Buyer A should not receive Buyer B's orders"
        );

        drop(event_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_ownership_filtering_orders_seller_sees_own() {
        let registry = SubscriptionRegistry::new();

        // Seller sub
        let (sub_tx, mut sub_rx) = mpsc::channel(16);
        let sub = Subscription {
            id: "sub_seller".to_string(),
            collection: Arc::from("orders"),
            filter_hash: 0,
            document_id: None,
            user_id: Some("seller_123".to_string()),
            sender: sub_tx,
        };
        registry.subscribe("conn_seller", sub);

        let (dispatcher, event_tx) = ChangeDispatcher::new(registry);
        let handle = tokio::spawn(dispatcher.run());

        // Emit an order change where this seller is the seller
        emit_change(
            &event_tx,
            ChangeAction::Update,
            "orders",
            "orders:ord_1",
            serde_json::json!({
                "buyerId": "buyer_456",
                "sellerId": "seller_123",
                "status": "shipped"
            }),
        )
        .await;

        // Seller should receive it
        let msg = tokio::time::timeout(std::time::Duration::from_secs(1), sub_rx.recv())
            .await
            .expect("timeout")
            .expect("channel closed");

        assert_eq!(msg.subscription_id, "sub_seller");

        drop(event_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_ownership_filtering_cart_single_owner() {
        let registry = SubscriptionRegistry::new();

        // User A cart sub
        let (sub_tx_a, mut sub_rx_a) = mpsc::channel(16);
        let sub_a = Subscription {
            id: "sub_cart_a".to_string(),
            collection: Arc::from("cart"),
            filter_hash: 0,
            document_id: None,
            user_id: Some("user_a".to_string()),
            sender: sub_tx_a,
        };
        registry.subscribe("conn_a", sub_a);

        // User B cart sub
        let (sub_tx_b, mut sub_rx_b) = mpsc::channel(16);
        let sub_b = Subscription {
            id: "sub_cart_b".to_string(),
            collection: Arc::from("cart"),
            filter_hash: 0,
            document_id: None,
            user_id: Some("user_b".to_string()),
            sender: sub_tx_b,
        };
        registry.subscribe("conn_b", sub_b);

        let (dispatcher, event_tx) = ChangeDispatcher::new(registry);
        let handle = tokio::spawn(dispatcher.run());

        // Emit a cart change for USER A
        emit_change(
            &event_tx,
            ChangeAction::Update,
            "cart",
            "cart:cart_a",
            serde_json::json!({
                "userId": "user_a",
                "items": []
            }),
        )
        .await;

        // User A should receive it
        let msg_a = tokio::time::timeout(std::time::Duration::from_secs(1), sub_rx_a.recv())
            .await
            .expect("timeout")
            .expect("channel closed");
        assert_eq!(msg_a.subscription_id, "sub_cart_a");

        // User B should NOT receive it
        let result_b =
            tokio::time::timeout(std::time::Duration::from_millis(100), sub_rx_b.recv()).await;
        assert!(
            result_b.is_err(),
            "User B should not receive User A's cart updates"
        );

        drop(event_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_ownership_filtering_public_collection_no_filtering() {
        let registry = SubscriptionRegistry::new();

        // Two subscribers with different user IDs, both subscribed to products
        let (sub_tx_1, mut sub_rx_1) = mpsc::channel(16);
        let sub_1 = Subscription {
            id: "sub_prod_1".to_string(),
            collection: Arc::from("products"),
            filter_hash: 0,
            document_id: None,
            user_id: Some("user_1".to_string()),
            sender: sub_tx_1,
        };
        registry.subscribe("conn_1", sub_1);

        let (sub_tx_2, mut sub_rx_2) = mpsc::channel(16);
        let sub_2 = Subscription {
            id: "sub_prod_2".to_string(),
            collection: Arc::from("products"),
            filter_hash: 0,
            document_id: None,
            user_id: Some("user_2".to_string()),
            sender: sub_tx_2,
        };
        registry.subscribe("conn_2", sub_2);

        let (dispatcher, event_tx) = ChangeDispatcher::new(registry);
        let handle = tokio::spawn(dispatcher.run());

        // Emit a public product change (has no ownership fields)
        emit_change(
            &event_tx,
            ChangeAction::Update,
            "products",
            "products:prod_xyz",
            serde_json::json!({
                "title": "New Widget",
                "price": 4999
            }),
        )
        .await;

        // Both subscribers should receive it (public collection)
        let msg_1 = tokio::time::timeout(std::time::Duration::from_secs(1), sub_rx_1.recv())
            .await
            .expect("timeout")
            .expect("channel closed");
        assert_eq!(msg_1.subscription_id, "sub_prod_1");

        let msg_2 = tokio::time::timeout(std::time::Duration::from_secs(1), sub_rx_2.recv())
            .await
            .expect("timeout")
            .expect("channel closed");
        assert_eq!(msg_2.subscription_id, "sub_prod_2");

        drop(event_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_ownership_filtering_anonymous_passes_through() {
        let registry = SubscriptionRegistry::new();

        // Anonymous subscriber (user_id = None)
        let (sub_tx, mut sub_rx) = mpsc::channel(16);
        let sub = Subscription {
            id: "sub_anon".to_string(),
            collection: Arc::from("orders"),
            filter_hash: 0,
            document_id: None,
            user_id: None,
            sender: sub_tx,
        };
        registry.subscribe("conn_anon", sub);

        let (dispatcher, event_tx) = ChangeDispatcher::new(registry);
        let handle = tokio::spawn(dispatcher.run());

        // Emit an order change (would normally be filtered)
        emit_change(
            &event_tx,
            ChangeAction::Create,
            "orders",
            "orders:ord_1",
            serde_json::json!({
                "buyerId": "buyer_xyz",
                "sellerId": "seller_abc",
                "status": "pending"
            }),
        )
        .await;

        // Anonymous sub should still receive it (system/admin)
        let msg = tokio::time::timeout(std::time::Duration::from_secs(1), sub_rx.recv())
            .await
            .expect("timeout")
            .expect("channel closed");

        assert_eq!(msg.subscription_id, "sub_anon");

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
        assert_eq!(json["data"][fields::STATUS], "shipped");

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
