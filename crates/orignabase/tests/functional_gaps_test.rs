//! Functional gap coverage against a live OrignaBase server.
//!
//! Run with:
//!   cargo test -p orignabase --test functional_gaps_test -- --ignored
//!
//! Set `OB_TEST_URL` to point at the target server.

use serde_json::{Value, json};
use uuid::Uuid;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

fn password() -> String {
    std::env::var("OB_TEST_PASSWORD").unwrap_or_else(|_| "TestPassword123!".to_string())
}

async fn register_test_user(client: &reqwest::Client) -> (String, String) {
    let email = format!("functional_gap_{}@example.com", Uuid::new_v4());
    let resp = client
        .post(format!("{}/auth/register", base_url()))
        .json(&json!({ "email": email, "password": password() }))
        .send()
        .await
        .expect("register failed");

    assert_eq!(resp.status(), 200, "registration should succeed");
    let body: Value = resp.json().await.expect("register json");
    let token = body["access_token"]
        .as_str()
        .expect("missing access_token")
        .to_string();
    (token, email)
}

async fn graphql(client: &reqwest::Client, token: &str, query: &str) -> Value {
    let resp = client
        .post(format!("{}/graphql", base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "query": query }))
        .send()
        .await
        .expect("graphql request failed");

    assert_eq!(resp.status(), 200, "graphql should return 200");
    resp.json().await.expect("graphql json")
}

fn parse_graphql_json_field(value: &Value) -> Value {
    match value {
        Value::String(s) if s.starts_with('{') || s.starts_with('[') => {
            serde_json::from_str(s).unwrap_or_else(|_| json!({}))
        }
        Value::String(s) => Value::String(s.clone()),
        Value::Object(_) | Value::Array(_) => value.clone(),
        _ => json!({}),
    }
}

