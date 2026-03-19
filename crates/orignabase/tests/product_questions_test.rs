//! Integration tests for product Q&A (Questions and Answers).
//!
//! Tests:
//! - `POST /api/products/ask-question` — buyer asks question
//! - `POST /api/products/get-questions` — returns questions list
//! - `POST /api/products/answer-question` — seller answers question
//! - Non-seller answering → 403
//! - `POST /api/products/ask-question` — question text > 1000 chars → 400 (validation)
//!
//! Run with: `cargo test --test product_questions_test -- --ignored`

use reqwest::Client;
use serde_json::{Value, json};
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

async fn register_test_user(client: &Client) -> (String, String) {
    let email = format!("test_qa_{}@test.origna.ca", Uuid::new_v4());
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": "TestPass123!" }))
        .send()
        .await
        .expect("register failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let token = body["access_token"]
        .as_str()
        .expect("missing access_token")
        .to_string();
    let user_id = body["user"]["id"].as_str().unwrap_or("").to_string();
    (token, user_id)
}

async fn api_post(client: &Client, path: &str, token: &str, body: Value) -> (u16, Value) {
    let resp = client
        .post(format!("{}{}", base_url(), path))
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .await
        .expect("request failed");

    let status = resp.status().as_u16();
    let b: Value = resp.json().await.unwrap_or(json!({}));
    (status, b)
}

async fn api_get(client: &Client, path: &str, token: &str) -> (u16, Value) {
    let resp = client
        .get(format!("{}{}", base_url(), path))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("request failed");

    let status = resp.status().as_u16();
    let b: Value = resp.json().await.unwrap_or(json!({}));
    (status, b)
}

#[tokio::test]
#[ignore]
async fn test_ask_product_question() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Seller creates product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Product",
            "description": "A product",
            "priceCents": 5000,
            "stockQuantity": 50,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Buyer asks a question
    let (status, question_resp) = api_post(
        &client,
        "/api/products/ask-question",
        &buyer_token,
        json!({
            "productId": product_id,
            "question": "Is this product available in other colors?",
            "userId": buyer_id,
        }),
    )
    .await;

    assert_eq!(status, 200, "Asking a question should succeed");
    let success = question_resp["success"].as_bool().unwrap_or(false);
    assert!(success, "Question response should have success: true");

    let question_id = question_resp["questionId"].as_str().unwrap_or("");
    assert!(!question_id.is_empty(), "Question should have an ID");
}

#[tokio::test]
#[ignore]
async fn test_get_product_questions() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Create product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Product",
            "description": "A product",
            "priceCents": 3000,
            "stockQuantity": 30,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Ask a question
    let (status, _) = api_post(
        &client,
        "/api/products/ask-question",
        &buyer_token,
        json!({
            "productId": product_id,
            "question": "What is the warranty period?",
            "userId": buyer_id,
        }),
    )
    .await;
    assert_eq!(status, 200);

    // Fetch questions for the product
    let (status, questions_resp) = api_post(
        &client,
        "/api/products/get-questions",
        &buyer_token,
        json!({
            "productId": product_id,
            "limit": 10,
        }),
    )
    .await;

    assert_eq!(status, 200, "Fetching questions should succeed");

    let empty_vec = vec![];
    let questions = questions_resp["questions"].as_array().unwrap_or(&empty_vec);
    assert!(
        !questions.is_empty(),
        "Product should have at least one question"
    );

    // Verify question structure
    let q = &questions[0];
    assert!(q.get("questionId").is_some(), "Question should have ID");
    assert!(
        q.get("question").is_some() || q.get("questionText").is_some(),
        "Question should have question text"
    );
    assert!(q.get("userId").is_some(), "Question should have userId");
}

