//! Chat messaging handlers — product-scoped buyer↔seller messaging.
//! Ported from: functions/handlers/chat.py

use axum::{Extension, Json, Router, extract::State, routing::post};
use ob_auth::middleware::AuthContext;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::HandlersState;
use crate::shared::rate_limiter::check_user_rate_limit;
use crate::shared::schema::{collections, fields};
use crate::shared::validation::{sanitize_html, validate_uid};

// ─── Constants ──────────────────────────────────────────────────────────────

const MAX_MESSAGE_LENGTH: usize = 2000;
const MAX_IMAGES_PER_MESSAGE: usize = 5;
const MAX_MESSAGES_PER_THREAD: i64 = 10_000;
const CHAT_RATE_LIMIT_PER_MINUTE: u64 = 60;

// ─── Request / Response Structs ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetOrCreateChatRequest {
    pub other_user_id: String,
    pub product_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetOrCreateChatResponse {
    pub chat_id: String,
    pub is_new: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageRequest {
    pub chat_id: String,
    pub message_text: Option<String>,
    pub image_urls: Option<Vec<String>>,
    pub message_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageResponse {
    pub success: bool,
    pub message_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkReadRequest {
    pub chat_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkReadResponse {
    pub success: bool,
    pub count: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteMessageRequest {
    pub chat_id: String,
    pub message_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuccessResponse {
    pub success: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportMessageRequest {
    pub chat_id: String,
    pub message_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportResponse {
    pub success: bool,
    pub report_id: String,
}

// ─── Router ─────────────────────────────────────────────────────────────────

// ─── Support Chat Handler ───────────────────────────────────────────────────
// Customer support chat with Claude AI integration

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportChatRequest {
    pub messages: Vec<SupportMessage>,
    pub customer_email: String,
    pub customer_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportChatResponse {
    pub reply: String,
    pub escalated: bool,
}

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: i32,
    system: String,
    messages: Vec<AnthropicMessage>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContent {
    text: String,
}

/// Support chat endpoint: accepts customer messages and returns AI-generated responses
pub async fn support_chat(
    State(state): State<HandlersState>,
    Json(payload): Json<SupportChatRequest>,
) -> Result<Json<SupportChatResponse>, (axum::http::StatusCode, String)> {
    // Get Anthropic API key from secrets
    let api_key = state
        .config
        .secrets
        .get("anthropic_api_key")
        .ok_or_else(|| {
            tracing::error!("Missing ANTHROPIC_API_KEY secret");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Configuration error: AI service unavailable".to_string(),
            )
        })?;

    // System prompt for customer support
    let system_prompt = "You are a helpful and friendly customer support agent for OrignaGTA, a Canadian e-commerce platform specializing in handcrafted goods and sustainable products.

Your responsibilities:
- Help customers with product inquiries, order issues, shipping, and returns
- Be empathetic and professional in all interactions
- Provide accurate information about our policies
- Offer practical solutions
- If you cannot resolve an issue or the customer repeatedly needs escalation, indicate that the issue should be escalated to a human agent

Always be clear and concise. Format responses in a friendly, conversational tone.";

    // Build the request to Anthropic API
    let anthropic_request = AnthropicRequest {
        model: "claude-3-5-sonnet-20241022".to_string(),
        max_tokens: 1024,
        system: system_prompt.to_string(),
        messages: payload
            .messages
            .iter()
            .map(|m| AnthropicMessage {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect(),
    };

    // Call Anthropic API with timeout
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state
            .http_client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&anthropic_request)
            .send(),
    )
    .await
    .map_err(|_| {
        tracing::error!("Anthropic API timeout");
        (
            axum::http::StatusCode::GATEWAY_TIMEOUT,
            "Support service timeout".to_string(),
        )
    })?
    .map_err(|e| {
        tracing::error!("Anthropic API request error: {}", e);
        (
            axum::http::StatusCode::BAD_GATEWAY,
            "Support service unavailable".to_string(),
        )
    })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        tracing::error!("Anthropic API error: {} {}", status, body);
        return Err((
            axum::http::StatusCode::BAD_GATEWAY,
            "Support service error".to_string(),
        ));
    }

    let anthropic_response: AnthropicResponse = response.json().await.map_err(|e| {
        tracing::error!("Failed to parse Anthropic response: {}", e);
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "Response parsing error".to_string(),
        )
    })?;

    // Extract the reply text
    let reply = anthropic_response
        .content
        .first()
        .map(|c| c.text.clone())
        .unwrap_or_default();

    // Detect if escalation is needed based on keywords in the response
    let escalated = reply.to_lowercase().contains("escalat")
        || reply.to_lowercase().contains("human agent")
        || reply.to_lowercase().contains("supervisor")
        || reply.to_lowercase().contains("manager");

    Ok(Json(SupportChatResponse { reply, escalated }))
}

// ─── Router ─────────────────────────────────────────────────────────────────

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/chat/get-or-create", post(get_or_create_chat))
        .route("/api/chat/send", post(send_message))
        .route("/api/chat/mark-read", post(mark_messages_read))
        .route("/api/chat/delete-message", post(delete_message))
        .route("/api/chat/report", post(report_message))
        .route("/api/support/chat", post(support_chat))
        .with_state(state)
}
// ─── Helpers ────────────────────────────────────────────────────────────────

use std::sync::OnceLock;

static ZERO_WIDTH_RE: OnceLock<regex_lite::Regex> = OnceLock::new();
static SCRIPT_TAG_RE: OnceLock<regex_lite::Regex> = OnceLock::new();
static JS_SCHEME_RE: OnceLock<regex_lite::Regex> = OnceLock::new();
static EMAIL_RE: OnceLock<regex_lite::Regex> = OnceLock::new();
static URL_RE: OnceLock<regex_lite::Regex> = OnceLock::new();
static WWW_RE: OnceLock<regex_lite::Regex> = OnceLock::new();
static PHONE_RE: OnceLock<regex_lite::Regex> = OnceLock::new();

fn _sanitize_text(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    // Strip zero-width chars and other invisible whitespace
    let zero_width = ZERO_WIDTH_RE.get_or_init(|| {
        regex_lite::Regex::new(r"[\u{200B}\u{200C}\u{200D}\u{FEFF}]").expect("valid regex")
    });
    let text = zero_width.replace_all(text, "");

    // Strip HTML and script tags
    let script_tag = SCRIPT_TAG_RE.get_or_init(|| {
        regex_lite::Regex::new("(?i)<script[^>]*>.*?</script>").expect("valid regex")
    });
    let text = script_tag.replace_all(&text, "");

    let clean = sanitize_html(&text);

    let js_scheme = JS_SCHEME_RE
        .get_or_init(|| regex_lite::Regex::new("(?i)javascript:").expect("valid regex"));
    let text = js_scheme.replace_all(&clean, "");

    // Redact email addresses
    let email_pat = EMAIL_RE.get_or_init(|| {
        regex_lite::Regex::new(r"(?i)\b[\w._%+\-]+(\s*[@\[(]at[\])]\s*|@)[\w.\-]+\.[a-zA-Z]{2,}\b")
            .expect("valid regex")
    });
    let text = email_pat.replace_all(&text, "[email removed]");

    // Redact URLs and web links
    let url_pat =
        URL_RE.get_or_init(|| regex_lite::Regex::new(r"(?i)https?://[^\s]+").expect("valid regex"));
    let text = url_pat.replace_all(&text, "[link removed]");
    let www_pat =
        WWW_RE.get_or_init(|| regex_lite::Regex::new(r"(?i)www\.[^\s]+").expect("valid regex"));
    let text = www_pat.replace_all(&text, "[link removed]");

    // Redact phone numbers (10-15 digits)
    let phone_pat = PHONE_RE.get_or_init(|| {
        regex_lite::Regex::new(r"(\+?[\d\s\-\.()]{10,20}\d)").expect("valid regex")
    });
    let text = phone_pat.replace_all(&text, |caps: &regex_lite::Captures| {
        let raw = &caps[0];
        let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 10 && digits.len() <= 15 {
            "[phone removed]".to_string()
        } else {
            raw.to_string()
        }
    });

    text.trim().to_string()
}

async fn check_premium(
    db: &ob_database::DatabaseClient,
    user_id: &str,
) -> Result<bool, ob_core::Error> {
    // In Python this was authoritative from subscriptions/{uid}
    let sub = match db.get_document(collections::SUBSCRIPTIONS, user_id).await {
        Ok(doc) => doc,
        Err(ob_core::Error::NotFound(_)) => return Ok(false),
        Err(e) => return Err(e),
    };
    if sub.is_null() {
        return Ok(false);
    }

    Ok(sub.get(fields::STATUS).and_then(|v| v.as_str()) == Some("active"))
}

fn product_scoped_chat_id(product_id: &str, buyer_id: &str) -> String {
    format!("{product_id}_{buyer_id}")
}

fn parse_rfc3339(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}
async fn buyer_has_chat_eligible_order(
    state: &HandlersState,
    buyer_id: &str,
    product_id: &str,
) -> Result<bool, ob_core::Error> {
    let query = format!(
        "SELECT * FROM {} WHERE data->>'userId' = $buyer_id AND data->>'{}' IN ('delivered', 'disputed') LIMIT 50",
        collections::ORDERS,
        fields::ORDER_STATUS
    );

    let orders = state
        .db
        .query_bind(&query, serde_json::json!({ "buyer_id": buyer_id }))
        .await?;

    Ok(orders.iter().any(|order| {
        order
            .get(fields::PRODUCT_IDS)
            .and_then(|v| v.as_array())
            .map(|ids| ids.iter().any(|id| id.as_str() == Some(product_id)))
            .unwrap_or(false)
    }))
}

// ─── Handlers ───────────────────────────────────────────────────────────────

async fn get_or_create_chat(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<GetOrCreateChatRequest>,
) -> Result<Json<GetOrCreateChatResponse>, ob_core::Error> {
    if !auth.authenticated {
        return Err(ob_core::Error::Auth("Authentication required.".into()));
    }
    let buyer_id = &auth.user_id;

    let product_id = req
        .product_id
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ob_core::Error::Validation("productId is required.".into()))?;

    validate_uid("productId", product_id)?;

    // Premium gate
    if !check_premium(&state.db, buyer_id).await? {
        return Err(ob_core::Error::Forbidden(
            "Premium subscription required to chat with sellers.".into(),
        ));
    }

    let product = state
        .db
        .get_document(collections::PRODUCTS, product_id)
        .await?;

    if product.is_null() {
        return Err(ob_core::Error::NotFound("Product not found.".into()));
    }

    let product_status = product
        .get(fields::LIFECYCLE_STATUS)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if product_status != "ACTIVE" {
        return Err(ob_core::Error::NotFound(
            "Product is no longer active.".into(),
        ));
    }

    let seller_id = product
        .get(fields::SELLER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if seller_id == buyer_id {
        return Err(ob_core::Error::Forbidden(
            "You cannot chat with yourself.".into(),
        ));
    }

    // Eligible order check
    if !buyer_has_chat_eligible_order(&state, buyer_id, product_id).await? {
        return Err(ob_core::Error::Validation(
            "You must have a delivered order for this product before starting a chat with the seller.".into(),
        ));
    }

    let chat_id = product_scoped_chat_id(product_id, buyer_id);

    // Attempt atomic create (upsert with condition or just check first)
    let existing_chat = match state.db.get_document(collections::CHATS, &chat_id).await {
        Ok(doc) => doc,
        Err(ob_core::Error::NotFound(_)) => serde_json::Value::Null,
        Err(e) => return Err(e),
    };

    if !existing_chat.is_null() {
        return Ok(Json(GetOrCreateChatResponse {
            chat_id,
            is_new: false,
        }));
    }

    let now = chrono::Utc::now().to_rfc3339();
    let product_name = product
        .get(fields::NAME)
        .and_then(|v| v.as_str())
        .unwrap_or("Product");
    let product_image = product
        .get(fields::IMAGE_URLS)
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let new_chat = serde_json::json!({
        fields::CHAT_ID: chat_id,
        fields::PRODUCT_ID: product_id,
        fields::BUYER_ID: buyer_id,
        fields::SELLER_ID: seller_id,
        fields::PRODUCT_TITLE: product_name,
        fields::PRODUCT_IMAGE_URL: product_image,
        fields::BUYER_UNREAD_COUNT: 0,
        fields::SELLER_UNREAD_COUNT: 0,
        fields::MESSAGE_COUNT: 0,
        fields::CREATED_AT: now,
        fields::UPDATED_AT: now,
    });

    state
        .db
        .upsert_document(collections::CHATS, &chat_id, new_chat)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to create chat: {e}")))?;

    Ok(Json(GetOrCreateChatResponse {
        chat_id,
        is_new: true,
    }))
}

async fn send_message(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Json<SendMessageResponse>, ob_core::Error> {
    if !auth.authenticated {
        return Err(ob_core::Error::Auth("Authentication required.".into()));
    }
    let uid = &auth.user_id;

    let text_raw = req.message_text.as_deref().unwrap_or("");
    let image_urls = req.image_urls.as_ref();

    if req.chat_id.is_empty() || (text_raw.is_empty() && image_urls.is_none_or(|v| v.is_empty())) {
        return Err(ob_core::Error::Validation(
            "chatId and text/images required.".into(),
        ));
    }

    if let Some(urls) = image_urls {
        if urls.len() > MAX_IMAGES_PER_MESSAGE {
            return Err(ob_core::Error::Validation(format!(
                "Maximum {MAX_IMAGES_PER_MESSAGE} images per message."
            )));
        }
        // HTTPS-only enforcement
        for url in urls {
            if !url.starts_with("https://") {
                return Err(ob_core::Error::Validation(
                    "Image URLs must use HTTPS".into(),
                ));
            }
        }
        // Simplified CDN check - in production would use full business_rules::CDN_BASE_URL
        for url in urls {
            if !url.contains("storage.googleapis.com") && !url.contains("cdn") {
                return Err(ob_core::Error::Validation(
                    "Chat images must be uploaded to the Origna CDN before sending.".into(),
                ));
            }
        }
    }

    let text = _sanitize_text(text_raw);
    if !text_raw.trim().is_empty() && text.is_empty() {
        return Err(ob_core::Error::Validation(
            "Message text is too short after sanitization.".into(),
        ));
    }
    if text.len() > MAX_MESSAGE_LENGTH {
        return Err(ob_core::Error::Validation(format!(
            "Message exceeds {MAX_MESSAGE_LENGTH} characters."
        )));
    }

    let chat = state
        .db
        .get_document(collections::CHATS, &req.chat_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Chat thread not found.".into()))?;

    let buyer_id = chat
        .get(fields::BUYER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let seller_id = chat
        .get(fields::SELLER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if uid != buyer_id && uid != seller_id {
        return Err(ob_core::Error::Forbidden("Access denied.".into()));
    }

    // Deduplication guard
    let last_text = chat.get(fields::LAST_MESSAGE_TEXT).and_then(|v| v.as_str());
    let last_update = chat
        .get(fields::UPDATED_AT)
        .and_then(|v| v.as_str())
        .and_then(parse_rfc3339);
    if !text.is_empty()
        && last_text == Some(&text)
        && let Some(update_ts) = last_update
        && (chrono::Utc::now() - update_ts).num_seconds() < 5
    {
        return Err(ob_core::Error::Validation("Message already sent.".into()));
    }

    // Thread capacity check
    let msg_count = chat
        .get(fields::MESSAGE_COUNT)
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if msg_count >= MAX_MESSAGES_PER_THREAD {
        return Err(ob_core::Error::Validation(
            "Chat limit reached for this thread.".into(),
        ));
    }

    // Premium check for buyer
    if uid == buyer_id && !check_premium(&state.db, uid).await? {
        return Err(ob_core::Error::Forbidden("Premium required.".into()));
    }

    // Rate limiting
    check_user_rate_limit(
        &state.db,
        uid,
        "send_message",
        CHAT_RATE_LIMIT_PER_MINUTE,
        1,
    )
    .await?;

    let message_id = req
        .message_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let msg_collection = format!("{}__{}", collections::CHATS, collections::CHAT_MESSAGES);

    // Idempotency check for explicit message_id
    if req.message_id.is_some() {
        let existing = state.db.get_document(&msg_collection, &message_id).await?;
        if !existing.is_null() {
            return Ok(Json(SendMessageResponse {
                success: true,
                message_id,
            }));
        }
    }

    let now = chrono::Utc::now().to_rfc3339();
    let sender = state.db.get_document(collections::USERS, uid).await?;
    let sender_name = sender
        .get(fields::NAME)
        .and_then(|v| v.as_str())
        .unwrap_or("Someone");

    let msg_doc = json!({
        fields::CHAT_ID: req.chat_id,
        fields::SENDER_ID: uid,
        fields::SENDER_DISPLAY_NAME: sender_name,
        fields::MESSAGE_TEXT: text,
        fields::IMAGE_URLS: image_urls.cloned().unwrap_or_default(),
        fields::CREATED_AT: now,
        fields::READ: false,
        fields::DELETED: false,
    });

    state
        .db
        .upsert_document(&msg_collection, &message_id, msg_doc)
        .await?;

    // Update thread
    let target_unread = if uid == buyer_id {
        fields::SELLER_UNREAD_COUNT
    } else {
        fields::BUYER_UNREAD_COUNT
    };
    let mut thread_update = json!({
        fields::LAST_MESSAGE: if text.len() > 100 { &text[..100] } else { &text },
        fields::LAST_MESSAGE_TEXT: text,
        fields::LAST_MESSAGE_AT: now,
        fields::UPDATED_AT: now,
        fields::MESSAGE_COUNT: msg_count + 1,
        target_unread: chat.get(target_unread).and_then(|v| v.as_i64()).unwrap_or(0) + 1,
    });

    // Metrics
    if uid == buyer_id && chat.get(fields::FIRST_BUYER_MESSAGE_AT).is_none() {
        thread_update[fields::FIRST_BUYER_MESSAGE_AT] = json!(now);
    } else if uid == seller_id
        && chat.get(fields::FIRST_SELLER_REPLY_AT).is_none()
        && let Some(first_buyer_at) = chat
            .get(fields::FIRST_BUYER_MESSAGE_AT)
            .and_then(|v| v.as_str())
            .and_then(parse_rfc3339)
    {
        let hours = (chrono::Utc::now() - first_buyer_at).num_minutes() as f64 / 60.0;
        thread_update[fields::FIRST_SELLER_REPLY_AT] = json!(now);
        thread_update[fields::FIRST_REPLY_HOURS] = json!(hours);
    }

    state
        .db
        .update_document(collections::CHATS, &req.chat_id, thread_update)
        .await?;

    // Notify recipient (Best effort)
    let _recipient_id = if uid == buyer_id { seller_id } else { buyer_id };
    let _ = crate::push::send_push(
        &state.http_client,
        "orignabase", // Project ID placeholder
        "",           // Would need service account
        "",           // Would need recipient token
        &format!("Message from {sender_name}"),
        if text.is_empty() {
            "Sent an image"
        } else {
            &text
        },
        None,
    )
    .await;

    Ok(Json(SendMessageResponse {
        success: true,
        message_id,
    }))
}

async fn mark_messages_read(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<MarkReadRequest>,
) -> Result<Json<MarkReadResponse>, ob_core::Error> {
    if !auth.authenticated {
        return Err(ob_core::Error::Auth("Authentication required.".into()));
    }
    let uid = &auth.user_id;

    if req.chat_id.is_empty() {
        return Err(ob_core::Error::Validation("chatId is required.".into()));
    }

    let chat = state
        .db
        .get_document(collections::CHATS, &req.chat_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Chat thread not found.".into()))?;

    let buyer_id = chat
        .get(fields::BUYER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let seller_id = chat
        .get(fields::SELLER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if uid != buyer_id && uid != seller_id {
        return Err(ob_core::Error::Forbidden("Access denied.".into()));
    }

    // Mark messages read in batch
    let msg_collection = format!("{}__{}", collections::CHATS, collections::CHAT_MESSAGES);
    let query = format!(
        "UPDATE {} SET data = data || '{{\"read\": true}}'::jsonb WHERE data->>'chatId' = $chat_id AND data->>'senderId' != $uid AND data @> '{{\"read\": false}}'::jsonb RETURNING id, data::TEXT, created_at, updated_at",
        msg_collection
    );

    let updated = state
        .db
        .query_bind(&query, json!({ "chat_id": req.chat_id, "uid": uid }))
        .await?;
    let count = updated.len() as i64;

    if count > 0 {
        let unread_field = if uid == buyer_id {
            fields::BUYER_UNREAD_COUNT
        } else {
            fields::SELLER_UNREAD_COUNT
        };
        state
            .db
            .update_document(collections::CHATS, &req.chat_id, json!({ unread_field: 0 }))
            .await?;
    }

    Ok(Json(MarkReadResponse {
        success: true,
        count,
    }))
}

async fn delete_message(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<DeleteMessageRequest>,
) -> Result<Json<SuccessResponse>, ob_core::Error> {
    if !auth.authenticated {
        return Err(ob_core::Error::Auth("Authentication required.".into()));
    }
    let uid = &auth.user_id;

    if req.chat_id.is_empty() || req.message_id.is_empty() {
        return Err(ob_core::Error::Validation(
            "chatId and messageId required.".into(),
        ));
    }

    let msg_collection = format!("{}__{}", collections::CHATS, collections::CHAT_MESSAGES);
    let msg = state
        .db
        .get_document(&msg_collection, &req.message_id)
        .await?;
    if msg.is_null() {
        return Err(ob_core::Error::NotFound("Message not found.".into()));
    }

    if msg
        .get(fields::DELETED)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Ok(Json(SuccessResponse { success: true }));
    }

    let sender_id = msg
        .get(fields::SENDER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_sender = sender_id == uid;
    let is_admin = auth.has_role("admin");

    if !is_sender && !is_admin {
        return Err(ob_core::Error::Forbidden(
            "Only the sender or an admin can delete a message.".into(),
        ));
    }

    state
        .db
        .update_document(
            &msg_collection,
            &req.message_id,
            json!({
                fields::DELETED: true,
                fields::MESSAGE_TEXT: "",
                fields::IMAGE_URLS: Vec::<String>::new(),
                fields::UPDATED_AT: chrono::Utc::now().to_rfc3339(),
            }),
        )
        .await?;

    Ok(Json(SuccessResponse { success: true }))
}

async fn report_message(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<ReportMessageRequest>,
) -> Result<Json<ReportResponse>, ob_core::Error> {
    if !auth.authenticated {
        return Err(ob_core::Error::Auth("Authentication required.".into()));
    }
    let uid = &auth.user_id;

    if req.chat_id.is_empty() || req.message_id.is_empty() {
        return Err(ob_core::Error::Validation(
            "chatId and messageId required.".into(),
        ));
    }

    let chat = state
        .db
        .get_document(collections::CHATS, &req.chat_id)
        .await?;
    if chat.is_null() {
        return Err(ob_core::Error::NotFound("Chat thread not found.".into()));
    }

    let buyer_id = chat
        .get(fields::BUYER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let seller_id = chat
        .get(fields::SELLER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if uid != buyer_id && uid != seller_id {
        return Err(ob_core::Error::Forbidden(
            "You are not a participant in this chat.".into(),
        ));
    }

    let msg_collection = format!("{}__{}", collections::CHATS, collections::CHAT_MESSAGES);
    let msg = state
        .db
        .get_document(&msg_collection, &req.message_id)
        .await?;
    if msg.is_null() {
        return Err(ob_core::Error::NotFound("Message not found.".into()));
    }

    let report_id = uuid::Uuid::new_v4().to_string();
    let report_data = json!({
        fields::REPORT_ID: report_id,
        fields::CHAT_ID: req.chat_id,
        fields::MESSAGE_ID: req.message_id,
        fields::REPORTER_ID: uid,
        fields::REASON: req.reason.unwrap_or_else(|| "Inappropriate content".into()),
        fields::MESSAGE_TEXT: msg.get(fields::MESSAGE_TEXT).and_then(|v| v.as_str()).unwrap_or(""),
        fields::SENDER_ID: msg.get(fields::SENDER_ID).and_then(|v| v.as_str()).unwrap_or(""),
        fields::STATUS: "pending",
        fields::CREATED_AT: chrono::Utc::now().to_rfc3339(),
    });

    state
        .db
        .upsert_document(collections::MESSAGE_REPORTS, &report_id, report_data)
        .await?;

    Ok(Json(ReportResponse {
        success: true,
        report_id,
    }))
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use std::sync::Arc;

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

    fn auth_ctx(uid: &str) -> AuthContext {
        AuthContext {
            user_id: uid.into(),
            authenticated: true,
            ..AuthContext::anonymous()
        }
    }

    #[test]
    fn test_sanitize_chat_text() {
        assert_eq!(_sanitize_text("<b>hello</b>"), "hello");
        assert!(!_sanitize_text("<script>alert('xss')</script>").contains("<script>"));
        assert!(!_sanitize_text("Click: javascript:void(0)").contains("javascript:"));

        let result = _sanitize_text("Contact me at test@example.com please");
        assert!(result.contains("[email removed]"));
        assert!(!result.contains("test@example.com"));

        let result = _sanitize_text("Email me at test (at) example.com");
        assert!(result.contains("[email removed]"));

        assert!(_sanitize_text("Check http://example.com").contains("[link removed]"));
        assert!(_sanitize_text("Visit https://evil.com").contains("[link removed]"));
        assert!(_sanitize_text("Go to www.example.com").contains("[link removed]"));

        assert!(_sanitize_text("Call me at 416-555-1234").contains("[phone removed]"));

        // Zero-width
        assert_eq!(
            _sanitize_text("test\u{200B}@\u{200B}example.com"),
            "[email removed]"
        );
    }

    #[test]
    fn test_product_scoped_chat_id() {
        assert_eq!(product_scoped_chat_id("p1", "b1"), "p1_b1");
    }

    #[test]
    fn test_parse_rfc3339() {
        assert!(parse_rfc3339("2026-03-10T12:00:00Z").is_some());
        assert!(parse_rfc3339("invalid").is_none());
    }

    #[tokio::test]
    async fn test_get_or_create_chat_unauthenticated() {
        let state = setup_state().await;
        let auth = AuthContext::anonymous();
        let req = GetOrCreateChatRequest {
            other_user_id: "s1".into(),
            product_id: Some("p1".into()),
        };
        let result = get_or_create_chat(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Authentication required")
        );
    }

    #[tokio::test]
    async fn test_send_message_unauthenticated() {
        let state = setup_state().await;
        let auth = AuthContext::anonymous();
        let req = SendMessageRequest {
            chat_id: "c1".into(),
            message_text: Some("hi".into()),
            image_urls: None,
            message_id: None,
        };
        let result = send_message(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mark_read_unauthenticated() {
        let state = setup_state().await;
        let auth = AuthContext::anonymous();
        let req = MarkReadRequest {
            chat_id: "c1".into(),
        };
        let result = mark_messages_read(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_delete_message_unauthenticated() {
        let state = setup_state().await;
        let auth = AuthContext::anonymous();
        let req = DeleteMessageRequest {
            chat_id: "c1".into(),
            message_id: "m1".into(),
        };
        let result = delete_message(State(state), Extension(auth), Json(req)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_or_create_chat_full_flow() {
        let state = setup_state().await;
        let buyer_id = uuid::Uuid::new_v4().to_string();
        let seller_id = uuid::Uuid::new_v4().to_string();
        let product_id = uuid::Uuid::new_v4().to_string();
        let order_id = uuid::Uuid::new_v4().to_string();
        let auth = auth_ctx(&buyer_id);

        // 1. Setup Data
        // Active premium subscription
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                &buyer_id,
                json!({ fields::STATUS: "active" }),
            )
            .await
            .unwrap();

        // Active product
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                &product_id,
                json!({
                    fields::NAME: "Maple Syrup",
                    fields::SELLER_ID: &seller_id,
                    fields::LIFECYCLE_STATUS: "ACTIVE",
                    fields::IMAGE_URLS: ["https://cdn.test/1.jpg"]
                }),
            )
            .await
            .unwrap();

        // 2. Test without eligible order (should fail)
        let req = GetOrCreateChatRequest {
            other_user_id: seller_id.clone(),
            product_id: Some(product_id.clone()),
        };
        let result =
            get_or_create_chat(State(state.clone()), Extension(auth.clone()), Json(req)).await;
        assert!(result.is_err(), "Expected error but got success");
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("delivered order"),
            "Expected 'delivered order' error, got: {err_str}"
        );

        // 3. Add eligible order
        state
            .db
            .upsert_document(
                collections::ORDERS,
                &order_id,
                json!({
                    "userId": &buyer_id,
                    "productIds": [&product_id],
                    fields::ORDER_STATUS: "delivered"
                }),
            )
            .await
            .unwrap();

        // 4. Test success create
        let req = GetOrCreateChatRequest {
            other_user_id: seller_id.clone(),
            product_id: Some(product_id.clone()),
        };
        let result = get_or_create_chat(
            State(state.clone()),
            Extension(auth.clone()),
            Json(req.clone()),
        )
        .await;
        assert!(result.is_ok(), "{result:?}");
        let Json(resp) = result.unwrap();
        assert!(resp.is_new);
        assert_eq!(resp.chat_id, format!("{}_{}", product_id, buyer_id));

        // 5. Test idempotency (should return existing)
        let result =
            get_or_create_chat(State(state.clone()), Extension(auth.clone()), Json(req)).await;
        assert!(result.is_ok(), "{result:?}");
        let Json(resp) = result.unwrap();
        assert!(!resp.is_new);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_send_message_full_flow() {
        let state = setup_state().await;
        let buyer_id = &uuid::Uuid::new_v4().to_string();
        let seller_id = &uuid::Uuid::new_v4().to_string();
        let chat_id_str = format!("chat_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let chat_id = chat_id_str.as_str();
        let auth = auth_ctx(buyer_id);

        // Setup chat thread
        state
            .db
            .upsert_document(
                collections::CHATS,
                chat_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id,
                    fields::MESSAGE_COUNT: 0,
                    fields::SELLER_UNREAD_COUNT: 0,
                    fields::UPDATED_AT: "2020-01-01T00:00:00Z"
                }),
            )
            .await
            .unwrap();

        // Setup buyer premium
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                buyer_id,
                json!({ fields::STATUS: "active" }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                buyer_id,
                json!({ fields::NAME: "Buyer" }),
            )
            .await
            .unwrap();

        // Send message
        let req = SendMessageRequest {
            chat_id: chat_id.into(),
            message_text: Some("Hello seller!".into()),
            image_urls: None,
            message_id: None,
        };
        let result = send_message(State(state.clone()), Extension(auth.clone()), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(!resp.message_id.is_empty());

        // Verify thread update
        let chat = state
            .db
            .get_document(collections::CHATS, chat_id)
            .await
            .unwrap();
        assert_eq!(chat[fields::MESSAGE_COUNT], 1);
        assert_eq!(chat[fields::SELLER_UNREAD_COUNT], 1);
        assert_eq!(chat[fields::LAST_MESSAGE_TEXT], "Hello seller!");

        // Test Deduplication (within 5s) — pre-set dedup state to avoid race with thread update
        let now = chrono::Utc::now().to_rfc3339();
        state
            .db
            .upsert_document(
                collections::CHATS,
                chat_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id,
                    fields::MESSAGE_COUNT: 1,
                    fields::SELLER_UNREAD_COUNT: 1,
                    fields::LAST_MESSAGE_TEXT: "Hello seller!",
                    fields::UPDATED_AT: now,
                }),
            )
            .await
            .unwrap();
        let req_dup = SendMessageRequest {
            chat_id: chat_id.into(),
            message_text: Some("Hello seller!".into()),
            image_urls: None,
            message_id: None,
        };
        let result_dup =
            send_message(State(state.clone()), Extension(auth.clone()), Json(req_dup)).await;
        assert!(
            result_dup.is_err(),
            "Dedup should reject duplicate message within 5s"
        );
        assert!(result_dup.unwrap_err().to_string().contains("already sent"));
    }

    #[tokio::test]
    async fn test_mark_read_flow() {
        let state = setup_state().await;
        let buyer_id = &uuid::Uuid::new_v4().to_string();
        let seller_id = &uuid::Uuid::new_v4().to_string();
        let chat_id_str = format!("mrf_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let chat_id = chat_id_str.as_str();
        let auth = auth_ctx(buyer_id);

        // Setup chat
        state
            .db
            .upsert_document(
                collections::CHATS,
                chat_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id,
                    fields::BUYER_UNREAD_COUNT: 2
                }),
            )
            .await
            .unwrap();

        // Setup unread messages from seller
        let msg_coll = format!("{}__{}", collections::CHATS, collections::CHAT_MESSAGES);
        let m1 = uuid::Uuid::new_v4().to_string();
        let m2 = uuid::Uuid::new_v4().to_string();
        state
            .db
            .upsert_document(
                &msg_coll,
                &m1,
                json!({
                    fields::CHAT_ID: chat_id,
                    fields::SENDER_ID: seller_id,
                    fields::READ: false
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                &msg_coll,
                &m2,
                json!({
                    fields::CHAT_ID: chat_id,
                    fields::SENDER_ID: seller_id,
                    fields::READ: false
                }),
            )
            .await
            .unwrap();

        // Mark read
        let req = MarkReadRequest {
            chat_id: chat_id.into(),
        };
        let result =
            mark_messages_read(State(state.clone()), Extension(auth.clone()), Json(req)).await;
        assert!(
            result.is_ok(),
            "mark_messages_read failed: {:?}",
            result.err()
        );
        let Json(resp) = result.unwrap();
        assert_eq!(resp.count, 2);

        // Verify unread count reset
        let chat = state
            .db
            .get_document(collections::CHATS, chat_id)
            .await
            .unwrap();
        assert_eq!(chat[fields::BUYER_UNREAD_COUNT], 0);
    }

    #[tokio::test]
    async fn test_delete_message_flow() {
        let state = setup_state().await;
        let uid = "u1";
        let auth = auth_ctx(uid);
        let msg_id = "m1";
        let msg_coll = format!("{}__{}", collections::CHATS, collections::CHAT_MESSAGES);

        state
            .db
            .upsert_document(
                &msg_coll,
                msg_id,
                json!({
                    fields::SENDER_ID: uid,
                    fields::DELETED: false
                }),
            )
            .await
            .unwrap();

        let req = DeleteMessageRequest {
            chat_id: "c1".into(),
            message_id: msg_id.into(),
        };
        let result = delete_message(State(state.clone()), Extension(auth.clone()), Json(req)).await;
        assert!(result.is_ok());

        let msg = state.db.get_document(&msg_coll, msg_id).await.unwrap();
        assert!(msg[fields::DELETED].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_send_message_rejects_http_image_urls() {
        let state = setup_state().await;
        let auth = auth_ctx("buyer_1");

        let req = SendMessageRequest {
            chat_id: "c1".into(),
            message_text: Some("hi".into()),
            image_urls: Some(vec!["http://evil.com/img.jpg".into()]),
            message_id: None,
        };
        let result = send_message(State(state.clone()), Extension(auth.clone()), Json(req)).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("HTTPS"), "Expected HTTPS error, got: {err}");
    }

    #[tokio::test]
    async fn test_send_message_rejects_ftp_image_urls() {
        let state = setup_state().await;
        let auth = auth_ctx("buyer_1");

        let req = SendMessageRequest {
            chat_id: "c1".into(),
            message_text: Some("hi".into()),
            image_urls: Some(vec!["ftp://files.example.com/pic.png".into()]),
            message_id: None,
        };
        let result = send_message(State(state.clone()), Extension(auth.clone()), Json(req)).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("HTTPS"), "Expected HTTPS error, got: {err}");
    }

    #[tokio::test]
    async fn test_send_message_accepts_https_image_urls() {
        let state = setup_state().await;
        let buyer_id = &uuid::Uuid::new_v4().to_string();
        let seller_id = &uuid::Uuid::new_v4().to_string();
        let chat_id_str = format!("https_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let chat_id = chat_id_str.as_str();
        let auth = auth_ctx(buyer_id);

        // Setup chat + premium
        state
            .db
            .upsert_document(
                collections::CHATS,
                chat_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id,
                    fields::MESSAGE_COUNT: 0,
                    fields::SELLER_UNREAD_COUNT: 0,
                    fields::UPDATED_AT: "2020-01-01T00:00:00Z"
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                buyer_id,
                json!({ fields::STATUS: "active" }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                buyer_id,
                json!({ fields::NAME: "Buyer" }),
            )
            .await
            .unwrap();

        let req = SendMessageRequest {
            chat_id: chat_id.into(),
            message_text: None,
            image_urls: Some(vec!["https://storage.googleapis.com/bucket/img.jpg".into()]),
            message_id: None,
        };
        let result = send_message(State(state.clone()), Extension(auth.clone()), Json(req)).await;
        assert!(
            result.is_ok(),
            "HTTPS CDN URL should be accepted: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_send_message_dedup_same_text_within_5s() {
        let state = setup_state().await;
        let buyer_id = &uuid::Uuid::new_v4().to_string();
        let seller_id = &uuid::Uuid::new_v4().to_string();
        let chat_id_str = format!("dedup_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let chat_id = chat_id_str.as_str();
        let auth = auth_ctx(buyer_id);
        let now = chrono::Utc::now().to_rfc3339();

        // Setup chat with lastMessageText matching what we'll send, updated just now
        state
            .db
            .upsert_document(
                collections::CHATS,
                chat_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id,
                    fields::MESSAGE_COUNT: 0,
                    fields::SELLER_UNREAD_COUNT: 0,
                    fields::LAST_MESSAGE_TEXT: "Hello seller!",
                    fields::UPDATED_AT: now
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                buyer_id,
                json!({ fields::STATUS: "active" }),
            )
            .await
            .unwrap();

        let req = SendMessageRequest {
            chat_id: chat_id.into(),
            message_text: Some("Hello seller!".into()),
            image_urls: None,
            message_id: None,
        };
        let result = send_message(State(state.clone()), Extension(auth.clone()), Json(req)).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("already sent"),
            "Expected dedup error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_send_message_dedup_allows_different_text() {
        let state = setup_state().await;
        let buyer_id = &uuid::Uuid::new_v4().to_string();
        let seller_id = &uuid::Uuid::new_v4().to_string();
        let chat_id_str = format!("diff_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let chat_id = chat_id_str.as_str();
        let now = chrono::Utc::now().to_rfc3339();
        let auth = auth_ctx(buyer_id);

        state
            .db
            .upsert_document(
                collections::CHATS,
                chat_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id,
                    fields::MESSAGE_COUNT: 0,
                    fields::SELLER_UNREAD_COUNT: 0,
                    fields::LAST_MESSAGE_TEXT: "First message",
                    fields::UPDATED_AT: now
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                buyer_id,
                json!({ fields::STATUS: "active" }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                buyer_id,
                json!({ fields::NAME: "Buyer" }),
            )
            .await
            .unwrap();

        let req = SendMessageRequest {
            chat_id: chat_id.into(),
            message_text: Some("Different message".into()),
            image_urls: None,
            message_id: None,
        };
        let result = send_message(State(state.clone()), Extension(auth.clone()), Json(req)).await;
        assert!(
            result.is_ok(),
            "Different text should not trigger dedup: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_report_message_flow() {
        let state = setup_state().await;
        let buyer_id = &uuid::Uuid::new_v4().to_string();
        let seller_id = &uuid::Uuid::new_v4().to_string();
        let chat_id_str = format!("rpt_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let chat_id = chat_id_str.as_str();
        let msg_id = &uuid::Uuid::new_v4().to_string();
        let auth = auth_ctx(buyer_id);

        state
            .db
            .upsert_document(
                collections::CHATS,
                chat_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id
                }),
            )
            .await
            .unwrap();

        let msg_coll = format!("{}__{}", collections::CHATS, collections::CHAT_MESSAGES);
        state
            .db
            .upsert_document(
                &msg_coll,
                msg_id,
                json!({
                    fields::SENDER_ID: seller_id,
                    fields::MESSAGE_TEXT: "scam"
                }),
            )
            .await
            .unwrap();

        let req = ReportMessageRequest {
            chat_id: chat_id.into(),
            message_id: msg_id.into(),
            reason: Some("bad".into()),
        };
        let result = report_message(State(state.clone()), Extension(auth.clone()), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(!resp.report_id.is_empty());
    }

    #[tokio::test]
    async fn test_get_or_create_chat_requires_product_id() {
        let state = setup_state().await;
        let err = get_or_create_chat(
            State(state),
            Extension(auth_ctx("buyer_1")),
            Json(GetOrCreateChatRequest {
                other_user_id: "seller_1".into(),
                product_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("productId is required"));
    }

    #[tokio::test]
    async fn test_get_or_create_chat_rejects_non_premium_buyer() {
        let state = setup_state().await;
        let buyer_id = uuid::Uuid::new_v4().to_string();
        let seller_id = uuid::Uuid::new_v4().to_string();
        let product_id = uuid::Uuid::new_v4().to_string();
        let err = get_or_create_chat(
            State(state),
            Extension(auth_ctx(&buyer_id)),
            Json(GetOrCreateChatRequest {
                other_user_id: seller_id,
                product_id: Some(product_id),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Premium subscription required"));
    }

    #[tokio::test]
    async fn test_send_message_chat_not_found() {
        let state = setup_state().await;
        let err = send_message(
            State(state),
            Extension(auth_ctx("buyer_1")),
            Json(SendMessageRequest {
                chat_id: "missing_chat".into(),
                message_text: Some("hello".into()),
                image_urls: None,
                message_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Chat thread not found"));
    }

    #[tokio::test]
    async fn test_mark_read_chat_not_found() {
        let state = setup_state().await;
        let err = mark_messages_read(
            State(state),
            Extension(auth_ctx("buyer_1")),
            Json(MarkReadRequest {
                chat_id: "missing_chat".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Chat thread not found"));
    }

    #[tokio::test]
    async fn test_delete_message_already_deleted_is_idempotent() {
        let state = setup_state().await;
        let uid = uuid::Uuid::new_v4().to_string();
        let msg_id = uuid::Uuid::new_v4().to_string();
        let msg_coll = format!("{}__{}", collections::CHATS, collections::CHAT_MESSAGES);
        state
            .db
            .upsert_document(
                &msg_coll,
                &msg_id,
                json!({
                    fields::SENDER_ID: &uid,
                    fields::DELETED: true,
                }),
            )
            .await
            .unwrap();

        let result = delete_message(
            State(state),
            Extension(auth_ctx(&uid)),
            Json(DeleteMessageRequest {
                chat_id: "c1".into(),
                message_id: msg_id,
            }),
        )
        .await;
        assert!(result.is_ok());
    }

    // --- Coverage: phone number non-match branch (line 144) ---

    #[test]
    fn test_sanitize_text_short_number_not_redacted() {
        // A number with fewer than 10 digits should NOT be redacted
        let result = _sanitize_text("Call me at 12345");
        assert!(
            !result.contains("[phone removed]"),
            "Short number should not be redacted: {result}"
        );
    }

    // --- Coverage: check_premium branches (lines 156, 159) ---

    #[tokio::test]
    async fn test_check_premium_missing_subscription() {
        let state = setup_state().await;
        let result = check_premium(&state.db, "no_sub_user").await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_check_premium_null_subscription() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                "null_sub",
                serde_json::Value::Null,
            )
            .await
            .ok();
        let result = check_premium(&state.db, "null_sub").await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_check_premium_inactive_subscription() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                "inactive_sub",
                json!({ fields::STATUS: "cancelled" }),
            )
            .await
            .unwrap();
        let result = check_premium(&state.db, "inactive_sub").await.unwrap();
        assert!(!result);
    }

    // --- Coverage: product is null (line 235), product not active (line 243), self-chat (line 252) ---

    #[tokio::test]
    async fn test_get_or_create_chat_product_not_found() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                "buyer_pnf",
                json!({ fields::STATUS: "active" }),
            )
            .await
            .unwrap();

        let err = get_or_create_chat(
            State(state),
            Extension(auth_ctx("buyer_pnf")),
            Json(GetOrCreateChatRequest {
                other_user_id: "s1".into(),
                product_id: Some("missing_prod".into()),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found") || err.to_string().contains("Not found"));
    }

    #[tokio::test]
    async fn test_get_or_create_chat_product_inactive() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                "buyer_ina",
                json!({ fields::STATUS: "active" }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_ina",
                json!({
                    fields::SELLER_ID: "seller_1",
                    fields::LIFECYCLE_STATUS: "draft",
                }),
            )
            .await
            .unwrap();

        let err = get_or_create_chat(
            State(state),
            Extension(auth_ctx("buyer_ina")),
            Json(GetOrCreateChatRequest {
                other_user_id: "seller_1".into(),
                product_id: Some("prod_ina".into()),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("no longer active"));
    }

    #[tokio::test]
    async fn test_get_or_create_chat_self_chat() {
        let state = setup_state().await;
        let uid = "self_chat_user";
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                uid,
                json!({ fields::STATUS: "active" }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_self",
                json!({
                    fields::SELLER_ID: uid,
                    fields::LIFECYCLE_STATUS: "ACTIVE",
                }),
            )
            .await
            .unwrap();

        let err = get_or_create_chat(
            State(state),
            Extension(auth_ctx(uid)),
            Json(GetOrCreateChatRequest {
                other_user_id: uid.into(),
                product_id: Some("prod_self".into()),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("cannot chat with yourself"));
    }

    // --- Coverage: send_message validation branches ---

    #[tokio::test]
    async fn test_send_message_empty_chat_id_and_no_content() {
        let state = setup_state().await;
        let err = send_message(
            State(state),
            Extension(auth_ctx("u1")),
            Json(SendMessageRequest {
                chat_id: "".into(),
                message_text: None,
                image_urls: None,
                message_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("chatId and text/images required"));
    }

    #[tokio::test]
    async fn test_send_message_too_many_images() {
        let state = setup_state().await;
        let urls: Vec<String> = (0..6)
            .map(|i| format!("https://cdn.test/{i}.jpg"))
            .collect();
        let err = send_message(
            State(state),
            Extension(auth_ctx("u1")),
            Json(SendMessageRequest {
                chat_id: "c1".into(),
                message_text: Some("hi".into()),
                image_urls: Some(urls),
                message_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Maximum"));
    }

    #[tokio::test]
    async fn test_send_message_non_cdn_url_rejected() {
        let state = setup_state().await;
        let err = send_message(
            State(state),
            Extension(auth_ctx("u1")),
            Json(SendMessageRequest {
                chat_id: "c1".into(),
                message_text: Some("hi".into()),
                image_urls: Some(vec!["https://evil.com/img.jpg".into()]),
                message_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("CDN"));
    }

    #[tokio::test]
    async fn test_send_message_text_too_long() {
        let state = setup_state().await;
        let buyer_id = "buyer_long";
        let seller_id = "seller_long";
        let chat_id = "chat_long";

        state
            .db
            .upsert_document(
                collections::CHATS,
                chat_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id,
                    fields::MESSAGE_COUNT: 0,
                    fields::SELLER_UNREAD_COUNT: 0,
                    fields::UPDATED_AT: "2020-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                buyer_id,
                json!({ fields::STATUS: "active" }),
            )
            .await
            .unwrap();

        let long_text = "a".repeat(2001);
        let err = send_message(
            State(state),
            Extension(auth_ctx(buyer_id)),
            Json(SendMessageRequest {
                chat_id: chat_id.into(),
                message_text: Some(long_text),
                image_urls: None,
                message_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("exceeds"));
    }

    #[tokio::test]
    async fn test_send_message_access_denied_non_participant() {
        let state = setup_state().await;
        let chat_id = "chat_access";
        state
            .db
            .upsert_document(
                collections::CHATS,
                chat_id,
                json!({
                    fields::BUYER_ID: "buyer_1",
                    fields::SELLER_ID: "seller_1",
                    fields::MESSAGE_COUNT: 0,
                    fields::UPDATED_AT: "2020-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();

        let err = send_message(
            State(state),
            Extension(auth_ctx("intruder")),
            Json(SendMessageRequest {
                chat_id: chat_id.into(),
                message_text: Some("hello".into()),
                image_urls: None,
                message_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Access denied"));
    }

    #[tokio::test]
    async fn test_send_message_thread_capacity_exceeded() {
        let state = setup_state().await;
        let buyer_id = "buyer_cap";
        let seller_id = "seller_cap";
        let chat_id = "chat_cap";

        state
            .db
            .upsert_document(
                collections::CHATS,
                chat_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id,
                    fields::MESSAGE_COUNT: 10_000,
                    fields::SELLER_UNREAD_COUNT: 0,
                    fields::UPDATED_AT: "2020-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                buyer_id,
                json!({ fields::STATUS: "active" }),
            )
            .await
            .unwrap();

        let err = send_message(
            State(state),
            Extension(auth_ctx(buyer_id)),
            Json(SendMessageRequest {
                chat_id: chat_id.into(),
                message_text: Some("hello".into()),
                image_urls: None,
                message_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Chat limit reached"));
    }

    #[tokio::test]
    async fn test_send_message_buyer_not_premium() {
        let state = setup_state().await;
        let buyer_id = "buyer_np";
        let seller_id = "seller_np";
        let chat_id = "chat_np";

        state
            .db
            .upsert_document(
                collections::CHATS,
                chat_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id,
                    fields::MESSAGE_COUNT: 0,
                    fields::SELLER_UNREAD_COUNT: 0,
                    fields::UPDATED_AT: "2020-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        // No premium subscription

        let err = send_message(
            State(state),
            Extension(auth_ctx(buyer_id)),
            Json(SendMessageRequest {
                chat_id: chat_id.into(),
                message_text: Some("hello".into()),
                image_urls: None,
                message_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Premium required"));
    }

    #[tokio::test]
    async fn test_send_message_idempotency_returns_existing() {
        let state = setup_state().await;
        let buyer_id = "buyer_idem";
        let seller_id = "seller_idem";
        let chat_id = "chat_idem";
        let msg_id = "msg_idem_1";

        state
            .db
            .upsert_document(
                collections::CHATS,
                chat_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id,
                    fields::MESSAGE_COUNT: 0,
                    fields::SELLER_UNREAD_COUNT: 0,
                    fields::UPDATED_AT: "2020-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::SUBSCRIPTIONS,
                buyer_id,
                json!({ fields::STATUS: "active" }),
            )
            .await
            .unwrap();

        let msg_coll = format!("{}__{}", collections::CHATS, collections::CHAT_MESSAGES);
        state
            .db
            .upsert_document(
                &msg_coll,
                msg_id,
                json!({
                    fields::CHAT_ID: chat_id,
                    fields::SENDER_ID: buyer_id,
                    fields::MESSAGE_TEXT: "hello",
                }),
            )
            .await
            .unwrap();

        let result = send_message(
            State(state),
            Extension(auth_ctx(buyer_id)),
            Json(SendMessageRequest {
                chat_id: chat_id.into(),
                message_text: Some("hello".into()),
                image_urls: None,
                message_id: Some(msg_id.into()),
            }),
        )
        .await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert_eq!(resp.message_id, msg_id);
    }

    // --- Coverage: seller first reply metrics (lines 438-443) ---

    #[tokio::test]
    #[serial_test::serial]
    async fn test_send_message_seller_first_reply_metrics() {
        let state = setup_state().await;
        let buyer_id = &uuid::Uuid::new_v4().to_string();
        let seller_id = &uuid::Uuid::new_v4().to_string();
        let chat_id_str = format!("rply_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let chat_id = chat_id_str.as_str();

        let earlier = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        state
            .db
            .upsert_document(
                collections::CHATS,
                chat_id,
                json!({
                    fields::BUYER_ID: buyer_id,
                    fields::SELLER_ID: seller_id,
                    fields::MESSAGE_COUNT: 1,
                    fields::BUYER_UNREAD_COUNT: 0,
                    fields::SELLER_UNREAD_COUNT: 0,
                    "firstBuyerMessageAt": earlier,
                    fields::UPDATED_AT: "2020-01-01T00:00:00Z",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                seller_id,
                json!({ fields::NAME: "Seller" }),
            )
            .await
            .unwrap();

        let result = send_message(
            State(state.clone()),
            Extension(auth_ctx(seller_id)),
            Json(SendMessageRequest {
                chat_id: chat_id.into(),
                message_text: Some("Hi buyer!".into()),
                image_urls: None,
                message_id: None,
            }),
        )
        .await;
        assert!(result.is_ok());

        let chat = state
            .db
            .get_document(collections::CHATS, chat_id)
            .await
            .unwrap();
        assert!(chat.get("firstSellerReplyAt").is_some());
        assert!(chat.get("firstReplyHours").is_some());
    }

    // --- Coverage: mark_messages_read validation branches ---

    #[tokio::test]
    async fn test_mark_read_empty_chat_id() {
        let state = setup_state().await;
        let err = mark_messages_read(
            State(state),
            Extension(auth_ctx("u1")),
            Json(MarkReadRequest { chat_id: "".into() }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("chatId is required"));
    }

    #[tokio::test]
    async fn test_mark_read_access_denied() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::CHATS,
                "chat_mr_access",
                json!({
                    fields::BUYER_ID: "buyer_1",
                    fields::SELLER_ID: "seller_1",
                }),
            )
            .await
            .unwrap();

        let err = mark_messages_read(
            State(state),
            Extension(auth_ctx("intruder")),
            Json(MarkReadRequest {
                chat_id: "chat_mr_access".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Access denied"));
    }

    // --- Coverage: delete_message validation branches ---

    #[tokio::test]
    async fn test_delete_message_empty_fields() {
        let state = setup_state().await;
        let err = delete_message(
            State(state),
            Extension(auth_ctx("u1")),
            Json(DeleteMessageRequest {
                chat_id: "".into(),
                message_id: "".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("chatId and messageId required"));
    }

    #[tokio::test]
    async fn test_delete_message_not_found() {
        let state = setup_state().await;
        let err = delete_message(
            State(state),
            Extension(auth_ctx("u1")),
            Json(DeleteMessageRequest {
                chat_id: "c1".into(),
                message_id: "missing_msg".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found") || err.to_string().contains("Not found"));
    }

    #[tokio::test]
    async fn test_delete_message_not_sender_or_admin() {
        let state = setup_state().await;
        let msg_coll = format!("{}__{}", collections::CHATS, collections::CHAT_MESSAGES);
        state
            .db
            .upsert_document(
                &msg_coll,
                "m_other",
                json!({
                    fields::SENDER_ID: "other_user",
                    fields::DELETED: false,
                }),
            )
            .await
            .unwrap();

        let err = delete_message(
            State(state),
            Extension(auth_ctx("not_sender")),
            Json(DeleteMessageRequest {
                chat_id: "c1".into(),
                message_id: "m_other".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Only the sender or an admin"));
    }

    // --- Coverage: report_message validation branches ---

    #[tokio::test]
    async fn test_report_message_unauthenticated() {
        let state = setup_state().await;
        let err = report_message(
            State(state),
            Extension(AuthContext::anonymous()),
            Json(ReportMessageRequest {
                chat_id: "c1".into(),
                message_id: "m1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Authentication required"));
    }

    #[tokio::test]
    async fn test_report_message_empty_fields() {
        let state = setup_state().await;
        let err = report_message(
            State(state),
            Extension(auth_ctx("u1")),
            Json(ReportMessageRequest {
                chat_id: "".into(),
                message_id: "".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("chatId and messageId required"));
    }

    #[tokio::test]
    async fn test_report_message_chat_not_found() {
        let state = setup_state().await;
        let err = report_message(
            State(state),
            Extension(auth_ctx("u1")),
            Json(ReportMessageRequest {
                chat_id: "missing_chat".into(),
                message_id: "m1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found") || err.to_string().contains("Not found"));
    }

    #[tokio::test]
    async fn test_report_message_not_participant() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::CHATS,
                "chat_rep_np",
                json!({
                    fields::BUYER_ID: "buyer_1",
                    fields::SELLER_ID: "seller_1",
                }),
            )
            .await
            .unwrap();

        let err = report_message(
            State(state),
            Extension(auth_ctx("intruder")),
            Json(ReportMessageRequest {
                chat_id: "chat_rep_np".into(),
                message_id: "m1".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not a participant"));
    }

    #[tokio::test]
    async fn test_report_message_msg_not_found() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::CHATS,
                "chat_rep_mf",
                json!({
                    fields::BUYER_ID: "buyer_1",
                    fields::SELLER_ID: "seller_1",
                }),
            )
            .await
            .unwrap();

        let err = report_message(
            State(state),
            Extension(auth_ctx("buyer_1")),
            Json(ReportMessageRequest {
                chat_id: "chat_rep_mf".into(),
                message_id: "missing_msg".into(),
                reason: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found") || err.to_string().contains("Not found"));
    }

    #[test]
    fn test_sanitize_once_lock_regex_reuse() {
        // Verify OnceLock regexes are compiled once and reused across calls
        let first = _sanitize_text("test@example.com");
        let second = _sanitize_text("another@test.org");
        let third = _sanitize_text("https://evil.com +1-416-555-1234");
        assert!(first.contains("[email removed]"));
        assert!(second.contains("[email removed]"));
        assert!(third.contains("[link removed]"));
        assert!(third.contains("[phone removed]"));
        // If OnceLock were broken, this would panic on second use
    }

    #[test]
    fn test_sanitize_preserves_safe_text() {
        let safe = "Hello, this is a normal message about products.";
        assert_eq!(_sanitize_text(safe), safe);
    }

    #[test]
    fn test_sanitize_empty_input() {
        assert_eq!(_sanitize_text(""), "");
    }
}