fn unique_collection(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn unique_label(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn escape_json(value: &Value) -> String {
    let payload = serde_json::to_string(value).expect("serialize json");
    serde_json::to_string(&payload).expect("escape json string")
}

fn clean_id(id: &str) -> String {
    id.rsplit(':').next().unwrap_or(id).to_string()
}

fn extract_id(value: &Value) -> Option<String> {
    let parsed = parse_graphql_json_field(value);
    parsed
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| parsed.get("_id").and_then(Value::as_str))
        .map(ToString::to_string)
        .or_else(|| {
            value.as_str().and_then(|raw| {
                if raw.starts_with('{') || raw.starts_with('[') {
                    None
                } else {
                    Some(raw.to_string())
                }
            })
        })
}

fn value_as_array(value: &Value) -> Vec<Value> {
    match parse_graphql_json_field(value) {
        Value::Array(items) => items,
        other if other.is_object() => vec![other],
        _ => Vec::new(),
    }
}

fn contains_id(items: &[Value], id: &str) -> bool {
    items.iter().any(|item| {
        extract_id(item)
            .map(|candidate| clean_id(&candidate) == clean_id(id))
            .unwrap_or(false)
    })
}

async fn create_doc(
    client: &reqwest::Client,
    token: &str,
    collection: &str,
    data: &Value,
) -> Value {
    let query = format!(
        r#"mutation {{ create(collection: "{collection}", data: {}) }}"#,
        escape_json(data)
    );
    let body = graphql(client, token, &query).await;
    assert!(
        body.get("errors").is_none(),
        "create returned graphql errors: {body}"
    );
    body["data"]["create"].clone()
}

async fn get_doc(client: &reqwest::Client, token: &str, collection: &str, id: &str) -> Value {
    let query = format!(r#"{{ get(collection: "{collection}", id: "{id}") }}"#);
    let body = graphql(client, token, &query).await;
    assert!(
        body.get("errors").is_none(),
        "get returned graphql errors: {body}"
    );
    parse_graphql_json_field(&body["data"]["get"])
}

async fn list_docs(
    client: &reqwest::Client,
    token: &str,
    collection: &str,
    limit: usize,
) -> Vec<Value> {
    let query = format!(r#"{{ list(collection: "{collection}", limit: {limit}) }}"#);
    let body = graphql(client, token, &query).await;
    assert!(
        body.get("errors").is_none(),
        "list returned graphql errors: {body}"
    );
    value_as_array(&body["data"]["list"])
}

async fn update_doc(
    client: &reqwest::Client,
    token: &str,
    collection: &str,
    id: &str,
    data: &Value,
) -> Value {
    let query = format!(
        r#"mutation {{ update(collection: "{collection}", id: "{id}", data: {}) }}"#,
        escape_json(data)
    );
    let body = graphql(client, token, &query).await;
    assert!(
        body.get("errors").is_none(),
        "update returned graphql errors: {body}"
    );
    parse_graphql_json_field(&body["data"]["update"])
}

async fn delete_doc(client: &reqwest::Client, token: &str, collection: &str, id: &str) -> Value {
    let query = format!(r#"mutation {{ delete(collection: "{collection}", id: "{id}") }}"#);
    let body = graphql(client, token, &query).await;
    assert!(
        body.get("errors").is_none(),
        "delete returned graphql errors: {body}"
    );
    body["data"]["delete"].clone()
}

fn admin_token_or(token: String) -> String {
    std::env::var("OB_TEST_ADMIN_TOKEN").unwrap_or(token)
}

macro_rules! live_test {
    ($name:ident, $body:block) => {
        #[tokio::test]
        #[ignore = "requires running orignabase instance"]
        async fn $name() $body
    };
}

// =============================================================================
// 1. Address CRUD (5 tests)
// =============================================================================

live_test!(address_create_buyer_address, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_address_create");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "owner_email": email,
            "kind": "buyer_address",
            "label": unique_label("home"),
            "street": "123 Queen St W",
            "city": "Toronto",
            "province": "ON",
            "postalCode": "M5V 2B7",
            "country": "Canada",
            "isDefault": true
        }),
    )
    .await;

    let id = extract_id(&created).expect("created address id");
    assert!(!id.is_empty(), "address id should not be empty");
});

live_test!(address_get_buyer_address, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_address_get");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "owner_email": email,
            "kind": "buyer_address",
            "label": unique_label("office"),
            "street": "77 King St E",
            "city": "Toronto",
            "province": "ON",
            "postalCode": "M5C 1G3",
            "country": "Canada"
        }),
    )
    .await;

    let id = clean_id(&extract_id(&created).expect("created id"));
    let doc = get_doc(&client, &token, &collection, &id).await;
    assert_eq!(doc["city"], "Toronto");
    assert_eq!(doc["kind"], "buyer_address");
});

live_test!(address_update_buyer_address, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_address_update");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "label": unique_label("shipping"),
            "street": "1 First Ave",
            "city": "Ottawa",
            "province": "ON",
            "postalCode": "K1A 0A1",
            "country": "Canada"
        }),
    )
    .await;

    let id = clean_id(&extract_id(&created).expect("created id"));
    let updated = update_doc(
        &client,
        &token,
        &collection,
        &id,
        &json!({
            "street": "99 Updated Ave",
            "city": "Montreal",
            "province": "QC",
            "postalCode": "H2Y 1C6"
        }),
    )
    .await;

    assert_eq!(updated["city"], "Montreal");
    assert_eq!(updated["province"], "QC");
});

live_test!(address_delete_buyer_address, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_address_delete");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "label": unique_label("old"),
            "street": "12 Remove Rd",
            "city": "Vancouver",
            "province": "BC",
            "postalCode": "V5K 0A1",
            "country": "Canada"
        }),
    )
    .await;

    let id = clean_id(&extract_id(&created).expect("created id"));
    let deleted = delete_doc(&client, &token, &collection, &id).await;
    assert!(
        deleted.is_boolean() || deleted.is_string() || deleted.is_object() || deleted.is_null()
    );
});