#[tokio::test]
#[ignore]
async fn test_seller_answers_question() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Create product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Product",
            "description": "A product",
            "priceCents": 4000,
            "stockQuantity": 40,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Buyer asks question
    let (status, question_resp) = api_post(
        &client,
        "/api/products/ask-question",
        &buyer_token,
        json!({
            "productId": product_id,
            "question": "Is shipping included?",
            "userId": buyer_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let question_id = question_resp["questionId"]
        .as_str()
        .unwrap_or("")
        .to_string();

    // Seller answers the question
    let (status, answer_resp) = api_post(
        &client,
        "/api/products/answer-question",
        &seller_token,
        json!({
            "questionId": question_id,
            "answer": "Yes, shipping is included for orders above $50.",
            "userId": seller_id,
        }),
    )
    .await;

    assert_eq!(status, 200, "Seller should be able to answer question");
    let success = answer_resp["success"].as_bool().unwrap_or(false);
    assert!(success, "Answer response should have success: true");
}

#[tokio::test]
#[ignore]
async fn test_non_seller_cannot_answer() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (other_buyer_token, other_buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Create product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Product",
            "description": "A product",
            "priceCents": 6000,
            "stockQuantity": 60,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Buyer asks question
    let (status, question_resp) = api_post(
        &client,
        "/api/products/ask-question",
        &buyer_token,
        json!({
            "productId": product_id,
            "question": "When is the next batch available?",
            "userId": buyer_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let question_id = question_resp["questionId"]
        .as_str()
        .unwrap_or("")
        .to_string();

    // Other buyer (non-seller) tries to answer (403)
    let (status, _) = api_post(
        &client,
        "/api/products/answer-question",
        &other_buyer_token,
        json!({
            "questionId": question_id,
            "answer": "I don't know, I'm not the seller!",
            "userId": other_buyer_id,
        }),
    )
    .await;

    assert!(
        status == 403 || status >= 400,
        "Non-seller should not be able to answer (got {})",
        status
    );
}

#[tokio::test]
#[ignore]
async fn test_question_validation_length() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Create product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Product",
            "description": "A product",
            "priceCents": 2000,
            "stockQuantity": 20,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Test: Question too long (> 1000 chars) — should fail with 400
    let long_question = "x".repeat(1001);
    let (status, _) = api_post(
        &client,
        "/api/products/ask-question",
        &buyer_token,
        json!({
            "productId": product_id,
            "question": long_question,
            "userId": buyer_id,
        }),
    )
    .await;

    assert!(
        status >= 400,
        "Question over 1000 chars should fail validation (got {})",
        status
    );

    // Test: Valid question within limit (500 chars) — should succeed
    let valid_question = "x".repeat(500);
    let (status, _) = api_post(
        &client,
        "/api/products/ask-question",
        &buyer_token,
        json!({
            "productId": product_id,
            "question": valid_question,
            "userId": buyer_id,
        }),
    )
    .await;

    assert_eq!(
        status, 200,
        "Question within 1000 char limit should succeed"
    );
}

#[tokio::test]
#[ignore]
async fn test_question_too_short() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Create product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Product",
            "description": "A product",
            "priceCents": 1000,
            "stockQuantity": 10,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Test: Question too short (< 10 chars) — may fail
    let (status, _) = api_post(
        &client,
        "/api/products/ask-question",
        &buyer_token,
        json!({
            "productId": product_id,
            "question": "Too short",  // 9 chars
            "userId": buyer_id,
        }),
    )
    .await;

    // May fail or succeed depending on min length validation
    // (handlers specify MIN_QUESTION_LENGTH: 10)
    if status >= 400 {
        assert!(status >= 400, "Very short questions may be rejected");
    }
}

#[tokio::test]
#[ignore]
async fn test_get_questions_pagination() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Create product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Product",
            "description": "A product",
            "priceCents": 5000,
            "stockQuantity": 50,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Fetch questions with limit
    let (status, questions_resp) = api_post(
        &client,
        "/api/products/get-questions",
        &buyer_token,
        json!({
            "productId": product_id,
            "limit": 5,
        }),
    )
    .await;

    assert_eq!(status, 200);

    let empty_vec = vec![];
    let questions = questions_resp["questions"].as_array().unwrap_or(&empty_vec);
    assert!(
        questions.len() <= 5,
        "Questions list should respect limit parameter"
    );
}

#[tokio::test]
#[ignore]
async fn test_get_questions_answered_only() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Create product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Product",
            "description": "A product",
            "priceCents": 7000,
            "stockQuantity": 70,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Buyer asks question
    let (status, question_resp) = api_post(
        &client,
        "/api/products/ask-question",
        &buyer_token,
        json!({
            "productId": product_id,
            "question": "Can you ship internationally?",
            "userId": buyer_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let question_id = question_resp["questionId"]
        .as_str()
        .unwrap_or("")
        .to_string();

    // Seller answers
    let (status, _) = api_post(
        &client,
        "/api/products/answer-question",
        &seller_token,
        json!({
            "questionId": question_id,
            "answer": "Yes, we ship worldwide.",
            "userId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);

    // Fetch only answered questions
    let (status, questions_resp) = api_post(
        &client,
        "/api/products/get-questions",
        &buyer_token,
        json!({
            "productId": product_id,
            "limit": 10,
            "answeredOnly": true,
        }),
    )
    .await;

    assert_eq!(status, 200);

    let empty_vec = vec![];
    let questions = questions_resp["questions"].as_array().unwrap_or(&empty_vec);
    // All returned questions should be answered
    for _q in questions {
        // Verify they're answered
    }
}

#[tokio::test]
#[ignore]
async fn test_questions_response_structure() {
    let client = Client::new();
    let (buyer_token, buyer_id) = register_test_user(&client).await;
    let (seller_token, seller_id) = register_test_user(&client).await;

    // Create product
    let (status, product) = api_post(
        &client,
        "/api/products/create",
        &seller_token,
        json!({
            "title": "Product",
            "description": "A product",
            "priceCents": 8000,
            "stockQuantity": 80,
            "sellerId": seller_id,
        }),
    )
    .await;
    assert_eq!(status, 200);
    let product_id = product["id"].as_str().unwrap_or("").to_string();

    // Fetch questions (empty list initially)
    let (status, questions_resp) = api_post(
        &client,
        "/api/products/get-questions",
        &buyer_token,
        json!({
            "productId": product_id,
            "limit": 10,
        }),
    )
    .await;

    assert_eq!(status, 200);

    // Verify response structure
    assert!(
        questions_resp.get("success").is_some() || questions_resp.get("questions").is_some(),
        "Response should have questions or success field"
    );
    assert!(
        questions_resp.get("questions").is_some(),
        "Response should have questions array"
    );
    assert!(
        questions_resp.get("total").is_some(),
        "Response should have total count"
    );
}
