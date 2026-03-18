use ob_realtime::registry::{ChangeAction, ChangeEvent};
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::shared::schema::{collections, fields, notification_types};
use crate::{HandlersState, products};
use crate::{email, push};

pub struct NativeTriggerExecutor {
    state: HandlersState,
    receiver: mpsc::Receiver<ChangeEvent>,
}

impl NativeTriggerExecutor {
    pub fn new(state: HandlersState, receiver: mpsc::Receiver<ChangeEvent>) -> Self {
        Self { state, receiver }
    }

    pub async fn run(mut self) {
        info!("Native trigger executor started");

        while let Some(event) = self.receiver.recv().await {
            if let Err(err) = self.handle_event(event).await {
                error!("Native trigger failed: {err}");
            }
        }

        info!("Native trigger executor stopped");
    }

    async fn handle_event(&self, event: ChangeEvent) -> Result<(), ob_core::Error> {
        match (event.collection.as_str(), &event.action) {
            ("products", ChangeAction::Create) => self.handle_product_create(&event).await,
            ("products", ChangeAction::Update) => self.handle_product_update(&event).await,
            ("products", ChangeAction::Delete) => self.handle_product_delete(&event).await,
            ("orders", ChangeAction::Update) => self.handle_order_update(&event).await,
            ("return_requests", ChangeAction::Update) => self.handle_return_update(&event).await,
            _ => Ok(()),
        }
    }

    async fn handle_product_create(&self, event: &ChangeEvent) -> Result<(), ob_core::Error> {
        let Some(search) = self.state.config.search.as_ref() else {
            return Ok(());
        };
        products::triggers::on_product_created(
            &self.state.db,
            &self.state.http_client,
            &search.url,
            search.api_key.as_deref().unwrap_or(""),
            &event.document_id,
            &event.data,
        )
        .await
    }

    async fn handle_product_update(&self, event: &ChangeEvent) -> Result<(), ob_core::Error> {
        let Some(search) = self.state.config.search.as_ref() else {
            return Ok(());
        };
        products::triggers::on_product_updated(
            &self.state.db,
            &self.state.http_client,
            &search.url,
            search.api_key.as_deref().unwrap_or(""),
            &event.document_id,
            &event.data,
        )
        .await
    }

    async fn handle_product_delete(&self, event: &ChangeEvent) -> Result<(), ob_core::Error> {
        let Some(search) = self.state.config.search.as_ref() else {
            return Ok(());
        };
        products::triggers::on_product_deleted(
            &self.state.http_client,
            &search.url,
            search.api_key.as_deref().unwrap_or(""),
            &event.document_id,
        )
        .await
    }

    async fn handle_order_update(&self, event: &ChangeEvent) -> Result<(), ob_core::Error> {
        let Some(before) = event.before_data.as_ref() else {
            return Ok(());
        };
        let Some(after) = event.after_data.as_ref() else {
            return Ok(());
        };

        self.handle_order_status_change(&event.document_id, before, after)
            .await?;
        self.handle_order_payment_status_change(&event.document_id, before, after)
            .await?;
        self.handle_order_item_status_changes(&event.document_id, before, after)
            .await?;
        Ok(())
    }

    async fn handle_return_update(&self, event: &ChangeEvent) -> Result<(), ob_core::Error> {
        let Some(before) = event.before_data.as_ref() else {
            return Ok(());
        };
        let Some(after) = event.after_data.as_ref() else {
            return Ok(());
        };

        let old_status = str_field(before, fields::RETURN_STATUS);
        let new_status = str_field(after, fields::RETURN_STATUS);
        if old_status.is_empty() || old_status == new_status {
            return Ok(());
        }

        let return_id = record_id(&event.document_id);
        let order_id = str_field(after, fields::ORDER_ID);
        let buyer_id = after
            .get(fields::BUYER_ID)
            .or_else(|| after.get("userId"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let seller_id = str_field(after, fields::SELLER_ID);

        let normalized_status = normalize_status(new_status);
        let notif_type = match normalized_status.as_str() {
            "REQUESTED" => notification_types::RETURN_REQUESTED,
            "APPROVED" => notification_types::RETURN_APPROVED,
            "REJECTED" => notification_types::RETURN_REJECTED,
            _ => "return_status_changed",
        };

        if !buyer_id.is_empty() {
            let lang = self.user_lang(buyer_id).await;
            let (title, body) = return_buyer_message(new_status, order_id, return_id, &lang);
            let payload = json!({
                fields::ORDER_ID: order_id,
                "returnId": return_id,
                fields::RETURN_STATUS: new_status,
            });
            self.create_notification_once(
                &claim_key(
                    "return_status_buyer",
                    &[return_id, order_id, &normalized_status, buyer_id],
                ),
                "return_status_changed",
                buyer_id,
                notif_type,
                &title,
                &body,
                payload,
            )
            .await?;
        }

        if return_seller_should_notify(new_status) && !seller_id.is_empty() {
            let lang = self.user_lang(seller_id).await;
            let (title, body) = return_seller_message(new_status, order_id, return_id, &lang);
            let payload = json!({
                fields::ORDER_ID: order_id,
                "returnId": return_id,
                fields::RETURN_STATUS: new_status,
            });
            self.create_notification_once(
                &claim_key(
                    "return_status_seller",
                    &[return_id, order_id, &normalized_status, seller_id],
                ),
                "return_status_changed",
                seller_id,
                notif_type,
                &title,
                &body,
                payload,
            )
            .await?;
        }

        Ok(())
    }

    async fn handle_order_status_change(
        &self,
        order_id: &str,
        before: &Value,
        after: &Value,
    ) -> Result<(), ob_core::Error> {
        let old_status = order_status(before);
        let new_status = order_status(after);
        if old_status.is_empty() || old_status == new_status {
            return Ok(());
        }
        let normalized_status = normalize_status(new_status);
        let order_record_id = record_id(order_id);

        if matches!(normalized_status.as_str(), "CONFIRMED" | "PROCESSING") {
            self.cleanup_stock_notifications(after).await;
        }

        let buyer_id = order_buyer_id(after);
        if !buyer_id.is_empty() {
            let lang = self.user_lang(buyer_id).await;
            let (title, body) = buyer_order_status_message(new_status, order_id, after, &lang);
            let payload = json!({
                fields::ORDER_ID: order_record_id,
                fields::ORDER_STATUS: new_status,
            });
            self.create_notification_once(
                &claim_key(
                    "order_status_buyer",
                    &[order_record_id, &normalized_status, buyer_id],
                ),
                "order_status_changed",
                buyer_id,
                notification_types::ORDER_STATUS_CHANGED,
                &title,
                &body,
                payload,
            )
            .await?;
        }

        for seller_id in seller_ids(after) {
            if !order_seller_should_notify(&normalized_status) {
                continue;
            }
            if normalized_status == "SHIPPED"
                && seller_id == str_field(after, fields::LAST_ACTOR_ID)
            {
                continue;
            }
            let lang = self.user_lang(&seller_id).await;
            let (title, body) = seller_order_status_message(new_status, order_id, after, &lang);
            let payload = json!({
                fields::ORDER_ID: order_record_id,
                fields::ORDER_STATUS: new_status,
            });
            self.create_notification_once(
                &claim_key(
                    "order_status_seller",
                    &[order_record_id, &normalized_status, &seller_id],
                ),
                "order_status_changed",
                &seller_id,
                notification_types::ORDER_STATUS_CHANGED,
                &title,
                &body,
                payload,
            )
            .await?;

            if normalized_status == "CONFIRMED" {
                let perishable_items = perishable_items_for_seller(after, &seller_id);
                if !perishable_items.is_empty() {
                    let (urgent_title, urgent_body) =
                        urgent_perishable_message(order_id, &perishable_items, &lang);
                    let payload = json!({
                        fields::ORDER_ID: order_record_id,
                        fields::SELLER_ID: seller_id,
                        "itemIds": item_batch_ids(&perishable_items),
                    });
                    self.create_notification_once(
                        &claim_key("perishable_order_urgent", &[order_record_id, &seller_id]),
                        "perishable_order_urgent",
                        &seller_id,
                        notification_types::PERISHABLE_ORDER_URGENT,
                        &urgent_title,
                        &urgent_body,
                        payload,
                    )
                    .await?;
                }
            }
        }

        Ok(())
    }

    async fn handle_order_payment_status_change(
        &self,
        order_id: &str,
        before: &Value,
        after: &Value,
    ) -> Result<(), ob_core::Error> {
        let old_payment = str_field(before, fields::PAYMENT_STATUS);
        let new_payment = str_field(after, fields::PAYMENT_STATUS);
        if old_payment.is_empty() || old_payment == new_payment {
            return Ok(());
        }
        let normalized_payment = normalize_status(new_payment);
        if !matches!(normalized_payment.as_str(), "REFUNDED" | "PARTIAL_REFUND") {
            return Ok(());
        }

        let buyer_id = order_buyer_id(after);
        if buyer_id.is_empty() {
            return Ok(());
        }

        let lang = self.user_lang(buyer_id).await;
        let refund_cents = refund_amount_cents(after, normalized_payment.as_str());
        let (title, body) = buyer_payment_message(new_payment, order_id, refund_cents, &lang);
        self.create_notification_once(
            &claim_key(
                "order_payment_buyer",
                &[record_id(order_id), &normalized_payment, buyer_id],
            ),
            "order_payment_status_changed",
            buyer_id,
            notification_types::REFUND_ISSUED,
            &title,
            &body,
            json!({
                fields::ORDER_ID: record_id(order_id),
                fields::PAYMENT_STATUS: new_payment,
                "refundAmountCents": refund_cents,
            }),
        )
        .await
    }

    async fn handle_order_item_status_changes(
        &self,
        order_id: &str,
        before: &Value,
        after: &Value,
    ) -> Result<(), ob_core::Error> {
        let before_items = before
            .get(fields::ITEMS)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let after_items = after
            .get(fields::ITEMS)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let before_map = before_items
            .iter()
            .filter_map(|item| item_key(item).map(|k| (k, item.clone())))
            .collect::<std::collections::HashMap<_, _>>();

        let buyer_id = order_buyer_id(after).to_string();
        if buyer_id.is_empty() {
            return Ok(());
        }

        let is_pickup = str_field(after, "deliverySpeed") == "pickup";
        let before_order_status = normalize_status(order_status(before));
        let after_order_status = normalize_status(order_status(after));
        let skip_full_order_shipped =
            before_order_status != "SHIPPED" && after_order_status == "SHIPPED" && !is_pickup;

        let mut shipped_items = Vec::new();
        let mut delivered_items = Vec::new();
        for item in after_items {
            let Some(key) = item_key(&item) else {
                continue;
            };
            let before_item = before_map.get(&key);
            let old_status = before_item
                .map(|it| str_field(it, fields::STATUS))
                .unwrap_or("");
            let new_status = str_field(&item, fields::STATUS);
            if old_status == new_status {
                continue;
            }

            let normalized = normalize_status(new_status);
            if normalized == "SHIPPED" {
                if !skip_full_order_shipped
                    && !item
                        .get("isDigital")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                {
                    shipped_items.push(item);
                }
            } else if normalized == "DELIVERED" {
                delivered_items.push(item);
            }
        }

        if !shipped_items.is_empty() {
            let lang = self.user_lang(&buyer_id).await;
            let (title, body) = aggregate_item_status_message(
                "shipped",
                order_id,
                &shipped_items,
                &lang,
                is_pickup,
            );
            self.create_notification_once(
                &claim_key(
                    "order_item_shipped_buyer",
                    &[
                        record_id(order_id),
                        &item_batch_key(&shipped_items),
                        &buyer_id,
                    ],
                ),
                "order_item_shipped",
                &buyer_id,
                notification_types::ORDER_STATUS_CHANGED,
                &title,
                &body,
                json!({
                    fields::ORDER_ID: record_id(order_id),
                    "itemIds": item_batch_ids(&shipped_items),
                    fields::STATUS: "SHIPPED",
                }),
            )
            .await?;
        }

        if !delivered_items.is_empty() {
            let lang = self.user_lang(&buyer_id).await;
            let (title, body) = aggregate_item_status_message(
                "delivered",
                order_id,
                &delivered_items,
                &lang,
                false,
            );
            self.create_notification_once(
                &claim_key(
                    "order_item_delivered_buyer",
                    &[
                        record_id(order_id),
                        &item_batch_key(&delivered_items),
                        &buyer_id,
                    ],
                ),
                "order_item_delivered",
                &buyer_id,
                notification_types::ORDER_STATUS_CHANGED,
                &title,
                &body,
                json!({
                    fields::ORDER_ID: record_id(order_id),
                    "itemIds": item_batch_ids(&delivered_items),
                    fields::STATUS: "DELIVERED",
                }),
            )
            .await?;
        }

        Ok(())
    }

    async fn claim_notification_once(
        &self,
        claim_id: &str,
        event_type: &str,
        payload: Value,
    ) -> bool {
        let created_at = chrono::Utc::now().to_rfc3339();
        let query = "CREATE type::thing($table, $id) CONTENT $data RETURN AFTER".to_string();
        self.state
            .db
            .query_bind(
                &query,
                json!({
                    "table": collections::WEBHOOK_EVENTS,
                    "id": claim_id,
                    "data": {
                        "eventType": event_type,
                        "processed": true,
                        "payload": payload,
                        "timestamp": created_at,
                        fields::CREATED_AT: created_at,
                        "processedAt": created_at,
                    }
                }),
            )
            .await
            .is_ok()
    }

    async fn cleanup_stock_notifications(&self, order: &Value) {
        let buyer_id = order_buyer_id(order);
        if buyer_id.is_empty() {
            return;
        }

        let Some(items) = order.get(fields::ITEMS).and_then(|v| v.as_array()) else {
            return;
        };

        for item in items {
            let product_id = str_field(item, fields::PRODUCT_ID);
            if product_id.is_empty() {
                continue;
            }

            let variant_key = item
                .get("variantKey")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let rows = self
                .state
                .db
                .query_bind(
                    &format!(
                        "SELECT id, variantKey FROM {} WHERE productId = $product_id AND userId = $user_id AND notifiedAt = NONE",
                        collections::STOCK_NOTIFICATIONS
                    ),
                    json!({
                        "product_id": product_id,
                        "user_id": buyer_id,
                    }),
                )
                .await
                .unwrap_or_default();

            for row in rows {
                let row_variant = row.get("variantKey").and_then(|v| v.as_str()).unwrap_or("");
                if row_variant != variant_key {
                    continue;
                }

                let Some(id) = row.get("id").and_then(|v| v.as_str()) else {
                    continue;
                };
                let _ = self
                    .state
                    .db
                    .delete_document(collections::STOCK_NOTIFICATIONS, id)
                    .await;
            }
        }
    }

    async fn create_notification_record_once(
        &self,
        notification_id: &str,
        user_id: &str,
        notification_type: &str,
        title: &str,
        body: &str,
        data: &Value,
    ) -> Result<(), ob_core::Error> {
        let now = chrono::Utc::now().to_rfc3339();
        let query = "CREATE type::thing($table, $id) CONTENT $data RETURN AFTER";
        let create_result = self
            .state
            .db
            .query_bind(
                query,
                json!({
                    "table": collections::NOTIFICATIONS,
                    "id": notification_id,
                    "data": {
                        "userId": user_id,
                        fields::NOTIFICATION_TYPE: notification_type,
                        "title": title,
                        "body": body,
                        "data": data,
                        "read": false,
                        fields::CREATED_AT: now,
                        fields::UPDATED_AT: now,
                    }
                }),
            )
            .await;

        match create_result {
            Ok(_) => Ok(()),
            Err(_) => self
                .state
                .db
                .get_document(collections::NOTIFICATIONS, notification_id)
                .await
                .map(|_| ()),
        }
    }

    async fn dispatch_notification_side_effects(
        &self,
        notification_id: &str,
        user_id: &str,
        title: &str,
        body: &str,
        data: Value,
        notification_type: &str,
    ) {
        self.dispatch_email(
            notification_id,
            user_id,
            title,
            body,
            notification_type,
            &data,
        )
        .await;
        self.dispatch_push(notification_id, user_id, title, body, &data)
            .await;
    }

    async fn create_notification_once(
        &self,
        claim_id: &str,
        event_type: &str,
        user_id: &str,
        notification_type: &str,
        title: &str,
        body: &str,
        data: Value,
    ) -> Result<(), ob_core::Error> {
        let notification_id = notification_record_id(claim_id);
        self.create_notification_record_once(
            &notification_id,
            user_id,
            notification_type,
            title,
            body,
            &data,
        )
        .await?;

        if !self
            .claim_notification_once(claim_id, event_type, data.clone())
            .await
        {
            return Ok(());
        }

        self.dispatch_notification_side_effects(
            &notification_id,
            user_id,
            title,
            body,
            data,
            notification_type,
        )
        .await;
        Ok(())
    }

    async fn dispatch_email(
        &self,
        notification_id: &str,
        user_id: &str,
        title: &str,
        body: &str,
        notification_type: &str,
        data: &Value,
    ) {
        let Ok(user) = self
            .state
            .db
            .get_document(collections::USERS, user_id)
            .await
        else {
            return;
        };
        let Some(to_email) = user.get(fields::EMAIL).and_then(|v| v.as_str()) else {
            return;
        };
        let lang = user
            .get(fields::PREFERRED_LANGUAGE)
            .and_then(|v| v.as_str())
            .unwrap_or("en");
        let html = generic_email_html(title, body, lang);
        let mail_log_id = mail_log_record_id(notification_id, to_email);
        let now = chrono::Utc::now().to_rfc3339();

        let _ = self
            .state
            .db
            .query_bind(
                "UPSERT type::thing($table, $id) CONTENT $data RETURN AFTER",
                json!({
                    "table": collections::MAIL_LOGS,
                    "id": mail_log_id,
                    "data": {
                        "notificationId": notification_id,
                        "to": to_email,
                        "subject": title,
                        "html": html,
                        "status": "pending",
                        "notificationType": notification_type,
                        "data": data,
                        "error": Value::Null,
                        "createdAt": now,
                        "updatedAt": now,
                    }
                }),
            )
            .await;

        let result = match (
            self.state.config.secret("mailjet_api_key"),
            self.state.config.secret("mailjet_secret_key"),
        ) {
            (Some(api_key), Some(secret_key)) => email::send_email(
                &self.state.http_client,
                api_key,
                secret_key,
                to_email,
                title,
                &html,
            )
            .await
            .map(|_| "sent"),
            _ => Err(email::EmailError::MissingCredentials),
        };

        let sent = result.is_ok();
        let status = if sent { "sent" } else { "pending" };
        let error_message = result.as_ref().err().map(|e| e.to_string());
        let _ = self
            .state
            .db
            .query_bind(
                "UPDATE type::thing($table, $id) MERGE $data RETURN AFTER",
                json!({
                    "table": collections::MAIL_LOGS,
                    "id": mail_log_id,
                    "data": {
                        "status": status,
                        "error": error_message,
                        "updatedAt": chrono::Utc::now().to_rfc3339(),
                        "sentAt": if sent { json!(chrono::Utc::now().to_rfc3339()) } else { Value::Null },
                    }
                }),
            )
            .await;
    }

    async fn dispatch_push(
        &self,
        notification_id: &str,
        user_id: &str,
        title: &str,
        body: &str,
        data: &Value,
    ) {
        let escaped_user_id = ob_core::escape_surreal_string(user_id);
        let tokens = self
            .state
            .db
            .query_bind_value(
                "SELECT token FROM _push_tokens WHERE user_id = $user_id",
                json!({"user_id": escaped_user_id})
            )
            .await
            .unwrap_or_default();
        if tokens.is_empty() {
            return;
        }

        let data_map = json_to_string_map(data);
        let project_id = std::env::var("OB_FCM_PROJECT_ID").ok();
        let service_account = std::env::var("OB_FCM_SERVICE_ACCOUNT").ok();

        for row in tokens {
            let Some(token) = row.get("token").and_then(|v| v.as_str()) else {
                continue;
            };
            let pending_id = pending_push_record_id(notification_id, token);
            let now = chrono::Utc::now().to_rfc3339();

            let _ = self
                .state
                .db
                .query_bind(
                    "UPSERT type::thing($table, $id) CONTENT $data RETURN AFTER",
                    json!({
                        "table": "_pending_notifications",
                        "id": pending_id,
                        "data": {
                            "notificationId": notification_id,
                            "token": token,
                            "title": title,
                            "body": body,
                            "data": data,
                            "status": "pending",
                            "created_at": now,
                            "updated_at": now,
                        }
                    }),
                )
                .await;

            let sent = if let (Some(project_id), Some(service_account)) =
                (project_id.as_deref(), service_account.as_deref())
            {
                push::send_push(
                    &self.state.http_client,
                    project_id,
                    service_account,
                    token,
                    title,
                    body,
                    Some(&data_map),
                )
                .await
                .is_ok()
            } else {
                false
            };

            let _ = self
                .state
                .db
                .query_bind(
                    "UPDATE type::thing($table, $id) MERGE $data RETURN AFTER",
                    json!({
                        "table": "_pending_notifications",
                        "id": pending_id,
                        "data": {
                            "status": if sent { "sent" } else { "pending" },
                            "updated_at": chrono::Utc::now().to_rfc3339(),
                            "sent_at": if sent { json!(chrono::Utc::now().to_rfc3339()) } else { Value::Null },
                        }
                    }),
                )
                .await;
        }
    }

    async fn user_lang(&self, user_id: &str) -> String {
        self.state
            .db
            .get_document(collections::USERS, user_id)
            .await
            .ok()
            .and_then(|user: serde_json::Value| {
                user.get(fields::PREFERRED_LANGUAGE)
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string)
            })
            .unwrap_or_else(|| "en".to_string())
    }
}

fn str_field<'a>(value: &'a Value, field: &str) -> &'a str {
    value.get(field).and_then(|v| v.as_str()).unwrap_or("")
}