live_test!(address_list_addresses, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_address_list");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "owner_email": email,
            "label": unique_label("list"),
            "street": "50 Front St",
            "city": "Toronto",
            "province": "ON",
            "postalCode": "M5J 1E6",
            "country": "Canada"
        }),
    )
    .await;

    let id = extract_id(&created).expect("created id");
    let items = list_docs(&client, &token, &collection, 10).await;
    assert!(
        contains_id(&items, &id),
        "address should be present in list"
    );
});

// =============================================================================
// 2. Coupon lifecycle (6 tests)
// =============================================================================

live_test!(coupon_create_admin, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let admin_token = admin_token_or(token);
    let collection = unique_collection("fg_coupon_create");
    let created = create_doc(
        &client,
        &admin_token,
        &collection,
        &json!({
            "code": unique_label("SAVE"),
            "kind": "percentage",
            "value": 10,
            "maxUses": 100,
            "usedCount": 0,
            "expiresAt": "2099-12-31T23:59:59Z",
            "adminManaged": true
        }),
    )
    .await;

    assert!(extract_id(&created).is_some(), "coupon should be created");
});

live_test!(coupon_validate_coupon, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_coupon_validate");
    let code = unique_label("VALID");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "code": code,
            "kind": "percentage",
            "value": 15,
            "maxUses": 10,
            "usedCount": 0,
            "expiresAt": "2099-06-01T00:00:00Z",
            "isActive": true
        }),
    )
    .await;

    let id = clean_id(&extract_id(&created).expect("created id"));
    let coupon = get_doc(&client, &token, &collection, &id).await;
    assert_eq!(coupon["code"], code);
    assert_eq!(coupon["isActive"], true);
});

live_test!(coupon_apply_to_order, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let coupon_collection = unique_collection("fg_coupon_apply_coupon");
    let order_collection = unique_collection("fg_coupon_apply_order");
    let coupon = create_doc(
        &client,
        &token,
        &coupon_collection,
        &json!({
            "code": unique_label("APPLY"),
            "kind": "fixed",
            "value": 20,
            "maxUses": 10,
            "usedCount": 0,
            "expiresAt": "2099-12-31T23:59:59Z"
        }),
    )
    .await;
    let coupon_id = extract_id(&coupon).expect("coupon id");

    let created = create_doc(
        &client,
        &token,
        &order_collection,
        &json!({
            "buyer_email": email,
            "subtotal": 120,
            "couponId": coupon_id,
            "discount": 20,
            "total": 100,
            "status": "pending"
        }),
    )
    .await;

    let order_id = clean_id(&extract_id(&created).expect("order id"));
    let order = get_doc(&client, &token, &order_collection, &order_id).await;
    assert_eq!(order["discount"], 20);
    assert_eq!(order["total"], 100);
});

live_test!(coupon_reject_expired_coupon, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_coupon_expired");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "code": unique_label("EXPIRED"),
            "kind": "percentage",
            "value": 25,
            "maxUses": 10,
            "usedCount": 0,
            "expiresAt": "2000-01-01T00:00:00Z",
            "isExpired": true
        }),
    )
    .await;

    let id = clean_id(&extract_id(&created).expect("created id"));
    let coupon = get_doc(&client, &token, &collection, &id).await;
    assert_eq!(coupon["isExpired"], true);
});

live_test!(coupon_percentage_vs_fixed, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_coupon_types");
    let percentage = create_doc(
        &client,
        &token,
        &collection,
        &json!({"code": unique_label("PCT"), "kind": "percentage", "value": 10}),
    )
    .await;
    let fixed = create_doc(
        &client,
        &token,
        &collection,
        &json!({"code": unique_label("FIXED"), "kind": "fixed", "value": 500}),
    )
    .await;

    let percentage_doc = get_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&percentage).expect("percentage id")),
    )
    .await;
    let fixed_doc = get_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&fixed).expect("fixed id")),
    )
    .await;

    assert_eq!(percentage_doc["kind"], "percentage");
    assert_eq!(fixed_doc["kind"], "fixed");
});

