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

    fn make_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func (export "alloc") (param i32) (result i32) i32.const 0)
                (func (export "handle") (param i32 i32) (result i64) i64.const 0)
            )"#,
        )
        .unwrap()
    }

    #[test]
    fn test_find_http_trigger_match() {
        let reg = make_registry();
        let wasm = make_wasm();
        reg.register(
            "api_hello",
            &wasm,
            vec![TriggerType::Http {
                method: "GET".into(),
                path: "/api/hello".into(),
            }],
            None,
        )
        .unwrap();

        let found = reg.find_http_trigger("GET", "/api/hello");
        assert_eq!(found, Some("api_hello".to_string()));

        // Non-matching method
        assert!(reg.find_http_trigger("POST", "/api/hello").is_none());
        // Non-matching path
        assert!(reg.find_http_trigger("GET", "/api/bye").is_none());
    }

    #[test]
    fn test_find_cron_triggers_empty() {
        let reg = make_registry();
        assert!(reg.find_cron_triggers().is_empty());
    }

    #[test]
    fn test_find_cron_triggers_match() {
        let reg = make_registry();
        let wasm = make_wasm();
        reg.register(
            "nightly_job",
            &wasm,
            vec![TriggerType::Cron {
                schedule: "0 0 * * *".into(),
            }],
            Some("Runs at midnight".into()),
        )
        .unwrap();

        let crons = reg.find_cron_triggers();
        assert_eq!(crons.len(), 1);
        assert_eq!(crons[0].0, "nightly_job");
        assert_eq!(crons[0].1, "0 0 * * *");
    }

    #[test]
    fn test_get_module_existing() {
        let reg = make_registry();
        let wasm = make_wasm();
        reg.register("my_func", &wasm, vec![], None).unwrap();

        let module = reg.get_module("my_func");
        assert!(module.is_ok());
    }

    #[test]
    fn test_get_module_nonexistent() {
        let reg = make_registry();
        assert!(reg.get_module("nope").is_err());
    }

    #[test]
    fn test_get_meta_existing() {
        let reg = make_registry();
        let wasm = make_wasm();
        reg.register(
            "my_func",
            &wasm,
            vec![TriggerType::Cron {
                schedule: "*/5 * * * *".into(),
            }],
            Some("A test function".into()),
        )
        .unwrap();

        let meta = reg.get_meta("my_func").unwrap();
        assert_eq!(meta.name, "my_func");
        assert_eq!(meta.description, Some("A test function".to_string()));
        assert_eq!(meta.triggers.len(), 1);
        assert_eq!(meta.wasm_size, wasm.len() as u64);
        assert!(!meta.created_at.is_empty());
        assert!(!meta.updated_at.is_empty());
    }

    #[test]
    fn test_register_multiple_triggers() {
        let reg = make_registry();
        let wasm = make_wasm();
        let triggers = vec![
            TriggerType::Http {
                method: "POST".into(),
                path: "/webhook".into(),
            },
            TriggerType::Database {
                collection: "orders".into(),
                event: DbEvent::Create,
            },
            TriggerType::Cron {
                schedule: "0 */6 * * *".into(),
            },
        ];

        let meta = reg
            .register("multi_trigger", &wasm, triggers, None)
            .unwrap();
        assert_eq!(meta.triggers.len(), 3);

        // Should be found by each trigger type
        assert_eq!(
            reg.find_http_trigger("POST", "/webhook"),
            Some("multi_trigger".to_string())
        );
        assert_eq!(
            reg.find_db_triggers("orders", &DbEvent::Create),
            vec!["multi_trigger".to_string()]
        );
        let crons = reg.find_cron_triggers();
        assert_eq!(crons.len(), 1);
        assert_eq!(crons[0].0, "multi_trigger");
    }

    #[test]
    fn test_runtime_accessor() {
        let reg = make_registry();
        // Just verify runtime() returns without panicking
        let _runtime = reg.runtime();
    }

    #[test]
    fn test_function_meta_serde() {
        let meta = FunctionMeta {
            name: "test_fn".to_string(),
            triggers: vec![
                TriggerType::Http {
                    method: "GET".into(),
                    path: "/test".into(),
                },
                TriggerType::Database {
                    collection: "items".into(),
                    event: DbEvent::Update,
                },
                TriggerType::Cron {
                    schedule: "0 0 * * *".into(),
                },
            ],
            description: Some("A test function".into()),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            wasm_size: 1234,
        };

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: FunctionMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test_fn");
        assert_eq!(deserialized.triggers.len(), 3);
        assert_eq!(deserialized.description, Some("A test function".to_string()));
        assert_eq!(deserialized.wasm_size, 1234);
    }

    #[test]
    fn test_trigger_type_serde() {
        let http = TriggerType::Http {
            method: "POST".into(),
            path: "/api".into(),
        };
        let json = serde_json::to_string(&http).unwrap();
        let back: TriggerType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, http);

        let db = TriggerType::Database {
            collection: "users".into(),
            event: DbEvent::Delete,
        };
        let json = serde_json::to_string(&db).unwrap();
        let back: TriggerType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, db);

        let cron = TriggerType::Cron {
            schedule: "*/10 * * * *".into(),
        };
        let json = serde_json::to_string(&cron).unwrap();
        let back: TriggerType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cron);
    }

    #[test]
    fn test_db_event_serde() {
        for event in [DbEvent::Create, DbEvent::Update, DbEvent::Delete] {
            let json = serde_json::to_string(&event).unwrap();
            let back: DbEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back, event);
        }
    }

    #[test]
    fn test_list_with_registered_functions() {
        let reg = make_registry();
        let wasm = make_wasm();
        reg.register("fn_a", &wasm, vec![], None).unwrap();
        reg.register("fn_b", &wasm, vec![], Some("B".into())).unwrap();

        let list = reg.list();
        assert_eq!(list.len(), 2);
        let names: Vec<String> = list.iter().map(|m| m.name.clone()).collect();
        assert!(names.contains(&"fn_a".to_string()));
        assert!(names.contains(&"fn_b".to_string()));
    }
}
