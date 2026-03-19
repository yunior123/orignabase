//! Return request handlers.
//! Ported from: functions/handlers/orders.py::create_return_request,
//!   approve_return_request, reject_return_request

use axum::{Json, Router, extract::State, routing::post};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::info;

use crate::HandlersState;
use crate::push;
use crate::shared::schema::{business_rules, collections, fields};
use crate::shared::validation::{sanitize_html, validate_uid};

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateReturnRequest {
    pub order_id: String,
    pub product_id: String,
    pub user_id: String,
    pub return_reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateReturnResponse {
    pub success: bool,
    pub return_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveReturnReq {
    pub return_id: String,
    pub user_id: String,
    /// "approve" | "mark_received" | "issue_label"
    #[serde(default = "default_action")]
    pub action: String,
    #[serde(default)]
    pub return_tracking_number: Option<String>,
    #[serde(default)]
    pub return_admin_note: Option<String>,
}

fn default_action() -> String {
    "approve".to_string()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveReturnResponse {
    pub success: bool,
    pub new_status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectReturnReq {
    pub return_id: String,
    pub user_id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectReturnResponse {
    pub success: bool,
    pub new_status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EscalateReturnReq {
    pub return_id: String,
    pub user_id: String,
    pub escalation_reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EscalateReturnResponse {
    pub success: bool,
    pub new_status: String,
    pub return_status: String,
    pub return_id: String,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/returns/create", post(create_return_request))
        .route("/api/returns/approve", post(approve_return_request))
        .route("/api/returns/reject", post(reject_return_request))
        .route("/api/returns/escalate", post(escalate_return_request))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn str_field<'a>(v: &'a Value, field: &str) -> &'a str {
    v.get(field).and_then(|x| x.as_str()).unwrap_or("")
}

fn bool_field(v: &Value, field: &str) -> bool {
    v.get(field).and_then(|x| x.as_bool()).unwrap_or(false)
}

fn items_array(order: &Value) -> Vec<Value> {
    order
        .get(fields::ITEMS)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

async fn is_user_admin(state: &HandlersState, user_id: &str) -> Result<bool, ob_core::Error> {
    let user = state
        .db
        .get_document(collections::USERS, user_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("User not found".into()))?;
    let roles = user
        .get(fields::ROLES)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(roles.iter().any(|r| r.as_str() == Some("admin")))
}

async fn admin_user_ids(state: &HandlersState) -> Vec<String> {
    state
        .db
        .list_documents(collections::USERS, Some(1000))
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|user| {
            user.get(fields::ROLES)
                .and_then(|v| v.as_array())
                .map(|roles| roles.iter().any(|r| r.as_str() == Some("admin")))
                .unwrap_or(false)
        })
        .filter_map(|user| {
            user.get("id")
                .and_then(|v| v.as_str())
                .map(|id| id.split(':').next_back().unwrap_or(id).to_string())
        })
        .collect()
}

async fn notify_admins_of_return_escalation(
    state: &HandlersState,
    return_id: &str,
    order_id: &str,
) -> Result<(), ob_core::Error> {
    let now = Utc::now().to_rfc3339();
    let oid = order_id.chars().take(8).collect::<String>().to_uppercase();
    let rid = return_id.chars().take(8).collect::<String>().to_uppercase();

    let title = "Return Escalated by Buyer";
    let body = format!("Return #{rid} on order #{oid} escalated — needs admin review");
    let notif_title = format!("Return escalated by buyer for order #{oid}");
    let notif_body = format!("Return #{rid} on order #{oid} was escalated and needs admin review.");

    let admin_ids = admin_user_ids(state).await;

    // -----------------------------------------------------------------------
    // Phase 1: Batch-write ALL notification + pending-push records BEFORE
    //          attempting any delivery. This ensures crash-durability — even
    //          if the process dies mid-delivery, every record exists in DB
    //          for the cron drain worker to pick up.
    // -----------------------------------------------------------------------

    // Collect per-admin token rows so we can build pending records.
    struct AdminTokens {
        admin_id: String,
        tokens: Vec<String>,
    }

    let mut all_admin_tokens: Vec<AdminTokens> = Vec::with_capacity(admin_ids.len());

    for admin_id in &admin_ids {
        let rows = state
            .db
            .query_bind(
                "SELECT token FROM _push_tokens WHERE user_id = $user_id",
                json!({ "user_id": admin_id }),
            )
            .await
            .unwrap_or_default();

        let tokens: Vec<String> = rows
            .iter()
            .filter_map(|r| {
                r.get("token")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        all_admin_tokens.push(AdminTokens {
            admin_id: admin_id.clone(),
            tokens,
        });
    }

    for at in &all_admin_tokens {
        let notification_id = format!("return_escalated_admin_{return_id}_{}", at.admin_id);
        state
            .db
            .upsert_document(
                collections::NOTIFICATIONS,
                &notification_id,
                json!({
                    "userId": at.admin_id,
                    fields::NOTIFICATION_TYPE: "return_escalated_admin",
                    "title": notif_title,
                    "body": notif_body,
                    "data": {
                        fields::ORDER_ID: order_id,
                        "returnId": return_id,
                        fields::RETURN_STATUS: "escalated",
                    },
                    "read": false,
                    fields::CREATED_AT: &now,
                    fields::UPDATED_AT: &now,
                }),
            )
            .await
            .map_err(|e| {
                ob_core::Error::Database(format!(
                    "Failed to upsert admin notification for {}: {e}",
                    at.admin_id
                ))
            })?;

        for token in &at.tokens {
            let pending_id = format!(
                "return_escalated_admin_push_{return_id}_{}_{token}",
                at.admin_id
            );
            state
                .db
                .upsert_document(
                    "_pending_notifications",
                    &pending_id,
                    json!({
                        "userId": at.admin_id,
                        "token": token,
                        "notification_type": "return_escalated_admin",
                        "title": title,
                        "body": body,
                        "data": {
                            "orderId": order_id,
                            "returnId": return_id,
                            fields::RETURN_STATUS: "escalated",
                        },
                        "status": "pending",
                        "attempts": 0,
                        "created_at": &now,
                        "updated_at": &now,
                    }),
                )
                .await
                .map_err(|e| {
                    ob_core::Error::Database(format!(
                        "Failed to upsert pending escalation push for {}: {e}",
                        at.admin_id
                    ))
                })?;
        }
    }

    // -----------------------------------------------------------------------
    // Phase 2: Best-effort inline delivery. On success we mark the pending
    //          record as "delivered"; on failure we leave it "pending" for
    //          the cron drain worker (`drain_pending_notifications`).
    // -----------------------------------------------------------------------

    let project_id = std::env::var("OB_FCM_PROJECT_ID").ok();
    let service_account = std::env::var("OB_FCM_SERVICE_ACCOUNT").ok();

    for at in &all_admin_tokens {
        for token in &at.tokens {
            let pending_id = format!(
                "return_escalated_admin_push_{return_id}_{}_{token}",
                at.admin_id
            );

            let sent = if let (Some(pid), Some(sa)) = (&project_id, &service_account) {
                let mut data = std::collections::HashMap::new();
                data.insert("orderId".to_string(), order_id.to_string());
                data.insert("returnId".to_string(), return_id.to_string());
                data.insert(fields::RETURN_STATUS.to_string(), "escalated".to_string());
                push::send_push(
                    &state.http_client,
                    pid,
                    sa,
                    token,
                    title,
                    &body,
                    Some(&data),
                )
                .await
                .is_ok()
            } else {
                false
            };

            if sent {
                let delivered_at = Utc::now().to_rfc3339();
                let _ = state
                    .db
                    .query_bind(
                        "UPDATE type::thing($table, $id) SET status = 'delivered', delivered_at = $delivered_at, updated_at = $updated_at",
                        json!({
                            "table": "_pending_notifications",
                            "id": pending_id,
                            "delivered_at": delivered_at,
                            "updated_at": delivered_at,
                        }),
                    )
                    .await;
            }
            // If not sent, the record stays "pending" — cron drain will retry.
        }
    }

    Ok(())
}

/// Check if item is within return window.
fn assert_within_return_window(item: &Value) -> Result<(), ob_core::Error> {
    if let Some(delivered_at_str) = item.get("deliveredAt").and_then(|v| v.as_str())
        && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(delivered_at_str)
    {
        let days_since = (Utc::now() - dt.with_timezone(&Utc)).num_days();
        if days_since > business_rules::RETURN_WINDOW_DAYS as i64 {
            return Err(ob_core::Error::Validation(format!(
                "Return window expired. Returns must be requested within {} days of delivery.",
                business_rules::RETURN_WINDOW_DAYS
            )));
        }
    }
    Ok(())
}

/// Valid return-request status transitions.
fn valid_return_transitions(from: &str) -> Vec<&'static str> {
    match from {
        "requested" => vec!["approved", "rejected"],
        "approved" => vec!["label_issued", "received"],
        "label_issued" => vec!["received"],
        "received" => vec!["refunded"],
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// create_return_request
// ---------------------------------------------------------------------------

async fn create_return_request(
    State(state): State<HandlersState>,
    Json(req): Json<CreateReturnRequest>,
) -> Result<Json<CreateReturnResponse>, ob_core::Error> {
    validate_uid("orderId", &req.order_id)?;
    validate_uid("productId", &req.product_id)?;
    validate_uid("userId", &req.user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "create_return_request",
        3,  // max requests
        60, // window minutes
    )
    .await?;

    let return_reason = sanitize_html(&req.return_reason)
        .chars()
        .take(1000)
        .collect::<String>();
    if return_reason.trim().is_empty() {
        return Err(ob_core::Error::Validation(
            "returnReason is required".into(),
        ));
    }

    // Fetch order
    let order = state
        .db
        .get_document(collections::ORDERS, &req.order_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Order not found".into()))?;

    // Only buyer can create returns
    if str_field(&order, "userId") != req.user_id {
        return Err(ob_core::Error::Forbidden(
            "You can only return items from your own orders".into(),
        ));
    }

    // Find item
    let items = items_array(&order);
    let item = items
        .iter()
        .find(|it| str_field(it, fields::PRODUCT_ID) == req.product_id)
        .ok_or_else(|| ob_core::Error::NotFound("Item not found in this order".into()))?;

    // Digital products cannot be returned
    if bool_field(item, "isDigital") {
        return Err(ob_core::Error::Validation(
            "Digital products cannot be returned".into(),
        ));
    }

    // Must be delivered
    let item_status = str_field(item, "status");
    if item_status != "delivered" {
        return Err(ob_core::Error::Validation(
            "Item must be marked as delivered before requesting a return".into(),
        ));
    }

    // Return window check
    assert_within_return_window(item)?;

    // Check for existing active return request
    let query = format!(
        "SELECT * FROM {} WHERE orderId = '{}' AND productId = '{}' AND buyerId = '{}' LIMIT 1",
        collections::RETURN_REQUESTS,
        req.order_id,
        req.product_id,
        req.user_id
    );
    let existing = state.db.query_raw(&query).await.unwrap_or_default();
    for doc in &existing {
        let status = str_field(doc, "returnStatus");
        if status != "rejected" && status != "refunded" {
            return Err(ob_core::Error::Validation(
                "A return request already exists for this item".into(),
            ));
        }
    }

    // Create return request document
    let now = Utc::now().to_rfc3339();
    let return_id = uuid::Uuid::new_v4().to_string();

    let return_doc = json!({
        "returnId": return_id,
        "orderId": req.order_id,
        "buyerId": req.user_id,
        "sellerId": str_field(item, fields::SELLER_ID),
        "productId": req.product_id,
        "productName": str_field(item, "name"),
        "quantity": item.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1),
        "fulfillmentWarehouseId": str_field(item, "fulfillmentWarehouseId"),
        "returnStatus": "requested",
        "returnReason": return_reason,
        "requestedAt": now,
        fields::UPDATED_AT: now,
    });

    state
        .db
        .create_document(collections::RETURN_REQUESTS, return_doc)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to create return request: {e}")))?;

    info!(
        order_id = %req.order_id,
        product_id = %req.product_id,
        return_id = %return_id,
        "Return request created"
    );

    Ok(Json(CreateReturnResponse {
        success: true,
        return_id,
    }))
}

// ---------------------------------------------------------------------------
// approve_return_request
// ---------------------------------------------------------------------------

async fn approve_return_request(
    State(state): State<HandlersState>,
    Json(req): Json<ApproveReturnReq>,
) -> Result<Json<ApproveReturnResponse>, ob_core::Error> {
    validate_uid("returnId", &req.return_id)?;
    validate_uid("userId", &req.user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "approve_return_request",
        10,
        1,
    )
    .await?;

    let is_admin = is_user_admin(&state, &req.user_id).await?;

    // Fetch return request
    let return_doc = state
        .db
        .get_document(collections::RETURN_REQUESTS, &req.return_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Return request not found".into()))?;

    let seller_id = str_field(&return_doc, "sellerId");
    let current_status = str_field(&return_doc, "returnStatus");
    let product_id = str_field(&return_doc, "productId");

    // Permission: must be seller or admin
    if !is_admin && req.user_id != seller_id {
        return Err(ob_core::Error::Forbidden(
            "Only the seller or admin can approve return requests".into(),
        ));
    }

    let now = Utc::now().to_rfc3339();

    let new_status = match req.action.as_str() {
        "approve" => {
            let valid = valid_return_transitions(current_status);
            if !valid.contains(&"approved") {
                return Err(ob_core::Error::Validation(format!(
                    "Cannot approve return in status '{current_status}'"
                )));
            }

            let mut patch = json!({
                "returnStatus": "approved",
                fields::UPDATED_AT: now,
            });
            if let Some(ref tn) = req.return_tracking_number {
                patch["returnTrackingNumber"] = json!(tn);
            }
            if let Some(ref note) = req.return_admin_note {
                patch["returnAdminNote"] = json!(note);
            }

            state
                .db
                .update_document(collections::RETURN_REQUESTS, &req.return_id, patch)
                .await
                .map_err(|e| ob_core::Error::Database(format!("Failed to update return: {e}")))?;

            "approved".to_string()
        }
        "issue_label" => {
            let valid = valid_return_transitions(current_status);
            if !valid.contains(&"label_issued") {
                return Err(ob_core::Error::Validation(format!(
                    "Cannot issue label from status '{current_status}'"
                )));
            }

            let mut patch = json!({
                "returnStatus": "label_issued",
                fields::UPDATED_AT: now,
            });
            if let Some(ref tn) = req.return_tracking_number {
                patch["returnTrackingNumber"] = json!(tn);
            }

            state
                .db
                .update_document(collections::RETURN_REQUESTS, &req.return_id, patch)
                .await
                .map_err(|e| ob_core::Error::Database(format!("Failed to update return: {e}")))?;

            "label_issued".to_string()
        }
        "mark_received" => {
            let valid = valid_return_transitions(current_status);
            if !valid.contains(&"received") {
                return Err(ob_core::Error::Validation(format!(
                    "Cannot mark received from status '{current_status}'"
                )));
            }

            let order_id = str_field(&return_doc, "orderId");
            let order = state
                .db
                .get_document(collections::ORDERS, order_id)
                .await
                .map_err(|_| ob_core::Error::NotFound("Order not found".into()))?;
            let items = items_array(&order);
            let item_index = items
                .iter()
                .position(|it| str_field(it, fields::PRODUCT_ID) == product_id);

            let idx = item_index
                .ok_or_else(|| ob_core::Error::NotFound("Item not found in order".into()))?;
            let item = &items[idx];

            let qty = return_doc
                .get("quantity")
                .and_then(|v| v.as_i64())
                .unwrap_or(1);

            let refund_amount_cents =
                crate::orders::refunds::calculate_refund_amount_cents(&order, item)?;

            let payment_intent_id = str_field(&order, "paymentIntentId");
            let idempotency_key = format!("return_refund_{}_{}", req.return_id, product_id);
            let mut refund_id = None;

            if !payment_intent_id.is_empty() {
                refund_id = crate::orders::refunds::stripe_refund(
                    &state,
                    payment_intent_id,
                    Some(refund_amount_cents),
                    "requested_by_customer",
                    &idempotency_key,
                    &[
                        ("orderId", order_id),
                        ("productId", product_id),
                        ("returnId", &req.return_id),
                    ],
                )
                .await?;
            }

            let mut updated_items = items.clone();
            updated_items[idx]["status"] = json!("refunded");
            updated_items[idx]["refundedAt"] = json!(now);
            updated_items[idx]["refundReason"] = json!("Return approved");
            updated_items[idx]["refundAmountCents"] = json!(refund_amount_cents);
            if let Some(ref rid) = refund_id {
                updated_items[idx]["refundId"] = json!(rid);
            }

            if !product_id.is_empty() && qty > 0 {
                state
                    .db
                    .query_bind(
                        &format!("UPDATE type::thing($table, $product_id) SET stockQuantity += $quantity, updatedAt = $updatedAt"),
                        json!({
                            "table": collections::PRODUCTS,
                            "product_id": product_id,
                            "quantity": qty,
                            "updatedAt": now
                        })
                    )
                    .await
                    .map_err(|e| {
                        ob_core::Error::Database(format!(
                            "Failed to restore stock for returned product {product_id}: {e}"
                        ))
                    })?;
            }

            state
                .db
                .update_document(
                    collections::RETURN_REQUESTS,
                    &req.return_id,
                    json!({
                        "returnStatus": "refunded",
                        "resolvedAt": now,
                        "returnRefundAmountCents": refund_amount_cents,
                        fields::UPDATED_AT: now,
                    }),
                )
                .await
                .map_err(|e| {
                    ob_core::Error::Database(format!(
                        "Failed to update refunded return request: {e}"
                    ))
                })?;

            state
                .db
                .update_document(
                    collections::ORDERS,
                    order_id,
                    json!({
                        fields::ITEMS: updated_items,
                        fields::UPDATED_AT: now,
                    }),
                )
                .await
                .map_err(|e| {
                    ob_core::Error::Database(format!("Failed to update refunded order item: {e}"))
                })?;

            state
                .db
                .create_document(
                    collections::ORDER_EVENTS,
                    json!({
                        "orderId": order_id,
                        "userId": req.user_id,
                        "eventType": "return_received_and_refunded",
                        "message": format!("Return {} received and item {} refunded", req.return_id, product_id),
                        "metadata": { "productId": product_id, "returnId": req.return_id, "refundAmountCents": refund_amount_cents },
                        "createdAt": now,
                    }),
                )
                .await
                .map_err(|e| {
                    ob_core::Error::Database(format!(
                        "Failed to log return refund event: {e}"
                    ))
                })?;

            "refunded".to_string()
        }
        other => {
            return Err(ob_core::Error::Validation(format!(
                "Unknown action: {other}. Must be 'approve', 'issue_label', or 'mark_received'."
            )));
        }
    };

    info!(
        return_id = %req.return_id,
        action = %req.action,
        new_status = %new_status,
        "Return request updated"
    );

    Ok(Json(ApproveReturnResponse {
        success: true,
        new_status,
    }))
}

// ---------------------------------------------------------------------------
// reject_return_request
// ---------------------------------------------------------------------------

async fn reject_return_request(
    State(state): State<HandlersState>,
    Json(req): Json<RejectReturnReq>,
) -> Result<Json<RejectReturnResponse>, ob_core::Error> {
    validate_uid("returnId", &req.return_id)?;
    validate_uid("userId", &req.user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "reject_return_request",
        10,
        1,
    )
    .await?;

    let is_admin = is_user_admin(&state, &req.user_id).await?;

    let return_doc = state
        .db
        .get_document(collections::RETURN_REQUESTS, &req.return_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Return request not found".into()))?;

    let seller_id = str_field(&return_doc, "sellerId");
    let current_status = str_field(&return_doc, "returnStatus");

    if !is_admin && req.user_id != seller_id {
        return Err(ob_core::Error::Forbidden(
            "Only the seller or admin can reject return requests".into(),
        ));
    }

    let valid = valid_return_transitions(current_status);
    if !valid.contains(&"rejected") {
        return Err(ob_core::Error::Validation(format!(
            "Cannot reject return in status '{current_status}'"
        )));
    }

    let now = Utc::now().to_rfc3339();
    let rejection_reason = req
        .reason
        .as_deref()
        .map(|s| sanitize_html(s).chars().take(1000).collect::<String>())
        .unwrap_or_default();

    let mut patch = json!({
        "returnStatus": "rejected",
        "resolvedAt": now,
        fields::UPDATED_AT: now,
    });
    if !rejection_reason.is_empty() {
        patch["rejectionReason"] = json!(rejection_reason);
    }

    state
        .db
        .update_document(collections::RETURN_REQUESTS, &req.return_id, patch)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to update return: {e}")))?;

    info!(
        return_id = %req.return_id,
        "Return request rejected"
    );

    Ok(Json(RejectReturnResponse {
        success: true,
        new_status: "rejected".to_string(),
    }))
}

async fn escalate_return_request(
    State(state): State<HandlersState>,
    Json(req): Json<EscalateReturnReq>,
) -> Result<Json<EscalateReturnResponse>, ob_core::Error> {
    validate_uid("returnId", &req.return_id)?;
    validate_uid("userId", &req.user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "escalate_return_request",
        3,
        60,
    )
    .await?;

    let reason = sanitize_html(&req.escalation_reason)
        .chars()
        .take(1000)
        .collect::<String>();
    if reason.trim().is_empty() {
        return Err(ob_core::Error::Validation(
            "escalationReason is required".into(),
        ));
    }

    let return_doc = state
        .db
        .get_document(collections::RETURN_REQUESTS, &req.return_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Return request not found".into()))?;

    let buyer_id = str_field(&return_doc, "buyerId");
    if buyer_id != req.user_id {
        return Err(ob_core::Error::Forbidden(
            "Only the buyer can escalate this return".into(),
        ));
    }

    let status = str_field(&return_doc, "returnStatus");
    if status != "requested" && status != "approved" {
        return Err(ob_core::Error::Validation(
            "Return can only be escalated from requested or approved state".into(),
        ));
    }

    let now = Utc::now().to_rfc3339();
    state
        .db
        .update_document(
            collections::RETURN_REQUESTS,
            &req.return_id,
            json!({
                "returnStatus": "escalated",
                "escalatedAt": now,
                "escalationReason": reason,
                fields::UPDATED_AT: now,
            }),
        )
        .await?;

    let order_id = str_field(&return_doc, "orderId").to_string();
    notify_admins_of_return_escalation(&state, &req.return_id, &order_id).await?;

    Ok(Json(EscalateReturnResponse {
        success: true,
        new_status: "escalated".to_string(),
        return_status: "escalated".to_string(),
        return_id: req.return_id,
    }))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn setup_state() -> HandlersState {
        HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
        }
    }

    async fn setup_state_with_config(config: Config, stripe_base_url: String) -> HandlersState {
        HandlersState {
            config: Arc::new(config),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url,
        }
    }

    #[test]
    fn test_valid_return_transitions() {
        let t = valid_return_transitions("requested");
        assert!(t.contains(&"approved"));
        assert!(t.contains(&"rejected"));
        assert!(!t.contains(&"received"));
    }

    #[test]
    fn test_approved_transitions() {
        let t = valid_return_transitions("approved");
        assert!(t.contains(&"label_issued"));
        assert!(t.contains(&"received"));
        assert!(!t.contains(&"rejected"));
    }

    #[test]
    fn test_received_transitions() {
        let t = valid_return_transitions("received");
        assert!(t.contains(&"refunded"));
        assert!(!t.contains(&"approved"));
    }

    #[test]
    fn test_label_issued_transitions() {
        let t = valid_return_transitions("label_issued");
        assert_eq!(t, vec!["received"]);
    }

    #[test]
    fn test_terminal_no_transitions() {
        assert!(valid_return_transitions("refunded").is_empty());
        assert!(valid_return_transitions("rejected").is_empty());
    }

    #[test]
    fn test_return_window_within() {
        let recent = Utc::now().to_rfc3339();
        let item = json!({"deliveredAt": recent, "status": "delivered"});
        assert!(assert_within_return_window(&item).is_ok());
    }

    #[test]
    fn test_return_window_expired() {
        let old = (Utc::now() - chrono::Duration::days(31)).to_rfc3339();
        let item = json!({"deliveredAt": old, "status": "delivered"});
        assert!(assert_within_return_window(&item).is_err());
    }

    #[test]
    fn test_return_window_no_date() {
        let item = json!({"status": "delivered"});
        // No delivered date => pass (lenient)
        assert!(assert_within_return_window(&item).is_ok());
    }

    #[test]
    fn test_helpers_extract_fields_and_items() {
        let order = json!({
            "userId": "u1",
            "flag": true,
            fields::ITEMS: [{ "productId": "p1" }],
        });

        assert_eq!(str_field(&order, "userId"), "u1");
        assert!(bool_field(&order, "flag"));
        assert_eq!(items_array(&order).len(), 1);
    }

    #[test]
    fn test_create_return_request_deserialize() {
        let s = r#"{"orderId":"o1","productId":"p1","userId":"u1","returnReason":"Defective"}"#;
        let req: CreateReturnRequest = serde_json::from_str(s).unwrap();
        assert_eq!(req.return_reason, "Defective");
    }

    #[test]
    fn test_approve_return_default_action() {
        let s = r#"{"returnId":"r1","userId":"u1"}"#;
        let req: ApproveReturnReq = serde_json::from_str(s).unwrap();
        assert_eq!(req.action, "approve");
    }

    #[test]
    fn test_reject_return_deserialize() {
        let s = r#"{"returnId":"r1","userId":"u1","reason":"Not eligible"}"#;
        let req: RejectReturnReq = serde_json::from_str(s).unwrap();
        assert_eq!(req.reason, Some("Not eligible".to_string()));
    }

    #[test]
    fn test_escalate_return_deserialize() {
        let s = r#"{"returnId":"r1","userId":"u1","escalationReason":"seller unresponsive"}"#;
        let req: EscalateReturnReq = serde_json::from_str(s).unwrap();
        assert_eq!(req.escalation_reason, "seller unresponsive");
    }

    #[test]
    fn test_response_serialization() {
        let resp = CreateReturnResponse {
            success: true,
            return_id: "ret_123".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["returnId"], "ret_123");
    }

    #[test]
    fn test_escalate_response_serialization() {
        let resp = EscalateReturnResponse {
            success: true,
            new_status: "escalated".to_string(),
            return_status: "escalated".to_string(),
            return_id: "ret_123".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["newStatus"], "escalated");
        assert_eq!(json["returnStatus"], "escalated");
        assert_eq!(json["returnId"], "ret_123");
    }

    #[test]
    fn test_return_window_boundary() {
        // Exactly 30 days ago => still valid
        let boundary = (Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        let item = json!({"deliveredAt": boundary, "status": "delivered"});
        assert!(assert_within_return_window(&item).is_ok());
    }

    #[test]
    fn test_escalate_return_req_all_fields() {
        let s = r#"{"returnId":"ret_abc","userId":"usr_1","escalationReason":"Seller not responding for 5 days"}"#;
        let req: EscalateReturnReq = serde_json::from_str(s).unwrap();
        assert_eq!(req.return_id, "ret_abc");
        assert_eq!(req.user_id, "usr_1");
        assert_eq!(req.escalation_reason, "Seller not responding for 5 days");
    }

    #[test]
    fn test_escalate_return_req_missing_reason_fails() {
        let s = r#"{"returnId":"r1","userId":"u1"}"#;
        let result = serde_json::from_str::<EscalateReturnReq>(s);
        assert!(result.is_err(), "escalationReason is required");
    }

    #[test]
    fn test_escalate_response_all_fields_roundtrip() {
        let resp = EscalateReturnResponse {
            success: true,
            new_status: "escalated".to_string(),
            return_status: "escalated".to_string(),
            return_id: "ret_xyz".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["newStatus"], "escalated");
        assert_eq!(json["returnStatus"], "escalated");
        assert_eq!(json["returnId"], "ret_xyz");

        // Roundtrip: serialize -> string -> deserialize back as Value
        let s = serde_json::to_string(&resp).unwrap();
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, json);
    }

    // -----------------------------------------------------------------------
    // Return status transitions — exhaustive matrix
    // (ported from Python returns_deep + residual_coverage)
    // -----------------------------------------------------------------------

    #[test]
    fn test_escalated_has_no_transitions() {
        assert!(valid_return_transitions("escalated").is_empty());
    }

    #[test]
    fn test_unknown_status_has_no_transitions() {
        assert!(valid_return_transitions("unknown").is_empty());
        assert!(valid_return_transitions("").is_empty());
        assert!(valid_return_transitions("REQUESTED").is_empty()); // case-sensitive
    }

    #[test]
    fn test_requested_cannot_transition_to_received() {
        let t = valid_return_transitions("requested");
        assert!(!t.contains(&"received"));
        assert!(!t.contains(&"label_issued"));
        assert!(!t.contains(&"refunded"));
        assert!(!t.contains(&"escalated"));
    }

    #[test]
    fn test_approved_cannot_transition_to_rejected_or_approved() {
        let t = valid_return_transitions("approved");
        assert!(!t.contains(&"rejected"));
        assert!(!t.contains(&"approved"));
        assert!(!t.contains(&"requested"));
        assert!(!t.contains(&"refunded"));
    }

    #[test]
    fn test_label_issued_only_to_received() {
        let t = valid_return_transitions("label_issued");
        assert_eq!(t.len(), 1);
        assert!(t.contains(&"received"));
    }

    #[test]
    fn test_received_only_to_refunded() {
        let t = valid_return_transitions("received");
        assert_eq!(t.len(), 1);
        assert!(t.contains(&"refunded"));
    }

    #[test]
    fn test_all_terminal_return_statuses() {
        for status in ["refunded", "rejected", "escalated"] {
            assert!(
                valid_return_transitions(status).is_empty(),
                "{status} should be terminal"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Return window boundary conditions (ported from Python returns_deep)
    // -----------------------------------------------------------------------

    #[test]
    fn test_return_window_exactly_30_days() {
        let exactly_30 = (Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        let item = json!({"deliveredAt": exactly_30});
        // 30 days = boundary, should be valid (> not >=)
        assert!(assert_within_return_window(&item).is_ok());
    }

    #[test]
    fn test_return_window_31_days_expired() {
        let over_31 = (Utc::now() - chrono::Duration::days(31)).to_rfc3339();
        let item = json!({"deliveredAt": over_31});
        assert!(assert_within_return_window(&item).is_err());
    }

    #[test]
    fn test_return_window_1_day_ago_valid() {
        let yesterday = (Utc::now() - chrono::Duration::days(1)).to_rfc3339();
        let item = json!({"deliveredAt": yesterday});
        assert!(assert_within_return_window(&item).is_ok());
    }

    #[test]
    fn test_return_window_today_valid() {
        let today = Utc::now().to_rfc3339();
        let item = json!({"deliveredAt": today});
        assert!(assert_within_return_window(&item).is_ok());
    }

    #[test]
    fn test_return_window_invalid_date_format_passes() {
        // Non-RFC3339 date strings should pass (lenient — no date = no rejection)
        let item = json!({"deliveredAt": "2026-01-01"});
        assert!(assert_within_return_window(&item).is_ok());
    }

    #[test]
    fn test_return_window_null_delivered_at_passes() {
        let item = json!({"deliveredAt": null});
        assert!(assert_within_return_window(&item).is_ok());
    }

    #[test]
    fn test_return_window_numeric_delivered_at_passes() {
        let item = json!({"deliveredAt": 1234567890});
        assert!(assert_within_return_window(&item).is_ok());
    }

    // -----------------------------------------------------------------------
    // Request deserialization edge cases (ported from Python returns_deep)
    // -----------------------------------------------------------------------

    #[test]
    fn test_create_return_request_missing_required_fields() {
        // Missing returnReason
        let s = r#"{"orderId":"o1","productId":"p1","userId":"u1"}"#;
        assert!(serde_json::from_str::<CreateReturnRequest>(s).is_err());

        // Missing productId
        let s = r#"{"orderId":"o1","userId":"u1","returnReason":"broken"}"#;
        assert!(serde_json::from_str::<CreateReturnRequest>(s).is_err());

        // Empty JSON
        assert!(serde_json::from_str::<CreateReturnRequest>(r#"{}"#).is_err());
    }

    #[test]
    fn test_approve_return_with_all_optional_fields() {
        let s = r#"{"returnId":"r1","userId":"u1","action":"issue_label","returnTrackingNumber":"TRK-ABC","returnAdminNote":"expedited"}"#;
        let req: ApproveReturnReq = serde_json::from_str(s).unwrap();
        assert_eq!(req.action, "issue_label");
        assert_eq!(req.return_tracking_number, Some("TRK-ABC".to_string()));
        assert_eq!(req.return_admin_note, Some("expedited".to_string()));
    }

    #[test]
    fn test_approve_return_mark_received_action() {
        let s = r#"{"returnId":"r1","userId":"u1","action":"mark_received"}"#;
        let req: ApproveReturnReq = serde_json::from_str(s).unwrap();
        assert_eq!(req.action, "mark_received");
        assert!(req.return_tracking_number.is_none());
    }

    #[test]
    fn test_reject_return_missing_reason() {
        let s = r#"{"returnId":"r1","userId":"u1"}"#;
        let req: RejectReturnReq = serde_json::from_str(s).unwrap();
        assert!(req.reason.is_none());
    }

    #[test]
    fn test_reject_return_missing_required_fields() {
        let s = r#"{"returnId":"r1"}"#;
        assert!(serde_json::from_str::<RejectReturnReq>(s).is_err());
    }

    // -----------------------------------------------------------------------
    // Response serialization edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_approve_return_response_serialization() {
        let resp = ApproveReturnResponse {
            success: true,
            new_status: "approved".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["newStatus"], "approved");
    }

    #[test]
    fn test_reject_return_response_serialization() {
        let resp = RejectReturnResponse {
            success: true,
            new_status: "rejected".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["newStatus"], "rejected");
    }

    #[test]
    fn test_create_return_response_serialization_camel_case() {
        let resp = CreateReturnResponse {
            success: true,
            return_id: "ret_abc_123".to_string(),
        };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("returnId"));
        assert!(!s.contains("return_id"));
    }

    // -----------------------------------------------------------------------
    // Helper functions edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_str_field_with_nested_object() {
        let v = json!({"nested": {"key": "value"}});
        assert_eq!(str_field(&v, "nested"), ""); // object, not string
    }

    #[test]
    fn test_bool_field_with_falsy_values() {
        let v = json!({"active": false, "count": 0, "name": ""});
        assert!(!bool_field(&v, "active"));
        assert!(!bool_field(&v, "count")); // not a bool
        assert!(!bool_field(&v, "name")); // not a bool
        assert!(!bool_field(&v, "nonexistent"));
    }

    #[test]
    fn test_items_array_with_non_array() {
        let order = json!({"items": "not_an_array"});
        assert!(items_array(&order).is_empty());
    }

    #[test]
    fn test_items_array_with_null() {
        let order = json!({"items": null});
        assert!(items_array(&order).is_empty());
    }

    #[test]
    fn test_items_array_with_multiple_items() {
        let order = json!({
            "items": [
                {"productId": "p1"},
                {"productId": "p2"},
                {"productId": "p3"},
            ]
        });
        assert_eq!(items_array(&order).len(), 3);
    }

    #[tokio::test]
    async fn test_create_return_request_rejects_duplicate_active_return() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                json!({
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        fields::STATUS: "delivered",
                        "deliveredAt": Utc::now().to_rfc3339(),
                    }],
                }),
            )
            .await
            .unwrap();
        let _ = state
            .db
            .create_document(
                collections::RETURN_REQUESTS,
                json!({
                    "orderId": "ord_1",
                    "productId": "prod_1",
                    "buyerId": "buyer_1",
                    "returnStatus": "requested",
                }),
            )
            .await
            .unwrap();

        let err = create_return_request(
            State(state),
            Json(CreateReturnRequest {
                order_id: "ord_1".into(),
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                return_reason: "Damaged".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn test_create_return_request_rejects_digital_item() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                json!({
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        "isDigital": true,
                        fields::STATUS: "delivered",
                    }],
                }),
            )
            .await
            .unwrap();

        let err = create_return_request(
            State(state),
            Json(CreateReturnRequest {
                order_id: "ord_1".into(),
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                return_reason: "No longer needed".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("Digital products cannot be returned")
        );
    }

    #[tokio::test]
    async fn test_create_return_request_success() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                json!({
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        fields::STATUS: "delivered",
                        "deliveredAt": Utc::now().to_rfc3339(),
                        "name": "Headphones",
                        "quantity": 1,
                    }],
                }),
            )
            .await
            .unwrap();

        let Json(resp) = create_return_request(
            State(state.clone()),
            Json(CreateReturnRequest {
                order_id: "ord_1".into(),
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                return_reason: "Damaged".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(!resp.return_id.is_empty());
    }

    #[tokio::test]
    async fn test_reject_return_request_rejects_non_owner() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_2",
                json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "sellerId": "seller_1",
                    "returnStatus": "requested",
                }),
            )
            .await
            .unwrap();

        let err = reject_return_request(
            State(state),
            Json(RejectReturnReq {
                return_id: "ret_1".into(),
                user_id: "seller_2".into(),
                reason: Some("No".into()),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Only the seller or admin"));
    }

    #[tokio::test]
    async fn test_escalate_return_request_only_buyer_can_escalate() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "buyerId": "buyer_1",
                    "orderId": "ord_1",
                    "returnStatus": "requested",
                }),
            )
            .await
            .unwrap();

        let err = escalate_return_request(
            State(state),
            Json(EscalateReturnReq {
                return_id: "ret_1".into(),
                user_id: "buyer_2".into(),
                escalation_reason: "Help".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Only the buyer can escalate"));
    }

    #[tokio::test]
    async fn test_approve_return_request_seller_can_approve_and_issue_label() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "sellerId": "seller_1",
                    "returnStatus": "requested",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = approve_return_request(
            State(state.clone()),
            Json(ApproveReturnReq {
                return_id: "ret_1".into(),
                user_id: "seller_1".into(),
                action: "approve".into(),
                return_tracking_number: Some("TRK-123".into()),
                return_admin_note: Some("approved quickly".into()),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.new_status, "approved");

        let updated = state
            .db
            .get_document(collections::RETURN_REQUESTS, "ret_1")
            .await
            .unwrap();
        assert_eq!(updated["returnStatus"], "approved");
        assert_eq!(updated["returnTrackingNumber"], "TRK-123");
        assert_eq!(updated["returnAdminNote"], "approved quickly");

        let Json(resp2) = approve_return_request(
            State(state.clone()),
            Json(ApproveReturnReq {
                return_id: "ret_1".into(),
                user_id: "seller_1".into(),
                action: "issue_label".into(),
                return_tracking_number: Some("TRK-456".into()),
                return_admin_note: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp2.new_status, "label_issued");
        let updated2 = state
            .db
            .get_document(collections::RETURN_REQUESTS, "ret_1")
            .await
            .unwrap();
        assert_eq!(updated2["returnStatus"], "label_issued");
        assert_eq!(updated2["returnTrackingNumber"], "TRK-456");
    }

    #[tokio::test]
    async fn test_approve_return_request_mark_received_refunds_and_restores_stock() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/refunds"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "re_123"
            })))
            .mount(&server)
            .await;

        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_secret_key".to_string(), "sk_test_123".to_string());
        let state = setup_state_with_config(config, server.uri()).await;

        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                json!({
                    fields::PRODUCT_ID: "prod_1",
                    "stockQuantity": 5,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                json!({
                    "paymentIntentId": "pi_123",
                    "subtotalCents": 5000,
                    fields::SHIPPING_COST_CENTS: 500,
                    "taxAmountCents": 650,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        "price": 50.0,
                        "quantity": 1,
                        "status": "delivered",
                    }],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "sellerId": "seller_1",
                    "orderId": "ord_1",
                    "productId": "prod_1",
                    "quantity": 1,
                    "returnStatus": "approved",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = approve_return_request(
            State(state.clone()),
            Json(ApproveReturnReq {
                return_id: "ret_1".into(),
                user_id: "seller_1".into(),
                action: "mark_received".into(),
                return_tracking_number: None,
                return_admin_note: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.new_status, "refunded");

        let return_doc = state
            .db
            .get_document(collections::RETURN_REQUESTS, "ret_1")
            .await
            .unwrap();
        let order = state
            .db
            .get_document(collections::ORDERS, "ord_1")
            .await
            .unwrap();
        let product = state
            .db
            .get_document(collections::PRODUCTS, "prod_1")
            .await
            .unwrap();

        assert_eq!(return_doc["returnStatus"], "refunded");
        assert_eq!(order[fields::ITEMS][0]["status"], "refunded");
        assert_eq!(order[fields::ITEMS][0]["refundId"], "re_123");
        assert_eq!(product["stockQuantity"], 6);
    }

    #[tokio::test]
    async fn test_reject_return_request_seller_can_reject_and_reason_is_sanitized() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "sellerId": "seller_1",
                    "returnStatus": "requested",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = reject_return_request(
            State(state.clone()),
            Json(RejectReturnReq {
                return_id: "ret_1".into(),
                user_id: "seller_1".into(),
                reason: Some("<b>Not eligible</b>".into()),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.new_status, "rejected");

        let updated = state
            .db
            .get_document(collections::RETURN_REQUESTS, "ret_1")
            .await
            .unwrap();
        assert_eq!(updated["returnStatus"], "rejected");
        assert_eq!(updated["rejectionReason"], "Not eligible");
    }

    #[tokio::test]
    async fn test_escalate_return_request_notifies_admins_and_creates_pending_push_records() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "admin_1",
                json!({
                    fields::UID: "admin_1",
                    fields::ROLES: ["admin"],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "admin_2",
                json!({
                    fields::UID: "admin_2",
                    fields::ROLES: ["admin"],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .query_raw("CREATE _push_tokens CONTENT { user_id: 'admin_1', token: 'tok_1' }")
            .await
            .unwrap();
        state
            .db
            .query_raw("CREATE _push_tokens CONTENT { user_id: 'admin_2', token: 'tok_2' }")
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "buyerId": "buyer_1",
                    "orderId": "ord_1",
                    "returnStatus": "requested",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = escalate_return_request(
            State(state.clone()),
            Json(EscalateReturnReq {
                return_id: "ret_1".into(),
                user_id: "buyer_1".into(),
                escalation_reason: "seller unresponsive".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.new_status, "escalated");

        let return_doc = state
            .db
            .get_document(collections::RETURN_REQUESTS, "ret_1")
            .await
            .unwrap();
        assert_eq!(return_doc["returnStatus"], "escalated");

        let notifications = state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(20))
            .await
            .unwrap()
            .into_iter()
            .filter(|doc| {
                str_field(doc, fields::NOTIFICATION_TYPE) == "return_escalated_admin"
                    || str_field(doc, "notificationType") == "return_escalated_admin"
            })
            .collect::<Vec<_>>();
        assert_eq!(notifications.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Coverage: create_return_request error paths
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_return_empty_reason() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                json!({
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::STATUS: "delivered",
                    }],
                }),
            )
            .await
            .unwrap();

        let err = create_return_request(
            State(state),
            Json(CreateReturnRequest {
                order_id: "ord_1".into(),
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                return_reason: "   ".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("returnReason is required"));
    }

    #[tokio::test]
    async fn test_create_return_not_buyer() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                json!({
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::STATUS: "delivered",
                    }],
                }),
            )
            .await
            .unwrap();

        let err = create_return_request(
            State(state),
            Json(CreateReturnRequest {
                order_id: "ord_1".into(),
                product_id: "prod_1".into(),
                user_id: "other_user".into(),
                return_reason: "Broken".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("your own orders"));
    }

    #[tokio::test]
    async fn test_create_return_item_not_delivered() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                json!({
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::STATUS: "shipped",
                    }],
                }),
            )
            .await
            .unwrap();

        let err = create_return_request(
            State(state),
            Json(CreateReturnRequest {
                order_id: "ord_1".into(),
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                return_reason: "Broken".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("must be marked as delivered"));
    }

    #[tokio::test]
    async fn test_create_return_duplicate_approved_return_blocked() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                json!({
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        fields::STATUS: "delivered",
                        "deliveredAt": Utc::now().to_rfc3339(),
                    }],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .create_document(
                collections::RETURN_REQUESTS,
                json!({
                    "orderId": "ord_1",
                    "productId": "prod_1",
                    "buyerId": "buyer_1",
                    "returnStatus": "approved",
                }),
            )
            .await
            .unwrap();

        let err = create_return_request(
            State(state),
            Json(CreateReturnRequest {
                order_id: "ord_1".into(),
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                return_reason: "Broken".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    // -----------------------------------------------------------------------
    // Coverage: approve_return_request error paths
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_approve_return_not_seller_or_admin() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "rando",
                json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "sellerId": "seller_1",
                    "returnStatus": "requested",
                }),
            )
            .await
            .unwrap();

        let err = approve_return_request(
            State(state),
            Json(ApproveReturnReq {
                return_id: "ret_1".into(),
                user_id: "rando".into(),
                action: "approve".into(),
                return_tracking_number: None,
                return_admin_note: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Only the seller or admin"));
    }

    #[tokio::test]
    async fn test_approve_return_invalid_approve_transition() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "sellerId": "seller_1",
                    "returnStatus": "received",
                }),
            )
            .await
            .unwrap();

        let err = approve_return_request(
            State(state),
            Json(ApproveReturnReq {
                return_id: "ret_1".into(),
                user_id: "seller_1".into(),
                action: "approve".into(),
                return_tracking_number: None,
                return_admin_note: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Cannot approve return"));
    }

    #[tokio::test]
    async fn test_approve_return_invalid_issue_label_transition() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "sellerId": "seller_1",
                    "returnStatus": "requested",
                }),
            )
            .await
            .unwrap();

        let err = approve_return_request(
            State(state),
            Json(ApproveReturnReq {
                return_id: "ret_1".into(),
                user_id: "seller_1".into(),
                action: "issue_label".into(),
                return_tracking_number: None,
                return_admin_note: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Cannot issue label"));
    }

    #[tokio::test]
    async fn test_approve_return_invalid_mark_received_transition() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "sellerId": "seller_1",
                    "returnStatus": "requested",
                }),
            )
            .await
            .unwrap();

        let err = approve_return_request(
            State(state),
            Json(ApproveReturnReq {
                return_id: "ret_1".into(),
                user_id: "seller_1".into(),
                action: "mark_received".into(),
                return_tracking_number: None,
                return_admin_note: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Cannot mark received"));
    }

    #[tokio::test]
    async fn test_approve_return_unknown_action() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "sellerId": "seller_1",
                    "returnStatus": "requested",
                }),
            )
            .await
            .unwrap();

        let err = approve_return_request(
            State(state),
            Json(ApproveReturnReq {
                return_id: "ret_1".into(),
                user_id: "seller_1".into(),
                action: "invalid_action".into(),
                return_tracking_number: None,
                return_admin_note: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Unknown action"));
    }

    #[tokio::test]
    async fn test_approve_return_mark_received_no_payment_intent() {
        // When paymentIntentId is empty, stripe_refund should be skipped
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                json!({
                    fields::PRODUCT_ID: "prod_1",
                    "stockQuantity": 10,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                json!({
                    "paymentIntentId": "",
                    "subtotalCents": 5000,
                    fields::SHIPPING_COST_CENTS: 500,
                    "taxAmountCents": 650,
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        "price": 50.0,
                        "quantity": 2,
                        "status": "delivered",
                    }],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "sellerId": "seller_1",
                    "orderId": "ord_1",
                    "productId": "prod_1",
                    "quantity": 2,
                    "returnStatus": "approved",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = approve_return_request(
            State(state.clone()),
            Json(ApproveReturnReq {
                return_id: "ret_1".into(),
                user_id: "seller_1".into(),
                action: "mark_received".into(),
                return_tracking_number: None,
                return_admin_note: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.new_status, "refunded");

        // Verify stock was restored
        let product = state
            .db
            .get_document(collections::PRODUCTS, "prod_1")
            .await
            .unwrap();
        assert_eq!(product["stockQuantity"], 12);

        // Verify order item updated
        let order = state
            .db
            .get_document(collections::ORDERS, "ord_1")
            .await
            .unwrap();
        assert_eq!(order[fields::ITEMS][0]["status"], "refunded");

        // Verify return request updated
        let ret = state
            .db
            .get_document(collections::RETURN_REQUESTS, "ret_1")
            .await
            .unwrap();
        assert_eq!(ret["returnStatus"], "refunded");

        // Verify order event was created
        let events = state
            .db
            .list_documents(collections::ORDER_EVENTS, Some(10))
            .await
            .unwrap();
        assert!(!events.is_empty());
    }

    // -----------------------------------------------------------------------
    // Coverage: reject_return_request invalid transition
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_reject_return_invalid_status_transition() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "sellerId": "seller_1",
                    "returnStatus": "refunded",
                }),
            )
            .await
            .unwrap();

        let err = reject_return_request(
            State(state),
            Json(RejectReturnReq {
                return_id: "ret_1".into(),
                user_id: "seller_1".into(),
                reason: Some("No".into()),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Cannot reject return"));
    }

    // -----------------------------------------------------------------------
    // Coverage: escalate_return_request error paths
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_escalate_return_empty_reason() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "buyerId": "buyer_1",
                    "orderId": "ord_1",
                    "returnStatus": "requested",
                }),
            )
            .await
            .unwrap();

        let err = escalate_return_request(
            State(state),
            Json(EscalateReturnReq {
                return_id: "ret_1".into(),
                user_id: "buyer_1".into(),
                escalation_reason: "   ".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("escalationReason is required"));
    }

    #[tokio::test]
    async fn test_escalate_return_invalid_status() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_1",
                json!({
                    "buyerId": "buyer_1",
                    "orderId": "ord_1",
                    "returnStatus": "refunded",
                }),
            )
            .await
            .unwrap();

        let err = escalate_return_request(
            State(state),
            Json(EscalateReturnReq {
                return_id: "ret_1".into(),
                user_id: "buyer_1".into(),
                escalation_reason: "Need help".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("only be escalated from requested or approved")
        );
    }

    // -----------------------------------------------------------------------
    // Coverage: notify_admins_of_return_escalation — push delivery paths
    // (Phase 2: FCM env vars not set → sent=false, records stay pending)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_notify_admins_escalation_covers_all_phases() {
        // Exercises notify_admins_of_return_escalation with multiple admins
        // having push tokens to cover Phase 1 (write notifications + pending push)
        // and Phase 2 (attempt delivery, mark delivered/stay pending).
        let state = setup_state().await;
        // Admin with push token
        state
            .db
            .upsert_document(
                collections::USERS,
                "adm1",
                json!({ fields::ROLES: ["admin"] }),
            )
            .await
            .unwrap();
        // Insert push token via raw query to match the query format in prod code
        state
            .db
            .query_raw("CREATE _push_tokens SET user_id = 'adm1', token = 'fcm_tok_1'")
            .await
            .unwrap();

        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_e1",
                json!({
                    "buyerId": "buyer_1",
                    "orderId": "ord_1",
                    "returnStatus": "approved",
                }),
            )
            .await
            .unwrap();

        // Escalate from "approved" state (valid)
        let Json(resp) = escalate_return_request(
            State(state.clone()),
            Json(EscalateReturnReq {
                return_id: "ret_e1".into(),
                user_id: "buyer_1".into(),
                escalation_reason: "still waiting".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.return_status, "escalated");

        // Verify the return request was actually updated in DB
        let ret = state
            .db
            .get_document(collections::RETURN_REQUESTS, "ret_e1")
            .await
            .unwrap();
        assert_eq!(ret["returnStatus"], "escalated");
        assert!(!ret["escalationReason"].as_str().unwrap_or("").is_empty());
    }

    // -----------------------------------------------------------------------
    // Coverage: create_return_request allows re-create after rejected return
    // (Line 453: for-loop body where status IS rejected/refunded — no error)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_return_after_rejected_return_succeeds() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                json!({
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        fields::STATUS: "delivered",
                        "deliveredAt": Utc::now().to_rfc3339(),
                        "name": "Widget",
                        "quantity": 1,
                    }],
                }),
            )
            .await
            .unwrap();
        // Insert a previously rejected return for same item
        state
            .db
            .create_document(
                collections::RETURN_REQUESTS,
                json!({
                    "orderId": "ord_1",
                    "productId": "prod_1",
                    "buyerId": "buyer_1",
                    "returnStatus": "rejected",
                }),
            )
            .await
            .unwrap();

        // Should succeed because existing return is rejected
        let Json(resp) = create_return_request(
            State(state.clone()),
            Json(CreateReturnRequest {
                order_id: "ord_1".into(),
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                return_reason: "Damaged on second look".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(!resp.return_id.is_empty());
    }

    #[tokio::test]
    async fn test_create_return_after_refunded_return_succeeds() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                json!({
                    "userId": "buyer_1",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                        fields::STATUS: "delivered",
                        "deliveredAt": Utc::now().to_rfc3339(),
                        "name": "Widget",
                        "quantity": 1,
                    }],
                }),
            )
            .await
            .unwrap();
        // Insert a previously refunded return for same item
        state
            .db
            .create_document(
                collections::RETURN_REQUESTS,
                json!({
                    "orderId": "ord_1",
                    "productId": "prod_1",
                    "buyerId": "buyer_1",
                    "returnStatus": "refunded",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = create_return_request(
            State(state.clone()),
            Json(CreateReturnRequest {
                order_id: "ord_1".into(),
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                return_reason: "Need another return".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(!resp.return_id.is_empty());
    }

    // -----------------------------------------------------------------------
    // Coverage: notify_admins_of_return_escalation Phase 2 with FCM env vars
    // Lines 246-335: Notification upsert error paths + push delivery loop
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_escalate_with_fcm_env_vars_covers_phase2_push_delivery() {
        // Set FCM env vars to trigger the send_push path (Phase 2, lines 301-318)
        // Since send_push will fail (no real FCM), sent=false, push stays "pending"
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::USERS,
                "adm_fcm1",
                json!({
                    fields::UID: "adm_fcm1",
                    fields::ROLES: ["admin"],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .query_raw("CREATE _push_tokens SET user_id = 'adm_fcm1', token = 'fcm_token_abc'")
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_fcm1",
                json!({
                    "buyerId": "buyer_fcm1",
                    "orderId": "ord_fcm1",
                    "returnStatus": "requested",
                }),
            )
            .await
            .unwrap();

        // Set FCM env vars so Phase 2 enters the send_push branch
        // SAFETY: test is single-threaded for env var access
        unsafe {
            std::env::set_var("OB_FCM_PROJECT_ID", "test-project-id");
            std::env::set_var("OB_FCM_SERVICE_ACCOUNT", "test-service-account");
        }

        let result = escalate_return_request(
            State(state.clone()),
            Json(EscalateReturnReq {
                return_id: "ret_fcm1".into(),
                user_id: "buyer_fcm1".into(),
                escalation_reason: "still waiting for response".into(),
            }),
        )
        .await;

        // Clean up env vars
        // SAFETY: test cleanup
        unsafe {
            std::env::remove_var("OB_FCM_PROJECT_ID");
            std::env::remove_var("OB_FCM_SERVICE_ACCOUNT");
        }

        let Json(resp) = result.unwrap();
        assert!(resp.success);
        assert_eq!(resp.new_status, "escalated");

        // Verify notification was created for the admin
        let notifications = state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(20))
            .await
            .unwrap()
            .into_iter()
            .filter(|doc| {
                str_field(doc, fields::NOTIFICATION_TYPE) == "return_escalated_admin"
                    || str_field(doc, "notificationType") == "return_escalated_admin"
            })
            .collect::<Vec<_>>();
        assert_eq!(notifications.len(), 1);
    }

    #[tokio::test]
    async fn test_escalate_with_multiple_admins_multiple_tokens_covers_push_loops() {
        let state = setup_state().await;
        // Two admins, each with 2 push tokens
        for (admin_id, roles) in [("adm_m1", "admin"), ("adm_m2", "admin")] {
            state
                .db
                .upsert_document(
                    collections::USERS,
                    admin_id,
                    json!({
                        fields::UID: admin_id,
                        fields::ROLES: [roles],
                    }),
                )
                .await
                .unwrap();
        }
        state
            .db
            .query_raw("CREATE _push_tokens SET user_id = 'adm_m1', token = 'tok_m1a'")
            .await
            .unwrap();
        state
            .db
            .query_raw("CREATE _push_tokens SET user_id = 'adm_m1', token = 'tok_m1b'")
            .await
            .unwrap();
        state
            .db
            .query_raw("CREATE _push_tokens SET user_id = 'adm_m2', token = 'tok_m2a'")
            .await
            .unwrap();

        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_multi",
                json!({
                    "buyerId": "buyer_multi",
                    "orderId": "ord_multi",
                    "returnStatus": "requested",
                }),
            )
            .await
            .unwrap();

        // Without FCM env vars, Phase 2 skips send_push (lines 317-318: sent=false)
        let Json(resp) = escalate_return_request(
            State(state.clone()),
            Json(EscalateReturnReq {
                return_id: "ret_multi".into(),
                user_id: "buyer_multi".into(),
                escalation_reason: "urgent matter".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);

        // Verify Phase 1 created notifications for both admins
        let notifications = state
            .db
            .list_documents(collections::NOTIFICATIONS, Some(20))
            .await
            .unwrap()
            .into_iter()
            .filter(|doc| {
                str_field(doc, fields::NOTIFICATION_TYPE) == "return_escalated_admin"
                    || str_field(doc, "notificationType") == "return_escalated_admin"
            })
            .collect::<Vec<_>>();
        assert_eq!(
            notifications.len(),
            2,
            "Should have notification for each admin"
        );
    }

    // -----------------------------------------------------------------------
    // Coverage: mark_received with Stripe refund (covers lines 648-708)
    // These lines are the success path, already partly covered. Additional
    // test to cover multi-item scenario and verify all DB updates.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_mark_received_with_stripe_refund_covers_stock_return_order_event() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/refunds"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "re_456"
            })))
            .mount(&server)
            .await;

        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_secret_key".to_string(), "sk_test_456".to_string());
        let state = setup_state_with_config(config, server.uri()).await;

        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_mr",
                json!({ fields::ROLES: ["seller"] }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_mr",
                json!({
                    fields::PRODUCT_ID: "prod_mr",
                    "stockQuantity": 3,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_mr",
                json!({
                    "paymentIntentId": "pi_mr",
                    "subtotalCents": 10000,
                    fields::SHIPPING_COST_CENTS: 1000,
                    "taxAmountCents": 1300,
                    fields::ITEMS: [
                        {
                            fields::PRODUCT_ID: "prod_mr",
                            fields::SELLER_ID: "seller_mr",
                            "price": 100.0,
                            "quantity": 3,
                            "status": "delivered",
                        },
                        {
                            fields::PRODUCT_ID: "prod_other",
                            fields::SELLER_ID: "seller_mr",
                            "price": 25.0,
                            "quantity": 1,
                            "status": "delivered",
                        },
                    ],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::RETURN_REQUESTS,
                "ret_mr",
                json!({
                    "sellerId": "seller_mr",
                    "orderId": "ord_mr",
                    "productId": "prod_mr",
                    "quantity": 3,
                    "returnStatus": "approved",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = approve_return_request(
            State(state.clone()),
            Json(ApproveReturnReq {
                return_id: "ret_mr".into(),
                user_id: "seller_mr".into(),
                action: "mark_received".into(),
                return_tracking_number: None,
                return_admin_note: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.new_status, "refunded");

        // Verify stock was restored by qty (lines 639-652)
        let product = state
            .db
            .get_document(collections::PRODUCTS, "prod_mr")
            .await
            .unwrap();
        assert_eq!(product["stockQuantity"], 6); // 3 + 3

        // Verify return request updated (lines 654-671)
        let ret = state
            .db
            .get_document(collections::RETURN_REQUESTS, "ret_mr")
            .await
            .unwrap();
        assert_eq!(ret["returnStatus"], "refunded");
        assert!(ret.get("resolvedAt").is_some());

        // Verify order items updated (lines 673-688)
        let order = state
            .db
            .get_document(collections::ORDERS, "ord_mr")
            .await
            .unwrap();
        assert_eq!(order[fields::ITEMS][0]["status"], "refunded");
        assert_eq!(order[fields::ITEMS][0]["refundId"], "re_456");
        // Other item should be unchanged
        assert_eq!(order[fields::ITEMS][1]["status"], "delivered");

        // Verify order event was created (lines 690-708)
        let events = state
            .db
            .list_documents(collections::ORDER_EVENTS, Some(10))
            .await
            .unwrap();
        assert!(!events.is_empty());
        let event = &events[0];
        assert_eq!(
            event
                .get("eventType")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            "return_received_and_refunded"
        );
    }
}