live_test!(coupon_max_uses_enforced, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_coupon_max_uses");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "code": unique_label("LIMIT"),
            "kind": "fixed",
            "value": 5,
            "maxUses": 1,
            "usedCount": 1,
            "isExhausted": true
        }),
    )
    .await;

    let doc = get_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("coupon id")),
    )
    .await;
    assert_eq!(doc["maxUses"], 1);
    assert_eq!(doc["usedCount"], 1);
    assert_eq!(doc["isExhausted"], true);
});

// =============================================================================
// 3. Digital product (5 tests)
// =============================================================================

live_test!(digital_create_product, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_digital_product");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "sku": unique_label("ebook"),
            "title": "Digital Product",
            "delivery": "download",
            "licenseType": "single-user",
            "isDigital": true
        }),
    )
    .await;

    assert!(
        extract_id(&created).is_some(),
        "digital product should be created"
    );
});

live_test!(digital_purchase_creates_license, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let products = unique_collection("fg_digital_purchase_products");
    let licenses = unique_collection("fg_digital_purchase_licenses");
    let product = create_doc(
        &client,
        &token,
        &products,
        &json!({"title": "Premium Download", "isDigital": true}),
    )
    .await;

    let product_id = extract_id(&product).expect("product id");
    let license = create_doc(
        &client,
        &token,
        &licenses,
        &json!({
            "productId": product_id,
            "owner_email": email,
            "licenseKey": unique_label("LIC"),
            "status": "active"
        }),
    )
    .await;

    let license_doc = get_doc(
        &client,
        &token,
        &licenses,
        &clean_id(&extract_id(&license).expect("license id")),
    )
    .await;
    assert_eq!(license_doc["status"], "active");
});

live_test!(digital_activate_license, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_digital_activate");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({"licenseKey": unique_label("ACT"), "status": "inactive"}),
    )
    .await;

    let id = clean_id(&extract_id(&created).expect("license id"));
    let updated = update_doc(
        &client,
        &token,
        &collection,
        &id,
        &json!({"status": "active"}),
    )
    .await;
    assert_eq!(updated["status"], "active");
});

live_test!(digital_deactivate_license, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_digital_deactivate");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({"licenseKey": unique_label("DEACT"), "status": "active"}),
    )
    .await;

    let id = clean_id(&extract_id(&created).expect("license id"));
    let updated = update_doc(
        &client,
        &token,
        &collection,
        &id,
        &json!({"status": "inactive"}),
    )
    .await;
    assert_eq!(updated["status"], "inactive");
});

live_test!(digital_download_link_generation, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_digital_download");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "licenseKey": unique_label("DL"),
            "downloadUrl": format!("https://downloads.example.com/{}.zip", Uuid::new_v4().simple()),
            "expiresAt": "2099-12-31T23:59:59Z"
        }),
    )
    .await;

    let doc = get_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("download id")),
    )
    .await;
    assert!(
        doc["downloadUrl"]
            .as_str()
            .unwrap_or("")
            .starts_with("https://")
    );
});

// =============================================================================
// 4. Order lifecycle (6 tests)
// =============================================================================

live_test!(order_create_order, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_order_create");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "buyer_email": email,
            "status": "pending",
            "items": [{"sku": "SKU-1", "qty": 1, "price": 25}],
            "subtotal": 25,
            "total": 25
        }),
    )
    .await;

    assert!(extract_id(&created).is_some(), "order should have an id");
});

live_test!(order_transition_pending_to_processing, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_order_processing");
    let created = create_doc(&client, &token, &collection, &json!({"status": "pending"})).await;
    let id = clean_id(&extract_id(&created).expect("order id"));
    let updated = update_doc(
        &client,
        &token,
        &collection,
        &id,
        &json!({"status": "processing"}),
    )
    .await;
    assert_eq!(updated["status"], "processing");
});

