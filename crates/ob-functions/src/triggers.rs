use crate::registry::{DbEvent, FunctionRegistry};
use crate::runtime::WasmRuntime;
use ob_realtime::registry::{ChangeAction, ChangeEvent};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct DbTriggerExecutor {
    registry: Arc<FunctionRegistry>,
    runtime: Arc<WasmRuntime>,
    receiver: mpsc::Receiver<ChangeEvent>,
}

impl DbTriggerExecutor {
    pub fn new(
        registry: Arc<FunctionRegistry>,
        runtime: Arc<WasmRuntime>,
        receiver: mpsc::Receiver<ChangeEvent>,
    ) -> Self {
        Self {
            registry,
            runtime,
            receiver,
        }
    }

    pub async fn run(mut self) {
        tracing::info!("DB trigger executor started");

        while let Some(event) = self.receiver.recv().await {
            let db_event = match event.action {
                ChangeAction::Create => DbEvent::Create,
                ChangeAction::Update => DbEvent::Update,
                ChangeAction::Delete => DbEvent::Delete,
            };

            let matching_fns = self.registry.find_db_triggers(&event.collection, &db_event);

            for fn_name in matching_fns {
                let input = serde_json::json!({
                    "trigger": "database",
                    "collection": event.collection,
                    "action": format!("{:?}", event.action),
                    "document_id": event.document_id,
                    "data": event.data,
                    "timestamp": event.timestamp,
                });

                match self.registry.get_module(&fn_name) {
                    Ok(module) => {
                        match self
                            .runtime
                            .execute(&module, "handle", &input.to_string())
                            .await
                        {
                            Ok(result) => {
                                tracing::info!(
                                    function = %fn_name,
                                    collection = %event.collection,
                                    "DB trigger executed: {}",
                                    &result[..result.len().min(200)]
                                );
                            }
                            Err(e) => {
                                tracing::error!(function = %fn_name, "DB trigger failed: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(function = %fn_name, "DB trigger module not found: {e}");
                    }
                }
            }
        }

        tracing::info!("DB trigger executor stopped");
    }
}
