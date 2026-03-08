use dashmap::DashMap;
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use wasmi::Module;

use crate::runtime::WasmRuntime;

/// Trigger type for when a function should execute.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TriggerType {
    /// HTTP request trigger (maps to a route).
    Http { method: String, path: String },
    /// Database event trigger (fires on collection changes).
    Database { collection: String, event: DbEvent },
    /// Cron schedule trigger.
    Cron { schedule: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DbEvent {
    Create,
    Update,
    Delete,
}

/// Metadata about a registered function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionMeta {
    pub name: String,
    pub triggers: Vec<TriggerType>,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Size of the WASM binary in bytes.
    pub wasm_size: u64,
}

/// A registered function with its compiled module.
pub struct RegisteredFunction {
    pub meta: FunctionMeta,
    pub module: Module,
}

/// Thread-safe registry of all deployed WASM functions.
pub struct FunctionRegistry {
    runtime: Arc<WasmRuntime>,
    functions: DashMap<String, RegisteredFunction>,
}

impl FunctionRegistry {
    pub fn new(runtime: Arc<WasmRuntime>) -> Self {
        Self {
            runtime,
            functions: DashMap::new(),
        }
    }

    /// Register a new function from WASM bytes.
    pub fn register(
        &self,
        name: &str,
        wasm_bytes: &[u8],
        triggers: Vec<TriggerType>,
        description: Option<String>,
    ) -> Result<FunctionMeta> {
        let module = self.runtime.compile(wasm_bytes)?;
        let now = chrono::Utc::now().to_rfc3339();

        let meta = FunctionMeta {
            name: name.to_string(),
            triggers,
            description,
            created_at: now.clone(),
            updated_at: now,
            wasm_size: wasm_bytes.len() as u64,
        };

        self.functions.insert(
            name.to_string(),
            RegisteredFunction {
                meta: meta.clone(),
                module,
            },
        );

        tracing::info!(function = name, "Registered WASM function");
        Ok(meta)
    }

    /// Unregister a function.
    pub fn unregister(&self, name: &str) -> Result<()> {
        self.functions
            .remove(name)
            .ok_or_else(|| Error::NotFound(format!("Function '{name}' not found")))?;
        tracing::info!(function = name, "Unregistered WASM function");
        Ok(())
    }

    /// Get a function's compiled module by name.
    pub fn get_module(&self, name: &str) -> Result<Module> {
        self.functions
            .get(name)
            .map(|f| f.module.clone())
            .ok_or_else(|| Error::NotFound(format!("Function '{name}' not found")))
    }

    /// Get function metadata.
    pub fn get_meta(&self, name: &str) -> Result<FunctionMeta> {
        self.functions
            .get(name)
            .map(|f| f.meta.clone())
            .ok_or_else(|| Error::NotFound(format!("Function '{name}' not found")))
    }

    /// List all registered functions.
    pub fn list(&self) -> Vec<FunctionMeta> {
        self.functions
            .iter()
            .map(|entry| entry.value().meta.clone())
            .collect()
    }

    /// Find functions triggered by a database event.
    pub fn find_db_triggers(&self, collection: &str, event: &DbEvent) -> Vec<String> {
        self.functions
            .iter()
            .filter(|entry| {
                entry.value().meta.triggers.iter().any(|t| {
                    matches!(t, TriggerType::Database { collection: c, event: e } if c == collection && e == event)
                })
            })
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Find functions triggered by an HTTP request.
    pub fn find_http_trigger(&self, method: &str, path: &str) -> Option<String> {
        self.functions
            .iter()
            .find(|entry| {
                entry.value().meta.triggers.iter().any(|t| {
                    matches!(t, TriggerType::Http { method: m, path: p } if m == method && p == path)
                })
            })
            .map(|entry| entry.key().clone())
    }

    /// Find functions with cron triggers.
    pub fn find_cron_triggers(&self) -> Vec<(String, String)> {
        self.functions
            .iter()
            .flat_map(|entry| {
                entry
                    .value()
                    .meta
                    .triggers
                    .iter()
                    .filter_map(|t| match t {
                        TriggerType::Cron { schedule } => {
                            Some((entry.key().clone(), schedule.clone()))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Get the runtime reference.
    pub fn runtime(&self) -> &WasmRuntime {
        &self.runtime
    }

    /// Total number of registered functions.
    pub fn count(&self) -> usize {
        self.functions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::FunctionLimits;

    fn make_registry() -> FunctionRegistry {
        let runtime = Arc::new(WasmRuntime::new(FunctionLimits::default()).unwrap());
        FunctionRegistry::new(runtime)
    }

    #[test]
    fn test_find_db_triggers_empty() {
        let reg = make_registry();
        let triggers = reg.find_db_triggers("products", &DbEvent::Create);
        assert!(triggers.is_empty());
    }

    #[test]
    fn test_find_http_trigger_empty() {
        let reg = make_registry();
        assert!(reg.find_http_trigger("GET", "/api/hello").is_none());
    }

    #[test]
    fn test_list_empty() {
        let reg = make_registry();
        assert_eq!(reg.count(), 0);
        assert!(reg.list().is_empty());
    }

    #[test]
    fn test_unregister_nonexistent() {
        let reg = make_registry();
        assert!(reg.unregister("nope").is_err());
    }

    #[test]
    fn test_get_meta_nonexistent() {
        let reg = make_registry();
        assert!(reg.get_meta("nope").is_err());
    }

    #[test]
    fn test_register_and_find() {
        let reg = make_registry();
        let wasm = wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func (export "alloc") (param i32) (result i32) i32.const 0)
                (func (export "handle") (param i32 i32) (result i64) i64.const 0)
            )"#,
        )
        .unwrap();

        let meta = reg
            .register(
                "on_product_create",
                &wasm,
                vec![TriggerType::Database {
                    collection: "products".into(),
                    event: DbEvent::Create,
                }],
                Some("Test function".into()),
            )
            .unwrap();

        assert_eq!(meta.name, "on_product_create");
        assert_eq!(reg.count(), 1);

        let triggers = reg.find_db_triggers("products", &DbEvent::Create);
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0], "on_product_create");

        // Unregister
        reg.unregister("on_product_create").unwrap();
        assert_eq!(reg.count(), 0);
    }
}