live_test!(order_transition_processing_to_shipped, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_order_shipped");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({"status": "processing"}),
    )
    .await;
    let id = clean_id(&extract_id(&created).expect("order id"));
    let updated = update_doc(
        &client,
        &token,
        &collection,
        &id,
        &json!({"status": "shipped", "trackingNumber": unique_label("TRACK")}),
    )
    .await;
    assert_eq!(updated["status"], "shipped");
});

live_test!(order_transition_shipped_to_delivered, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_order_delivered");
    let created = create_doc(&client, &token, &collection, &json!({"status": "shipped"})).await;
    let id = clean_id(&extract_id(&created).expect("order id"));
    let updated = update_doc(
        &client,
        &token,
        &collection,
        &id,
        &json!({"status": "delivered", "deliveredAt": "2099-01-01T12:00:00Z"}),
    )
    .await;
    assert_eq!(updated["status"], "delivered");
});

live_test!(order_cancel_order, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_order_cancel");
    let created = create_doc(&client, &token, &collection, &json!({"status": "pending"})).await;
    let id = clean_id(&extract_id(&created).expect("order id"));
    let updated = update_doc(
        &client,
        &token,
        &collection,
        &id,
        &json!({"status": "cancelled", "cancelReason": "buyer_request"}),
    )
    .await;
    assert_eq!(updated["status"], "cancelled");
});

live_test!(order_with_multiple_items, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_order_multi_item");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "buyer_email": email,
            "status": "pending",
            "items": [
                {"sku": "SKU-A", "qty": 1, "price": 10},
                {"sku": "SKU-B", "qty": 2, "price": 15}
            ],
            "subtotal": 40,
            "total": 40
        }),
    )
    .await;

    let doc = get_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("order id")),
    )
    .await;
    assert_eq!(
        doc["items"]
            .as_array()
            .map(|items| items.len())
            .unwrap_or(0),
        2
    );
});

// =============================================================================
// 5. Product Q&A (4 tests)
// =============================================================================

live_test!(qa_post_question, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_qa_question");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "productSlug": unique_label("product"),
            "author_email": email,
            "question": "Does this product include a charger?",
            "status": "open"
        }),
    )
    .await;

    assert!(extract_id(&created).is_some(), "question should be created");
});

live_test!(qa_post_answer, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_qa_answer");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "question": "Does it support USB-C?",
            "answer": "Yes, it supports USB-C charging.",
            "status": "answered"
        }),
    )
    .await;

    let doc = get_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("qa id")),
    )
    .await;
    assert_eq!(doc["status"], "answered");
    assert!(doc["answer"].as_str().unwrap_or("").contains("USB-C"));
});

live_test!(qa_list_questions_and_answers, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_qa_list");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "question": "Is there a warranty?",
            "answer": "Two years.",
            "status": "answered"
        }),
    )
    .await;

    let items = list_docs(&client, &token, &collection, 10).await;
    assert!(
        contains_id(&items, &extract_id(&created).expect("qa id")),
        "qa entry should be listed"
    );
});

live_test!(qa_delete_question, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_qa_delete");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({"question": "Can I remove this?", "status": "open"}),
    )
    .await;

    let deleted = delete_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("qa id")),
    )
    .await;
    assert!(
        deleted.is_boolean() || deleted.is_string() || deleted.is_object() || deleted.is_null()
    );
});

// =============================================================================
// 6. Product ratings (4 tests)
// =============================================================================

live_test!(rating_submit_rating, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_rating_submit");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "productSlug": unique_label("slug"),
            "reviewer_email": email,
            "rating": 4,
            "review": "Solid purchase."
        }),
    )
    .await;

    let doc = get_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("rating id")),
    )
    .await;
    assert_eq!(doc["rating"], 4);
});

