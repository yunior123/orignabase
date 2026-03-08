use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::mpsc;

/// A unique subscription identifier.
pub type SubscriptionId = String;

/// A message sent to a WebSocket client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealtimeMessage {
    pub subscription_id: SubscriptionId,
    pub event: ChangeEvent,
}

/// A change event from the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeEvent {
    pub action: ChangeAction,
    pub collection: String,
    pub document_id: String,
    pub data: serde_json::Value,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeAction {
    Create,
    Update,
    Delete,
}

/// A subscription entry tracking what a client is listening to.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub id: SubscriptionId,
    pub collection: String,
    pub filter_hash: u64,
    pub user_id: Option<String>,
    pub sender: mpsc::Sender<RealtimeMessage>,
}

/// Thread-safe registry of all active subscriptions.
///
/// Maps: `(collection, filter_hash)` → set of subscription IDs
/// Maps: `subscription_id` → Subscription
pub struct SubscriptionRegistry {
    /// subscription_id → Subscription
    subscriptions: DashMap<SubscriptionId, Subscription>,
    /// (collection, filter_hash) → set of subscription IDs
    dependency_map: DashMap<(String, u64), HashSet<SubscriptionId>>,
    /// connection_id → set of subscription IDs (for cleanup on disconnect)
    connections: DashMap<String, HashSet<SubscriptionId>>,
}

impl SubscriptionRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            subscriptions: DashMap::new(),
            dependency_map: DashMap::new(),
            connections: DashMap::new(),
        })
    }

    /// Register a new subscription.
    pub fn subscribe(&self, connection_id: &str, subscription: Subscription) {
        let sub_id = subscription.id.clone();
        let key = (subscription.collection.clone(), subscription.filter_hash);

        // Add to dependency map
        self.dependency_map
            .entry(key)
            .or_default()
            .insert(sub_id.clone());

        // Track by connection for cleanup
        self.connections
            .entry(connection_id.to_string())
            .or_default()
            .insert(sub_id.clone());

        // Store subscription
        self.subscriptions.insert(sub_id, subscription);
    }

    /// Unsubscribe a specific subscription.
    pub fn unsubscribe(&self, subscription_id: &str) {
        if let Some((_, sub)) = self.subscriptions.remove(subscription_id) {
            let key = (sub.collection, sub.filter_hash);
            if let Some(mut set) = self.dependency_map.get_mut(&key) {
                set.remove(subscription_id);
                if set.is_empty() {
                    drop(set);
                    self.dependency_map.remove(&key);
                }
            }
        }
    }

    /// Remove all subscriptions for a connection (on disconnect).
    pub fn disconnect(&self, connection_id: &str) {
        if let Some((_, sub_ids)) = self.connections.remove(connection_id) {
            for sub_id in sub_ids {
                self.unsubscribe(&sub_id);
            }
        }
    }

    /// Find all subscriptions affected by a change to a collection.
    pub fn find_affected(&self, collection: &str, filter_hash: u64) -> Vec<Subscription> {
        let key = (collection.to_string(), filter_hash);
        if let Some(sub_ids) = self.dependency_map.get(&key) {
            sub_ids
                .iter()
                .filter_map(|id| self.subscriptions.get(id).map(|s| s.clone()))
                .collect()
        } else {
            vec![]
        }
    }

    /// Find ALL subscriptions for a collection (regardless of filter).
    /// Used when we can't determine the specific filter hash of a change.
    pub fn find_all_for_collection(&self, collection: &str) -> Vec<Subscription> {
        self.subscriptions
            .iter()
            .filter(|entry| entry.value().collection == collection)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get total active subscription count.
    pub fn subscription_count(&self) -> usize {
        self.subscriptions.len()
    }

    /// Get total active connection count.
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }
}

impl Default for SubscriptionRegistry {
    fn default() -> Self {
        Self {
            subscriptions: DashMap::new(),
            dependency_map: DashMap::new(),
            connections: DashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sub(
        id: &str,
        collection: &str,
        filter_hash: u64,
    ) -> (Subscription, mpsc::Receiver<RealtimeMessage>) {
        let (tx, rx) = mpsc::channel(16);
        let sub = Subscription {
            id: id.to_string(),
            collection: collection.to_string(),
            filter_hash,
            user_id: None,
            sender: tx,
        };
        (sub, rx)
    }

    #[test]
    fn test_subscribe_and_find() {
        let registry = SubscriptionRegistry::new();
        let (sub, _rx) = make_sub("sub1", "products", 123);

        registry.subscribe("conn1", sub);

        assert_eq!(registry.subscription_count(), 1);
        assert_eq!(registry.connection_count(), 1);

        let affected = registry.find_affected("products", 123);
        assert_eq!(affected.len(), 1);
        assert_eq!(affected[0].id, "sub1");
    }

    #[test]
    fn test_unsubscribe() {
        let registry = SubscriptionRegistry::new();
        let (sub, _rx) = make_sub("sub1", "products", 123);
        registry.subscribe("conn1", sub);

        registry.unsubscribe("sub1");
        assert_eq!(registry.subscription_count(), 0);
        assert_eq!(registry.find_affected("products", 123).len(), 0);
    }

    #[test]
    fn test_disconnect_cleans_all() {
        let registry = SubscriptionRegistry::new();
        let (sub1, _rx1) = make_sub("sub1", "products", 123);
        let (sub2, _rx2) = make_sub("sub2", "orders", 456);
        registry.subscribe("conn1", sub1);
        registry.subscribe("conn1", sub2);

        assert_eq!(registry.subscription_count(), 2);

        registry.disconnect("conn1");
        assert_eq!(registry.subscription_count(), 0);
        assert_eq!(registry.connection_count(), 0);
    }

    #[test]
    fn test_find_all_for_collection() {
        let registry = SubscriptionRegistry::new();
        let (sub1, _rx1) = make_sub("sub1", "products", 100);
        let (sub2, _rx2) = make_sub("sub2", "products", 200);
        let (sub3, _rx3) = make_sub("sub3", "orders", 300);
        registry.subscribe("conn1", sub1);
        registry.subscribe("conn2", sub2);
        registry.subscribe("conn3", sub3);

        let product_subs = registry.find_all_for_collection("products");
        assert_eq!(product_subs.len(), 2);
    }

    #[test]
    fn test_multiple_connections() {
        let registry = SubscriptionRegistry::new();
        let (sub1, _rx1) = make_sub("sub1", "products", 123);
        let (sub2, _rx2) = make_sub("sub2", "products", 123);
        registry.subscribe("conn1", sub1);
        registry.subscribe("conn2", sub2);

        assert_eq!(registry.connection_count(), 2);

        // Both should be found for same filter
        let affected = registry.find_affected("products", 123);
        assert_eq!(affected.len(), 2);

        // Disconnect one
        registry.disconnect("conn1");
        assert_eq!(registry.subscription_count(), 1);
        assert_eq!(registry.find_affected("products", 123).len(), 1);
    }
}
