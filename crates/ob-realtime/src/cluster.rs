//! NATS JetStream cluster bridge for multi-node realtime sync.
//!
//! When running multiple orignabase instances, the cluster bridge ensures
//! that change events from any node reach all subscribers across the cluster.
//!
//! Requires the `cluster` feature flag.

#[cfg(feature = "cluster")]
mod inner {
    use crate::registry::{ChangeAction, ChangeEvent};
    use async_nats::jetstream;
    use futures_util::StreamExt;
    use serde::{Deserialize, Serialize};
    use tokio::sync::mpsc;

    /// Serializable envelope for cluster change events.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ClusterEvent {
        pub action: String,
        pub collection: String,
        pub document_id: String,
        pub data: serde_json::Value,
        pub timestamp: String,
        /// Node ID that originated the event (to avoid echo).
        pub origin_node: String,
    }

    impl ClusterEvent {
        pub fn from_change(event: &ChangeEvent, node_id: &str) -> Self {
            Self {
                action: match event.action {
                    ChangeAction::Create => "create".to_string(),
                    ChangeAction::Update => "update".to_string(),
                    ChangeAction::Delete => "delete".to_string(),
                },
                collection: event.collection.clone(),
                document_id: event.document_id.clone(),
                data: event.data.clone(),
                timestamp: event.timestamp.clone(),
                origin_node: node_id.to_string(),
            }
        }

        pub fn to_change_event(&self) -> ChangeEvent {
            ChangeEvent {
                action: match self.action.as_str() {
                    "create" => ChangeAction::Create,
                    "update" => ChangeAction::Update,
                    "delete" => ChangeAction::Delete,
                    _ => ChangeAction::Update,
                },
                collection: self.collection.clone(),
                document_id: self.document_id.clone(),
                data: self.data.clone(),
                timestamp: self.timestamp.clone(),
            }
        }
    }

    /// Configuration for the NATS cluster bridge.
    #[derive(Debug, Clone)]
    pub struct ClusterConfig {
        /// NATS server URL (e.g., "nats://localhost:4222")
        pub nats_url: String,
        /// JetStream stream name for change events
        pub stream_name: String,
        /// Unique node ID for this instance
        pub node_id: String,
    }

    impl Default for ClusterConfig {
        fn default() -> Self {
            Self {
                nats_url: "nats://localhost:4222".to_string(),
                stream_name: "ORIGNABASE_CHANGES".to_string(),
                node_id: uuid::Uuid::new_v4().to_string(),
            }
        }
    }

    /// NATS JetStream bridge that syncs change events across cluster nodes.
    pub struct ClusterBridge {
        config: ClusterConfig,
        /// Receives local change events to publish to NATS.
        local_rx: mpsc::Receiver<ChangeEvent>,
        /// Sends remote change events to the local dispatcher.
        remote_tx: mpsc::Sender<ChangeEvent>,
    }

    impl ClusterBridge {
        /// Create a new cluster bridge.
        ///
        /// Returns the bridge and a sender for local change events.
        /// The `remote_tx` should be the dispatcher's event channel.
        pub fn new(
            config: ClusterConfig,
            remote_tx: mpsc::Sender<ChangeEvent>,
        ) -> (Self, mpsc::Sender<ChangeEvent>) {
            let (local_tx, local_rx) = mpsc::channel(2048);
            (
                Self {
                    config,
                    local_rx,
                    remote_tx,
                },
                local_tx,
            )
        }

        /// Run the cluster bridge. This:
        /// 1. Connects to NATS
        /// 2. Creates/binds a JetStream stream
        /// 3. Publishes local events to `orignabase.changes.{collection}`
        /// 4. Subscribes to all change subjects and feeds remote events locally
        pub async fn run(mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let client = async_nats::connect(&self.config.nats_url).await?;
            let jetstream = jetstream::new(client.clone());

            // Ensure the stream exists
            let stream_config = jetstream::stream::Config {
                name: self.config.stream_name.clone(),
                subjects: vec!["orignabase.changes.>".to_string()],
                max_age: std::time::Duration::from_secs(3600), // 1h retention
                ..Default::default()
            };

            jetstream
                .get_or_create_stream(stream_config)
                .await?;

            tracing::info!(
                node_id = %self.config.node_id,
                nats_url = %self.config.nats_url,
                "Cluster bridge connected to NATS JetStream"
            );

            // Create a durable consumer for this node
            let consumer_name = format!("orignabase-node-{}", &self.config.node_id[..8]);
            let stream = jetstream
                .get_stream(&self.config.stream_name)
                .await?;

            let consumer = stream
                .get_or_create_consumer(
                    &consumer_name,
                    jetstream::consumer::pull::Config {
                        durable_name: Some(consumer_name.clone()),
                        filter_subject: "orignabase.changes.>".to_string(),
                        ..Default::default()
                    },
                )
                .await?;

            let mut messages = consumer.messages().await?;
            let node_id = self.config.node_id.clone();
            let remote_tx = self.remote_tx.clone();

            // Spawn subscriber task
            let subscriber_node_id = node_id.clone();
            let subscriber_handle = tokio::spawn(async move {
                while let Some(Ok(msg)) = messages.next().await {
                    if let Ok(event) =
                        serde_json::from_slice::<ClusterEvent>(&msg.payload)
                    {
                        // Skip events from this node (avoid echo)
                        if event.origin_node == subscriber_node_id {
                            let _ = msg.ack().await;
                            continue;
                        }

                        tracing::debug!(
                            origin = %event.origin_node,
                            collection = %event.collection,
                            "Received cluster change event"
                        );

                        let change = event.to_change_event();
                        if remote_tx.send(change).await.is_err() {
                            tracing::warn!("Failed to forward cluster event to local dispatcher");
                            break;
                        }
                    }
                    let _ = msg.ack().await;
                }
            });

            // Publisher loop: forward local events to NATS
            while let Some(event) = self.local_rx.recv().await {
                let subject = format!("orignabase.changes.{}", event.collection);
                let cluster_event = ClusterEvent::from_change(&event, &node_id);

                match serde_json::to_vec(&cluster_event) {
                    Ok(payload) => {
                        if let Err(e) = jetstream
                            .publish(subject.clone(), payload.into())
                            .await
                        {
                            tracing::error!(subject = %subject, error = %e, "Failed to publish to NATS");
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to serialize cluster event: {e}");
                    }
                }
            }

            subscriber_handle.abort();
            tracing::info!("Cluster bridge stopped");
            Ok(())
        }
    }
}

#[cfg(feature = "cluster")]
pub use inner::*;

/// No-op types when cluster feature is disabled.
#[cfg(not(feature = "cluster"))]
pub mod disabled {
    /// Cluster configuration (no-op without `cluster` feature).
    #[derive(Debug, Clone, Default)]
    pub struct ClusterConfig {
        pub nats_url: String,
        pub stream_name: String,
        pub node_id: String,
    }
}

#[cfg(not(feature = "cluster"))]
pub use disabled::*;