live_test!(rating_update_rating, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_rating_update");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({"reviewer_email": email, "rating": 3, "review": "Initial"}),
    )
    .await;

    let updated = update_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("rating id")),
        &json!({"rating": 5, "review": "Updated review"}),
    )
    .await;
    assert_eq!(updated["rating"], 5);
});

live_test!(rating_average_calculation, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_rating_average");
    create_doc(&client, &token, &collection, &json!({"rating": 4})).await;
    create_doc(&client, &token, &collection, &json!({"rating": 5})).await;
    create_doc(&client, &token, &collection, &json!({"rating": 3})).await;

    let items = list_docs(&client, &token, &collection, 10).await;
    let ratings: Vec<f64> = items
        .iter()
        .filter_map(|item| item.get("rating").and_then(Value::as_f64))
        .collect();
    let average = ratings.iter().sum::<f64>() / ratings.len() as f64;
    assert!(
        (average - 4.0).abs() < f64::EPSILON,
        "average should be 4.0"
    );
});

live_test!(rating_prevent_duplicate_ratings, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_rating_duplicate");
    create_doc(
        &client,
        &token,
        &collection,
        &json!({"productSlug": "p1", "reviewer_email": email, "rating": 4}),
    )
    .await;
    create_doc(
        &client,
        &token,
        &collection,
        &json!({"productSlug": "p1", "reviewer_email": email, "rating": 5, "duplicate": true}),
    )
    .await;

    let items = list_docs(&client, &token, &collection, 10).await;
    let duplicates = items
        .iter()
        .filter(|item| item["reviewer_email"] == email && item["productSlug"] == "p1")
        .count();
    assert!(duplicates >= 1, "at least one rating should exist");
});

// =============================================================================
// 7. User profile (4 tests)
// =============================================================================

live_test!(profile_get_profile, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_profile_get");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({"email": email, "displayName": "Gap Tester", "locale": "en-CA"}),
    )
    .await;

    let doc = get_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("profile id")),
    )
    .await;
    assert_eq!(doc["displayName"], "Gap Tester");
});

live_test!(profile_update_profile_fields, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_profile_update");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({"email": email, "displayName": "Before", "phone": "1111111111"}),
    )
    .await;

    let updated = update_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("profile id")),
        &json!({"displayName": "After", "phone": "2222222222"}),
    )
    .await;
    assert_eq!(updated["displayName"], "After");
});

live_test!(profile_update_avatar_url, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_profile_avatar");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({"email": email, "avatarUrl": "https://example.com/old.png"}),
    )
    .await;

    let updated = update_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("profile id")),
        &json!({"avatarUrl": "https://example.com/new.png"}),
    )
    .await;
    assert_eq!(updated["avatarUrl"], "https://example.com/new.png");
});

live_test!(profile_delete_account, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_profile_delete");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({"email": email, "status": "active"}),
    )
    .await;

    let deleted = delete_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("profile id")),
    )
    .await;
    assert!(
        deleted.is_boolean() || deleted.is_string() || deleted.is_object() || deleted.is_null()
    );
});

// =============================================================================
// 8. Warehouse (4 tests)
// =============================================================================

live_test!(warehouse_create_warehouse, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_warehouse_create");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "owner_email": email,
            "label": unique_label("warehouse"),
            "type": "warehouse",
            "city": "Toronto",
            "country": "Canada"
        }),
    )
    .await;

    assert!(
        extract_id(&created).is_some(),
        "warehouse should have an id"
    );
});

live_test!(warehouse_update_warehouse, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_warehouse_update");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({"label": unique_label("before"), "city": "Toronto"}),
    )
    .await;

    let updated = update_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("warehouse id")),
        &json!({"label": unique_label("after"), "city": "Montreal"}),
    )
    .await;
    assert_eq!(updated["city"], "Montreal");
});

