//! Product Q&A handlers.
//! Ported from: functions/handlers/products.py (ask_product_question, answer_product_question,
//! get_product_questions)

use axum::{Json, Router, extract::State, routing::post};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::info;

use crate::HandlersState;
use crate::shared::schema::{collections, fields};
use crate::shared::validation::{sanitize_html, validate_uid};

const MIN_QUESTION_LENGTH: usize = 10;
const MAX_QUESTION_LENGTH: usize = 500;
const MIN_ANSWER_LENGTH: usize = 10;
const MAX_ANSWER_LENGTH: usize = 2000;
const DEFAULT_QA_LIMIT: u32 = 20;
const MAX_QA_LIMIT: u32 = 50;

// ─── Request/Response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskQuestionRequest {
    pub product_id: String,
    #[serde(alias = "questionText")]
    pub question: String,
    pub user_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskQuestionResponse {
    pub success: bool,
    pub question_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerQuestionRequest {
    pub question_id: String,
    #[serde(alias = "answerText")]
    pub answer: String,
    pub user_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerQuestionResponse {
    pub success: bool,
    pub answered: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuestionsRequest {
    pub product_id: String,
    #[serde(default = "default_qa_limit")]
    pub limit: u32,
    #[serde(default)]
    pub answered_only: bool,
}

fn default_qa_limit() -> u32 {
    DEFAULT_QA_LIMIT
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuestionsResponse {
    pub success: bool,
    pub questions: Vec<QuestionItem>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionItem {
    pub question_id: String,
    pub question_text: String,
    pub answer_text: Option<String>,
    pub is_answered: bool,
    pub upvotes: i64,
    pub created_at: Option<String>,
    pub answered_at: Option<String>,
}

// ─── Router ─────────────────────────────────────────────────────────────────

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/products/questions/ask", post(ask_question))
        .route("/api/products/questions/answer", post(answer_question))
        .route("/api/products/questions/list", post(list_questions))
        .with_state(state)
}

// ─── Handlers ───────────────────────────────────────────────────────────────

async fn ask_question(
    State(state): State<HandlersState>,
    Json(req): Json<AskQuestionRequest>,
) -> Result<Json<AskQuestionResponse>, ob_core::Error> {
    validate_uid("productId", &req.product_id)?;
    validate_uid("userId", &req.user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "ask_question",
        10, // 10 questions
        60, // per hour
    )
    .await?;

    // Sanitize and validate question length
    let question = sanitize_html(req.question.trim());
    let question = if question.len() > MAX_QUESTION_LENGTH {
        question[..MAX_QUESTION_LENGTH].to_string()
    } else {
        question
    };

    if question.len() < MIN_QUESTION_LENGTH {
        return Err(ob_core::Error::Validation(
            "Question must be at least 10 characters".into(),
        ));
    }

    // Verify product exists and get seller ID
    let product = state
        .db
        .get_document(collections::PRODUCTS, &req.product_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Product not found".into()))?;

    if product.is_null() {
        return Err(ob_core::Error::NotFound("Product not found".into()));
    }

    // Derive seller_id from product document (prevents spoofing)
    let seller_id = product
        .get(fields::SELLER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Create question document
    let question_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let question_doc = serde_json::json!({
        "questionId": question_id,
        fields::PRODUCT_ID: req.product_id,
        fields::SELLER_ID: seller_id,
        "askerId": req.user_id,
        "questionText": question,
        "answerText": null,
        "answeredAt": null,
        "answeredBy": null,
        "isAnswered": false,
        "upvotes": 0,
        fields::CREATED_AT: now,
    });

    state
        .db
        .create_document(collections::PRODUCT_QUESTIONS, question_doc)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to create question: {e}")))?;

    info!(
        product_id = %req.product_id,
        user_id = %req.user_id,
        question_id = %question_id,
        "Product question asked"
    );

    Ok(Json(AskQuestionResponse {
        success: true,
        question_id,
    }))
}

async fn answer_question(
    State(state): State<HandlersState>,
    Json(req): Json<AnswerQuestionRequest>,
) -> Result<Json<AnswerQuestionResponse>, ob_core::Error> {
    validate_uid("questionId", &req.question_id)?;
    validate_uid("userId", &req.user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "answer_question",
        30, // 30 answers
        60, // per hour
    )
    .await?;

    // Sanitize and validate answer length
    let answer = sanitize_html(req.answer.trim());
    let answer = if answer.len() > MAX_ANSWER_LENGTH {
        answer[..MAX_ANSWER_LENGTH].to_string()
    } else {
        answer
    };

    if answer.len() < MIN_ANSWER_LENGTH {
        return Err(ob_core::Error::Validation(
            "Answer must be at least 10 characters".into(),
        ));
    }

    // Fetch question
    let question = state
        .db
        .get_document(collections::PRODUCT_QUESTIONS, &req.question_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Question not found".into()))?;

    if question.is_null() {
        return Err(ob_core::Error::NotFound("Question not found".into()));
    }

    let seller_id = question
        .get(fields::SELLER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Check admin role
    let user = state
        .db
        .get_document(collections::USERS, &req.user_id)
        .await
        .unwrap_or(Value::Null);

    let roles = user
        .get(fields::ROLES)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();

    let is_admin = roles.contains(&"admin");

    // Only product's seller or admin can answer
    if !is_admin && req.user_id != seller_id {
        return Err(ob_core::Error::Forbidden(
            "Only the seller or an admin can answer this question".into(),
        ));
    }

    // Update question with answer
    let now = chrono::Utc::now().to_rfc3339();
    let update = serde_json::json!({
        "answerText": answer,
        "answeredAt": now,
        "answeredBy": req.user_id,
        "isAnswered": true,
    });

    state
        .db
        .update_document(collections::PRODUCT_QUESTIONS, &req.question_id, update)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to update question: {e}")))?;

    info!(
        question_id = %req.question_id,
        user_id = %req.user_id,
        "Product question answered"
    );

    Ok(Json(AnswerQuestionResponse {
        success: true,
        answered: true,
    }))
}

async fn list_questions(
    State(state): State<HandlersState>,
    Json(req): Json<ListQuestionsRequest>,
) -> Result<Json<ListQuestionsResponse>, ob_core::Error> {
    validate_uid("productId", &req.product_id)?;

    let limit = req.limit.min(MAX_QA_LIMIT);

    let mut conditions = vec![format!(
        "{} = '{}'",
        fields::PRODUCT_ID,
        ob_core::escape_surreal_string(&req.product_id)
    )];

    if req.answered_only {
        conditions.push("isAnswered = true".to_string());
    }

    let where_clause = format!(" WHERE {}", conditions.join(" AND "));

    let query = format!(
        "SELECT * FROM {}{} ORDER BY {} DESC LIMIT {}",
        collections::PRODUCT_QUESTIONS,
        where_clause,
        fields::CREATED_AT,
        limit,
    );

    let rows: Vec<Value> = state
        .db
        .query_raw(&query)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to fetch questions: {e}")))?;

    let questions: Vec<QuestionItem> = rows
        .iter()
        .map(|doc| QuestionItem {
            question_id: doc
                .get("questionId")
                .or_else(|| doc.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            question_text: doc
                .get("questionText")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            answer_text: doc
                .get("answerText")
                .and_then(|v| v.as_str())
                .map(String::from),
            is_answered: doc
                .get("isAnswered")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            upvotes: doc.get("upvotes").and_then(|v| v.as_i64()).unwrap_or(0),
            created_at: doc
                .get(fields::CREATED_AT)
                .and_then(|v| v.as_str())
                .map(String::from),
            answered_at: doc
                .get("answeredAt")
                .and_then(|v| v.as_str())
                .map(String::from),
        })
        .collect();

    let total = questions.len();

    Ok(Json(ListQuestionsResponse {
        success: true,
        questions,
        total,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use serde_json::json;
    use std::sync::Arc;

    async fn setup_state() -> HandlersState {
        HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        }
    }

    #[test]
    fn test_ask_question_request_deser() {
        let json = r#"{"productId":"p1","question":"How does this work exactly?","userId":"u1"}"#;
        let req: AskQuestionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.product_id, "p1");
        assert!(req.question.len() >= MIN_QUESTION_LENGTH);
    }

    #[test]
    fn test_question_length_validation() {
        let short = "Hi?";
        assert!(short.len() < MIN_QUESTION_LENGTH);
        let ok = "How does this product compare to the previous version?";
        assert!(ok.len() >= MIN_QUESTION_LENGTH);
    }

    #[test]
    fn test_question_item_serialize() {
        let item = QuestionItem {
            question_id: "q1".into(),
            question_text: "Is this waterproof?".into(),
            answer_text: Some("Yes, rated IPX7.".into()),
            is_answered: true,
            upvotes: 5,
            created_at: Some("2026-01-01T00:00:00Z".into()),
            answered_at: Some("2026-01-02T00:00:00Z".into()),
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("\"isAnswered\":true"));
        assert!(json.contains("IPX7"));
    }

    // ── Ported from test_handlers_products_engagement_deep.py (Q&A tests) ──

    #[test]
    fn test_question_too_short_rejected() {
        let short = "Hi?";
        assert!(short.len() < MIN_QUESTION_LENGTH);
        let exactly_min = "A".repeat(MIN_QUESTION_LENGTH);
        assert!(exactly_min.len() >= MIN_QUESTION_LENGTH);
    }

    #[test]
    fn test_question_truncated_at_max() {
        let long = "x".repeat(MAX_QUESTION_LENGTH + 100);
        let sanitized = sanitize_html(long.trim());
        let truncated = if sanitized.len() > MAX_QUESTION_LENGTH {
            sanitized[..MAX_QUESTION_LENGTH].to_string()
        } else {
            sanitized
        };
        assert_eq!(truncated.len(), MAX_QUESTION_LENGTH);
    }

    #[test]
    fn test_answer_too_short_rejected() {
        let short = "No.";
        assert!(short.len() < MIN_ANSWER_LENGTH);
        let exactly_min = "B".repeat(MIN_ANSWER_LENGTH);
        assert!(exactly_min.len() >= MIN_ANSWER_LENGTH);
    }

    #[test]
    fn test_answer_truncated_at_max() {
        let long = "y".repeat(MAX_ANSWER_LENGTH + 100);
        let truncated = if long.len() > MAX_ANSWER_LENGTH {
            long[..MAX_ANSWER_LENGTH].to_string()
        } else {
            long
        };
        assert_eq!(truncated.len(), MAX_ANSWER_LENGTH);
    }

    #[test]
    fn test_answer_question_request_deser() {
        let json =
            r#"{"questionId": "q1", "answer": "Yes, this ships to Quebec.", "userId": "seller_1"}"#;
        let req: AnswerQuestionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.question_id, "q1");
        assert_eq!(req.user_id, "seller_1");
        assert!(req.answer.len() >= MIN_ANSWER_LENGTH);
    }

    #[test]
    fn test_list_questions_request_defaults() {
        let json = r#"{"productId": "prod_1"}"#;
        let req: ListQuestionsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.limit, DEFAULT_QA_LIMIT);
        assert!(!req.answered_only);
    }

    #[test]
    fn test_list_questions_limit_clamping() {
        let json = r#"{"productId": "prod_1", "limit": 200}"#;
        let req: ListQuestionsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.limit.min(MAX_QA_LIMIT), MAX_QA_LIMIT);
    }

    #[test]
    fn test_list_questions_answered_only_filter() {
        let json = r#"{"productId": "prod_1", "answeredOnly": true}"#;
        let req: ListQuestionsRequest = serde_json::from_str(json).unwrap();
        assert!(req.answered_only);
    }

    #[test]
    fn test_question_item_unanswered() {
        let item = QuestionItem {
            question_id: "q2".into(),
            question_text: "Does this work with USB-C?".into(),
            answer_text: None,
            is_answered: false,
            upvotes: 0,
            created_at: Some("2026-03-10T00:00:00Z".into()),
            answered_at: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("\"isAnswered\":false"));
        assert!(json.contains("\"answerText\":null"));
        assert!(json.contains("\"answeredAt\":null"));
    }

    #[test]
    fn test_question_html_sanitization() {
        let malicious = "<img src=x onerror=alert('xss')>Is this safe?";
        let sanitized = sanitize_html(malicious);
        assert!(!sanitized.contains("<img"));
        assert!(!sanitized.contains("onerror"));
        assert!(sanitized.contains("Is this safe?"));
    }

    #[test]
    fn test_list_questions_response_serialize() {
        let resp = ListQuestionsResponse {
            success: true,
            questions: vec![],
            total: 0,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"total\":0"));
        assert!(json.contains("\"questions\":[]"));
    }

    #[test]
    fn test_ask_question_response_serialize() {
        let resp = AskQuestionResponse {
            success: true,
            question_id: "q_abc123".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"questionId\":\"q_abc123\""));
    }

    #[test]
    fn test_answer_question_response_serialize() {
        let resp = AnswerQuestionResponse {
            success: true,
            answered: true,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"answered\":true"));
    }

    #[tokio::test]
    async fn test_ask_question_success_creates_question_with_seller_derived_from_product() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                json!({
                    fields::PRODUCT_ID: "prod_1",
                    fields::SELLER_ID: "seller_1",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = ask_question(
            State(state.clone()),
            Json(AskQuestionRequest {
                product_id: "prod_1".into(),
                question: "<b>How long is shipping?</b>".into(),
                user_id: "buyer_1".into(),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(!resp.question_id.is_empty());

        let rows = state
            .db
            .query_bind_value(
                "SELECT * FROM product_questions WHERE questionId = $questionId",
                json!({"questionId": resp.question_id})
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][fields::SELLER_ID], "seller_1");
        assert_eq!(rows[0]["askerId"], "buyer_1");
        assert_eq!(rows[0]["isAnswered"], false);
        assert_eq!(rows[0]["upvotes"], 0);
        assert_eq!(rows[0]["questionText"], "How long is shipping?");
    }

    #[tokio::test]
    async fn test_ask_question_rejects_short_text_and_missing_product() {
        let state = setup_state().await;

        let short = ask_question(
            State(state.clone()),
            Json(AskQuestionRequest {
                product_id: "prod_1".into(),
                question: "short".into(),
                user_id: "buyer_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            short
                .to_string()
                .contains("Question must be at least 10 characters")
        );

        let missing_product = ask_question(
            State(state),
            Json(AskQuestionRequest {
                product_id: "prod_missing".into(),
                question: "How long is shipping?".into(),
                user_id: "buyer_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(missing_product.to_string().contains("Product not found"));
    }

    #[tokio::test]
    async fn test_answer_question_success_for_seller_and_admin() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCT_QUESTIONS,
                "q_1",
                json!({
                    fields::SELLER_ID: "seller_1",
                    "questionId": "q_1",
                    "questionText": "Does it support Quebec shipping?",
                    "isAnswered": false,
                }),
            )
            .await
            .unwrap();

        let Json(seller_resp) = answer_question(
            State(state.clone()),
            Json(AnswerQuestionRequest {
                question_id: "q_1".into(),
                answer: "<p>Yes, it does.</p>".into(),
                user_id: "seller_1".into(),
            }),
        )
        .await
        .unwrap();
        assert!(seller_resp.answered);

        let answered = state
            .db
            .get_document(collections::PRODUCT_QUESTIONS, "q_1")
            .await
            .unwrap();
        assert_eq!(answered["answerText"], "Yes, it does.");
        assert_eq!(answered["answeredBy"], "seller_1");
        assert_eq!(answered["isAnswered"], true);

        state
            .db
            .upsert_document(
                collections::PRODUCT_QUESTIONS,
                "q_2",
                json!({
                    fields::SELLER_ID: "seller_2",
                    "questionId": "q_2",
                    "questionText": "Is there warranty coverage included?",
                    "isAnswered": false,
                }),
            )
            .await
            .unwrap();
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

        let Json(admin_resp) = answer_question(
            State(state.clone()),
            Json(AnswerQuestionRequest {
                question_id: "q_2".into(),
                answer: "Yes, one year warranty is included.".into(),
                user_id: "admin_1".into(),
            }),
        )
        .await
        .unwrap();
        assert!(admin_resp.success);

        let admin_answered = state
            .db
            .get_document(collections::PRODUCT_QUESTIONS, "q_2")
            .await
            .unwrap();
        assert_eq!(admin_answered["answeredBy"], "admin_1");
        assert_eq!(admin_answered["isAnswered"], true);
    }

    #[tokio::test]
    async fn test_answer_question_rejects_short_missing_and_unauthorized() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCT_QUESTIONS,
                "q_1",
                json!({
                    fields::SELLER_ID: "seller_1",
                    "questionId": "q_1",
                    "questionText": "Does it fit standard mounts?",
                }),
            )
            .await
            .unwrap();

        let short = answer_question(
            State(state.clone()),
            Json(AnswerQuestionRequest {
                question_id: "q_1".into(),
                answer: "short".into(),
                user_id: "seller_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            short
                .to_string()
                .contains("Answer must be at least 10 characters")
        );

        let forbidden = answer_question(
            State(state.clone()),
            Json(AnswerQuestionRequest {
                question_id: "q_1".into(),
                answer: "Yes, it fits standard mounts.".into(),
                user_id: "buyer_2".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            forbidden
                .to_string()
                .contains("Only the seller or an admin can answer")
        );

        let missing = answer_question(
            State(state),
            Json(AnswerQuestionRequest {
                question_id: "q_missing".into(),
                answer: "Yes, it fits standard mounts.".into(),
                user_id: "seller_1".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(missing.to_string().contains("Question not found"));
    }

    #[tokio::test]
    async fn test_ask_question_truncates_long_question() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_trunc",
                json!({
                    fields::PRODUCT_ID: "prod_trunc",
                    fields::SELLER_ID: "seller_1",
                }),
            )
            .await
            .unwrap();

        // Question longer than MAX_QUESTION_LENGTH (500) should be truncated
        let long_question = "x".repeat(MAX_QUESTION_LENGTH + 200);
        let Json(resp) = ask_question(
            State(state.clone()),
            Json(AskQuestionRequest {
                product_id: "prod_trunc".into(),
                question: long_question,
                user_id: "buyer_1".into(),
            }),
        )
        .await
        .unwrap();
        assert!(resp.success);

        // Verify the stored question is truncated
        let rows = state
            .db
            .query_bind_value(
                "SELECT * FROM product_questions WHERE questionId = $questionId",
                json!({"questionId": resp.question_id})
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        let stored_text = rows[0]["questionText"].as_str().unwrap();
        assert_eq!(stored_text.len(), MAX_QUESTION_LENGTH);
    }

    #[tokio::test]
    async fn test_answer_question_truncates_long_answer() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCT_QUESTIONS,
                "q_trunc",
                json!({
                    fields::SELLER_ID: "seller_1",
                    "questionId": "q_trunc",
                    "questionText": "Does it support long answers?",
                    "isAnswered": false,
                }),
            )
            .await
            .unwrap();

        // Answer longer than MAX_ANSWER_LENGTH (2000) should be truncated
        let long_answer = "y".repeat(MAX_ANSWER_LENGTH + 500);
        let Json(resp) = answer_question(
            State(state.clone()),
            Json(AnswerQuestionRequest {
                question_id: "q_trunc".into(),
                answer: long_answer,
                user_id: "seller_1".into(),
            }),
        )
        .await
        .unwrap();
        assert!(resp.answered);

        let doc = state
            .db
            .get_document(collections::PRODUCT_QUESTIONS, "q_trunc")
            .await
            .unwrap();
        let stored_answer = doc["answerText"].as_str().unwrap();
        assert_eq!(stored_answer.len(), MAX_ANSWER_LENGTH);
    }

    #[tokio::test]
    async fn test_list_questions_filters_answered_only_and_uses_fallback_id() {
        let state = setup_state().await;
        state
            .db
            .query_bind(
                "CREATE type::thing($table, $id) CONTENT $data",
                json!({
                    "table": collections::PRODUCT_QUESTIONS,
                    "id": uuid::Uuid::new_v4().to_string(),
                    "data": {
                        fields::PRODUCT_ID: "prod_1",
                        "questionText": "Is this available in blue?",
                        "answerText": "Yes, blue is in stock.",
                        "isAnswered": true,
                        "upvotes": 4,
                        fields::CREATED_AT: "2026-03-10T10:00:00Z",
                        "answeredAt": "2026-03-10T11:00:00Z"
                    }
                }),
            )
            .await
            .unwrap();
        state
            .db
            .create_document(
                collections::PRODUCT_QUESTIONS,
                json!({
                    "questionId": "q_2",
                    fields::PRODUCT_ID: "prod_1",
                    "questionText": "Is pickup available downtown?",
                    "answerText": null,
                    "isAnswered": false,
                    "upvotes": 1,
                    fields::CREATED_AT: "2026-03-10T12:00:00Z",
                    "answeredAt": null,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .create_document(
                collections::PRODUCT_QUESTIONS,
                json!({
                    "questionId": "q_other",
                    fields::PRODUCT_ID: "prod_2",
                    "questionText": "Other product question",
                    "isAnswered": true,
                    "upvotes": 0,
                    fields::CREATED_AT: "2026-03-10T09:00:00Z",
                }),
            )
            .await
            .unwrap();

        let Json(all_resp) = list_questions(
            State(state.clone()),
            Json(ListQuestionsRequest {
                product_id: "prod_1".into(),
                limit: 50,
                answered_only: false,
            }),
        )
        .await
        .unwrap();
        assert_eq!(all_resp.total, 2);
        assert_eq!(all_resp.questions.len(), 2);
        assert_eq!(all_resp.questions[0].question_id, "q_2");
        assert!(
            all_resp.questions[1]
                .question_id
                .starts_with("product_questions:")
        );

        let Json(answered_resp) = list_questions(
            State(state),
            Json(ListQuestionsRequest {
                product_id: "prod_1".into(),
                limit: 5,
                answered_only: true,
            }),
        )
        .await
        .unwrap();
        assert_eq!(answered_resp.total, 1);
        assert_eq!(answered_resp.questions[0].is_answered, true);
        assert_eq!(
            answered_resp.questions[0].answer_text.as_deref(),
            Some("Yes, blue is in stock.")
        );
    }
}
