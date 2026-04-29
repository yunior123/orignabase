use async_graphql::{Context, Object, Result as GqlResult};
use ob_auth::AuthContext;
use ob_database::DatabaseClient;
use ob_realtime::registry::{ChangeAction, ChangeEvent};
use ob_search::SearchClient;
use ob_security::{RuleEngine, SecurityContext};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

/// If `data` is a JSON-encoded string, parse it into a Value.
/// This handles the case where GraphQL mutations pass data as a string
/// (e.g., from Flutter SDK's double-encoding: `jsonEncode(jsonEncode(data))`).
fn normalize_data(data: Value) -> Value {
    if let Value::String(s) = &data {
        serde_json::from_str(s).unwrap_or(data)
    } else {
        data
    }
}

fn config_field<'a>(config: &'a Value, field: &str) -> Option<&'a Value> {
    config
        .get(field)
        .or_else(|| config.get("data").and_then(|data| data.get(field)))
}

pub struct QueryRoot;

#[allow(clippy::too_many_arguments)]
#[Object]
impl QueryRoot {
    /// Get a single document by collection and ID.
    async fn get(&self, ctx: &Context<'_>, collection: String, id: String) -> GqlResult<Value> {
        let db = ctx.data::<DatabaseClient>()?;
        let rules = ctx.data::<Arc<RuleEngine>>()?;

        // Fetch document FIRST so RLS (isOwner) can evaluate against it
        let doc = db.get_document(&collection, &id).await.map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })?;

        let auth = ctx
            .data_opt::<AuthContext>()
            .cloned()
            .unwrap_or_else(AuthContext::anonymous);

        let sec_ctx = SecurityContext {
            user_id: if auth.authenticated {
                Some(auth.user_id.clone())
            } else {
                None
            },
            roles: auth.roles.clone(),
            authenticated: auth.authenticated,
            resource: Some(doc.clone()),
            incoming: None,
        };

        if !rules.check(&collection, "read", &sec_ctx).map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })? {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        Ok(doc)
    }

    /// List documents in a collection with optional filters and cursor pagination.
    async fn list(
        &self,
        ctx: &Context<'_>,
        collection: String,
        #[graphql(default)] filters: Option<Value>,
        order_by: Option<String>,
        #[graphql(default = false)] descending: bool,
        #[graphql(default)] limit: Option<i32>,
        #[graphql(default)] offset: Option<i32>,
        #[graphql(default, desc = "Cursor-based pagination: document ID to start after")]
        start_after: Option<String>,
        #[graphql(default, desc = "Field projection: list of fields to return")] fields: Option<
            Vec<String>,
        >,
    ) -> GqlResult<Vec<Value>> {
        let db = ctx.data::<DatabaseClient>()?;
        let rules = ctx.data::<Arc<RuleEngine>>()?;

        let auth = ctx
            .data_opt::<AuthContext>()
            .cloned()
            .unwrap_or_else(AuthContext::anonymous);

        let sec_ctx = SecurityContext {
            user_id: if auth.authenticated {
                Some(auth.user_id.clone())
            } else {
                None
            },
            roles: auth.roles.clone(),
            authenticated: auth.authenticated,
            resource: None,
            incoming: None,
        };

        if !rules.check(&collection, "read", &sec_ctx).map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })? {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        // Normalize filters: if received as a JSON string, parse it into an object
        let filters = filters.map(normalize_data);

        let field_refs: Option<Vec<&str>> = fields
            .as_ref()
            .map(|f| f.iter().map(|s| s.as_str()).collect());

        // When cursor pagination is active, fetch limit+1 so clients can detect hasMore
        let effective_limit = limit.map(|n| {
            let clamped = n.clamp(1, 10_000) as usize;
            if start_after.is_some() {
                clamped + 1
            } else {
                clamped
            }
        });

        let query = ob_database::query::QueryTranslator::build_select_ext(
            &collection,
            filters.as_ref(),
            order_by.as_deref(),
            descending,
            effective_limit,
            offset.map(|n| n.max(0) as usize),
            field_refs.as_deref(),
            start_after.as_deref(),
        );

        let results = db.query_raw(&query).await.map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })?;

        Ok(results)
    }

    /// Get a remote config value by key.
    async fn config(&self, ctx: &Context<'_>, key: String) -> GqlResult<Value> {
        let db = ctx.data::<DatabaseClient>()?;

        let config = db.get_document("_config", &key).await.ok();

        let value = config
            .as_ref()
            .and_then(|config| config_field(config, "value"))
            .cloned()
            .unwrap_or(Value::Null);

        Ok(value)
    }

    /// Get all remote config key-value pairs.
    async fn config_all(&self, ctx: &Context<'_>) -> GqlResult<Value> {
        let db = ctx.data::<DatabaseClient>()?;

        let configs = db
            .query_raw("SELECT * FROM _config LIMIT 100")
            .await
            .map_err(|e| {
                tracing::error!("DB error: {e}");
                async_graphql::Error::new("Internal server error")
            })?;

        let map: serde_json::Map<String, Value> = configs
            .iter()
            .filter_map(|c| {
                let key = config_field(c, "key")?.as_str()?;
                let value = config_field(c, "value")?;
                Some((key.to_string(), value.clone()))
            })
            .collect();

        Ok(Value::Object(map))
    }

    /// Vector similarity search using SurrealDB's native vector functions.
    ///
    /// Searches for documents where `vector_field` is most similar to `embedding`
    /// using cosine similarity. Returns results ordered by similarity score.
    async fn vector_search(
        &self,
        ctx: &Context<'_>,
        collection: String,
        vector_field: String,
        embedding: Vec<f32>,
        #[graphql(default = 10)] top_k: Option<i32>,
        #[graphql(default)] threshold: Option<f64>,
    ) -> GqlResult<Vec<Value>> {
        let db = ctx.data::<DatabaseClient>()?;
        let rules = ctx.data::<Arc<RuleEngine>>()?;

        let auth = ctx
            .data_opt::<AuthContext>()
            .cloned()
            .unwrap_or_else(AuthContext::anonymous);

        let sec_ctx = SecurityContext {
            user_id: if auth.authenticated {
                Some(auth.user_id.clone())
            } else {
                None
            },
            roles: auth.roles.clone(),
            authenticated: auth.authenticated,
            resource: None,
            incoming: None,
        };

        if !rules.check(&collection, "read", &sec_ctx).map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })? {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        let top_k = top_k.map(|n| n.clamp(1, 10_000) as usize).unwrap_or(10);

        let results = db
            .vector_search(&collection, &vector_field, embedding, top_k, threshold)
            .await
            .map_err(|e| {
                tracing::error!("DB error: {e}");
                async_graphql::Error::new("Internal server error")
            })?;

        Ok(results)
    }

    /// Full-text search across a collection via Meilisearch.
    async fn search(
        &self,
        ctx: &Context<'_>,
        index: String,
        query: String,
        #[graphql(default)] limit: Option<i32>,
        #[graphql(default)] offset: Option<i32>,
        #[graphql(default)] filter: Option<String>,
    ) -> GqlResult<Value> {
        let search = ctx.data::<SearchClient>()?;
        let rules = ctx.data::<Arc<RuleEngine>>()?;

        // Enforce read access on the search index/collection
        let auth = ctx
            .data_opt::<AuthContext>()
            .cloned()
            .unwrap_or_else(AuthContext::anonymous);

        let sec_ctx = SecurityContext {
            user_id: if auth.authenticated {
                Some(auth.user_id.clone())
            } else {
                None
            },
            roles: auth.roles.clone(),
            authenticated: auth.authenticated,
            resource: None,
            incoming: None,
        };

        if !rules.check(&index, "read", &sec_ctx).map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })? {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        let result = search
            .search(
                &index,
                &query,
                limit.map(|n| n.clamp(1, 1000) as usize),
                offset.map(|n| n.max(0) as usize),
                filter.as_deref(),
            )
            .await
            .map_err(|e| {
                tracing::error!("DB error: {e}");
                async_graphql::Error::new("Internal server error")
            })?;

        Ok(serde_json::to_value(result).map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })?)
    }
}

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    /// Create a new document in a collection.
    async fn create(&self, ctx: &Context<'_>, collection: String, data: Value) -> GqlResult<Value> {
        let data = normalize_data(data);
        let db = ctx.data::<DatabaseClient>()?;
        let rules = ctx.data::<Arc<RuleEngine>>()?;

        let auth = ctx
            .data_opt::<AuthContext>()
            .cloned()
            .unwrap_or_else(AuthContext::anonymous);

        let sec_ctx = SecurityContext {
            user_id: if auth.authenticated {
                Some(auth.user_id.clone())
            } else {
                None
            },
            roles: auth.roles.clone(),
            authenticated: auth.authenticated,
            resource: None,
            incoming: Some(data.clone()),
        };

        if !rules.check(&collection, "create", &sec_ctx).map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })? {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        let doc = db.create_document(&collection, data).await.map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })?;

        // Emit realtime change event
        if let Ok(tx) = ctx.data::<mpsc::Sender<ChangeEvent>>() {
            let doc_id = doc.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
            if let Err(e) = tx
                .send(ChangeEvent {
                    action: ChangeAction::Create,
                    collection: collection.clone(),
                    document_id: doc_id.to_string(),
                    before_data: None,
                    after_data: Some(doc.clone()),
                    data: doc.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                })
                .await
            {
                tracing::warn!("Realtime event dropped: {e}");
            }
        }

        Ok(doc)
    }

    /// Update a document by collection and ID.
    ///
    /// Auto-detects FieldValue markers in data and routes to
    /// `update_with_field_values` when present.
    async fn update(
        &self,
        ctx: &Context<'_>,
        collection: String,
        id: String,
        data: Value,
    ) -> GqlResult<Value> {
        let data = normalize_data(data);
        let db = ctx.data::<DatabaseClient>()?;
        let rules = ctx.data::<Arc<RuleEngine>>()?;

        let auth = ctx
            .data_opt::<AuthContext>()
            .cloned()
            .unwrap_or_else(AuthContext::anonymous);

        // Fetch existing document for owner checks
        let existing = db.get_document(&collection, &id).await.map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })?;
        let before = existing.clone();

        let sec_ctx = SecurityContext {
            user_id: if auth.authenticated {
                Some(auth.user_id.clone())
            } else {
                None
            },
            roles: auth.roles.clone(),
            authenticated: auth.authenticated,
            resource: Some(existing),
            incoming: Some(data.clone()),
        };

        if !rules.check(&collection, "update", &sec_ctx).map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })? {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        // Auto-detect FieldValue markers and route accordingly
        let has_field_values = data.as_object().is_some_and(|obj| {
            obj.values().any(|v| {
                v.as_object().is_some_and(|inner| {
                    inner.contains_key("_serverTimestamp")
                        || inner.contains_key("_increment")
                        || inner.contains_key("_arrayUnion")
                        || inner.contains_key("_arrayRemove")
                        || inner.contains_key("_deleteField")
                })
            })
        });

        let doc = if has_field_values {
            db.update_with_field_values(&collection, &id, data)
                .await
                .map_err(|e| {
                    tracing::error!("DB error: {e}");
                    async_graphql::Error::new("Internal server error")
                })?
        } else {
            db.update_document(&collection, &id, data)
                .await
                .map_err(|e| {
                    tracing::error!("DB error: {e}");
                    async_graphql::Error::new("Internal server error")
                })?
        };

        // Emit realtime change event
        if let Ok(tx) = ctx.data::<mpsc::Sender<ChangeEvent>>() {
            let doc_id = doc.get("id").and_then(|v| v.as_str()).unwrap_or(&id);
            if let Err(e) = tx
                .send(ChangeEvent {
                    action: ChangeAction::Update,
                    collection: collection.clone(),
                    document_id: doc_id.to_string(),
                    before_data: Some(before),
                    after_data: Some(doc.clone()),
                    data: doc.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                })
                .await
            {
                tracing::warn!("Realtime event dropped: {e}");
            }
        }

        Ok(doc)
    }

    /// Create or replace a document by collection and explicit ID.
    async fn set(
        &self,
        ctx: &Context<'_>,
        collection: String,
        id: String,
        data: Value,
    ) -> GqlResult<Value> {
        let data = normalize_data(data);
        let db = ctx.data::<DatabaseClient>()?;
        let rules = ctx.data::<Arc<RuleEngine>>()?;

        let auth = ctx
            .data_opt::<AuthContext>()
            .cloned()
            .unwrap_or_else(AuthContext::anonymous);

        let existing = db.get_document(&collection, &id).await.ok();
        let action = if existing.is_some() {
            "update"
        } else {
            "create"
        };

        let sec_ctx = SecurityContext {
            user_id: if auth.authenticated {
                Some(auth.user_id.clone())
            } else {
                None
            },
            roles: auth.roles.clone(),
            authenticated: auth.authenticated,
            resource: existing.clone(),
            incoming: Some(data.clone()),
        };

        if !rules.check(&collection, action, &sec_ctx).map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })? {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        let doc = db
            .upsert_document(&collection, &id, data)
            .await
            .map_err(|e| {
                tracing::error!("DB error: {e}");
                async_graphql::Error::new("Internal server error")
            })?;

        if let Ok(tx) = ctx.data::<mpsc::Sender<ChangeEvent>>() {
            let change_action = if existing.is_some() {
                ChangeAction::Update
            } else {
                ChangeAction::Create
            };
            if let Err(e) = tx
                .send(ChangeEvent {
                    action: change_action,
                    collection: collection.clone(),
                    document_id: id.clone(),
                    before_data: existing,
                    after_data: Some(doc.clone()),
                    data: doc.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                })
                .await
            {
                tracing::warn!("Realtime event dropped: {e}");
            }
        }

        Ok(doc)
    }

    /// Delete a document by collection and ID.
    async fn delete(&self, ctx: &Context<'_>, collection: String, id: String) -> GqlResult<Value> {
        let db = ctx.data::<DatabaseClient>()?;
        let rules = ctx.data::<Arc<RuleEngine>>()?;

        let auth = ctx
            .data_opt::<AuthContext>()
            .cloned()
            .unwrap_or_else(AuthContext::anonymous);

        let existing = db.get_document(&collection, &id).await.map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })?;
        let before = existing.clone();

        let sec_ctx = SecurityContext {
            user_id: if auth.authenticated {
                Some(auth.user_id.clone())
            } else {
                None
            },
            roles: auth.roles.clone(),
            authenticated: auth.authenticated,
            resource: Some(existing),
            incoming: None,
        };

        if !rules.check(&collection, "delete", &sec_ctx).map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })? {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        let doc = db.delete_document(&collection, &id).await.map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })?;

        // Emit realtime change event
        if let Ok(tx) = ctx.data::<mpsc::Sender<ChangeEvent>>()
            && let Err(e) = tx
                .send(ChangeEvent {
                    action: ChangeAction::Delete,
                    collection: collection.clone(),
                    document_id: id.clone(),
                    before_data: Some(before),
                    after_data: None,
                    data: doc.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                })
                .await
        {
            tracing::warn!("Realtime event dropped: {e}");
        }

        Ok(doc)
    }

    /// Batch create multiple documents in a collection.
    async fn batch_create(
        &self,
        ctx: &Context<'_>,
        collection: String,
        docs: Vec<Value>,
    ) -> GqlResult<Vec<Value>> {
        // Normalize and flatten: SDK may send a single JSON-encoded string
        // containing all docs as an array, rather than individual elements.
        let docs: Vec<Value> = docs
            .into_iter()
            .flat_map(|d| {
                let normalized = normalize_data(d);
                if let Value::Array(arr) = normalized {
                    arr
                } else {
                    vec![normalized]
                }
            })
            .collect();
        let db = ctx.data::<DatabaseClient>()?;
        let rules = ctx.data::<Arc<RuleEngine>>()?;

        let auth = ctx
            .data_opt::<AuthContext>()
            .cloned()
            .unwrap_or_else(AuthContext::anonymous);

        // Check RLS for EACH document's incoming data
        for doc in &docs {
            let sec_ctx = SecurityContext {
                user_id: if auth.authenticated {
                    Some(auth.user_id.clone())
                } else {
                    None
                },
                roles: auth.roles.clone(),
                authenticated: auth.authenticated,
                resource: None,
                incoming: Some(doc.clone()),
            };

            if !rules.check(&collection, "create", &sec_ctx).map_err(|e| {
                tracing::error!("DB error: {e}");
                async_graphql::Error::new("Internal server error")
            })? {
                return Err(async_graphql::Error::new("Permission denied"));
            }
        }

        let results = db.batch_create(&collection, docs).await.map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })?;

        // Emit change events
        if let Ok(tx) = ctx.data::<mpsc::Sender<ChangeEvent>>() {
            for doc in &results {
                let doc_id = doc.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
                if let Err(e) = tx
                    .send(ChangeEvent {
                        action: ChangeAction::Create,
                        collection: collection.clone(),
                        document_id: doc_id.to_string(),
                        before_data: None,
                        after_data: Some(doc.clone()),
                        data: doc.clone(),
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    })
                    .await
                {
                    tracing::warn!("Realtime event dropped: {e}");
                }
            }
        }

        Ok(results)
    }

    /// Batch delete multiple documents by IDs.
    async fn batch_delete(
        &self,
        ctx: &Context<'_>,
        collection: String,
        ids: Vec<String>,
    ) -> GqlResult<Vec<Value>> {
        let db = ctx.data::<DatabaseClient>()?;
        let rules = ctx.data::<Arc<RuleEngine>>()?;

        let auth = ctx
            .data_opt::<AuthContext>()
            .cloned()
            .unwrap_or_else(AuthContext::anonymous);

        // Check RLS per document — fetch each doc and verify delete permission
        for id in &ids {
            let existing = db.get_document(&collection, id).await.map_err(|e| {
                tracing::error!("DB error: {e}");
                async_graphql::Error::new("Internal server error")
            })?;
            let sec_ctx = SecurityContext {
                user_id: if auth.authenticated {
                    Some(auth.user_id.clone())
                } else {
                    None
                },
                roles: auth.roles.clone(),
                authenticated: auth.authenticated,
                resource: Some(existing),
                incoming: None,
            };

            if !rules.check(&collection, "delete", &sec_ctx).map_err(|e| {
                tracing::error!("DB error: {e}");
                async_graphql::Error::new("Internal server error")
            })? {
                return Err(async_graphql::Error::new(format!(
                    "Permission denied for document {id}"
                )));
            }
        }

        let results = db
            .batch_delete(&collection, ids.clone())
            .await
            .map_err(|e| {
                tracing::error!("DB error: {e}");
                async_graphql::Error::new("Internal server error")
            })?;

        if let Ok(tx) = ctx.data::<mpsc::Sender<ChangeEvent>>() {
            for (i, doc) in results.iter().enumerate() {
                let doc_id = ids.get(i).map(|s| s.as_str()).unwrap_or("unknown");
                if let Err(e) = tx
                    .send(ChangeEvent {
                        action: ChangeAction::Delete,
                        collection: collection.clone(),
                        document_id: doc_id.to_string(),
                        before_data: Some(doc.clone()),
                        after_data: None,
                        data: doc.clone(),
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    })
                    .await
                {
                    tracing::warn!("Realtime event dropped: {e}");
                }
            }
        }

        Ok(results)
    }

    /// Batch update multiple documents.
    /// Each entry in `updates` should be a JSON object with `id` and `data` fields.
    async fn batch_update(
        &self,
        ctx: &Context<'_>,
        collection: String,
        updates: Value,
    ) -> GqlResult<Vec<Value>> {
        let updates = normalize_data(updates);
        let db = ctx.data::<DatabaseClient>()?;
        let rules = ctx.data::<Arc<RuleEngine>>()?;

        let auth = ctx
            .data_opt::<AuthContext>()
            .cloned()
            .unwrap_or_else(AuthContext::anonymous);

        let update_list = match updates {
            Value::Array(arr) => arr,
            _ => return Err(async_graphql::Error::new("updates must be an array")),
        };

        let mut results = Vec::with_capacity(update_list.len());
        for entry in update_list {
            let obj = entry.as_object().ok_or_else(|| {
                async_graphql::Error::new("Each update entry must be an object with id and data")
            })?;
            let id = obj
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| async_graphql::Error::new("Each update must have an id"))?;
            let data = obj
                .get("data")
                .cloned()
                .unwrap_or(Value::Object(serde_json::Map::new()));

            // Check RLS per document with existing resource + incoming data
            let existing = db.get_document(&collection, id).await.map_err(|e| {
                tracing::error!("DB error: {e}");
                async_graphql::Error::new("Internal server error")
            })?;
            let before = existing.clone();

            let sec_ctx = SecurityContext {
                user_id: if auth.authenticated {
                    Some(auth.user_id.clone())
                } else {
                    None
                },
                roles: auth.roles.clone(),
                authenticated: auth.authenticated,
                resource: Some(existing),
                incoming: Some(data.clone()),
            };

            if !rules.check(&collection, "update", &sec_ctx).map_err(|e| {
                tracing::error!("DB error: {e}");
                async_graphql::Error::new("Internal server error")
            })? {
                return Err(async_graphql::Error::new(format!(
                    "Permission denied for document {id}"
                )));
            }

            let doc = db
                .update_document(&collection, id, data)
                .await
                .map_err(|e| {
                    tracing::error!("DB error: {e}");
                    async_graphql::Error::new("Internal server error")
                })?;

            // Emit change event
            if let Ok(tx) = ctx.data::<mpsc::Sender<ChangeEvent>>()
                && let Err(e) = tx
                    .send(ChangeEvent {
                        action: ChangeAction::Update,
                        collection: collection.clone(),
                        document_id: id.to_string(),
                        before_data: Some(before),
                        after_data: Some(doc.clone()),
                        data: doc.clone(),
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    })
                    .await
            {
                tracing::warn!("Realtime event dropped: {e}");
            }

            results.push(doc);
        }

        Ok(results)
    }

    /// Update a document with FieldValue operations.
    ///
    /// Supports special markers in the data:
    /// - `{ "field": { "_serverTimestamp": true } }` — set to server time
    /// - `{ "field": { "_increment": 5 } }` — increment by value
    /// - `{ "field": { "_arrayUnion": ["a", "b"] } }` — add to array
    /// - `{ "field": { "_arrayRemove": ["a"] } }` — remove from array
    /// - `{ "field": { "_deleteField": true } }` — remove the field
    async fn update_with_field_values(
        &self,
        ctx: &Context<'_>,
        collection: String,
        id: String,
        data: Value,
    ) -> GqlResult<Value> {
        let data = normalize_data(data);
        let db = ctx.data::<DatabaseClient>()?;
        let rules = ctx.data::<Arc<RuleEngine>>()?;

        let auth = ctx
            .data_opt::<AuthContext>()
            .cloned()
            .unwrap_or_else(AuthContext::anonymous);

        let existing = db.get_document(&collection, &id).await.map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })?;
        let before = existing.clone();

        let sec_ctx = SecurityContext {
            user_id: if auth.authenticated {
                Some(auth.user_id.clone())
            } else {
                None
            },
            roles: auth.roles.clone(),
            authenticated: auth.authenticated,
            resource: Some(existing),
            incoming: Some(data.clone()),
        };

        if !rules.check(&collection, "update", &sec_ctx).map_err(|e| {
            tracing::error!("DB error: {e}");
            async_graphql::Error::new("Internal server error")
        })? {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        let doc = db
            .update_with_field_values(&collection, &id, data)
            .await
            .map_err(|e| {
                tracing::error!("DB error: {e}");
                async_graphql::Error::new("Internal server error")
            })?;

        if let Ok(tx) = ctx.data::<mpsc::Sender<ChangeEvent>>()
            && let Err(e) = tx
                .send(ChangeEvent {
                    action: ChangeAction::Update,
                    collection: collection.clone(),
                    document_id: id,
                    before_data: Some(before),
                    after_data: Some(doc.clone()),
                    data: doc.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                })
                .await
        {
            tracing::warn!("Realtime event dropped: {e}");
        }

        Ok(doc)
    }
}