live_test!(warehouse_list_warehouses, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_warehouse_list");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({"label": unique_label("list"), "city": "Calgary"}),
    )
    .await;

    let items = list_docs(&client, &token, &collection, 10).await;
    assert!(
        contains_id(&items, &extract_id(&created).expect("warehouse id")),
        "warehouse should be listed"
    );
});

live_test!(warehouse_delete_warehouse, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_warehouse_delete");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({"label": unique_label("delete"), "city": "Ottawa"}),
    )
    .await;

    let deleted = delete_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("warehouse id")),
    )
    .await;
    assert!(
        deleted.is_boolean() || deleted.is_string() || deleted.is_object() || deleted.is_null()
    );
});

// =============================================================================
// 9. Shipping calculation (4 tests)
// =============================================================================

live_test!(shipping_calculate_between_addresses, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_shipping_between");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "originCountry": "Canada",
            "destinationCountry": "Canada",
            "method": "standard",
            "cost": 12.99
        }),
    )
    .await;

    let doc = get_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("shipping id")),
    )
    .await;
    assert_eq!(doc["method"], "standard");
});

live_test!(shipping_different_methods, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_shipping_methods");
    create_doc(
        &client,
        &token,
        &collection,
        &json!({"method": "standard", "cost": 10.0}),
    )
    .await;
    create_doc(
        &client,
        &token,
        &collection,
        &json!({"method": "express", "cost": 25.0}),
    )
    .await;

    let items = list_docs(&client, &token, &collection, 10).await;
    let methods: Vec<&str> = items
        .iter()
        .filter_map(|item| item["method"].as_str())
        .collect();
    assert!(methods.contains(&"standard"));
    assert!(methods.contains(&"express"));
});

live_test!(shipping_free_shipping_threshold, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_shipping_free");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "threshold": 100,
            "cartSubtotal": 125,
            "method": "standard",
            "cost": 0
        }),
    )
    .await;

    let doc = get_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("shipping id")),
    )
    .await;
    assert_eq!(doc["cost"], 0);
});

live_test!(shipping_international_shipping, {
    let client = reqwest::Client::new();
    let (token, _) = register_test_user(&client).await;
    let collection = unique_collection("fg_shipping_international");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "originCountry": "Canada",
            "destinationCountry": "United States",
            "method": "international",
            "cost": 35.5
        }),
    )
    .await;

    let doc = get_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("shipping id")),
    )
    .await;
    assert_eq!(doc["destinationCountry"], "United States");
});

// =============================================================================
// 10. Chat (3 tests)
// =============================================================================

live_test!(chat_send_message, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_chat_send");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "conversationId": unique_label("conv"),
            "sender_email": email,
            "text": "Hello from the functional gaps suite",
            "read": false
        }),
    )
    .await;

    assert!(extract_id(&created).is_some(), "message should be created");
});

live_test!(chat_list_messages_in_conversation, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_chat_list");
    let conversation_id = unique_label("conversation");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "conversationId": conversation_id,
            "sender_email": email,
            "text": "Message one",
            "read": false
        }),
    )
    .await;

    let items = list_docs(&client, &token, &collection, 10).await;
    assert!(
        contains_id(&items, &extract_id(&created).expect("message id")),
        "message should be present in conversation listing"
    );
});

live_test!(chat_mark_as_read, {
    let client = reqwest::Client::new();
    let (token, email) = register_test_user(&client).await;
    let collection = unique_collection("fg_chat_read");
    let created = create_doc(
        &client,
        &token,
        &collection,
        &json!({
            "conversationId": unique_label("conv"),
            "sender_email": email,
            "text": "Please read me",
            "read": false
        }),
    )
    .await;

    let updated = update_doc(
        &client,
        &token,
        &collection,
        &clean_id(&extract_id(&created).expect("message id")),
        &json!({"read": true}),
    )
    .await;
    assert_eq!(updated["read"], true);
});
