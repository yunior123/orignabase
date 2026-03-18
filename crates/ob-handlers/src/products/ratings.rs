//! Product ratings and reviews.
//! Ported from: functions/handlers/products.py (submit_product_rating, get_product_ratings_paginated)

use axum::{Json, Router, extract::State, routing::post};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::info;

use crate::HandlersState;
use crate::shared::schema::{collections, fields};
use crate::shared::validation::{sanitize_html, validate_uid};

// ─── Request/Response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitRatingRequest {
    pub product_id: String,
    pub user_id: String,
    pub order_id: String,
    pub rating: f64,
    pub review_text: Option<String>,
    #[serde(default)]
    pub review_image_urls: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitRatingAtomicRequest {
    pub product_id: String,
    pub user_id: String,
    pub order_id: String,
    pub rating: f64,
    #[serde(default)]
    pub review_text: Option<String>,
    #[serde(default)]
    pub images: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitRatingResponse {
    pub success: bool,
    pub new_rating: f64,
    pub rating_count: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetRatingsRequest {
    pub product_id: String,
    #[serde(default = "default_ratings_limit")]
    pub limit: u32,
    pub start_after: Option<String>,
    pub min_rating: Option<f64>,
}

fn default_ratings_limit() -> u32 {
    10
}

const MAX_RATINGS_PAGE: u32 = 50;
const MAX_REVIEW_LENGTH: usize = 1000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetRatingsResponse {
    pub ratings: Vec<Value>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub total_fetched: usize,
}

// ─── Router ─────────────────────────────────────────────────────────────────

// ─── Review vote types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum VoteType {
    Helpful,
    Unhelpful,
    None,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewVoteRequest {
    review_id: String,
    vote: VoteType,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnswerReviewRequest {
    review_id: String,
    response_text: String,
}

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/products/submit-rating", post(submit_rating))
        .route(
            "/api/products/submit-rating-atomic",
            post(submit_rating_atomic),
        )
        .route("/api/products/ratings", post(get_ratings))
        .route("/api/products/review-vote", post(review_vote))
        .route("/api/products/answer-review", post(answer_review))
        .with_state(state)
}

// ─── Handlers ───────────────────────────────────────────────────────────────

async fn submit_rating(
    State(state): State<HandlersState>,
    Json(req): Json<SubmitRatingRequest>,
) -> Result<Json<SubmitRatingResponse>, ob_core::Error> {
    validate_uid("productId", &req.product_id)?;
    validate_uid("userId", &req.user_id)?;
    validate_uid("orderId", &req.order_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "submit_rating",
        5,  // 5 reviews
        60, // per hour
    )
    .await?;

    // Validate rating range
    if !(1.0..=5.0).contains(&req.rating) {
        return Err(ob_core::Error::Validation(
            "Rating must be between 1 and 5".into(),
        ));
    }

    // Sanitize review text
    let review_text = req.review_text.as_deref().unwrap_or("");
    let review = if review_text.is_empty() {
        String::new()
    } else {
        let sanitized = sanitize_html(review_text);
        if sanitized.len() > MAX_REVIEW_LENGTH {
            sanitized[..MAX_REVIEW_LENGTH].to_string()
        } else {
            sanitized
        }
    };

    // Verify order exists and belongs to user
    let order = state
        .db
        .get_document(collections::ORDERS, &req.order_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Order not found".into()))?;

    if order.is_null() {
        return Err(ob_core::Error::NotFound("Order not found".into()));
    }

    let order_buyer = order
        .get(fields::BUYER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if order_buyer != req.user_id {
        return Err(ob_core::Error::Forbidden("Order ownership mismatch".into()));
    }

    // Verify order is in ratable state (DELIVERED or DISPUTED)
    let order_status = order
        .get(fields::STATUS)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if order_status != "DELIVERED" && order_status != "DISPUTED" {
        return Err(ob_core::Error::Validation(
            "Order not in ratable state".into(),
        ));
    }

    // Verify product is in this order
    let items = order
        .get(fields::ITEMS)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let product_in_order = items.iter().any(|item| {
        item.get(fields::PRODUCT_ID).and_then(|v| v.as_str()) == Some(req.product_id.as_str())
    });

    if !product_in_order {
        return Err(ob_core::Error::Validation(
            "Product not in this order".into(),
        ));
    }

    // Block sellers from rating their own products
    let rated_item_seller = items
        .iter()
        .find(|item| {
            item.get(fields::PRODUCT_ID).and_then(|v| v.as_str()) == Some(req.product_id.as_str())
        })
        .and_then(|item| item.get(fields::SELLER_ID).and_then(|v| v.as_str()));

    if rated_item_seller == Some(req.user_id.as_str()) {
        return Err(ob_core::Error::Forbidden(
            "Sellers cannot rate their own products".into(),
        ));
    }

    // Check for duplicate rating (one per user per product)
    let dup_query = format!(
        "SELECT * FROM {} WHERE {} = '{}' AND {} = '{}' LIMIT 1",
        collections::PRODUCT_RATINGS,
        fields::PRODUCT_ID,
        ob_core::escape_surreal_string(&req.product_id),
        "userId",
        ob_core::escape_surreal_string(&req.user_id),
    );

    let existing: Vec<Value> = state.db.query_raw(&dup_query).await.unwrap_or_default();
    if !existing.is_empty() {
        return Err(ob_core::Error::Validation(
            "You have already rated this product".into(),
        ));
    }

    // Fetch current product rating data
    let product = state
        .db
        .get_document(collections::PRODUCTS, &req.product_id)
        .await
        .map_err(|_| ob_core::Error::NotFound("Product not found".into()))?;

    if product.is_null() {
        return Err(ob_core::Error::NotFound("Product not found".into()));
    }

    let curr_avg = product
        .get(fields::AVG_RATING)
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let curr_count = product
        .get(fields::TOTAL_REVIEWS)
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let new_count = curr_count + 1;
    let new_avg = ((curr_avg * curr_count as f64) + req.rating) / new_count as f64;

    // Create rating document
    let now = chrono::Utc::now().to_rfc3339();
    let rating_doc = serde_json::json!({
        fields::PRODUCT_ID: req.product_id,
        "userId": req.user_id,
        "orderId": req.order_id,
        fields::RATING: req.rating,
        fields::REVIEW_TEXT: review,
        "reviewImageUrls": req.review_image_urls,
        fields::HELPFUL_COUNT: 0,
        "verifiedPurchase": true,
        fields::CREATED_AT: now,
    });

    state
        .db
        .create_document(collections::PRODUCT_RATINGS, rating_doc)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to create rating: {e}")))?;

    // Update product aggregate
    let product_update = serde_json::json!({
        fields::AVG_RATING: new_avg,
        fields::TOTAL_REVIEWS: new_count,
        fields::UPDATED_AT: now,
    });

    state
        .db
        .update_document(collections::PRODUCTS, &req.product_id, product_update)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to update product rating: {e}")))?;

    info!(
        product_id = %req.product_id,
        user_id = %req.user_id,
        rating = req.rating,
        new_avg = new_avg,
        "Rating submitted"
    );

    Ok(Json(SubmitRatingResponse {
        success: true,
        new_rating: new_avg,
        rating_count: new_count,
    }))
}

async fn get_ratings(
    State(state): State<HandlersState>,
    Json(req): Json<GetRatingsRequest>,
) -> Result<Json<GetRatingsResponse>, ob_core::Error> {
    validate_uid("productId", &req.product_id)?;

    let limit = req.limit.min(MAX_RATINGS_PAGE);

    // Validate minRating
    if let Some(min) = req.min_rating
        && !(1.0..=5.0).contains(&min)
    {
        return Err(ob_core::Error::Validation(
            "minRating must be between 1 and 5".into(),
        ));
    }

    let mut conditions = vec![format!(
        "{} = '{}'",
        fields::PRODUCT_ID,
        ob_core::escape_surreal_string(&req.product_id)
    )];

    if let Some(min) = req.min_rating {
        conditions.push(format!("{} >= {}", fields::RATING, min));
    }

    let where_clause = format!(" WHERE {}", conditions.join(" AND "));
    let fetch_limit = limit + 1;

    let query = format!(
        "SELECT * FROM {}{} ORDER BY {} DESC LIMIT {}",
        collections::PRODUCT_RATINGS,
        where_clause,
        fields::CREATED_AT,
        fetch_limit,
    );

    let rows: Vec<Value> = state
        .db
        .query_raw(&query)
        .await
        .map_err(|e| ob_core::Error::Database(format!("Failed to fetch ratings: {e}")))?;

    let has_more = rows.len() > limit as usize;
    let ratings: Vec<Value> = rows.into_iter().take(limit as usize).collect();

    let next_cursor = if has_more {
        ratings
            .last()
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_str())
            .map(String::from)
    } else {
        None
    };

    let total_fetched = ratings.len();

    Ok(Json(GetRatingsResponse {
        ratings,
        next_cursor,
        has_more,
        total_fetched,
    }))
}

/// Vote on a review (helpful/unhelpful).
async fn review_vote(
    State(state): State<HandlersState>,
    auth: axum::extract::Extension<ob_auth::middleware::AuthContext>,
    Json(req): Json<ReviewVoteRequest>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    if !auth.authenticated || auth.user_id.is_empty() {
        return Err(ob_core::Error::Auth("Authentication required".into()));
    }
    let user_id = auth.user_id.clone();

    validate_uid("reviewId", &req.review_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &user_id,
        "review_vote",
        20, // 20 votes
        60, // per hour
    )
    .await?;

    let vote_str = match req.vote {
        VoteType::Helpful => "helpful",
        VoteType::Unhelpful => "unhelpful",
        VoteType::None => "none",
    };

    let vote_col = collections::REVIEW_VOTES;
    let find_query = format!(
        "SELECT * FROM {} WHERE reviewId = '{}' AND userId = '{}' LIMIT 1",
        vote_col,
        ob_core::escape_surreal_string(&req.review_id),
        ob_core::escape_surreal_string(&user_id)
    );
    let existing: Vec<serde_json::Value> = state.db.query_raw(&find_query).await.unwrap_or_default();

    if let Some(record) = existing.first() {
        let old_vote = record.get("vote").and_then(|v| v.as_str()).unwrap_or("");
        if old_vote == vote_str {
            // Unchanged, idempotent
            return Ok(Json(serde_json::json!({
                "success": true,
                "message": "Vote already recorded"
            })));
        }

        let record_id = record.get("id").and_then(|v| v.as_str()).unwrap_or("");
        
        let update_vote_query = format!(
            "UPDATE {} SET vote = '{}', {} = time::now()",
            record_id, vote_str, fields::UPDATED_AT
        );
        state.db.query_raw(&update_vote_query).await?;

        // Determine adjustments based on old and new vote
        let (helpful_adj, unhelpful_adj) = match (old_vote, vote_str) {
            ("helpful", "unhelpful") => (-1, 1),
            ("unhelpful", "helpful") => (1, -1),
            ("none", "helpful") => (1, 0),
            ("none", "unhelpful") => (0, 1),
            ("helpful", "none") => (-1, 0),
            ("unhelpful", "none") => (0, -1),
            _ => Default::default(), // Should not happen given logic above
        };

        if helpful_adj != 0 || unhelpful_adj != 0 {
            let update_ratings_query = format!(
                "UPDATE {}:{} SET helpfulVotes += {}, unhelpfulVotes += {}, {} = time::now()",
                collections::PRODUCT_RATINGS,
                ob_core::escape_surreal_string(&req.review_id),
                helpful_adj,
                unhelpful_adj,
                fields::UPDATED_AT
            );
            state.db.query_raw(&update_ratings_query).await?;
        }
    } else {
        let create_vote_query = format!(
            "CREATE {} SET reviewId = '{}', userId = '{}', vote = '{}', createdAt = time::now(), {} = time::now()",
            vote_col,
            ob_core::escape_surreal_string(&req.review_id),
            ob_core::escape_surreal_string(&user_id),
            vote_str,
            fields::UPDATED_AT
        );
        state.db.query_raw(&create_vote_query).await?;

        let (helpful_adj, unhelpful_adj) = match vote_str {
            "helpful" => (1, 0),
            "unhelpful" => (0, 1),
            _ => (0, 0),
        };

        if helpful_adj != 0 || unhelpful_adj != 0 {
            let update_ratings_query = format!(
                "UPDATE {}:{} SET helpfulVotes += {}, unhelpfulVotes += {}, {} = time::now()",
                collections::PRODUCT_RATINGS,
                ob_core::escape_surreal_string(&req.review_id),
                helpful_adj,
                unhelpful_adj,
                fields::UPDATED_AT
            );
            state.db.query_raw(&update_ratings_query).await?;
        }
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Vote recorded"
    })))
}