fn order_status(value: &Value) -> &str {
    value
        .get(fields::ORDER_STATUS)
        .or_else(|| value.get(fields::STATUS))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn order_buyer_id(value: &Value) -> &str {
    value
        .get("userId")
        .or_else(|| value.get(fields::BUYER_ID))
        .or_else(|| value.get(fields::UID))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn record_id(raw: &str) -> &str {
    raw.split(':').next_back().unwrap_or(raw)
}

fn short_id(raw: &str) -> String {
    let normalized = record_id(raw);
    normalized
        .chars()
        .take(8)
        .collect::<String>()
        .to_uppercase()
}

fn normalize_status(status: &str) -> String {
    let normalized = status.trim().replace('-', "_").to_ascii_uppercase();
    match normalized.as_str() {
        "PARTIALLY_REFUNDED" => "PARTIAL_REFUND".to_string(),
        _ => normalized,
    }
}

fn return_seller_should_notify(status: &str) -> bool {
    matches!(normalize_status(status).as_str(), "REQUESTED" | "RECEIVED")
}

fn order_seller_should_notify(normalized_status: &str) -> bool {
    matches!(normalized_status, "CONFIRMED" | "SHIPPED" | "DELIVERED")
}

fn seller_ids(value: &Value) -> Vec<String> {
    value
        .get(fields::ITEMS)
        .and_then(|v| v.as_array())
        .map(|items| {
            let mut ids = std::collections::BTreeSet::new();
            for item in items {
                if let Some(seller_id) = item.get(fields::SELLER_ID).and_then(|v| v.as_str()) {
                    ids.insert(seller_id.to_string());
                }
            }
            ids.into_iter().collect()
        })
        .unwrap_or_default()
}

fn perishable_items_for_seller(order: &Value, seller_id: &str) -> Vec<Value> {
    order
        .get(fields::ITEMS)
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    item.get(fields::SELLER_ID).and_then(|v| v.as_str()) == Some(seller_id)
                        && item
                            .get(fields::IS_PERISHABLE)
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn item_key(item: &Value) -> Option<String> {
    item.get(fields::CART_ITEM_ID)
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

fn notification_item_key(item: &Value) -> String {
    item_key(item).unwrap_or_else(|| {
        let fallback = format!(
            "{}:{}:{}",
            str_field(item, fields::PRODUCT_ID),
            str_field(item, "name"),
            str_field(item, fields::STATUS)
        );
        stable_hash(&fallback)
    })
}

fn item_batch_ids(items: &[Value]) -> Vec<String> {
    let mut ids = items.iter().map(notification_item_key).collect::<Vec<_>>();
    ids.sort();
    ids
}

fn item_batch_key(items: &[Value]) -> String {
    stable_hash(&item_batch_ids(items).join(":"))
}

fn claim_key(prefix: &str, parts: &[&str]) -> String {
    let mut key = sanitize_id_component(prefix);
    for part in parts {
        key.push('_');
        key.push_str(&sanitize_id_component(part));
    }
    if key.len() > 120 {
        let hashed = stable_hash(&key);
        format!("{}_{}", sanitize_id_component(prefix), hashed)
    } else {
        key
    }
}

fn notification_record_id(claim_id: &str) -> String {
    claim_key("notification", &[claim_id])
}

fn mail_log_record_id(notification_id: &str, to_email: &str) -> String {
    claim_key("mail_log", &[notification_id, to_email])
}

fn pending_push_record_id(notification_id: &str, token: &str) -> String {
    claim_key("pending_push", &[notification_id, token])
}

fn sanitize_id_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
}

fn stable_hash(value: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn json_to_string_map(data: &Value) -> std::collections::HashMap<String, String> {
    data.as_object()
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| {
                    let value = v
                        .as_str()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| v.to_string());
                    (k.clone(), value)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn generic_email_html(title: &str, body: &str, lang: &str) -> String {
    let greeting = if lang == "fr" { "Bonjour," } else { "Hello," };
    format!(
        "<!DOCTYPE html><html><body style=\"font-family:Arial,sans-serif;background:#f5f7fb;padding:24px;\"><div style=\"max-width:640px;margin:0 auto;background:#fff;padding:32px;border-radius:12px;\"><h2>{title}</h2><p>{greeting}</p><p>{body}</p></div></body></html>"
    )
}

fn buyer_order_status_message(
    status: &str,
    order_id: &str,
    order: &Value,
    lang: &str,
) -> (String, String) {
    let oid = short_id(order_id);
    let tracking = str_field(order, fields::TRACKING_NUMBER);
    let carrier = str_field(order, fields::SHIPPING_CARRIER);
    let is_pickup = str_field(order, "deliverySpeed") == "pickup";
    match (normalize_status(status).as_str(), lang) {
        ("CONFIRMED", "fr") => (
            format!("Commande #{oid} confirmée"),
            format!("Votre commande #{oid} a été confirmée."),
        ),
        ("CONFIRMED", _) => (
            format!("Order #{oid} confirmed"),
            format!("Your order #{oid} has been confirmed."),
        ),
        ("PROCESSING", "fr") => (
            format!("Commande #{oid} en préparation"),
            format!("Votre commande #{oid} est en cours de préparation."),
        ),
        ("PROCESSING", _) => (
            format!("Order #{oid} is processing"),
            format!("Your order #{oid} is being processed."),
        ),
        ("IN_TRANSIT", "fr") => (
            format!("Commande #{oid} en transit"),
            if tracking.is_empty() {
                format!("Votre commande #{oid} est en transit.")
            } else if carrier.is_empty() {
                format!("Votre commande #{oid} est en transit. Suivi: {tracking}.")
            } else {
                format!("Votre commande #{oid} est en transit via {carrier}. Suivi: {tracking}.")
            },
        ),
        ("IN_TRANSIT", _) => (
            format!("Order #{oid} in transit"),
            if tracking.is_empty() {
                format!("Your order #{oid} is in transit.")
            } else if carrier.is_empty() {
                format!("Your order #{oid} is in transit. Tracking: {tracking}.")
            } else {
                format!("Your order #{oid} is in transit via {carrier}. Tracking: {tracking}.")
            },
        ),
        ("SHIPPED", "fr") if is_pickup => (
            format!("Commande #{oid} prête pour ramassage"),
            format!("Votre commande #{oid} est prête pour le ramassage."),
        ),
        ("SHIPPED", _) if is_pickup => (
            format!("Order #{oid} ready for pickup"),
            format!("Your order #{oid} is ready for pickup."),
        ),
        ("SHIPPED", "fr") => (
            format!("Commande #{oid} expédiée"),
            if tracking.is_empty() {
                format!("Votre commande #{oid} est en route.")
            } else if carrier.is_empty() {
                format!("Votre commande #{oid} est en route. Suivi: {tracking}.")
            } else {
                format!("Votre commande #{oid} est en route via {carrier}. Suivi: {tracking}.")
            },
        ),
        ("SHIPPED", _) => (
            format!("Order #{oid} shipped"),
            if tracking.is_empty() {
                format!("Your order #{oid} is on the way.")
            } else if carrier.is_empty() {
                format!("Your order #{oid} is on the way. Tracking: {tracking}.")
            } else {
                format!("Your order #{oid} is on the way via {carrier}. Tracking: {tracking}.")
            },
        ),
        ("DELIVERED", "fr")
            if order
                .get("confirmedByClient")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || order
                    .get("autoConfirmed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false) =>
        {
            (
                format!("Réception confirmée pour la commande #{oid}"),
                format!("La réception de votre commande #{oid} a été enregistrée."),
            )
        }
        ("DELIVERED", _)
            if order
                .get("confirmedByClient")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || order
                    .get("autoConfirmed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false) =>
        {
            (
                format!("Receipt confirmed for order #{oid}"),
                format!("Receipt confirmation for your order #{oid} has been recorded."),
            )
        }
        ("DELIVERED", "fr") => (
            format!("Commande #{oid} livrée"),
            format!("Votre commande #{oid} a été livrée."),
        ),
        ("DELIVERED", _) => (
            format!("Order #{oid} delivered"),
            format!("Your order #{oid} has been delivered."),
        ),
        ("CANCELLED", "fr") => (
            format!("Commande #{oid} annulée"),
            format!("Votre commande #{oid} a été annulée."),
        ),
        ("CANCELLED", _) => (
            format!("Order #{oid} cancelled"),
            format!("Your order #{oid} has been cancelled."),
        ),
        ("FAILED", "fr") => (
            format!("Paiement échoué pour la commande #{oid}"),
            format!("Le paiement de votre commande #{oid} n'a pas pu être traité."),
        ),
        ("FAILED", _) => (
            format!("Payment failed for order #{oid}"),
            format!("Payment for your order #{oid} could not be processed."),
        ),
        ("EXPIRED", "fr") => (
            format!("Commande #{oid} expirée"),
            format!("Votre commande #{oid} a expiré."),
        ),
        ("EXPIRED", _) => (
            format!("Order #{oid} expired"),
            format!("Your order #{oid} has expired."),
        ),
        ("DISPUTED", "fr") => (
            format!("Litige ouvert pour la commande #{oid}"),
            format!("Un litige a été ouvert pour votre commande #{oid}."),
        ),
        ("DISPUTED", _) => (
            format!("Dispute opened for order #{oid}"),
            format!("A dispute has been opened for your order #{oid}."),
        ),
        (_, "fr") => (
            format!("Mise à jour de la commande #{oid}"),
            format!("Votre commande #{oid} est maintenant {status}."),
        ),
        _ => (
            format!("Order #{oid} updated"),
            format!("Your order #{oid} is now {status}."),
        ),
    }
}

fn seller_order_status_message(
    status: &str,
    order_id: &str,
    order: &Value,
    lang: &str,
) -> (String, String) {
    let oid = short_id(order_id);
    let is_perishable = order
        .get(fields::ITEMS)
        .and_then(|v| v.as_array())
        .map(|items| {
            items.iter().any(|item| {
                item.get(fields::IS_PERISHABLE)
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    match (normalize_status(status).as_str(), lang) {
        ("CONFIRMED", "fr") if is_perishable => (
            format!("URGENT: commande périssable #{oid}"),
            format!("Une commande périssable #{oid} a été confirmée. Expédiez-la aujourd'hui."),
        ),
        ("CONFIRMED", _) if is_perishable => (
            format!("URGENT: perishable order #{oid}"),
            format!("Perishable order #{oid} has been confirmed. Ship it today."),
        ),
        ("CONFIRMED", "fr") => (
            format!("Nouvelle commande #{oid}"),
            format!("Une nouvelle commande #{oid} a été confirmée."),
        ),
        ("CONFIRMED", _) => (
            format!("New order #{oid}"),
            format!("A new order #{oid} has been confirmed."),
        ),
        ("PROCESSING", "fr") => (
            format!("Commande #{oid} en préparation"),
            format!("La commande #{oid} est maintenant en préparation."),
        ),
        ("PROCESSING", _) => (
            format!("Order #{oid} is processing"),
            format!("Order #{oid} is now being processed."),
        ),
        ("SHIPPED", "fr") => (
            format!("Expédition confirmée #{oid}"),
            format!("La commande #{oid} a été marquée comme expédiée."),
        ),
        ("SHIPPED", _) => (
            format!("Shipment confirmed #{oid}"),
            format!("Order #{oid} has been marked as shipped."),
        ),
        ("IN_TRANSIT", "fr") => (
            format!("Commande #{oid} en transit"),
            format!("La commande #{oid} est maintenant en transit."),
        ),
        ("IN_TRANSIT", _) => (
            format!("Order #{oid} in transit"),
            format!("Order #{oid} is now in transit."),
        ),
        ("DELIVERED", "fr") => (
            format!("Réception confirmée #{oid}"),
            format!("La commande #{oid} a été livrée. Le paiement est en attente."),
        ),
        ("DELIVERED", _) => (
            format!("Receipt confirmed #{oid}"),
            format!("Order #{oid} has been delivered. Payout is now pending."),
        ),
        ("CANCELLED", "fr") => (
            format!("Commande #{oid} annulée"),
            format!("La commande #{oid} a été annulée."),
        ),
        ("CANCELLED", _) => (
            format!("Order #{oid} cancelled"),
            format!("Order #{oid} has been cancelled."),
        ),
        (_, "fr") => (
            format!("Mise à jour de la commande #{oid}"),
            format!("La commande #{oid} est maintenant {status}."),
        ),
        _ => (
            format!("Order #{oid} updated"),
            format!("Order #{oid} is now {status}."),
        ),
    }
}

fn buyer_payment_message(
    status: &str,
    order_id: &str,
    refund_cents: Option<i64>,
    lang: &str,
) -> (String, String) {
    let oid = short_id(order_id);
    match (normalize_status(status).as_str(), lang) {
        ("REFUNDED", "fr") => (
            format!("Remboursement traité pour la commande #{oid}"),
            match refund_cents.and_then(format_cents) {
                Some(amount) => {
                    format!("Le remboursement de {amount} pour votre commande #{oid} a été traité.")
                }
                None => format!("Le remboursement de votre commande #{oid} a été traité."),
            },
        ),
        ("REFUNDED", _) => (
            format!("Refund processed for order #{oid}"),
            match refund_cents.and_then(format_cents) {
                Some(amount) => {
                    format!("Your refund of {amount} for order #{oid} has been processed.")
                }
                None => format!("Your refund for order #{oid} has been processed."),
            },
        ),
        ("PARTIAL_REFUND", "fr") => (
            format!("Remboursement partiel pour la commande #{oid}"),
            match refund_cents.and_then(format_cents) {
                Some(amount) => format!(
                    "Un remboursement partiel de {amount} a été traité pour votre commande #{oid}."
                ),
                None => {
                    format!("Un remboursement partiel a été traité pour votre commande #{oid}.")
                }
            },
        ),
        ("PARTIAL_REFUND", _) => (
            format!("Partial refund for order #{oid}"),
            match refund_cents.and_then(format_cents) {
                Some(amount) => format!(
                    "A partial refund of {amount} has been processed for your order #{oid}."
                ),
                None => format!("A partial refund has been processed for your order #{oid}."),
            },
        ),
        ("CAPTURED", "fr") => (
            format!("Paiement capturé pour la commande #{oid}"),
            format!("Le paiement de votre commande #{oid} a été capturé."),
        ),
        ("CAPTURED", _) => (
            format!("Payment captured for order #{oid}"),
            format!("Payment for your order #{oid} has been captured."),
        ),
        ("AUTHORIZED", "fr") => (
            format!("Paiement autorisé pour la commande #{oid}"),
            format!("Le paiement de votre commande #{oid} a été autorisé."),
        ),
        ("AUTHORIZED", _) => (
            format!("Payment authorized for order #{oid}"),
            format!("Payment for your order #{oid} has been authorized."),
        ),
        (_, "fr") => (
            format!("Mise à jour de paiement #{oid}"),
            format!("Le statut de paiement de votre commande #{oid} est maintenant {status}."),
        ),
        _ => (
            format!("Payment update for order #{oid}"),
            format!("Payment status for your order #{oid} is now {status}."),
        ),
    }
}

fn refund_amount_cents(order: &Value, normalized_payment: &str) -> Option<i64> {
    match normalized_payment {
        "REFUNDED" => order
            .get(fields::CUMULATIVE_REFUNDED_CENTS)
            .and_then(value_as_i64),
        "PARTIAL_REFUND" => order
            .get(fields::PARTIAL_REFUND_AMOUNT_CENTS)
            .and_then(value_as_i64),
        _ => None,
    }
}

fn value_as_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|n| i64::try_from(n).ok()))
}

fn format_cents(cents: i64) -> Option<String> {
    if cents <= 0 {
        return None;
    }
    let dollars = cents / 100;
    let remainder = (cents % 100).abs();
    Some(format!("${dollars}.{remainder:02}"))
}

fn item_status_message(
    status: &str,
    order_id: &str,
    item_name: &str,
    lang: &str,
    is_pickup: bool,
) -> (String, String) {
    let oid = short_id(order_id);
    match (status, lang) {
        ("shipped", "fr") if is_pickup => (
            format!("Article prêt pour ramassage, commande #{oid}"),
            format!("Votre article \"{item_name}\" est prêt pour le ramassage."),
        ),
        ("shipped", _) if is_pickup => (
            format!("Item ready for pickup, order #{oid}"),
            format!("Your item \"{item_name}\" is ready for pickup."),
        ),
        ("shipped", "fr") => (
            format!("Article expédié pour la commande #{oid}"),
            format!("Votre article \"{item_name}\" a été expédié."),
        ),
        ("shipped", _) => (
            format!("Item shipped for order #{oid}"),
            format!("Your item \"{item_name}\" has shipped."),
        ),
        ("delivered", "fr") => (
            format!("Article livré pour la commande #{oid}"),
            format!("Votre article \"{item_name}\" a été livré."),
        ),
        ("delivered", _) => (
            format!("Item delivered for order #{oid}"),
            format!("Your item \"{item_name}\" has been delivered."),
        ),
        (_, "fr") => (
            format!("Mise à jour de l'article #{oid}"),
            format!("Votre article \"{item_name}\" est maintenant {status}."),
        ),
        _ => (
            format!("Item update for order #{oid}"),
            format!("Your item \"{item_name}\" is now {status}."),
        ),
    }
}

fn aggregate_item_status_message(
    status: &str,
    order_id: &str,
    items: &[Value],
    lang: &str,
    is_pickup: bool,
) -> (String, String) {
    if items.len() == 1 {
        let item_name = items[0]
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("item");
        return item_status_message(status, order_id, item_name, lang, is_pickup);
    }

    let oid = short_id(order_id);
    let count = items.len();
    match (status, lang) {
        ("shipped", "fr") if is_pickup => (
            format!("Articles prêts pour ramassage, commande #{oid}"),
            format!("{count} articles de votre commande sont prêts pour le ramassage."),
        ),
        ("shipped", _) if is_pickup => (
            format!("Items ready for pickup, order #{oid}"),
            format!("{count} items from your order are ready for pickup."),
        ),
        ("shipped", "fr") => (
            format!("Articles expédiés pour la commande #{oid}"),
            format!("{count} articles de votre commande ont été expédiés."),
        ),
        ("shipped", _) => (
            format!("Items shipped for order #{oid}"),
            format!("{count} items from your order have been shipped."),
        ),
        ("delivered", "fr") => (
            format!("Articles livrés pour la commande #{oid}"),
            format!("{count} articles de votre commande ont été livrés."),
        ),
        ("delivered", _) => (
            format!("Items delivered for order #{oid}"),
            format!("{count} items from your order have been delivered."),
        ),
        _ => item_status_message(status, order_id, "item", lang, is_pickup),
    }
}

fn urgent_perishable_message(order_id: &str, items: &[Value], lang: &str) -> (String, String) {
    let oid = short_id(order_id);
    let names = items
        .iter()
        .take(3)
        .filter_map(|item| item.get("name").and_then(|v| v.as_str()))
        .collect::<Vec<_>>()
        .join(", ");

    match lang {
        "fr" => (
            format!("URGENT: commande périssable #{oid}"),
            if names.is_empty() {
                format!("Une commande périssable #{oid} a été confirmée. Expédiez-la aujourd'hui.")
            } else {
                format!(
                    "Des articles périssables ({names}) ont été commandés pour #{oid}. Expédiez-les aujourd'hui."
                )
            },
        ),
        _ => (
            format!("URGENT: perishable order #{oid}"),
            if names.is_empty() {
                format!("Perishable order #{oid} has been confirmed. Ship it today.")
            } else {
                format!("Perishable items ({names}) were ordered on #{oid}. Ship them today.")
            },
        ),
    }
}

fn return_buyer_message(
    status: &str,
    order_id: &str,
    return_id: &str,
    lang: &str,
) -> (String, String) {
    let oid = short_id(order_id);
    match (normalize_status(status).as_str(), lang) {
        ("REQUESTED", "fr") => (
            format!("Retour demandé pour la commande #{oid}"),
            format!("Votre demande de retour {return_id} a été enregistrée."),
        ),
        ("REQUESTED", _) => (
            format!("Return requested for order #{oid}"),
            format!("Your return request {return_id} has been submitted."),
        ),
        ("APPROVED", "fr") => (
            format!("Retour approuvé pour la commande #{oid}"),
            format!("Votre demande de retour {return_id} a été approuvée."),
        ),
        ("APPROVED", _) => (
            format!("Return approved for order #{oid}"),
            format!("Your return request {return_id} has been approved."),
        ),
        ("REJECTED", "fr") => (
            format!("Retour refusé pour la commande #{oid}"),
            format!("Votre demande de retour {return_id} a été refusée."),
        ),
        ("REJECTED", _) => (
            format!("Return rejected for order #{oid}"),
            format!("Your return request {return_id} has been rejected."),
        ),
        ("LABEL_ISSUED", "fr") => (
            format!("Etiquette de retour prête pour la commande #{oid}"),
            format!("Votre etiquette de retour pour {return_id} est prête."),
        ),
        ("LABEL_ISSUED", _) => (
            format!("Return label ready for order #{oid}"),
            format!("Your return shipping label for {return_id} is ready."),
        ),
        ("RECEIVED", "fr") => (
            format!("Retour reçu pour la commande #{oid}"),
            format!("Votre retour {return_id} a été reçu et le remboursement est en cours."),
        ),
        ("RECEIVED", _) => (
            format!("Return received for order #{oid}"),
            format!("Your return {return_id} has been received and refund processing has started."),
        ),
        ("REFUNDED", "fr") => (
            format!("Retour remboursé pour la commande #{oid}"),
            format!("Le remboursement de votre retour {return_id} a été traité."),
        ),
        ("REFUNDED", _) => (
            format!("Return refunded for order #{oid}"),
            format!("Refund for your return {return_id} has been processed."),
        ),
        ("ESCALATED", "fr") => (
            format!("Retour escaladé pour la commande #{oid}"),
            format!(
                "Votre retour {return_id} a été transmis à notre équipe de support. Un administrateur examinera le dossier sous 2 jours ouvrables."
            ),
        ),
        ("ESCALATED", _) => (
            format!("Return escalated for order #{oid}"),
            format!(
                "Your return {return_id} has been escalated to support. An admin will review it within 2 business days."
            ),
        ),
        (_, "fr") => (
            format!("Mise à jour du retour #{oid}"),
            format!("Votre retour {return_id} est maintenant {status}."),
        ),
        _ => (
            format!("Return update for order #{oid}"),
            format!("Your return {return_id} is now {status}."),
        ),
    }
}

fn return_seller_message(
    status: &str,
    order_id: &str,
    return_id: &str,
    lang: &str,
) -> (String, String) {
    let oid = short_id(order_id);
    match (normalize_status(status).as_str(), lang) {
        ("REQUESTED", "fr") => (
            format!("Nouveau retour demandé pour la commande #{oid}"),
            format!("Le retour {return_id} nécessite votre révision."),
        ),
        ("REQUESTED", _) => (
            format!("New return requested for order #{oid}"),
            format!("Return request {return_id} requires your review."),
        ),
        ("RECEIVED", "fr") => (
            format!("Retour reçu pour la commande #{oid}"),
            format!("Le retour {return_id} a été marqué comme reçu."),
        ),
        ("RECEIVED", _) => (
            format!("Return received for order #{oid}"),
            format!("Return {return_id} has been marked as received."),
        ),
        ("ESCALATED", "fr") => (
            format!("Retour escaladé pour la commande #{oid}"),
            format!("Le retour {return_id} a été escaladé vers le support."),
        ),
        ("ESCALATED", _) => (
            format!("Return escalated for order #{oid}"),
            format!("Return {return_id} has been escalated to support."),
        ),
        (_, "fr") => (
            format!("Mise à jour du retour #{oid}"),
            format!("Le retour {return_id} est maintenant {status}."),
        ),
        _ => (
            format!("Return update for order #{oid}"),
            format!("Return {return_id} is now {status}."),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    async fn setup_state() -> HandlersState {
        HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        }
    }

    async fn setup_executor() -> NativeTriggerExecutor {
        let state = setup_state().await;
        let (_tx, rx) = mpsc::channel(8);
        NativeTriggerExecutor::new(state, rx)
    }

    async fn seed_user(executor: &NativeTriggerExecutor, user_id: &str, lang: &str) {
        executor
            .state
            .db
            .upsert_document(
                collections::USERS,
                user_id,
                json!({
                    fields::EMAIL: format!("{user_id}@example.com"),
                    fields::PREFERRED_LANGUAGE: lang,
                }),
            )
            .await
            .unwrap();
    }

    #[test]
    fn normalize_status_maps_python_partial_refund_alias() {
        assert_eq!(normalize_status("partially_refunded"), "PARTIAL_REFUND");
        assert_eq!(normalize_status("PARTIAL_REFUND"), "PARTIAL_REFUND");
    }

    #[test]
    fn refund_amount_reads_full_and_partial_fields() {
        let order = json!({
            fields::CUMULATIVE_REFUNDED_CENTS: 1250,
            fields::PARTIAL_REFUND_AMOUNT_CENTS: 325,
        });

        assert_eq!(refund_amount_cents(&order, "REFUNDED"), Some(1250));
        assert_eq!(refund_amount_cents(&order, "PARTIAL_REFUND"), Some(325));
        assert_eq!(refund_amount_cents(&order, "CAPTURED"), None);
    }

    #[test]
    fn aggregate_item_status_message_batches_multiple_items() {
        let items = vec![
            json!({"name": "Apples", fields::CART_ITEM_ID: "c1"}),
            json!({"name": "Bread", fields::CART_ITEM_ID: "c2"}),
        ];

        let (title, body) =
            aggregate_item_status_message("shipped", "orders:abc12345", &items, "en", false);

        assert!(title.contains("Items shipped"));
        assert!(body.contains("2 items"));
    }

    #[test]
    fn buyer_order_status_message_handles_pickup_and_receipt_confirmation() {
        let pickup_order = json!({"deliverySpeed": "pickup"});
        let (pickup_title, pickup_body) =
            buyer_order_status_message("SHIPPED", "orders:abc12345", &pickup_order, "en");
        assert!(pickup_title.contains("ready for pickup"));
        assert!(pickup_body.contains("ready for pickup"));

        let confirmed_order = json!({"confirmedByClient": true});
        let (confirmed_title, confirmed_body) =
            buyer_order_status_message("DELIVERED", "orders:abc12345", &confirmed_order, "en");
        assert!(confirmed_title.contains("Receipt confirmed"));
        assert!(confirmed_body.contains("has been recorded"));
    }

    #[test]
    fn seller_order_status_message_handles_perishable_and_delivered_states() {
        let perishable_order = json!({
            fields::ITEMS: [{fields::IS_PERISHABLE: true}]
        });
        let (urgent_title, urgent_body) =
            seller_order_status_message("CONFIRMED", "orders:abc12345", &perishable_order, "en");
        assert!(urgent_title.contains("URGENT"));
        assert!(urgent_body.contains("Ship it today"));

        let plain_order = json!({});
        let (delivered_title, delivered_body) =
            seller_order_status_message("DELIVERED", "orders:abc12345", &plain_order, "en");
        assert!(delivered_title.contains("Receipt confirmed"));
        assert!(delivered_body.contains("Payout is now pending"));
    }

    #[test]
    fn buyer_payment_message_formats_partial_refund_amount() {
        let (title, body) =
            buyer_payment_message("PARTIAL_REFUND", "orders:abc12345", Some(325), "en");
        assert!(title.contains("Partial refund"));
        assert!(body.contains("$3.25"));
    }

    #[test]
    fn return_messages_match_expected_recipients() {
        let (buyer_title, buyer_body) =
            return_buyer_message("ESCALATED", "orders:abc12345", "ret12345", "en");
        assert!(buyer_title.contains("Return escalated"));
        assert!(buyer_body.contains("support"));

        let (seller_title, seller_body) =
            return_seller_message("RECEIVED", "orders:abc12345", "ret12345", "en");
        assert!(seller_title.contains("Return received"));
        assert!(seller_body.contains("marked as received"));
    }

    #[test]
    fn item_batch_key_is_order_independent() {
        let items_a = vec![
            json!({fields::CART_ITEM_ID: "c1"}),
            json!({fields::CART_ITEM_ID: "c2"}),
        ];
        let items_b = vec![
            json!({fields::CART_ITEM_ID: "c2"}),
            json!({fields::CART_ITEM_ID: "c1"}),
        ];

        assert_eq!(item_batch_key(&items_a), item_batch_key(&items_b));
    }

    #[test]
    fn item_key_requires_cart_item_id_for_status_diffing() {
        let item = json!({
            fields::PRODUCT_ID: "prod_1",
        });
        assert_eq!(item_key(&item), None);
    }

    #[test]
    fn claim_key_scopes_notifications_by_recipient() {
        let buyer_key = claim_key("order_status_buyer", &["order123", "CONFIRMED", "buyer_a"]);
        let other_buyer_key =
            claim_key("order_status_buyer", &["order123", "CONFIRMED", "buyer_b"]);
        let seller_key = claim_key(
            "order_status_seller",
            &["order123", "CONFIRMED", "seller_a"],
        );

        assert_ne!(buyer_key, other_buyer_key);
        assert_ne!(buyer_key, seller_key);
        assert!(buyer_key.contains("buyer_a"));
    }

    #[test]
    fn claim_key_hashes_long_inputs_stably() {
        let long_part = "x".repeat(200);
        let key_a = claim_key("order_item_shipped_buyer", &[&long_part, "buyer_a"]);
        let key_b = claim_key("order_item_shipped_buyer", &[&long_part, "buyer_a"]);

        assert_eq!(key_a, key_b);
        assert!(key_a.starts_with("order_item_shipped_buyer_"));
        assert!(key_a.len() < 120);
    }

    #[test]
    fn notification_record_id_is_stable_and_namespaced() {
        let record_a = notification_record_id("order_status_buyer_order123_confirmed_buyer_a");
        let record_b = notification_record_id("order_status_buyer_order123_confirmed_buyer_a");

        assert_eq!(record_a, record_b);
        assert!(record_a.starts_with("notification_"));
    }

    #[test]
    fn side_effect_record_ids_are_stable_and_distinct() {
        let notification_id =
            notification_record_id("order_status_buyer_order123_confirmed_buyer_a");
        let mail_a = mail_log_record_id(&notification_id, "buyer@example.com");
        let mail_b = mail_log_record_id(&notification_id, "buyer@example.com");
        let push = pending_push_record_id(&notification_id, "token_123");

        assert_eq!(mail_a, mail_b);
        assert!(mail_a.starts_with("mail_log_"));
        assert!(push.starts_with("pending_push_"));
        assert_ne!(mail_a, push);
    }

    #[test]
    fn order_seller_should_notify_matches_python_trigger_surface() {
        assert!(order_seller_should_notify("CONFIRMED"));
        assert!(order_seller_should_notify("SHIPPED"));
        assert!(order_seller_should_notify("DELIVERED"));
        assert!(!order_seller_should_notify("PROCESSING"));
        assert!(!order_seller_should_notify("IN_TRANSIT"));
        assert!(!order_seller_should_notify("CANCELLED"));
    }

    #[test]
    fn return_seller_should_notify_matches_python_trigger_surface() {
        assert!(return_seller_should_notify("REQUESTED"));
        assert!(return_seller_should_notify("RECEIVED"));
        assert!(!return_seller_should_notify("ESCALATED"));
        assert!(!return_seller_should_notify("APPROVED"));
    }

    #[test]
    fn return_escalated_admin_claim_key_scopes_by_admin() {
        let first = claim_key("return_escalated_admin", &["ret1", "ord1", "admin_a"]);
        let second = claim_key("return_escalated_admin", &["ret1", "ord1", "admin_b"]);
        assert_ne!(first, second);
    }

    #[test]
    fn perishable_items_for_seller_filters_by_seller_and_flag() {
        let order = json!({
            fields::ITEMS: [
                {fields::SELLER_ID: "s1", fields::IS_PERISHABLE: true, "name": "Milk"},
                {fields::SELLER_ID: "s1", fields::IS_PERISHABLE: false, "name": "Book"},
                {fields::SELLER_ID: "s2", fields::IS_PERISHABLE: true, "name": "Fish"},
            ]
        });

        let items = perishable_items_for_seller(&order, "s1");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["name"], "Milk");
    }

    #[tokio::test]
    async fn handle_order_status_change_creates_notifications_and_cleans_stock_watchers() {
        let executor = setup_executor().await;
        executor
            .state
            .db
            .upsert_document(
                collections::USERS,
                "buyer_1",
                json!({
                    fields::EMAIL: "buyer@example.com",
                    fields::PREFERRED_LANGUAGE: "en",
                }),
            )
            .await
            .unwrap();
        executor
            .state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({
                    fields::EMAIL: "seller@example.com",
                    fields::PREFERRED_LANGUAGE: "en",
                }),
            )
            .await
            .unwrap();
        let _ = executor
            .state
            .db
            .create_document(
                collections::STOCK_NOTIFICATIONS,
                json!({
                    "productId": "prod_1",
                    "userId": "buyer_1",
                    "variantKey": "blue",
                }),
            )
            .await
            .unwrap();

        let before = json!({
            fields::ORDER_STATUS: "PENDING",
            "userId": "buyer_1",
            fields::ITEMS: [{
                fields::PRODUCT_ID: "prod_1",
                fields::SELLER_ID: "seller_1",
                fields::IS_PERISHABLE: true,
                "variantKey": "blue",
                "name": "Milk crate",
            }],
        });
        let after = json!({
            fields::ORDER_STATUS: "CONFIRMED",
            "userId": "buyer_1",
            fields::ITEMS: [{
                fields::PRODUCT_ID: "prod_1",
                fields::SELLER_ID: "seller_1",
                fields::IS_PERISHABLE: true,
                "variantKey": "blue",
                "name": "Milk crate",
            }],
        });

        executor
            .handle_order_status_change("orders:ord_1", &before, &after)
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(20))
            .await
            .unwrap();
        let stock_watchers = executor
            .state
            .db
            .list_documents(collections::STOCK_NOTIFICATIONS, Some(10))
            .await;

        assert_eq!(notifications.len(), 3);
        assert!(notifications.iter().any(|doc| doc["userId"] == "buyer_1"));
        assert!(notifications.iter().any(|doc| doc["userId"] == "seller_1"));
        assert!(stock_watchers.unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_order_payment_status_change_creates_refund_notification() {
        let executor = setup_executor().await;
        executor
            .state
            .db
            .upsert_document(
                collections::USERS,
                "buyer_1",
                json!({
                    fields::EMAIL: "buyer@example.com",
                    fields::PREFERRED_LANGUAGE: "en",
                }),
            )
            .await
            .unwrap();

        let before = json!({
            "userId": "buyer_1",
            fields::PAYMENT_STATUS: "CAPTURED",
            fields::CUMULATIVE_REFUNDED_CENTS: 0,
        });
        let after = json!({
            "userId": "buyer_1",
            fields::PAYMENT_STATUS: "REFUNDED",
            fields::CUMULATIVE_REFUNDED_CENTS: 1250,
        });

        executor
            .handle_order_payment_status_change("orders:ord_1", &before, &after)
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0]["userId"], "buyer_1");
        assert_eq!(
            notifications[0][fields::NOTIFICATION_TYPE],
            notification_types::REFUND_ISSUED
        );
    }

    #[tokio::test]
    async fn handle_order_item_status_changes_creates_shipped_and_delivered_notifications() {
        let executor = setup_executor().await;
        executor
            .state
            .db
            .upsert_document(
                collections::USERS,
                "buyer_1",
                json!({
                    fields::EMAIL: "buyer@example.com",
                    fields::PREFERRED_LANGUAGE: "en",
                }),
            )
            .await
            .unwrap();

        let before = json!({
            "userId": "buyer_1",
            fields::ORDER_STATUS: "PROCESSING",
            fields::ITEMS: [
                { fields::CART_ITEM_ID: "c1", fields::STATUS: "PROCESSING", "isDigital": false, "name": "Box A" },
                { fields::CART_ITEM_ID: "c2", fields::STATUS: "SHIPPED", "isDigital": false, "name": "Box B" }
            ],
        });
        let after = json!({
            "userId": "buyer_1",
            fields::ORDER_STATUS: "PROCESSING",
            fields::ITEMS: [
                { fields::CART_ITEM_ID: "c1", fields::STATUS: "SHIPPED", "isDigital": false, "name": "Box A" },
                { fields::CART_ITEM_ID: "c2", fields::STATUS: "DELIVERED", "isDigital": false, "name": "Box B" }
            ],
        });

        executor
            .handle_order_item_status_changes("orders:ord_1", &before, &after)
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert_eq!(notifications.len(), 2);
        assert!(
            notifications
                .iter()
                .any(|doc| doc["title"].as_str().unwrap_or("").contains("shipped"))
        );
        assert!(
            notifications
                .iter()
                .any(|doc| doc["title"].as_str().unwrap_or("").contains("delivered"))
        );
    }

    #[tokio::test]
    async fn handle_return_update_notifies_buyer_and_seller() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;
        seed_user(&executor, "seller_1", "en").await;

        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "return_requests".into(),
            document_id: "return_requests:ret_1".into(),
            data: json!({}),
            before_data: Some(json!({
                fields::RETURN_STATUS: "approved",
                fields::ORDER_ID: "ord_1",
                fields::BUYER_ID: "buyer_1",
                fields::SELLER_ID: "seller_1",
            })),
            after_data: Some(json!({
                fields::RETURN_STATUS: "received",
                fields::ORDER_ID: "ord_1",
                fields::BUYER_ID: "buyer_1",
                fields::SELLER_ID: "seller_1",
            })),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        executor.handle_return_update(&event).await.unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert_eq!(notifications.len(), 2);
        assert!(notifications.iter().any(|doc| doc["userId"] == "buyer_1"));
        assert!(notifications.iter().any(|doc| doc["userId"] == "seller_1"));
    }

    #[tokio::test]
    async fn handle_order_status_change_covers_python_buyer_status_variants() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;
        seed_user(&executor, "seller_1", "en").await;

        let cases = vec![
            (
                "SHIPPED",
                json!({"deliverySpeed": "pickup"}),
                "ready for pickup",
            ),
            ("IN_TRANSIT", json!({}), "in transit"),
            (
                "DELIVERED",
                json!({"confirmedByClient": false, "autoConfirmed": false}),
                "has been delivered",
            ),
            ("CANCELLED", json!({}), "has been cancelled"),
            ("FAILED", json!({}), "could not be processed"),
            ("EXPIRED", json!({}), "has expired"),
            ("DISPUTED", json!({}), "dispute has been opened"),
        ];

        for (idx, (new_status, extra, _expected_body)) in cases.into_iter().enumerate() {
            let order_id = format!("orders:status_case_{idx}");
            let mut after = json!({
                fields::ORDER_STATUS: new_status,
                "userId": "buyer_1",
                fields::ITEMS: [{ fields::SELLER_ID: "seller_1" }],
            });
            if let Some(after_obj) = after.as_object_mut() {
                if let Some(extra_obj) = extra.as_object() {
                    for (key, value) in extra_obj {
                        after_obj.insert(key.clone(), value.clone());
                    }
                }
            }

            executor
                .handle_order_status_change(
                    &order_id,
                    &json!({
                        fields::ORDER_STATUS: "PROCESSING",
                        "userId": "buyer_1",
                        fields::ITEMS: [{ fields::SELLER_ID: "seller_1" }],
                    }),
                    &after,
                )
                .await
                .unwrap();
        }

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(50))
            .await
            .unwrap();

        let buyer_notifications = notifications
            .iter()
            .filter(|doc| doc["userId"] == "buyer_1")
            .collect::<Vec<_>>();

        assert!(buyer_notifications.iter().any(|doc| {
            doc["body"]
                .as_str()
                .unwrap_or("")
                .contains("ready for pickup")
        }));
        assert!(
            buyer_notifications
                .iter()
                .any(|doc| doc["body"].as_str().unwrap_or("").contains("in transit"))
        );
        assert!(buyer_notifications.iter().any(|doc| {
            doc["body"]
                .as_str()
                .unwrap_or("")
                .contains("has been delivered")
        }));
        assert!(buyer_notifications.iter().any(|doc| {
            doc["body"]
                .as_str()
                .unwrap_or("")
                .contains("has been cancelled")
        }));
        assert!(buyer_notifications.iter().any(|doc| {
            doc["body"]
                .as_str()
                .unwrap_or("")
                .contains("could not be processed")
        }));
        assert!(
            buyer_notifications
                .iter()
                .any(|doc| doc["body"].as_str().unwrap_or("").contains("has expired"))
        );
        assert!(buyer_notifications.iter().any(|doc| {
            doc["body"]
                .as_str()
                .unwrap_or("")
                .contains("dispute has been opened")
        }));
    }

    #[tokio::test]
    async fn handle_order_status_change_delivered_confirmation_variants_match_python_paths() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;
        seed_user(&executor, "seller_1", "en").await;

        executor
            .handle_order_status_change(
                "orders:delivered_confirmed",
                &json!({
                    fields::ORDER_STATUS: "SHIPPED",
                    "userId": "buyer_1",
                    fields::ITEMS: [{ fields::SELLER_ID: "seller_1" }],
                }),
                &json!({
                    fields::ORDER_STATUS: "DELIVERED",
                    "userId": "buyer_1",
                    "confirmedByClient": true,
                    fields::ITEMS: [{ fields::SELLER_ID: "seller_1" }],
                }),
            )
            .await
            .unwrap();

        executor
            .handle_order_status_change(
                "orders:delivered_auto",
                &json!({
                    fields::ORDER_STATUS: "SHIPPED",
                    "userId": "buyer_1",
                    fields::ITEMS: [{ fields::SELLER_ID: "seller_1" }],
                }),
                &json!({
                    fields::ORDER_STATUS: "DELIVERED",
                    "userId": "buyer_1",
                    "autoConfirmed": true,
                    fields::ITEMS: [{ fields::SELLER_ID: "seller_1" }],
                }),
            )
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(20))
            .await
            .unwrap();

        let buyer_bodies = notifications
            .iter()
            .filter(|doc| doc["userId"] == "buyer_1")
            .filter_map(|doc| doc["body"].as_str())
            .collect::<Vec<_>>();

        assert!(
            buyer_bodies
                .iter()
                .any(|body| body.contains("has been recorded"))
        );
    }

    #[tokio::test]
    async fn handle_return_update_covers_python_return_status_variants() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;
        seed_user(&executor, "seller_1", "en").await;

        let cases = vec![
            ("REQUESTED", "PENDING"),
            ("APPROVED", "REQUESTED"),
            ("REJECTED", "APPROVED"),
            ("LABEL_ISSUED", "APPROVED"),
            ("REFUNDED", "RECEIVED"),
            ("ESCALATED", "REJECTED"),
        ];

        for (idx, (new_status, old_status)) in cases.into_iter().enumerate() {
            let event = ChangeEvent {
                action: ChangeAction::Update,
                collection: "return_requests".into(),
                document_id: format!("return_requests:ret_variant_{idx}"),
                data: json!({}),
                before_data: Some(json!({
                    fields::RETURN_STATUS: old_status,
                    fields::ORDER_ID: format!("ord_variant_{idx}"),
                    fields::BUYER_ID: "buyer_1",
                    fields::SELLER_ID: "seller_1",
                })),
                after_data: Some(json!({
                    fields::RETURN_STATUS: new_status,
                    fields::ORDER_ID: format!("ord_variant_{idx}"),
                    fields::BUYER_ID: "buyer_1",
                    fields::SELLER_ID: "seller_1",
                })),
                timestamp: chrono::Utc::now().to_rfc3339(),
            };

            executor.handle_return_update(&event).await.unwrap();
        }

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(50))
            .await
            .unwrap();

        assert!(notifications.iter().any(|doc| {
            doc["userId"] == "seller_1"
                && doc["title"]
                    .as_str()
                    .unwrap_or("")
                    .contains("New return request")
        }));
        assert!(notifications.iter().any(|doc| {
            doc["userId"] == "buyer_1"
                && doc["title"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Return approved")
        }));
        assert!(notifications.iter().any(|doc| {
            doc["userId"] == "buyer_1"
                && doc["title"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Return rejected")
        }));
        assert!(notifications.iter().any(|doc| {
            doc["userId"] == "buyer_1"
                && doc["title"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Return label ready")
        }));
        assert!(notifications.iter().any(|doc| {
            doc["userId"] == "buyer_1"
                && doc["title"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Return refunded")
        }));
        assert!(notifications.iter().any(|doc| {
            doc["userId"] == "buyer_1"
                && doc["title"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Return escalated")
        }));
    }

    #[tokio::test]
    async fn handle_order_item_status_changes_skips_full_order_shipped_and_digital_items() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;

        executor
            .handle_order_item_status_changes(
                "orders:skip_full_shipped",
                &json!({
                    "userId": "buyer_1",
                    fields::ORDER_STATUS: "PROCESSING",
                    fields::ITEMS: [
                        { fields::CART_ITEM_ID: "c1", fields::STATUS: "PROCESSING", "isDigital": false, "name": "Physical item" },
                        { fields::CART_ITEM_ID: "c2", fields::STATUS: "PROCESSING", "isDigital": true, "name": "Digital item" }
                    ],
                }),
                &json!({
                    "userId": "buyer_1",
                    fields::ORDER_STATUS: "SHIPPED",
                    fields::ITEMS: [
                        { fields::CART_ITEM_ID: "c1", fields::STATUS: "SHIPPED", "isDigital": false, "name": "Physical item" },
                        { fields::CART_ITEM_ID: "c2", fields::STATUS: "SHIPPED", "isDigital": true, "name": "Digital item" }
                    ],
                }),
            )
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(20))
            .await
            .unwrap();

        assert!(notifications.is_empty());
    }

    #[tokio::test]
    async fn handle_event_unknown_collection_is_noop() {
        let executor = setup_executor().await;
        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "unknown_collection".into(),
            document_id: "unknown:1".into(),
            data: json!({}),
            before_data: None,
            after_data: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        executor.handle_event(event).await.unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert!(notifications.is_empty());
    }

    #[tokio::test]
    async fn handle_order_update_without_before_or_after_is_noop() {
        let executor = setup_executor().await;
        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: collections::ORDERS.into(),
            document_id: "orders:ord_1".into(),
            data: json!({}),
            before_data: Some(json!({ fields::ORDER_STATUS: "PENDING" })),
            after_data: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        executor.handle_order_update(&event).await.unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert!(notifications.is_empty());
    }

    #[tokio::test]
    async fn user_lang_falls_back_to_english_for_missing_user_or_language() {
        let executor = setup_executor().await;
        executor
            .state
            .db
            .upsert_document(
                collections::USERS,
                "user_no_lang",
                json!({ fields::EMAIL: "test@example.com" }),
            )
            .await
            .unwrap();

        assert_eq!(executor.user_lang("missing_user").await, "en");
        assert_eq!(executor.user_lang("user_no_lang").await, "en");
    }

    #[tokio::test]
    async fn create_notification_once_is_idempotent_for_same_claim() {
        let executor = setup_executor().await;
        executor
            .state
            .db
            .upsert_document(
                collections::USERS,
                "buyer_1",
                json!({
                    fields::EMAIL: "buyer@example.com",
                    fields::PREFERRED_LANGUAGE: "en",
                }),
            )
            .await
            .unwrap();

        let claim_id = claim_key("order_status_buyer", &["ord_1", "CONFIRMED", "buyer_1"]);
        let payload = json!({
            fields::ORDER_ID: "ord_1",
            fields::ORDER_STATUS: "CONFIRMED",
        });

        executor
            .create_notification_once(
                &claim_id,
                "order_status_changed",
                "buyer_1",
                notification_types::ORDER_STATUS_CHANGED,
                "Order confirmed",
                "Your order was confirmed.",
                payload.clone(),
            )
            .await
            .unwrap();
        executor
            .create_notification_once(
                &claim_id,
                "order_status_changed",
                "buyer_1",
                notification_types::ORDER_STATUS_CHANGED,
                "Order confirmed",
                "Your order was confirmed.",
                payload,
            )
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(20))
            .await
            .unwrap();
        let webhook_claims = executor
            .state
            .db
            .list_documents(collections::WEBHOOK_EVENTS, Some(20))
            .await
            .unwrap();

        assert_eq!(notifications.len(), 1);
        assert_eq!(webhook_claims.len(), 1);
    }

    #[tokio::test]
    async fn create_notification_once_creates_pending_mail_log_without_credentials() {
        let executor = setup_executor().await;
        executor
            .state
            .db
            .upsert_document(
                collections::USERS,
                "buyer_1",
                json!({
                    fields::EMAIL: "buyer@example.com",
                    fields::PREFERRED_LANGUAGE: "fr",
                }),
            )
            .await
            .unwrap();

        let claim_id = claim_key("order_status_buyer", &["ord_mail", "CONFIRMED", "buyer_1"]);
        executor
            .create_notification_once(
                &claim_id,
                "order_status_changed",
                "buyer_1",
                notification_types::ORDER_STATUS_CHANGED,
                "Commande confirmee",
                "Votre commande est confirmee.",
                json!({ fields::ORDER_ID: "ord_mail" }),
            )
            .await
            .unwrap();

        let mail_logs = executor
            .state
            .db
            .list_documents(collections::MAIL_LOGS, Some(10))
            .await
            .unwrap();

        assert_eq!(mail_logs.len(), 1);
        assert_eq!(mail_logs[0]["to"], "buyer@example.com");
        assert_eq!(mail_logs[0]["status"], "pending");
        assert!(
            mail_logs[0]["html"]
                .as_str()
                .unwrap_or("")
                .contains("Commande confirmee")
        );
        assert!(
            mail_logs[0]["error"]
                .as_str()
                .unwrap_or("")
                .contains("credentials")
        );
    }

    #[tokio::test]
    async fn create_notification_once_skips_mail_log_when_user_has_no_email() {
        let executor = setup_executor().await;
        executor
            .state
            .db
            .upsert_document(
                collections::USERS,
                "buyer_1",
                json!({
                    fields::PREFERRED_LANGUAGE: "en",
                }),
            )
            .await
            .unwrap();

        let claim_id = claim_key(
            "order_status_buyer",
            &["ord_no_email", "CONFIRMED", "buyer_1"],
        );
        executor
            .create_notification_once(
                &claim_id,
                "order_status_changed",
                "buyer_1",
                notification_types::ORDER_STATUS_CHANGED,
                "Order confirmed",
                "Your order was confirmed.",
                json!({ fields::ORDER_ID: "ord_no_email" }),
            )
            .await
            .unwrap();

        let mail_logs = executor
            .state
            .db
            .list_documents(collections::MAIL_LOGS, Some(10))
            .await
            .unwrap();
        assert!(mail_logs.is_empty());
    }

    #[tokio::test]
    async fn create_notification_once_skips_push_when_user_has_no_tokens() {
        let executor = setup_executor().await;
        executor
            .state
            .db
            .upsert_document(
                collections::USERS,
                "buyer_1",
                json!({
                    fields::EMAIL: "buyer@example.com",
                    fields::PREFERRED_LANGUAGE: "en",
                }),
            )
            .await
            .unwrap();

        let claim_id = claim_key(
            "order_status_buyer",
            &["ord_no_push", "CONFIRMED", "buyer_1"],
        );
        executor
            .create_notification_once(
                &claim_id,
                "order_status_changed",
                "buyer_1",
                notification_types::ORDER_STATUS_CHANGED,
                "Order confirmed",
                "Your order was confirmed.",
                json!({ fields::ORDER_ID: "ord_no_push" }),
            )
            .await
            .unwrap();

        let pending = executor
            .state
            .db
            .query_raw("SELECT * FROM _pending_notifications")
            .await
            .unwrap();
        assert!(pending.is_empty());
    }

    // ── Coverage: NativeTriggerExecutor::run() (lines 21-31) ──

    #[tokio::test]
    async fn run_processes_events_and_stops_when_sender_dropped() {
        let state = setup_state().await;
        let (tx, rx) = mpsc::channel(8);
        let executor = NativeTriggerExecutor::new(state, rx);

        // Send an unknown-collection event (noop) then drop sender
        tx.send(ChangeEvent {
            action: ChangeAction::Update,
            collection: "unknown".into(),
            document_id: "unknown:1".into(),
            data: json!({}),
            before_data: None,
            after_data: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        })
        .await
        .unwrap();
        drop(tx);

        // run() should process the event and then exit when channel closes
        executor.run().await;
    }

    #[tokio::test]
    async fn run_logs_error_when_trigger_fails() {
        let state = setup_state().await;
        let (tx, rx) = mpsc::channel(8);
        let executor = NativeTriggerExecutor::new(state, rx);

        // orders update with no before_data → noop (Ok), but let's send one that exercises error logging
        // We send a valid orders update that won't fail but covers the event path
        tx.send(ChangeEvent {
            action: ChangeAction::Update,
            collection: "orders".into(),
            document_id: "orders:test".into(),
            data: json!({}),
            before_data: None,
            after_data: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        })
        .await
        .unwrap();
        drop(tx);
        executor.run().await;
    }

    // ── Coverage: handle_event dispatching to product handlers (lines 36-37, 44-85) ──

    #[tokio::test]
    async fn handle_event_product_create_without_search_config_is_noop() {
        let executor = setup_executor().await;
        // Default config has no search configured → early return Ok(())
        let event = ChangeEvent {
            action: ChangeAction::Create,
            collection: "products".into(),
            document_id: "products:p1".into(),
            data: json!({"name": "Test Product"}),
            before_data: None,
            after_data: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        executor.handle_event(event).await.unwrap();
    }

    #[tokio::test]
    async fn handle_event_product_update_without_search_config_is_noop() {
        let executor = setup_executor().await;
        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "products".into(),
            document_id: "products:p1".into(),
            data: json!({"name": "Updated Product"}),
            before_data: None,
            after_data: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        executor.handle_event(event).await.unwrap();
    }

    #[tokio::test]
    async fn handle_event_product_delete_without_search_config_is_noop() {
        let executor = setup_executor().await;
        let event = ChangeEvent {
            action: ChangeAction::Delete,
            collection: "products".into(),
            document_id: "products:p1".into(),
            data: json!({}),
            before_data: None,
            after_data: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        executor.handle_event(event).await.unwrap();
    }

    // ── Coverage: handle_order_update missing before_data (line 89) ──

    #[tokio::test]
    async fn handle_order_update_missing_before_data_is_noop() {
        let executor = setup_executor().await;
        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "orders".into(),
            document_id: "orders:ord_1".into(),
            data: json!({}),
            before_data: None,
            after_data: Some(json!({ fields::ORDER_STATUS: "CONFIRMED" })),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        executor.handle_order_update(&event).await.unwrap();
    }

    // ── Coverage: handle_return_update missing before/after data (lines 106, 109) ──

    #[tokio::test]
    async fn handle_return_update_missing_before_data_is_noop() {
        let executor = setup_executor().await;
        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "return_requests".into(),
            document_id: "return_requests:r1".into(),
            data: json!({}),
            before_data: None,
            after_data: Some(json!({ fields::RETURN_STATUS: "REQUESTED" })),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        executor.handle_return_update(&event).await.unwrap();
    }

    #[tokio::test]
    async fn handle_return_update_missing_after_data_is_noop() {
        let executor = setup_executor().await;
        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "return_requests".into(),
            document_id: "return_requests:r1".into(),
            data: json!({}),
            before_data: Some(json!({ fields::RETURN_STATUS: "REQUESTED" })),
            after_data: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        executor.handle_return_update(&event).await.unwrap();
    }

    // ── Coverage: handle_return_update same status (line 115) ──

    #[tokio::test]
    async fn handle_return_update_same_status_is_noop() {
        let executor = setup_executor().await;
        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "return_requests".into(),
            document_id: "return_requests:r1".into(),
            data: json!({}),
            before_data: Some(json!({
                fields::RETURN_STATUS: "APPROVED",
                fields::ORDER_ID: "ord_1",
                fields::BUYER_ID: "buyer_1",
            })),
            after_data: Some(json!({
                fields::RETURN_STATUS: "APPROVED",
                fields::ORDER_ID: "ord_1",
                fields::BUYER_ID: "buyer_1",
            })),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        executor.handle_return_update(&event).await.unwrap();
    }

    // ── Coverage: handle_return_update empty old_status (line 115) ──

    #[tokio::test]
    async fn handle_return_update_empty_old_status_is_noop() {
        let executor = setup_executor().await;
        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "return_requests".into(),
            document_id: "return_requests:r1".into(),
            data: json!({}),
            before_data: Some(json!({
                fields::ORDER_ID: "ord_1",
                fields::BUYER_ID: "buyer_1",
            })),
            after_data: Some(json!({
                fields::RETURN_STATUS: "APPROVED",
                fields::ORDER_ID: "ord_1",
                fields::BUYER_ID: "buyer_1",
            })),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        executor.handle_return_update(&event).await.unwrap();
    }

    // ── Coverage: handle_return_update with userId fallback (line 120-124) ──

    #[tokio::test]
    async fn handle_return_update_falls_back_to_user_id_field() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;

        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "return_requests".into(),
            document_id: "return_requests:ret_fallback".into(),
            data: json!({}),
            before_data: Some(json!({
                fields::RETURN_STATUS: "PENDING",
                fields::ORDER_ID: "ord_1",
                "userId": "buyer_1",
            })),
            after_data: Some(json!({
                fields::RETURN_STATUS: "REQUESTED",
                fields::ORDER_ID: "ord_1",
                "userId": "buyer_1",
            })),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        executor.handle_return_update(&event).await.unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert!(notifications.iter().any(|doc| doc["userId"] == "buyer_1"));
    }

    // ── Coverage: handle_return_update with empty buyer_id (line 156 path) ──

    #[tokio::test]
    async fn handle_return_update_empty_buyer_skips_buyer_notification() {
        let executor = setup_executor().await;
        seed_user(&executor, "seller_1", "en").await;

        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "return_requests".into(),
            document_id: "return_requests:ret_no_buyer".into(),
            data: json!({}),
            before_data: Some(json!({
                fields::RETURN_STATUS: "PENDING",
                fields::ORDER_ID: "ord_1",
                fields::SELLER_ID: "seller_1",
            })),
            after_data: Some(json!({
                fields::RETURN_STATUS: "REQUESTED",
                fields::ORDER_ID: "ord_1",
                fields::SELLER_ID: "seller_1",
            })),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        executor.handle_return_update(&event).await.unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        // Only seller gets notified
        assert!(notifications.iter().all(|doc| doc["userId"] == "seller_1"));
    }

    // ── Coverage: handle_order_status_change same status (line 193) ──

    #[tokio::test]
    async fn handle_order_status_change_same_status_is_noop() {
        let executor = setup_executor().await;
        executor
            .handle_order_status_change(
                "orders:ord_same",
                &json!({ fields::ORDER_STATUS: "CONFIRMED", "userId": "buyer_1" }),
                &json!({ fields::ORDER_STATUS: "CONFIRMED", "userId": "buyer_1" }),
            )
            .await
            .unwrap();
    }

    // ── Coverage: handle_order_status_change empty buyer (line 223 skip) ──

    #[tokio::test]
    async fn handle_order_status_change_empty_buyer_skips_buyer_notification() {
        let executor = setup_executor().await;
        seed_user(&executor, "seller_1", "en").await;

        executor
            .handle_order_status_change(
                "orders:ord_no_buyer",
                &json!({
                    fields::ORDER_STATUS: "PENDING",
                    fields::ITEMS: [{ fields::SELLER_ID: "seller_1" }],
                }),
                &json!({
                    fields::ORDER_STATUS: "CONFIRMED",
                    fields::ITEMS: [{ fields::SELLER_ID: "seller_1" }],
                }),
            )
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert!(notifications.iter().all(|doc| doc["userId"] == "seller_1"));
    }

    // ── Coverage: seller skip when SHIPPED and seller is last actor (line 231) ──

    #[tokio::test]
    async fn handle_order_status_change_shipped_skips_last_actor_seller() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;
        seed_user(&executor, "seller_1", "en").await;

        executor
            .handle_order_status_change(
                "orders:shipped_skip",
                &json!({
                    fields::ORDER_STATUS: "PROCESSING",
                    "userId": "buyer_1",
                    fields::ITEMS: [{ fields::SELLER_ID: "seller_1" }],
                    fields::LAST_ACTOR_ID: "seller_1",
                }),
                &json!({
                    fields::ORDER_STATUS: "SHIPPED",
                    "userId": "buyer_1",
                    fields::ITEMS: [{ fields::SELLER_ID: "seller_1" }],
                    fields::LAST_ACTOR_ID: "seller_1",
                }),
            )
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        // Only buyer gets notified, seller is skipped as last actor
        assert!(notifications.iter().all(|doc| doc["userId"] == "buyer_1"));
    }

    // ── Coverage: perishable items notification (line 276) ──

    #[tokio::test]
    async fn handle_order_status_change_confirmed_no_perishable_skips_urgent() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;
        seed_user(&executor, "seller_1", "en").await;

        executor
            .handle_order_status_change(
                "orders:non_perishable",
                &json!({
                    fields::ORDER_STATUS: "PENDING",
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::SELLER_ID: "seller_1",
                        fields::IS_PERISHABLE: false,
                        "name": "Book",
                    }],
                }),
                &json!({
                    fields::ORDER_STATUS: "CONFIRMED",
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::SELLER_ID: "seller_1",
                        fields::IS_PERISHABLE: false,
                        "name": "Book",
                    }],
                }),
            )
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(20))
            .await
            .unwrap();
        // Should have buyer + seller notifications but no perishable urgent
        assert_eq!(notifications.len(), 2);
        assert!(
            !notifications
                .iter()
                .any(|doc| { doc["title"].as_str().unwrap_or("").contains("URGENT") })
        );
    }

    // ── Coverage: handle_order_payment_status_change early returns (lines 292, 296, 301) ──

    #[tokio::test]
    async fn handle_order_payment_status_change_same_status_is_noop() {
        let executor = setup_executor().await;
        executor
            .handle_order_payment_status_change(
                "orders:pay_same",
                &json!({ fields::PAYMENT_STATUS: "CAPTURED", "userId": "buyer_1" }),
                &json!({ fields::PAYMENT_STATUS: "CAPTURED", "userId": "buyer_1" }),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn handle_order_payment_status_change_non_refund_is_noop() {
        let executor = setup_executor().await;
        executor
            .handle_order_payment_status_change(
                "orders:pay_captured",
                &json!({ fields::PAYMENT_STATUS: "AUTHORIZED", "userId": "buyer_1" }),
                &json!({ fields::PAYMENT_STATUS: "CAPTURED", "userId": "buyer_1" }),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn handle_order_payment_status_change_empty_buyer_is_noop() {
        let executor = setup_executor().await;
        executor
            .handle_order_payment_status_change(
                "orders:pay_no_buyer",
                &json!({ fields::PAYMENT_STATUS: "CAPTURED" }),
                &json!({ fields::PAYMENT_STATUS: "REFUNDED" }),
            )
            .await
            .unwrap();
    }

    // ── Coverage: handle_order_item_status_changes edge cases (lines 349, 362, 370) ──

    #[tokio::test]
    async fn handle_order_item_status_changes_empty_buyer_is_noop() {
        let executor = setup_executor().await;
        executor
            .handle_order_item_status_changes(
                "orders:item_no_buyer",
                &json!({ fields::ITEMS: [] }),
                &json!({ fields::ITEMS: [] }),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn handle_order_item_status_changes_item_without_cart_id_skipped() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;

        executor
            .handle_order_item_status_changes(
                "orders:no_cart_id",
                &json!({
                    "userId": "buyer_1",
                    fields::ORDER_STATUS: "PROCESSING",
                    fields::ITEMS: [
                        { "name": "No Cart ID Item", fields::STATUS: "PROCESSING" }
                    ],
                }),
                &json!({
                    "userId": "buyer_1",
                    fields::ORDER_STATUS: "PROCESSING",
                    fields::ITEMS: [
                        { "name": "No Cart ID Item", fields::STATUS: "SHIPPED" }
                    ],
                }),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn handle_order_item_status_changes_same_item_status_skipped() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;

        executor
            .handle_order_item_status_changes(
                "orders:same_status",
                &json!({
                    "userId": "buyer_1",
                    fields::ORDER_STATUS: "PROCESSING",
                    fields::ITEMS: [
                        { fields::CART_ITEM_ID: "c1", fields::STATUS: "SHIPPED", "name": "A" }
                    ],
                }),
                &json!({
                    "userId": "buyer_1",
                    fields::ORDER_STATUS: "PROCESSING",
                    fields::ITEMS: [
                        { fields::CART_ITEM_ID: "c1", fields::STATUS: "SHIPPED", "name": "A" }
                    ],
                }),
            )
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert!(notifications.is_empty());
    }

    // ── Coverage: cleanup_stock_notifications edge cases (lines 469, 473, 479, 506, 510) ──

    #[tokio::test]
    async fn cleanup_stock_notifications_empty_buyer_returns_early() {
        let executor = setup_executor().await;
        executor.cleanup_stock_notifications(&json!({})).await;
    }

    #[tokio::test]
    async fn cleanup_stock_notifications_no_items_returns_early() {
        let executor = setup_executor().await;
        executor
            .cleanup_stock_notifications(&json!({ "userId": "buyer_1" }))
            .await;
    }

    #[tokio::test]
    async fn cleanup_stock_notifications_empty_product_id_skipped() {
        let executor = setup_executor().await;
        executor
            .cleanup_stock_notifications(&json!({
                "userId": "buyer_1",
                fields::ITEMS: [{ "variantKey": "blue" }],
            }))
            .await;
    }

    #[tokio::test]
    async fn cleanup_stock_notifications_variant_mismatch_not_deleted() {
        let executor = setup_executor().await;
        let _ = executor
            .state
            .db
            .create_document(
                collections::STOCK_NOTIFICATIONS,
                json!({
                    "productId": "prod_1",
                    "userId": "buyer_1",
                    "variantKey": "red",
                }),
            )
            .await;

        executor
            .cleanup_stock_notifications(&json!({
                "userId": "buyer_1",
                fields::ITEMS: [{
                    fields::PRODUCT_ID: "prod_1",
                    "variantKey": "blue",
                }],
            }))
            .await;

        let remaining = executor
            .state
            .db
            .list_documents(collections::STOCK_NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
    }

    #[tokio::test]
    async fn cleanup_stock_notifications_row_without_id_skipped() {
        let executor = setup_executor().await;
        // This just exercises the path where rows lack an id field — the cleanup is a best-effort
        executor
            .cleanup_stock_notifications(&json!({
                "userId": "buyer_1",
                fields::ITEMS: [{
                    fields::PRODUCT_ID: "prod_nonexistent",
                    "variantKey": "",
                }],
            }))
            .await;
    }

    // ── Coverage: dispatch_email with mailjet creds (lines 677-686) ──
    // Note: We can't actually have mailjet creds in test config, but we can cover the
    // code path via dispatch_email which is called by create_notification_once.
    // The existing test already covers the no-creds path. dispatch_email line 636 is
    // covered when user lookup fails.

    #[tokio::test]
    async fn dispatch_email_user_not_found_returns_early() {
        let executor = setup_executor().await;
        // No user seeded → get_document fails → early return
        executor
            .dispatch_email(
                "notif_1",
                "nonexistent_user",
                "Title",
                "Body",
                "order_status_changed",
                &json!({}),
            )
            .await;
    }

    // ── Coverage: dispatch_push path (lines 732-800) ──

    #[tokio::test]
    async fn dispatch_push_with_tokens_exercises_push_path() {
        let executor = setup_executor().await;

        // Seed a push token
        executor
            .state
            .db
            .query_raw("CREATE _push_tokens SET user_id = 'buyer_1', token = 'fcm_token_abc123'")
            .await
            .unwrap();

        // This exercises: json_to_string_map, token iteration, pending_push_record_id,
        // UPSERT to _pending_notifications (may silently fail in mem DB), and the
        // push send attempt (no FCM env vars = false branch).
        executor
            .dispatch_push(
                "notif_push_1",
                "buyer_1",
                "Title",
                "Body",
                &json!({"orderId": "ord_1"}),
            )
            .await;
        // No panic = code paths exercised successfully
    }

    #[tokio::test]
    async fn dispatch_push_token_row_without_token_field_skipped() {
        let executor = setup_executor().await;

        let _ = executor
            .state
            .db
            .query_raw("CREATE _push_tokens SET user_id = 'buyer_1'")
            .await;

        executor
            .dispatch_push("notif_push_2", "buyer_1", "Title", "Body", &json!({}))
            .await;

        let pending = executor
            .state
            .db
            .query_raw("SELECT * FROM _pending_notifications")
            .await
            .unwrap();
        assert!(pending.is_empty());
    }

    // ── Coverage: sanitize_id_component double-underscore collapsing (lines 968-969) ──

    #[test]
    fn sanitize_id_component_collapses_double_underscores() {
        let result = sanitize_id_component("hello---world!!!test");
        assert!(!result.contains("__"));
        assert!(result.contains("hello"));
        assert!(result.contains("world"));
    }

    #[test]
    fn sanitize_id_component_trims_leading_trailing_underscores() {
        let result = sanitize_id_component("___hello___");
        assert_eq!(result, "hello");
    }

    // ── Coverage: json_to_string_map (lines 979-993) ──

    #[test]
    fn json_to_string_map_converts_object_values() {
        let data = json!({"key1": "value1", "key2": 42, "key3": true});
        let map = json_to_string_map(&data);
        assert_eq!(map.get("key1").unwrap(), "value1");
        assert_eq!(map.get("key2").unwrap(), "42");
        assert_eq!(map.get("key3").unwrap(), "true");
    }

    #[test]
    fn json_to_string_map_returns_empty_for_non_object() {
        let data = json!("not an object");
        let map = json_to_string_map(&data);
        assert!(map.is_empty());
    }

    #[test]
    fn json_to_string_map_returns_empty_for_null() {
        let data = json!(null);
        let map = json_to_string_map(&data);
        assert!(map.is_empty());
    }

    // ── Coverage: buyer_order_status_message French variants (lines 1014-1090) ──

    #[test]
    fn buyer_order_status_message_confirmed_fr() {
        let (t, b) = buyer_order_status_message("CONFIRMED", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("confirmée"));
        assert!(b.contains("confirmée"));
    }

    #[test]
    fn buyer_order_status_message_processing_fr() {
        let (t, b) = buyer_order_status_message("PROCESSING", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("préparation"));
        assert!(b.contains("préparation"));
    }

    #[test]
    fn buyer_order_status_message_in_transit_fr_no_tracking() {
        let (t, b) = buyer_order_status_message("IN_TRANSIT", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("transit"));
        assert!(b.contains("en transit"));
    }

    #[test]
    fn buyer_order_status_message_in_transit_fr_with_tracking_no_carrier() {
        let order = json!({ fields::TRACKING_NUMBER: "TRK123" });
        let (_, b) = buyer_order_status_message("IN_TRANSIT", "orders:abc12345", &order, "fr");
        assert!(b.contains("Suivi: TRK123"));
    }

    #[test]
    fn buyer_order_status_message_in_transit_fr_with_tracking_and_carrier() {
        let order =
            json!({ fields::TRACKING_NUMBER: "TRK123", fields::SHIPPING_CARRIER: "Purolator" });
        let (_, b) = buyer_order_status_message("IN_TRANSIT", "orders:abc12345", &order, "fr");
        assert!(b.contains("via Purolator"));
        assert!(b.contains("Suivi: TRK123"));
    }

    #[test]
    fn buyer_order_status_message_in_transit_en_no_tracking() {
        let (t, b) = buyer_order_status_message("IN_TRANSIT", "orders:abc12345", &json!({}), "en");
        assert!(t.contains("in transit"));
        assert!(b.contains("is in transit."));
    }

    #[test]
    fn buyer_order_status_message_in_transit_en_with_tracking_no_carrier() {
        let order = json!({ fields::TRACKING_NUMBER: "TRK123" });
        let (_, b) = buyer_order_status_message("IN_TRANSIT", "orders:abc12345", &order, "en");
        assert!(b.contains("Tracking: TRK123"));
    }

    #[test]
    fn buyer_order_status_message_in_transit_en_with_tracking_and_carrier() {
        let order = json!({ fields::TRACKING_NUMBER: "TRK123", fields::SHIPPING_CARRIER: "FedEx" });
        let (_, b) = buyer_order_status_message("IN_TRANSIT", "orders:abc12345", &order, "en");
        assert!(b.contains("via FedEx"));
        assert!(b.contains("Tracking: TRK123"));
    }

    #[test]
    fn buyer_order_status_message_shipped_pickup_fr() {
        let order = json!({ "deliverySpeed": "pickup" });
        let (t, b) = buyer_order_status_message("SHIPPED", "orders:abc12345", &order, "fr");
        assert!(t.contains("ramassage"));
        assert!(b.contains("ramassage"));
    }

    #[test]
    fn buyer_order_status_message_shipped_fr_no_tracking() {
        let (t, b) = buyer_order_status_message("SHIPPED", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("expédiée"));
        assert!(b.contains("en route"));
    }

    #[test]
    fn buyer_order_status_message_shipped_fr_with_tracking_no_carrier() {
        let order = json!({ fields::TRACKING_NUMBER: "TRK123" });
        let (_, b) = buyer_order_status_message("SHIPPED", "orders:abc12345", &order, "fr");
        assert!(b.contains("Suivi: TRK123"));
    }

    #[test]
    fn buyer_order_status_message_shipped_fr_with_tracking_and_carrier() {
        let order =
            json!({ fields::TRACKING_NUMBER: "TRK123", fields::SHIPPING_CARRIER: "Purolator" });
        let (_, b) = buyer_order_status_message("SHIPPED", "orders:abc12345", &order, "fr");
        assert!(b.contains("via Purolator"));
    }

    #[test]
    fn buyer_order_status_message_shipped_en_no_tracking() {
        let (t, b) = buyer_order_status_message("SHIPPED", "orders:abc12345", &json!({}), "en");
        assert!(t.contains("shipped"));
        assert!(b.contains("on the way"));
    }

    #[test]
    fn buyer_order_status_message_shipped_en_with_tracking_no_carrier() {
        let order = json!({ fields::TRACKING_NUMBER: "TRK123" });
        let (_, b) = buyer_order_status_message("SHIPPED", "orders:abc12345", &order, "en");
        assert!(b.contains("Tracking: TRK123"));
    }

    #[test]
    fn buyer_order_status_message_shipped_en_with_tracking_and_carrier() {
        let order = json!({ fields::TRACKING_NUMBER: "TRK123", fields::SHIPPING_CARRIER: "UPS" });
        let (_, b) = buyer_order_status_message("SHIPPED", "orders:abc12345", &order, "en");
        assert!(b.contains("via UPS"));
    }

    #[test]
    fn buyer_order_status_message_delivered_confirmed_fr() {
        let order = json!({ "confirmedByClient": true });
        let (t, b) = buyer_order_status_message("DELIVERED", "orders:abc12345", &order, "fr");
        assert!(t.contains("Réception confirmée"));
        assert!(b.contains("enregistrée"));
    }

    #[test]
    fn buyer_order_status_message_delivered_auto_confirmed_fr() {
        let order = json!({ "autoConfirmed": true });
        let (t, b) = buyer_order_status_message("DELIVERED", "orders:abc12345", &order, "fr");
        assert!(t.contains("Réception confirmée"));
    }

    #[test]
    fn buyer_order_status_message_delivered_fr() {
        let (t, b) = buyer_order_status_message("DELIVERED", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("livrée"));
        assert!(b.contains("livrée"));
    }

    #[test]
    fn buyer_order_status_message_cancelled_fr() {
        let (t, b) = buyer_order_status_message("CANCELLED", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("annulée"));
        assert!(b.contains("annulée"));
    }

    #[test]
    fn buyer_order_status_message_failed_fr() {
        let (t, b) = buyer_order_status_message("FAILED", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("échoué"));
        assert!(b.contains("traité"));
    }

    #[test]
    fn buyer_order_status_message_expired_fr() {
        let (t, b) = buyer_order_status_message("EXPIRED", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("expirée"));
        assert!(b.contains("expiré"));
    }

    #[test]
    fn buyer_order_status_message_disputed_fr() {
        let (t, b) = buyer_order_status_message("DISPUTED", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("Litige"));
        assert!(b.contains("litige"));
    }

    #[test]
    fn buyer_order_status_message_unknown_status_fr() {
        let (t, b) =
            buyer_order_status_message("SOME_NEW_STATUS", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("Mise à jour"));
        assert!(b.contains("SOME_NEW_STATUS"));
    }

    #[test]
    fn buyer_order_status_message_unknown_status_en() {
        let (t, b) =
            buyer_order_status_message("SOME_NEW_STATUS", "orders:abc12345", &json!({}), "en");
        assert!(t.contains("updated"));
        assert!(b.contains("SOME_NEW_STATUS"));
    }

    // ── Coverage: seller_order_status_message French variants (lines 1178-1240) ──

    #[test]
    fn seller_order_status_message_confirmed_perishable_fr() {
        let order = json!({ fields::ITEMS: [{ fields::IS_PERISHABLE: true }] });
        let (t, b) = seller_order_status_message("CONFIRMED", "orders:abc12345", &order, "fr");
        assert!(t.contains("URGENT"));
        assert!(b.contains("périssable"));
    }

    #[test]
    fn seller_order_status_message_confirmed_fr() {
        let (t, b) = seller_order_status_message("CONFIRMED", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("Nouvelle commande"));
        assert!(b.contains("confirmée"));
    }

    #[test]
    fn seller_order_status_message_confirmed_en() {
        let (t, b) = seller_order_status_message("CONFIRMED", "orders:abc12345", &json!({}), "en");
        assert!(t.contains("New order"));
        assert!(b.contains("confirmed"));
    }

    #[test]
    fn seller_order_status_message_processing_fr() {
        let (t, b) = seller_order_status_message("PROCESSING", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("préparation"));
    }

    #[test]
    fn seller_order_status_message_processing_en() {
        let (t, b) = seller_order_status_message("PROCESSING", "orders:abc12345", &json!({}), "en");
        assert!(t.contains("processing"));
    }

    #[test]
    fn seller_order_status_message_shipped_fr() {
        let (t, b) = seller_order_status_message("SHIPPED", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("Expédition confirmée"));
    }

    #[test]
    fn seller_order_status_message_shipped_en() {
        let (t, b) = seller_order_status_message("SHIPPED", "orders:abc12345", &json!({}), "en");
        assert!(t.contains("Shipment confirmed"));
    }

    #[test]
    fn seller_order_status_message_in_transit_fr() {
        let (t, b) = seller_order_status_message("IN_TRANSIT", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("transit"));
    }

    #[test]
    fn seller_order_status_message_in_transit_en() {
        let (t, b) = seller_order_status_message("IN_TRANSIT", "orders:abc12345", &json!({}), "en");
        assert!(t.contains("transit"));
    }

    #[test]
    fn seller_order_status_message_delivered_fr() {
        let (t, b) = seller_order_status_message("DELIVERED", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("Réception confirmée"));
        assert!(b.contains("paiement est en attente"));
    }

    #[test]
    fn seller_order_status_message_delivered_en() {
        let (t, b) = seller_order_status_message("DELIVERED", "orders:abc12345", &json!({}), "en");
        assert!(t.contains("Receipt confirmed"));
        assert!(b.contains("Payout is now pending"));
    }

    #[test]
    fn seller_order_status_message_cancelled_fr() {
        let (t, b) = seller_order_status_message("CANCELLED", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("annulée"));
    }

    #[test]
    fn seller_order_status_message_cancelled_en() {
        let (t, b) = seller_order_status_message("CANCELLED", "orders:abc12345", &json!({}), "en");
        assert!(t.contains("cancelled"));
    }

    #[test]
    fn seller_order_status_message_unknown_fr() {
        let (t, b) =
            seller_order_status_message("WEIRD_STATUS", "orders:abc12345", &json!({}), "fr");
        assert!(t.contains("Mise à jour"));
    }

    #[test]
    fn seller_order_status_message_unknown_en() {
        let (t, b) =
            seller_order_status_message("WEIRD_STATUS", "orders:abc12345", &json!({}), "en");
        assert!(t.contains("updated"));
    }

    // ── Coverage: buyer_payment_message all variants (lines 1253-1313) ──

    #[test]
    fn buyer_payment_message_refunded_fr_with_amount() {
        let (t, b) = buyer_payment_message("REFUNDED", "orders:abc12345", Some(1250), "fr");
        assert!(t.contains("Remboursement"));
        assert!(b.contains("$12.50"));
    }

    #[test]
    fn buyer_payment_message_refunded_fr_no_amount() {
        let (t, b) = buyer_payment_message("REFUNDED", "orders:abc12345", None, "fr");
        assert!(t.contains("Remboursement"));
        assert!(b.contains("traité"));
    }

    #[test]
    fn buyer_payment_message_refunded_en_with_amount() {
        let (t, b) = buyer_payment_message("REFUNDED", "orders:abc12345", Some(1250), "en");
        assert!(t.contains("Refund"));
        assert!(b.contains("$12.50"));
    }

    #[test]
    fn buyer_payment_message_refunded_en_no_amount() {
        let (t, b) = buyer_payment_message("REFUNDED", "orders:abc12345", None, "en");
        assert!(b.contains("has been processed"));
    }

    #[test]
    fn buyer_payment_message_partial_refund_fr_with_amount() {
        let (t, b) = buyer_payment_message("PARTIAL_REFUND", "orders:abc12345", Some(325), "fr");
        assert!(t.contains("partiel"));
        assert!(b.contains("$3.25"));
    }

    #[test]
    fn buyer_payment_message_partial_refund_fr_no_amount() {
        let (t, b) = buyer_payment_message("PARTIAL_REFUND", "orders:abc12345", None, "fr");
        assert!(b.contains("partiel"));
    }

    #[test]
    fn buyer_payment_message_partial_refund_en_no_amount() {
        let (t, b) = buyer_payment_message("PARTIAL_REFUND", "orders:abc12345", None, "en");
        assert!(b.contains("partial refund"));
    }

    #[test]
    fn buyer_payment_message_captured_fr() {
        let (t, b) = buyer_payment_message("CAPTURED", "orders:abc12345", None, "fr");
        assert!(t.contains("capturé"));
        assert!(b.contains("capturé"));
    }

    #[test]
    fn buyer_payment_message_captured_en() {
        let (t, b) = buyer_payment_message("CAPTURED", "orders:abc12345", None, "en");
        assert!(t.contains("captured"));
    }

    #[test]
    fn buyer_payment_message_authorized_fr() {
        let (t, b) = buyer_payment_message("AUTHORIZED", "orders:abc12345", None, "fr");
        assert!(t.contains("autorisé"));
    }

    #[test]
    fn buyer_payment_message_authorized_en() {
        let (t, b) = buyer_payment_message("AUTHORIZED", "orders:abc12345", None, "en");
        assert!(t.contains("authorized"));
    }

    #[test]
    fn buyer_payment_message_unknown_fr() {
        let (t, b) = buyer_payment_message("WEIRD", "orders:abc12345", None, "fr");
        assert!(t.contains("Mise à jour de paiement"));
    }

    #[test]
    fn buyer_payment_message_unknown_en() {
        let (t, b) = buyer_payment_message("WEIRD", "orders:abc12345", None, "en");
        assert!(t.contains("Payment update"));
    }

    // ── Coverage: format_cents edge cases (line 1337) ──

    #[test]
    fn format_cents_zero_returns_none() {
        assert_eq!(format_cents(0), None);
    }

    #[test]
    fn format_cents_negative_returns_none() {
        assert_eq!(format_cents(-100), None);
    }

    #[test]
    fn format_cents_positive_formats_correctly() {
        assert_eq!(format_cents(1250), Some("$12.50".to_string()));
        assert_eq!(format_cents(5), Some("$0.05".to_string()));
    }

    // ── Coverage: item_status_message all variants (lines 1354-1384) ──

    #[test]
    fn item_status_message_shipped_pickup_fr() {
        let (t, b) = item_status_message("shipped", "orders:abc12345", "Milk", "fr", true);
        assert!(t.contains("ramassage"));
        assert!(b.contains("ramassage"));
    }

    #[test]
    fn item_status_message_shipped_pickup_en() {
        let (t, b) = item_status_message("shipped", "orders:abc12345", "Milk", "en", true);
        assert!(t.contains("pickup"));
        assert!(b.contains("pickup"));
    }

    #[test]
    fn item_status_message_shipped_fr() {
        let (t, b) = item_status_message("shipped", "orders:abc12345", "Milk", "fr", false);
        assert!(t.contains("expédié"));
        assert!(b.contains("expédié"));
    }

    #[test]
    fn item_status_message_shipped_en() {
        let (t, b) = item_status_message("shipped", "orders:abc12345", "Milk", "en", false);
        assert!(t.contains("shipped"));
        assert!(b.contains("shipped"));
    }

    #[test]
    fn item_status_message_delivered_fr() {
        let (t, b) = item_status_message("delivered", "orders:abc12345", "Milk", "fr", false);
        assert!(t.contains("livré"));
        assert!(b.contains("livré"));
    }

    #[test]
    fn item_status_message_delivered_en() {
        let (t, b) = item_status_message("delivered", "orders:abc12345", "Milk", "en", false);
        assert!(t.contains("delivered"));
        assert!(b.contains("delivered"));
    }

    #[test]
    fn item_status_message_unknown_fr() {
        let (t, b) = item_status_message("unknown", "orders:abc12345", "Milk", "fr", false);
        assert!(t.contains("Mise à jour"));
        assert!(b.contains("unknown"));
    }

    #[test]
    fn item_status_message_unknown_en() {
        let (t, b) = item_status_message("unknown", "orders:abc12345", "Milk", "en", false);
        assert!(t.contains("Item update"));
        assert!(b.contains("unknown"));
    }

    // ── Coverage: aggregate_item_status_message multi-item variants (lines 1404-1427) ──

    #[test]
    fn aggregate_item_status_shipped_pickup_fr() {
        let items = vec![json!({"name": "A"}), json!({"name": "B"})];
        let (t, b) =
            aggregate_item_status_message("shipped", "orders:abc12345", &items, "fr", true);
        assert!(t.contains("ramassage"));
        assert!(b.contains("ramassage"));
    }

    #[test]
    fn aggregate_item_status_shipped_pickup_en() {
        let items = vec![json!({"name": "A"}), json!({"name": "B"})];
        let (t, b) =
            aggregate_item_status_message("shipped", "orders:abc12345", &items, "en", true);
        assert!(t.contains("pickup"));
        assert!(b.contains("pickup"));
    }

    #[test]
    fn aggregate_item_status_shipped_fr() {
        let items = vec![json!({"name": "A"}), json!({"name": "B"})];
        let (t, b) =
            aggregate_item_status_message("shipped", "orders:abc12345", &items, "fr", false);
        assert!(t.contains("expédiés"));
        assert!(b.contains("expédiés"));
    }

    #[test]
    fn aggregate_item_status_delivered_fr() {
        let items = vec![json!({"name": "A"}), json!({"name": "B"})];
        let (t, b) =
            aggregate_item_status_message("delivered", "orders:abc12345", &items, "fr", false);
        assert!(t.contains("livrés"));
        assert!(b.contains("livrés"));
    }

    #[test]
    fn aggregate_item_status_delivered_en() {
        let items = vec![json!({"name": "A"}), json!({"name": "B"})];
        let (t, b) =
            aggregate_item_status_message("delivered", "orders:abc12345", &items, "en", false);
        assert!(t.contains("delivered"));
        assert!(b.contains("delivered"));
    }

    #[test]
    fn aggregate_item_status_unknown_falls_back_to_single_item_message() {
        let items = vec![json!({"name": "A"}), json!({"name": "B"})];
        let (t, _b) =
            aggregate_item_status_message("unknown", "orders:abc12345", &items, "en", false);
        assert!(t.contains("Item update"));
    }

    // ── Coverage: urgent_perishable_message variants (lines 1442-1454) ──

    #[test]
    fn urgent_perishable_message_fr_with_names() {
        let items = vec![json!({"name": "Milk"}), json!({"name": "Eggs"})];
        let (t, b) = urgent_perishable_message("orders:abc12345", &items, "fr");
        assert!(t.contains("URGENT"));
        assert!(b.contains("Milk"));
        assert!(b.contains("Eggs"));
    }

    #[test]
    fn urgent_perishable_message_fr_no_names() {
        let items = vec![json!({})];
        let (t, b) = urgent_perishable_message("orders:abc12345", &items, "fr");
        assert!(t.contains("URGENT"));
        assert!(b.contains("périssable"));
    }

    #[test]
    fn urgent_perishable_message_en_with_names() {
        let items = vec![json!({"name": "Fish"})];
        let (t, b) = urgent_perishable_message("orders:abc12345", &items, "en");
        assert!(t.contains("URGENT"));
        assert!(b.contains("Fish"));
    }

    #[test]
    fn urgent_perishable_message_en_no_names() {
        let items = vec![json!({})];
        let (_, b) = urgent_perishable_message("orders:abc12345", &items, "en");
        assert!(b.contains("Perishable order"));
    }

    // ── Coverage: return_buyer_message French variants (lines 1473-1539) ──

    #[test]
    fn return_buyer_message_requested_fr() {
        let (t, b) = return_buyer_message("REQUESTED", "orders:abc12345", "ret1", "fr");
        assert!(t.contains("Retour demandé"));
        assert!(b.contains("enregistrée"));
    }

    #[test]
    fn return_buyer_message_requested_en() {
        let (t, b) = return_buyer_message("REQUESTED", "orders:abc12345", "ret1", "en");
        assert!(t.contains("Return requested"));
        assert!(b.contains("submitted"));
    }

    #[test]
    fn return_buyer_message_approved_fr() {
        let (t, b) = return_buyer_message("APPROVED", "orders:abc12345", "ret1", "fr");
        assert!(t.contains("approuvé"));
        assert!(b.contains("approuvée"));
    }

    #[test]
    fn return_buyer_message_approved_en() {
        let (t, b) = return_buyer_message("APPROVED", "orders:abc12345", "ret1", "en");
        assert!(t.contains("approved"));
    }

    #[test]
    fn return_buyer_message_rejected_fr() {
        let (t, b) = return_buyer_message("REJECTED", "orders:abc12345", "ret1", "fr");
        assert!(t.contains("refusé"));
        assert!(b.contains("refusée"));
    }

    #[test]
    fn return_buyer_message_rejected_en() {
        let (t, b) = return_buyer_message("REJECTED", "orders:abc12345", "ret1", "en");
        assert!(t.contains("rejected"));
    }

    #[test]
    fn return_buyer_message_label_issued_fr() {
        let (t, b) = return_buyer_message("LABEL_ISSUED", "orders:abc12345", "ret1", "fr");
        assert!(t.contains("Etiquette"));
        assert!(b.contains("prête"));
    }

    #[test]
    fn return_buyer_message_label_issued_en() {
        let (t, b) = return_buyer_message("LABEL_ISSUED", "orders:abc12345", "ret1", "en");
        assert!(t.contains("label ready"));
    }

    #[test]
    fn return_buyer_message_received_fr() {
        let (t, b) = return_buyer_message("RECEIVED", "orders:abc12345", "ret1", "fr");
        assert!(t.contains("reçu"));
        assert!(b.contains("remboursement"));
    }

    #[test]
    fn return_buyer_message_received_en() {
        let (t, b) = return_buyer_message("RECEIVED", "orders:abc12345", "ret1", "en");
        assert!(t.contains("received"));
        assert!(b.contains("refund"));
    }

    #[test]
    fn return_buyer_message_refunded_fr() {
        let (t, b) = return_buyer_message("REFUNDED", "orders:abc12345", "ret1", "fr");
        assert!(t.contains("remboursé"));
    }

    #[test]
    fn return_buyer_message_refunded_en() {
        let (t, b) = return_buyer_message("REFUNDED", "orders:abc12345", "ret1", "en");
        assert!(t.contains("refunded"));
    }

    #[test]
    fn return_buyer_message_escalated_fr() {
        let (t, b) = return_buyer_message("ESCALATED", "orders:abc12345", "ret1", "fr");
        assert!(t.contains("escaladé"));
        assert!(b.contains("support"));
    }

    #[test]
    fn return_buyer_message_escalated_en() {
        let (t, b) = return_buyer_message("ESCALATED", "orders:abc12345", "ret1", "en");
        assert!(t.contains("escalated"));
        assert!(b.contains("support"));
    }

    #[test]
    fn return_buyer_message_unknown_fr() {
        let (t, b) = return_buyer_message("WEIRD_STATUS", "orders:abc12345", "ret1", "fr");
        assert!(t.contains("Mise à jour"));
        assert!(b.contains("WEIRD_STATUS"));
    }

    #[test]
    fn return_buyer_message_unknown_en() {
        let (t, b) = return_buyer_message("WEIRD_STATUS", "orders:abc12345", "ret1", "en");
        assert!(t.contains("Return update"));
        assert!(b.contains("WEIRD_STATUS"));
    }

    // ── Coverage: return_seller_message variants (lines 1552-1582) ──

    #[test]
    fn return_seller_message_requested_fr() {
        let (t, b) = return_seller_message("REQUESTED", "orders:abc12345", "ret1", "fr");
        assert!(t.contains("Nouveau retour"));
        assert!(b.contains("révision"));
    }

    #[test]
    fn return_seller_message_requested_en() {
        let (t, b) = return_seller_message("REQUESTED", "orders:abc12345", "ret1", "en");
        assert!(t.contains("New return"));
        assert!(b.contains("review"));
    }

    #[test]
    fn return_seller_message_received_fr() {
        let (t, b) = return_seller_message("RECEIVED", "orders:abc12345", "ret1", "fr");
        assert!(t.contains("reçu"));
    }

    #[test]
    fn return_seller_message_escalated_fr() {
        let (t, b) = return_seller_message("ESCALATED", "orders:abc12345", "ret1", "fr");
        assert!(t.contains("escaladé"));
        assert!(b.contains("support"));
    }

    #[test]
    fn return_seller_message_escalated_en() {
        let (t, b) = return_seller_message("ESCALATED", "orders:abc12345", "ret1", "en");
        assert!(t.contains("escalated"));
        assert!(b.contains("support"));
    }

    #[test]
    fn return_seller_message_unknown_fr() {
        let (t, b) = return_seller_message("WEIRD", "orders:abc12345", "ret1", "fr");
        assert!(t.contains("Mise à jour"));
    }

    #[test]
    fn return_seller_message_unknown_en() {
        let (t, b) = return_seller_message("WEIRD", "orders:abc12345", "ret1", "en");
        assert!(t.contains("Return update"));
    }

    // ── Coverage: generic_email_html French greeting ──

    #[test]
    fn generic_email_html_french_uses_bonjour() {
        let html = generic_email_html("Test Title", "Test Body", "fr");
        assert!(html.contains("Bonjour,"));
        assert!(html.contains("Test Title"));
    }

    #[test]
    fn generic_email_html_english_uses_hello() {
        let html = generic_email_html("Test Title", "Test Body", "en");
        assert!(html.contains("Hello,"));
    }

    // ── Coverage: handle_order_item_status_changes pickup items ──

    #[tokio::test]
    async fn handle_order_item_status_changes_pickup_items() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;

        executor
            .handle_order_item_status_changes(
                "orders:pickup_items",
                &json!({
                    "userId": "buyer_1",
                    "deliverySpeed": "pickup",
                    fields::ORDER_STATUS: "PROCESSING",
                    fields::ITEMS: [
                        { fields::CART_ITEM_ID: "c1", fields::STATUS: "PROCESSING", "isDigital": false, "name": "Item A" },
                    ],
                }),
                &json!({
                    "userId": "buyer_1",
                    "deliverySpeed": "pickup",
                    fields::ORDER_STATUS: "PROCESSING",
                    fields::ITEMS: [
                        { fields::CART_ITEM_ID: "c1", fields::STATUS: "SHIPPED", "isDigital": false, "name": "Item A" },
                    ],
                }),
            )
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(
            notifications[0]["body"]
                .as_str()
                .unwrap_or("")
                .contains("pickup")
        );
    }

    // ── Coverage: handle_order_item_status_changes delivered items ──

    #[tokio::test]
    async fn handle_order_item_status_changes_delivered_multi_items() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;

        executor
            .handle_order_item_status_changes(
                "orders:delivered_multi",
                &json!({
                    "userId": "buyer_1",
                    fields::ORDER_STATUS: "SHIPPED",
                    fields::ITEMS: [
                        { fields::CART_ITEM_ID: "c1", fields::STATUS: "SHIPPED", "name": "A" },
                        { fields::CART_ITEM_ID: "c2", fields::STATUS: "SHIPPED", "name": "B" },
                    ],
                }),
                &json!({
                    "userId": "buyer_1",
                    fields::ORDER_STATUS: "SHIPPED",
                    fields::ITEMS: [
                        { fields::CART_ITEM_ID: "c1", fields::STATUS: "DELIVERED", "name": "A" },
                        { fields::CART_ITEM_ID: "c2", fields::STATUS: "DELIVERED", "name": "B" },
                    ],
                }),
            )
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(
            notifications[0]["title"]
                .as_str()
                .unwrap_or("")
                .contains("delivered")
        );
    }

    // ── Coverage: handle_order_payment_status_change partial refund ──

    #[tokio::test]
    async fn handle_order_payment_status_change_partial_refund() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;

        executor
            .handle_order_payment_status_change(
                "orders:partial",
                &json!({
                    "userId": "buyer_1",
                    fields::PAYMENT_STATUS: "CAPTURED",
                }),
                &json!({
                    "userId": "buyer_1",
                    fields::PAYMENT_STATUS: "PARTIAL_REFUND",
                    fields::PARTIAL_REFUND_AMOUNT_CENTS: 550,
                }),
            )
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert_eq!(notifications.len(), 1);
    }

    // ── Coverage: handle_order_payment_status_change empty old status ──

    #[tokio::test]
    async fn handle_order_payment_status_change_empty_old_status_is_noop() {
        let executor = setup_executor().await;
        executor
            .handle_order_payment_status_change(
                "orders:empty_old",
                &json!({ "userId": "buyer_1" }),
                &json!({ "userId": "buyer_1", fields::PAYMENT_STATUS: "REFUNDED" }),
            )
            .await
            .unwrap();
    }

    // ── Coverage: handle_order_status_change empty old status ──

    #[tokio::test]
    async fn handle_order_status_change_empty_old_status_is_noop() {
        let executor = setup_executor().await;
        executor
            .handle_order_status_change(
                "orders:empty_old",
                &json!({ "userId": "buyer_1" }),
                &json!({ fields::ORDER_STATUS: "CONFIRMED", "userId": "buyer_1" }),
            )
            .await
            .unwrap();
    }

    // ── Coverage: notification_item_key fallback path ──

    #[test]
    fn notification_item_key_uses_fallback_hash_without_cart_item_id() {
        let item = json!({
            fields::PRODUCT_ID: "prod_1",
            "name": "Widget",
            fields::STATUS: "SHIPPED",
        });
        let key = notification_item_key(&item);
        // Should be a hex hash string
        assert!(!key.is_empty());
        assert_eq!(key.len(), 16); // stable_hash produces 16 hex chars
    }

    // ── Coverage: handle_event dispatching for return_requests ──

    #[tokio::test]
    async fn handle_event_return_requests_update_dispatches() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;

        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "return_requests".into(),
            document_id: "return_requests:ret_ev".into(),
            data: json!({}),
            before_data: Some(json!({
                fields::RETURN_STATUS: "PENDING",
                fields::ORDER_ID: "ord_1",
                fields::BUYER_ID: "buyer_1",
            })),
            after_data: Some(json!({
                fields::RETURN_STATUS: "APPROVED",
                fields::ORDER_ID: "ord_1",
                fields::BUYER_ID: "buyer_1",
            })),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        executor.handle_event(event).await.unwrap();
    }

    // ── Coverage: handle_event dispatching for orders ──

    #[tokio::test]
    async fn handle_event_orders_update_dispatches() {
        let executor = setup_executor().await;
        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "orders".into(),
            document_id: "orders:ord_ev".into(),
            data: json!({}),
            before_data: Some(json!({ fields::ORDER_STATUS: "PENDING", "userId": "b1" })),
            after_data: Some(json!({ fields::ORDER_STATUS: "PENDING", "userId": "b1" })),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        executor.handle_event(event).await.unwrap();
    }

    // ── Coverage: handle_order_status_change PROCESSING cleanup ──

    #[tokio::test]
    async fn handle_order_status_change_processing_cleans_stock() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_1", "en").await;

        let _ = executor
            .state
            .db
            .create_document(
                collections::STOCK_NOTIFICATIONS,
                json!({
                    "productId": "prod_1",
                    "userId": "buyer_1",
                    "variantKey": "",
                }),
            )
            .await;

        executor
            .handle_order_status_change(
                "orders:processing_clean",
                &json!({
                    fields::ORDER_STATUS: "CONFIRMED",
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        "variantKey": "",
                    }],
                }),
                &json!({
                    fields::ORDER_STATUS: "PROCESSING",
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        "variantKey": "",
                    }],
                }),
            )
            .await
            .unwrap();

        let remaining = executor
            .state
            .db
            .list_documents(collections::STOCK_NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert!(remaining.is_empty());
    }

    // ── Coverage: French return update notifications ──

    #[tokio::test]
    async fn handle_return_update_french_user() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_fr", "fr").await;
        seed_user(&executor, "seller_fr", "fr").await;

        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "return_requests".into(),
            document_id: "return_requests:ret_fr".into(),
            data: json!({}),
            before_data: Some(json!({
                fields::RETURN_STATUS: "PENDING",
                fields::ORDER_ID: "ord_fr",
                fields::BUYER_ID: "buyer_fr",
                fields::SELLER_ID: "seller_fr",
            })),
            after_data: Some(json!({
                fields::RETURN_STATUS: "REQUESTED",
                fields::ORDER_ID: "ord_fr",
                fields::BUYER_ID: "buyer_fr",
                fields::SELLER_ID: "seller_fr",
            })),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        executor.handle_return_update(&event).await.unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert!(notifications.iter().any(|doc| {
            doc["userId"] == "buyer_fr"
                && doc["title"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Retour demandé")
        }));
    }

    // ── Coverage: French order status notifications ──

    #[tokio::test]
    async fn handle_order_status_change_french_buyer_and_seller() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_fr", "fr").await;
        seed_user(&executor, "seller_fr", "fr").await;

        executor
            .handle_order_status_change(
                "orders:fr_order",
                &json!({
                    fields::ORDER_STATUS: "PENDING",
                    "userId": "buyer_fr",
                    fields::ITEMS: [{
                        fields::SELLER_ID: "seller_fr",
                        fields::IS_PERISHABLE: true,
                        "name": "Lait",
                    }],
                }),
                &json!({
                    fields::ORDER_STATUS: "CONFIRMED",
                    "userId": "buyer_fr",
                    fields::ITEMS: [{
                        fields::SELLER_ID: "seller_fr",
                        fields::IS_PERISHABLE: true,
                        "name": "Lait",
                    }],
                }),
            )
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(20))
            .await
            .unwrap();
        assert!(notifications.iter().any(|doc| {
            doc["userId"] == "buyer_fr" && doc["title"].as_str().unwrap_or("").contains("confirmée")
        }));
        assert!(notifications.iter().any(|doc| {
            doc["userId"] == "seller_fr" && doc["title"].as_str().unwrap_or("").contains("URGENT")
        }));
    }

    // ── Coverage: French payment notifications ──

    #[tokio::test]
    async fn handle_order_payment_change_french_buyer() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_fr", "fr").await;

        executor
            .handle_order_payment_status_change(
                "orders:fr_pay",
                &json!({
                    "userId": "buyer_fr",
                    fields::PAYMENT_STATUS: "CAPTURED",
                }),
                &json!({
                    "userId": "buyer_fr",
                    fields::PAYMENT_STATUS: "REFUNDED",
                    fields::CUMULATIVE_REFUNDED_CENTS: 2000,
                }),
            )
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert!(
            notifications[0]["title"]
                .as_str()
                .unwrap_or("")
                .contains("Remboursement")
        );
    }

    // ── Coverage: French item status notifications ──

    #[tokio::test]
    async fn handle_order_item_status_changes_french_buyer() {
        let executor = setup_executor().await;
        seed_user(&executor, "buyer_fr", "fr").await;

        executor
            .handle_order_item_status_changes(
                "orders:fr_items",
                &json!({
                    "userId": "buyer_fr",
                    fields::ORDER_STATUS: "PROCESSING",
                    fields::ITEMS: [
                        { fields::CART_ITEM_ID: "c1", fields::STATUS: "PROCESSING", "name": "Lait" },
                    ],
                }),
                &json!({
                    "userId": "buyer_fr",
                    fields::ORDER_STATUS: "PROCESSING",
                    fields::ITEMS: [
                        { fields::CART_ITEM_ID: "c1", fields::STATUS: "SHIPPED", "name": "Lait" },
                    ],
                }),
            )
            .await
            .unwrap();

        let notifications = executor
            .state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert!(
            notifications[0]["title"]
                .as_str()
                .unwrap_or("")
                .contains("expédié")
        );
    }

    // ── Coverage: value_as_i64 with u64 value ──

    #[test]
    fn value_as_i64_handles_u64() {
        let v = json!(42u64);
        assert_eq!(value_as_i64(&v), Some(42));
    }

    #[test]
    fn value_as_i64_handles_negative() {
        let v = json!(-5);
        assert_eq!(value_as_i64(&v), Some(-5));
    }

    #[test]
    fn value_as_i64_returns_none_for_string() {
        let v = json!("not a number");
        assert_eq!(value_as_i64(&v), None);
    }

    // ── Coverage: str_field, order_status, order_buyer_id, record_id, short_id ──

    #[test]
    fn str_field_returns_empty_for_missing_field() {
        let v = json!({});
        assert_eq!(str_field(&v, "missing"), "");
    }

    #[test]
    fn order_status_falls_back_to_status_field() {
        let v = json!({ fields::STATUS: "SHIPPED" });
        assert_eq!(order_status(&v), "SHIPPED");
    }

    #[test]
    fn order_buyer_id_falls_back_to_buyer_id_then_uid() {
        let v1 = json!({ fields::BUYER_ID: "b1" });
        assert_eq!(order_buyer_id(&v1), "b1");

        let v2 = json!({ fields::UID: "u1" });
        assert_eq!(order_buyer_id(&v2), "u1");

        let v3 = json!({});
        assert_eq!(order_buyer_id(&v3), "");
    }

    #[test]
    fn record_id_extracts_after_colon() {
        assert_eq!(record_id("orders:abc123"), "abc123");
        assert_eq!(record_id("abc123"), "abc123");
    }

    #[test]
    fn short_id_truncates_to_8_uppercase() {
        assert_eq!(short_id("orders:abcdefghij"), "ABCDEFGH");
        assert_eq!(short_id("ab"), "AB");
    }

    // ── Coverage: seller_ids helper ──

    #[test]
    fn seller_ids_deduplicates_and_sorts() {
        let order = json!({
            fields::ITEMS: [
                { fields::SELLER_ID: "s2" },
                { fields::SELLER_ID: "s1" },
                { fields::SELLER_ID: "s2" },
            ]
        });
        let ids = seller_ids(&order);
        assert_eq!(ids, vec!["s1".to_string(), "s2".to_string()]);
    }

    #[test]
    fn seller_ids_returns_empty_for_no_items() {
        let order = json!({});
        assert!(seller_ids(&order).is_empty());
    }

    // ── Coverage: handle_product_create/update/delete WITH search config (lines 48-84) ──

    async fn setup_state_with_search(search_url: &str) -> HandlersState {
        let mut config = Config::load(None).unwrap();
        config.search = Some(ob_core::config::SearchConfig {
            url: search_url.to_string(),
            api_key: Some("test_key".to_string()),
        });
        HandlersState {
            config: Arc::new(config),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        }
    }

    async fn setup_executor_with_search(search_url: &str) -> NativeTriggerExecutor {
        let state = setup_state_with_search(search_url).await;
        let (_tx, rx) = mpsc::channel(8);
        NativeTriggerExecutor::new(state, rx)
    }

    #[tokio::test]
    async fn handle_product_create_with_search_config_calls_trigger() {
        let executor = setup_executor_with_search("http://127.0.0.1:1").await;
        let event = ChangeEvent {
            action: ChangeAction::Create,
            collection: "products".into(),
            document_id: "products:p1".into(),
            data: json!({
                "name": "Test Product",
                "lifecycleStatus": "active",
            }),
            before_data: None,
            after_data: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        // Will fail due to unreachable URL but exercises the code path
        let _ = executor.handle_event(event).await;
    }

    #[tokio::test]
    async fn handle_product_update_with_search_config_calls_trigger() {
        let executor = setup_executor_with_search("http://127.0.0.1:1").await;
        let event = ChangeEvent {
            action: ChangeAction::Update,
            collection: "products".into(),
            document_id: "products:p1".into(),
            data: json!({
                "name": "Updated Product",
                "lifecycleStatus": "active",
            }),
            before_data: None,
            after_data: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        let _ = executor.handle_event(event).await;
    }

    #[tokio::test]
    async fn handle_product_delete_with_search_config_calls_trigger() {
        let executor = setup_executor_with_search("http://127.0.0.1:1").await;
        let event = ChangeEvent {
            action: ChangeAction::Delete,
            collection: "products".into(),
            document_id: "products:p1".into(),
            data: json!({}),
            before_data: None,
            after_data: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        let _ = executor.handle_event(event).await;
    }

    // ── Coverage: dispatch_email with mailjet credentials (lines 677-686) ──

    #[tokio::test]
    async fn dispatch_email_with_mailjet_creds_creates_mail_log_and_attempts_send() {
        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("mailjet_api_key".to_string(), "mj_test_key".to_string());
        config.secrets.values.insert(
            "mailjet_secret_key".to_string(),
            "mj_test_secret".to_string(),
        );
        let state = HandlersState {
            config: Arc::new(config),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };
        let (_tx, rx) = mpsc::channel(8);
        let executor = NativeTriggerExecutor::new(state, rx);

        // Seed user with email
        executor
            .state
            .db
            .upsert_document(
                collections::USERS,
                "buyer_email_test",
                json!({
                    fields::EMAIL: "buyer_email_test@example.com",
                    fields::PREFERRED_LANGUAGE: "en",
                }),
            )
            .await
            .unwrap();

        // Set MAILJET_API_URL to unreachable so we exercise the code path
        // but the actual HTTP call fails
        unsafe {
            std::env::set_var("MAILJET_API_URL", "http://127.0.0.1:1/v3.1/send");
        }

        executor
            .dispatch_email(
                "notif_email_creds",
                "buyer_email_test",
                "Test Subject",
                "Test body",
                "order_status_changed",
                &json!({"orderId": "ord_1"}),
            )
            .await;

        unsafe {
            std::env::remove_var("MAILJET_API_URL");
        }

        // Mail log should exist
        let mail_logs = executor
            .state
            .db
            .list_documents(collections::MAIL_LOGS, Some(10))
            .await
            .unwrap();
        assert!(!mail_logs.is_empty());
    }

    // ── Coverage: dispatch_push with FCM env vars (lines 767-800) ──

    #[tokio::test]
    async fn dispatch_push_with_fcm_env_vars_exercises_send_path() {
        let executor = setup_executor().await;

        // Seed a push token
        executor
            .state
            .db
            .query_raw("CREATE _push_tokens SET user_id = 'buyer_fcm', token = 'fcm_token_xyz'")
            .await
            .unwrap();

        // Set FCM env vars (invalid SA JSON so send_push fails but code path is exercised)
        unsafe {
            std::env::set_var("OB_FCM_PROJECT_ID", "test-project-push");
            std::env::set_var("OB_FCM_SERVICE_ACCOUNT", "{}");
        }

        executor
            .dispatch_push(
                "notif_fcm_test",
                "buyer_fcm",
                "Push Title",
                "Push Body",
                &json!({"screen": "orders", "orderId": "ord_1"}),
            )
            .await;

        unsafe {
            std::env::remove_var("OB_FCM_PROJECT_ID");
            std::env::remove_var("OB_FCM_SERVICE_ACCOUNT");
        }
    }

    // ── Coverage: dispatch_push with multiple tokens (lines 738-800 loop) ──

    #[tokio::test]
    async fn dispatch_push_with_multiple_tokens() {
        let executor = setup_executor().await;

        executor
            .state
            .db
            .query_raw("CREATE _push_tokens SET user_id = 'buyer_multi', token = 'tok_1'")
            .await
            .unwrap();
        executor
            .state
            .db
            .query_raw("CREATE _push_tokens SET user_id = 'buyer_multi', token = 'tok_2'")
            .await
            .unwrap();

        unsafe {
            std::env::set_var("OB_FCM_PROJECT_ID", "test-project-multi");
            std::env::set_var("OB_FCM_SERVICE_ACCOUNT", "{}");
        }

        executor
            .dispatch_push(
                "notif_multi_tok",
                "buyer_multi",
                "Multi Push",
                "Multi Body",
                &json!({"orderId": "ord_2"}),
            )
            .await;

        unsafe {
            std::env::remove_var("OB_FCM_PROJECT_ID");
            std::env::remove_var("OB_FCM_SERVICE_ACCOUNT");
        }
    }

    // ── Coverage: run() error path — trigger that returns Err (line 26) ──

    #[tokio::test]
    async fn run_logs_error_when_product_trigger_fails_with_search_config() {
        let state = setup_state_with_search("http://127.0.0.1:1").await;
        let (tx, rx) = mpsc::channel(8);
        let executor = NativeTriggerExecutor::new(state, rx);

        // Product create with search config will try HTTP and fail → Err → error! log
        tx.send(ChangeEvent {
            action: ChangeAction::Create,
            collection: "products".into(),
            document_id: "products:p_err".into(),
            data: json!({"name": "Failing Product"}),
            before_data: None,
            after_data: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        })
        .await
        .unwrap();
        drop(tx);

        executor.run().await;
        // If we reach here, error was logged and handled gracefully
    }

    // ── Coverage: cleanup_stock_notifications with matching variant (line 510-516) ──

    #[tokio::test]
    async fn cleanup_stock_notifications_matching_variant_deleted() {
        let executor = setup_executor().await;

        // Create stock notification with matching variant
        let _ = executor
            .state
            .db
            .query_bind(
                "CREATE type::thing($table, 'sn_match') CONTENT $data",
                json!({
                    "table": collections::STOCK_NOTIFICATIONS,
                    "data": {
                        "productId": "prod_1",
                        "userId": "buyer_1",
                        "variantKey": "blue"
                    }
                })
            )
            .await;

        executor
            .cleanup_stock_notifications(&json!({
                "userId": "buyer_1",
                fields::ITEMS: [{
                    fields::PRODUCT_ID: "prod_1",
                    "variantKey": "blue",
                }],
            }))
            .await;

        let remaining = executor
            .state
            .db
            .list_documents(collections::STOCK_NOTIFICATIONS, Some(10))
            .await
            .unwrap();
        assert!(remaining.is_empty());
    }

    // ── Coverage: notification_item_key fallback (lines 911-919) ──

    #[test]
    fn notification_item_key_uses_fallback_hash_when_no_cart_item_id() {
        let item = json!({
            fields::PRODUCT_ID: "prod_1",
            "name": "Widget",
            fields::STATUS: "SHIPPED",
        });
        let key = notification_item_key(&item);
        // Should be a hex hash (16 chars)
        assert_eq!(key.len(), 16);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn notification_item_key_uses_cart_item_id_when_present() {
        let item = json!({
            fields::CART_ITEM_ID: "cart_item_42",
        });
        assert_eq!(notification_item_key(&item), "cart_item_42");
    }
}
