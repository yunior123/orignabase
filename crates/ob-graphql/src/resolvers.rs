use async_graphql::{Context, Object, Result as GqlResult};
use ob_auth::AuthContext;
use ob_database::DatabaseClient;
use ob_realtime::registry::{ChangeAction, ChangeEvent};
use ob_security::{RuleEngine, SecurityContext};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct QueryRoot;

#[allow(clippy::too_many_arguments)]
#[Object]
impl QueryRoot {
    /// Get a single document by collection and ID.
    async fn get(&self, ctx: &Context<'_>, collection: String, id: String) -> GqlResult<Value> {
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

        if !rules
            .check(&collection, "read", &sec_ctx)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?
        {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        let doc = db
            .get_document(&collection, &id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        Ok(doc)
    }

    /// List documents in a collection with optional filters.
    async fn list(
        &self,
        ctx: &Context<'_>,
        collection: String,
        #[graphql(default)] filters: Option<Value>,
        order_by: Option<String>,
        #[graphql(default = false)] descending: bool,
        #[graphql(default)] limit: Option<i32>,
        #[graphql(default)] offset: Option<i32>,
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

        if !rules
            .check(&collection, "read", &sec_ctx)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?
        {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        let query = ob_database::query::QueryTranslator::build_select(
            &collection,
            filters.as_ref(),
            order_by.as_deref(),
            descending,
            limit.map(|n| n as usize),
            offset.map(|n| n as usize),
        );

        let results = db
            .query_raw(&query)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        Ok(results)
    }
}

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    /// Create a new document in a collection.
    async fn create(&self, ctx: &Context<'_>, collection: String, data: Value) -> GqlResult<Value> {
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

        if !rules
            .check(&collection, "create", &sec_ctx)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?
        {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        let doc = db
            .create_document(&collection, data)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        // Emit realtime change event
        if let Ok(tx) = ctx.data::<mpsc::Sender<ChangeEvent>>() {
            let doc_id = doc
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let _ = tx
                .send(ChangeEvent {
                    action: ChangeAction::Create,
                    collection: collection.clone(),
                    document_id: doc_id.to_string(),
                    data: doc.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                })
                .await;
        }

        Ok(doc)
    }

    /// Update a document by collection and ID.
    async fn update(
        &self,
        ctx: &Context<'_>,
        collection: String,
        id: String,
        data: Value,
    ) -> GqlResult<Value> {
        let db = ctx.data::<DatabaseClient>()?;
        let rules = ctx.data::<Arc<RuleEngine>>()?;

        let auth = ctx
            .data_opt::<AuthContext>()
            .cloned()
            .unwrap_or_else(AuthContext::anonymous);

        // Fetch existing document for owner checks
        let existing = db
            .get_document(&collection, &id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

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

        if !rules
            .check(&collection, "update", &sec_ctx)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?
        {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        let doc = db
            .update_document(&collection, &id, data)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        // Emit realtime change event
        if let Ok(tx) = ctx.data::<mpsc::Sender<ChangeEvent>>() {
            let doc_id = doc
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(&id);
            let _ = tx
                .send(ChangeEvent {
                    action: ChangeAction::Update,
                    collection: collection.clone(),
                    document_id: doc_id.to_string(),
                    data: doc.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                })
                .await;
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

        let existing = db
            .get_document(&collection, &id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

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

        if !rules
            .check(&collection, "delete", &sec_ctx)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?
        {
            return Err(async_graphql::Error::new("Permission denied"));
        }

        let doc = db
            .delete_document(&collection, &id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        // Emit realtime change event
        if let Ok(tx) = ctx.data::<mpsc::Sender<ChangeEvent>>() {
            let _ = tx
                .send(ChangeEvent {
                    action: ChangeAction::Delete,
                    collection: collection.clone(),
                    document_id: id.clone(),
                    data: doc.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                })
                .await;
        }

        Ok(doc)
    }
}