async fn submit_rating_atomic(
    State(state): State<HandlersState>,
    Json(req): Json<SubmitRatingAtomicRequest>,
) -> Result<Json<SubmitRatingResponse>, ob_core::Error> {
    submit_rating(
        State(state),
        Json(SubmitRatingRequest {
            product_id: req.product_id,
            user_id: req.user_id,
            order_id: req.order_id,
            rating: req.rating,
            review_text: req.review_text,
            review_image_urls: req.images,
        }),
    )
    .await
}

async fn answer_review(
    State(state): State<HandlersState>,
    auth: axum::extract::Extension<ob_auth::middleware::AuthContext>,
    Json(req): Json<AnswerReviewRequest>,
) -> Result<Json<serde_json::Value>, ob_core::Error> {
    if !auth.authenticated || auth.user_id.is_empty() {
        return Err(ob_core::Error::Auth("Authentication required".into()));
    }
    let seller_id = auth.user_id.clone();

    validate_uid("reviewId", &req.review_id)?;
    validate_uid("sellerId", &seller_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &seller_id,
        "answer_review",
        20, // 20 answers
        60, // per hour
    )
    .await?;

    let response_text = sanitize_html(&req.response_text);
    if response_text.trim().is_empty() {
        return Err(ob_core::Error::Validation(
            "responseText is required".into(),
        ));
    }

    let review = state
        .db
        .get_document(collections::PRODUCT_RATINGS, &req.review_id)
        .await?;
    let product_id = review
        .get(fields::PRODUCT_ID)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ob_core::Error::Validation("Review missing productId".into()))?;
    let product = state
        .db
        .get_document(collections::PRODUCTS, product_id)
        .await?;
    let owner = product
        .get(fields::SELLER_ID)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if owner != seller_id && !auth.has_role("admin") {
        return Err(ob_core::Error::Forbidden(
            "Only the product seller can answer reviews".into(),
        ));
    }

    let now = chrono::Utc::now().to_rfc3339();
    state
        .db
        .update_document(
            collections::PRODUCT_RATINGS,
            &req.review_id,
            serde_json::json!({
                "sellerResponse": response_text,
                "sellerRespondedAt": now,
                fields::UPDATED_AT: now,
            }),
        )
        .await?;

    Ok(Json(serde_json::json!({ "success": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
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
        }
    }

    #[test]
    fn test_rating_request_deser() {
        let json = r#"{
            "productId": "prod1",
            "userId": "user1",
            "orderId": "ord1",
            "rating": 4.5,
            "reviewText": "Great product!"
        }"#;
        let req: SubmitRatingRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.rating, 4.5);
        assert_eq!(req.review_text.unwrap(), "Great product!");
    }

    #[test]
    fn test_rating_avg_calculation() {
        let curr_avg: f64 = 4.0;
        let curr_count: i64 = 10;
        let new_rating: f64 = 5.0;
        let new_count = curr_count + 1;
        let new_avg = ((curr_avg * curr_count as f64) + new_rating) / new_count as f64;
        assert!((new_avg - 4.0909).abs() < 0.001);
    }

    #[test]
    fn test_review_truncation() {
        let long_review = "a".repeat(2000);
        let sanitized = sanitize_html(&long_review);
        let truncated = if sanitized.len() > MAX_REVIEW_LENGTH {
            sanitized[..MAX_REVIEW_LENGTH].to_string()
        } else {
            sanitized
        };
        assert_eq!(truncated.len(), MAX_REVIEW_LENGTH);
    }

    // ── Ported from test_handlers_products_engagement_deep.py ──

    #[test]
    fn test_rating_range_boundary_values() {
        // Valid ratings
        assert!((1.0..=5.0).contains(&1.0));
        assert!((1.0..=5.0).contains(&5.0));
        assert!((1.0..=5.0).contains(&3.5));
        assert!((1.0..=5.0).contains(&1.1));
        assert!((1.0..=5.0).contains(&4.9));

        // Invalid ratings
        assert!(!(1.0..=5.0).contains(&0.0));
        assert!(!(1.0..=5.0).contains(&0.99));
        assert!(!(1.0..=5.0).contains(&5.01));
        assert!(!(1.0..=5.0).contains(&-1.0));
        assert!(!(1.0..=5.0).contains(&10.0));
    }

    #[test]
    fn test_rating_avg_first_review() {
        let curr_avg: f64 = 0.0;
        let curr_count: i64 = 0;
        let new_rating: f64 = 4.0;
        let new_count = curr_count + 1;
        let new_avg = ((curr_avg * curr_count as f64) + new_rating) / new_count as f64;
        assert!((new_avg - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_rating_avg_large_count() {
        let curr_avg: f64 = 4.5;
        let curr_count: i64 = 1000;
        let new_rating: f64 = 1.0;
        let new_count = curr_count + 1;
        let new_avg = ((curr_avg * curr_count as f64) + new_rating) / new_count as f64;
        // Adding a single 1-star to 1000 reviews at 4.5 barely changes it
        assert!(new_avg > 4.49 && new_avg < 4.50);
    }

    #[test]
    fn test_rating_request_missing_review_text_default() {
        let json = r#"{
            "productId": "prod1",
            "userId": "user1",
            "orderId": "ord1",
            "rating": 3.0
        }"#;
        let req: SubmitRatingRequest = serde_json::from_str(json).unwrap();
        assert!(req.review_text.is_none());
        assert!(req.review_image_urls.is_empty());
    }

    #[test]
    fn test_rating_request_with_images() {
        let json = r#"{
            "productId": "prod1",
            "userId": "user1",
            "orderId": "ord1",
            "rating": 5.0,
            "reviewImageUrls": ["https://cdn.example.com/r1.jpg", "https://cdn.example.com/r2.jpg"]
        }"#;
        let req: SubmitRatingRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.review_image_urls.len(), 2);
    }

    #[test]
    fn test_get_ratings_request_defaults() {
        let json = r#"{"productId": "prod1"}"#;
        let req: GetRatingsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.limit, 10);
        assert!(req.start_after.is_none());
        assert!(req.min_rating.is_none());
    }

    #[test]
    fn test_get_ratings_min_rating_validation() {
        // Valid min ratings
        assert!((1.0..=5.0).contains(&1.0));
        assert!((1.0..=5.0).contains(&5.0));
        // Invalid
        assert!(!(1.0..=5.0).contains(&0.5));
        assert!(!(1.0..=5.0).contains(&10.0));
        assert!(!(1.0..=5.0).contains(&-1.0));
    }

    #[test]
    fn test_get_ratings_limit_clamping() {
        let json = r#"{"productId": "prod1", "limit": 200}"#;
        let req: GetRatingsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.limit.min(MAX_RATINGS_PAGE), MAX_RATINGS_PAGE);
    }

    #[test]
    fn test_empty_review_text_sanitization() {
        let review_text = "";
        let review = if review_text.is_empty() {
            String::new()
        } else {
            sanitize_html(review_text)
        };
        assert!(review.is_empty());
    }

    #[test]
    fn test_review_html_sanitization() {
        let input = "<script>alert('xss')</script>Great product!";
        let sanitized = sanitize_html(input);
        assert!(!sanitized.contains("<script>"));
        assert!(sanitized.contains("Great product!"));
    }

    #[test]
    fn test_submit_rating_response_serialize() {
        let resp = SubmitRatingResponse {
            success: true,
            new_rating: 4.25,
            rating_count: 4,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("4.25"));
        assert!(json.contains("\"ratingCount\":4"));
    }

    #[test]
    fn test_rating_atomic_request_deser() {
        let json = r#"{
            "productId": "p1",
            "userId": "u1",
            "orderId": "o1",
            "rating": 4.0,
            "reviewText": "Nice",
            "images": ["https://cdn/img1.jpg"]
        }"#;
        let req: SubmitRatingAtomicRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.images.len(), 1);
        assert_eq!(req.review_text.as_deref(), Some("Nice"));
    }

    #[test]
    fn test_review_vote_request_deser() {
        let json = r#"{"reviewId": "r1", "vote": "helpful"}"#;
        let req: ReviewVoteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.vote, VoteType::Helpful);

        let json2 = r#"{"reviewId": "r1", "vote": "unhelpful"}"#;
        let req2: ReviewVoteRequest = serde_json::from_str(json2).unwrap();
        let vote_field = if req2.vote == VoteType::Helpful {
            "helpfulVotes"
        } else {
            "unhelpfulVotes"
        };
        assert_eq!(vote_field, "unhelpfulVotes");
    }

    #[test]
    fn test_answer_review_request_deser() {
        let json = r#"{"reviewId": "r1", "responseText": "Thank you for the feedback!"}"#;
        let req: AnswerReviewRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.review_id, "r1");
        assert!(!req.response_text.is_empty());
    }

    #[test]
    fn test_answer_review_empty_text_rejected() {
        let response_text = sanitize_html("   ");
        assert!(response_text.trim().is_empty());
    }

    #[test]
    fn test_get_ratings_response_serialize() {
        let resp = GetRatingsResponse {
            ratings: vec![serde_json::json!({"rating": 5.0})],
            next_cursor: None,
            has_more: false,
            total_fetched: 1,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"hasMore\":false"));
        assert!(json.contains("\"totalFetched\":1"));
    }

    #[tokio::test]
    async fn test_submit_rating_rejects_duplicate_rating() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                serde_json::json!({
                    fields::BUYER_ID: "buyer_1",
                    fields::STATUS: "DELIVERED",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                    }],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({
                    fields::SELLER_ID: "seller_1",
                    fields::AVG_RATING: 4.0,
                    fields::TOTAL_REVIEWS: 1,
                }),
            )
            .await
            .unwrap();
        let _ = state
            .db
            .create_document(
                collections::PRODUCT_RATINGS,
                serde_json::json!({
                    fields::PRODUCT_ID: "prod_1",
                    "userId": "buyer_1",
                }),
            )
            .await
            .unwrap();

        let err = submit_rating(
            State(state),
            Json(SubmitRatingRequest {
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                order_id: "ord_1".into(),
                rating: 5.0,
                review_text: Some("great".into()),
                review_image_urls: vec![],
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("already rated"));
    }

    #[tokio::test]
    async fn test_submit_rating_rejects_self_rating() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                serde_json::json!({
                    fields::BUYER_ID: "seller_1",
                    fields::STATUS: "DELIVERED",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                    }],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(collections::PRODUCTS, "prod_1", serde_json::json!({}))
            .await
            .unwrap();

        let err = submit_rating(
            State(state),
            Json(SubmitRatingRequest {
                product_id: "prod_1".into(),
                user_id: "seller_1".into(),
                order_id: "ord_1".into(),
                rating: 4.0,
                review_text: None,
                review_image_urls: vec![],
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("cannot rate their own"));
    }

    #[tokio::test]
    async fn test_submit_rating_success_updates_product_aggregate() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                serde_json::json!({
                    fields::BUYER_ID: "buyer_1",
                    fields::STATUS: "DELIVERED",
                    fields::ITEMS: [{
                        fields::PRODUCT_ID: "prod_1",
                        fields::SELLER_ID: "seller_1",
                    }],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({
                    fields::AVG_RATING: 4.0,
                    fields::TOTAL_REVIEWS: 1,
                    fields::SELLER_ID: "seller_1",
                }),
            )
            .await
            .unwrap();

        let Json(resp) = submit_rating(
            State(state.clone()),
            Json(SubmitRatingRequest {
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                order_id: "ord_1".into(),
                rating: 5.0,
                review_text: Some("Excellent".into()),
                review_image_urls: vec!["https://cdn.example.com/review.jpg".into()],
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.rating_count, 2);
        let product = state
            .db
            .get_document(collections::PRODUCTS, "prod_1")
            .await
            .unwrap();
        assert!(product[fields::AVG_RATING].as_f64().unwrap() > 4.4);
    }

    #[tokio::test]
    async fn test_get_ratings_filters_and_paginates() {
        let state = setup_state().await;
        for (id, rating, product, created_at) in [
            ("r1", 5.0, "prod_1", "2026-01-03T00:00:00Z"),
            ("r2", 4.0, "prod_1", "2026-01-02T00:00:00Z"),
            ("r3", 5.0, "prod_2", "2026-01-01T00:00:00Z"),
        ] {
            state
                .db
                .upsert_document(
                    collections::PRODUCT_RATINGS,
                    id,
                    serde_json::json!({
                        fields::PRODUCT_ID: product,
                        fields::RATING: rating,
                        fields::CREATED_AT: created_at,
                    }),
                )
                .await
                .unwrap();
        }

        let Json(resp) = get_ratings(
            State(state),
            Json(GetRatingsRequest {
                product_id: "prod_1".into(),
                limit: 1,
                start_after: None,
                min_rating: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.total_fetched, 1);
        assert!(resp.has_more);
        assert_eq!(resp.ratings[0][fields::PRODUCT_ID], "prod_1");
        assert_eq!(resp.ratings[0][fields::RATING], 5.0);
    }

    #[tokio::test]
    async fn test_answer_review_rejects_non_owner() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCT_RATINGS,
                "rev_1",
                serde_json::json!({ fields::PRODUCT_ID: "prod_1" }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::SELLER_ID: "seller_1" }),
            )
            .await
            .unwrap();

        let mut auth_ctx = ob_auth::middleware::AuthContext::anonymous();
        auth_ctx.authenticated = true;
        auth_ctx.user_id = "seller_2".into();

        let err = answer_review(
            State(state),
            axum::extract::Extension(auth_ctx),
            Json(AnswerReviewRequest {
                review_id: "rev_1".into(),
                response_text: "Thanks".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Only the product seller"));
    }

    // ── Coverage: submit_rating validation paths ──

    #[tokio::test]
    async fn test_submit_rating_invalid_rating_range() {
        let state = setup_state().await;
        let err = submit_rating(
            State(state),
            Json(SubmitRatingRequest {
                product_id: "prod_1".into(),
                user_id: "user_1".into(),
                order_id: "ord_1".into(),
                rating: 0.5,
                review_text: None,
                review_image_urls: vec![],
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("between 1 and 5"));
    }

    #[tokio::test]
    async fn test_submit_rating_long_review_truncated() {
        let state = setup_state().await;
        state.db.upsert_document(
            collections::ORDERS, "ord_1",
            serde_json::json!({
                fields::BUYER_ID: "buyer_1",
                fields::STATUS: "DELIVERED",
                fields::ITEMS: [{ fields::PRODUCT_ID: "prod_1", fields::SELLER_ID: "seller_1" }],
            }),
        ).await.unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({
                    fields::SELLER_ID: "seller_1",
                    fields::AVG_RATING: 0.0,
                    fields::TOTAL_REVIEWS: 0,
                }),
            )
            .await
            .unwrap();

        let long_text = "a".repeat(2000);
        let Json(resp) = submit_rating(
            State(state),
            Json(SubmitRatingRequest {
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                order_id: "ord_1".into(),
                rating: 4.0,
                review_text: Some(long_text),
                review_image_urls: vec![],
            }),
        )
        .await
        .unwrap();
        assert!(resp.success);
    }

    #[tokio::test]
    async fn test_submit_rating_order_not_found() {
        let state = setup_state().await;
        let err = submit_rating(
            State(state),
            Json(SubmitRatingRequest {
                product_id: "prod_1".into(),
                user_id: "user_1".into(),
                order_id: "nonexistent".into(),
                rating: 4.0,
                review_text: None,
                review_image_urls: vec![],
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_submit_rating_order_ownership_mismatch() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                serde_json::json!({
                    fields::BUYER_ID: "other_user",
                    fields::STATUS: "DELIVERED",
                    fields::ITEMS: [{ fields::PRODUCT_ID: "prod_1" }],
                }),
            )
            .await
            .unwrap();

        let err = submit_rating(
            State(state),
            Json(SubmitRatingRequest {
                product_id: "prod_1".into(),
                user_id: "user_1".into(),
                order_id: "ord_1".into(),
                rating: 4.0,
                review_text: None,
                review_image_urls: vec![],
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("ownership mismatch"));
    }

    #[tokio::test]
    async fn test_submit_rating_order_not_ratable() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                serde_json::json!({
                    fields::BUYER_ID: "user_1",
                    fields::STATUS: "PENDING_PAYMENT",
                    fields::ITEMS: [{ fields::PRODUCT_ID: "prod_1" }],
                }),
            )
            .await
            .unwrap();

        let err = submit_rating(
            State(state),
            Json(SubmitRatingRequest {
                product_id: "prod_1".into(),
                user_id: "user_1".into(),
                order_id: "ord_1".into(),
                rating: 4.0,
                review_text: None,
                review_image_urls: vec![],
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not in ratable state"));
    }

    #[tokio::test]
    async fn test_submit_rating_product_not_in_order() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::ORDERS,
                "ord_1",
                serde_json::json!({
                    fields::BUYER_ID: "user_1",
                    fields::STATUS: "DELIVERED",
                    fields::ITEMS: [{ fields::PRODUCT_ID: "other_prod" }],
                }),
            )
            .await
            .unwrap();

        let err = submit_rating(
            State(state),
            Json(SubmitRatingRequest {
                product_id: "prod_1".into(),
                user_id: "user_1".into(),
                order_id: "ord_1".into(),
                rating: 4.0,
                review_text: None,
                review_image_urls: vec![],
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not in this order"));
    }

    #[tokio::test]
    async fn test_submit_rating_product_not_found() {
        let state = setup_state().await;
        state.db.upsert_document(
            collections::ORDERS, "ord_1",
            serde_json::json!({
                fields::BUYER_ID: "buyer_1",
                fields::STATUS: "DELIVERED",
                fields::ITEMS: [{ fields::PRODUCT_ID: "prod_1", fields::SELLER_ID: "seller_1" }],
            }),
        ).await.unwrap();

        let err = submit_rating(
            State(state),
            Json(SubmitRatingRequest {
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                order_id: "ord_1".into(),
                rating: 4.0,
                review_text: None,
                review_image_urls: vec![],
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // ── Coverage: get_ratings with min_rating filter and invalid min_rating ──

    #[tokio::test]
    async fn test_get_ratings_invalid_min_rating() {
        let state = setup_state().await;
        let err = get_ratings(
            State(state),
            Json(GetRatingsRequest {
                product_id: "prod_1".into(),
                limit: 10,
                start_after: None,
                min_rating: Some(0.5),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("minRating must be between 1 and 5")
        );
    }

    #[tokio::test]
    async fn test_get_ratings_with_min_rating_filter() {
        let state = setup_state().await;
        for (id, rating) in [("r1", 5.0), ("r2", 2.0), ("r3", 4.0)] {
            state
                .db
                .upsert_document(
                    collections::PRODUCT_RATINGS,
                    id,
                    serde_json::json!({
                        fields::PRODUCT_ID: "prod_1",
                        fields::RATING: rating,
                        fields::CREATED_AT: "2026-01-01T00:00:00Z",
                    }),
                )
                .await
                .unwrap();
        }

        let Json(resp) = get_ratings(
            State(state),
            Json(GetRatingsRequest {
                product_id: "prod_1".into(),
                limit: 10,
                start_after: None,
                min_rating: Some(4.0),
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp.total_fetched, 2);
        assert!(!resp.has_more);
        assert!(resp.next_cursor.is_none());
    }

    // ── Coverage: review_vote handler ──

    #[tokio::test]
    async fn test_review_vote_helpful() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCT_RATINGS,
                "rev_1",
                serde_json::json!({ "helpfulVotes": 0 }),
            )
            .await
            .unwrap();

        let mut auth_ctx = ob_auth::middleware::AuthContext::anonymous();
        auth_ctx.authenticated = true;
        auth_ctx.user_id = "user_1".into();

        let Json(resp) = review_vote(
            State(state),
            axum::extract::Extension(auth_ctx),
            Json(ReviewVoteRequest {
                review_id: "rev_1".into(),
                vote: VoteType::Helpful,
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp["success"], true);
    }

    #[tokio::test]
    async fn test_review_vote_unhelpful() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCT_RATINGS,
                "rev_1",
                serde_json::json!({ "unhelpfulVotes": 0 }),
            )
            .await
            .unwrap();

        let mut auth_ctx = ob_auth::middleware::AuthContext::anonymous();
        auth_ctx.authenticated = true;
        auth_ctx.user_id = "user_1".into();

        let Json(resp) = review_vote(
            State(state),
            axum::extract::Extension(auth_ctx),
            Json(ReviewVoteRequest {
                review_id: "rev_1".into(),
                vote: VoteType::Unhelpful,
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp["success"], true);
    }

    // ── Coverage: submit_rating_atomic pass-through ──

    #[tokio::test]
    async fn test_submit_rating_atomic_passes_through() {
        let state = setup_state().await;
        state.db.upsert_document(
            collections::ORDERS, "ord_1",
            serde_json::json!({
                fields::BUYER_ID: "buyer_1",
                fields::STATUS: "DELIVERED",
                fields::ITEMS: [{ fields::PRODUCT_ID: "prod_1", fields::SELLER_ID: "seller_1" }],
            }),
        ).await.unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({
                    fields::SELLER_ID: "seller_1",
                    fields::AVG_RATING: 0.0,
                    fields::TOTAL_REVIEWS: 0,
                }),
            )
            .await
            .unwrap();

        let Json(resp) = submit_rating_atomic(
            State(state),
            Json(SubmitRatingAtomicRequest {
                product_id: "prod_1".into(),
                user_id: "buyer_1".into(),
                order_id: "ord_1".into(),
                rating: 5.0,
                review_text: Some("Great".into()),
                images: vec!["https://cdn/img.jpg".into()],
            }),
        )
        .await
        .unwrap();
        assert!(resp.success);
        assert_eq!(resp.rating_count, 1);
    }

    // ── Coverage: answer_review success + empty text ──

    #[tokio::test]
    async fn test_answer_review_success() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCT_RATINGS,
                "rev_1",
                serde_json::json!({ fields::PRODUCT_ID: "prod_1" }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                serde_json::json!({ fields::SELLER_ID: "seller_1" }),
            )
            .await
            .unwrap();

        let mut auth_ctx = ob_auth::middleware::AuthContext::anonymous();
        auth_ctx.authenticated = true;
        auth_ctx.user_id = "seller_1".into();

        let Json(resp) = answer_review(
            State(state),
            axum::extract::Extension(auth_ctx),
            Json(AnswerReviewRequest {
                review_id: "rev_1".into(),
                response_text: "Thank you!".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp["success"], true);
    }

    #[tokio::test]
    async fn test_answer_review_rejects_empty_text() {
        let state = setup_state().await;

        let mut auth_ctx = ob_auth::middleware::AuthContext::anonymous();
        auth_ctx.authenticated = true;
        auth_ctx.user_id = "seller_1".into();

        let err = answer_review(
            State(state),
            axum::extract::Extension(auth_ctx),
            Json(AnswerReviewRequest {
                review_id: "rev_1".into(),
                response_text: "   ".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("responseText is required"));
    }
}
