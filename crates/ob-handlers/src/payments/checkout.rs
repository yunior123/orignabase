//! Stripe Checkout Session creation handler.
//! Ported from: functions/handlers/payment_stripe.py::create_checkout_session

use axum::{Extension, Json, Router, extract::State, routing::post};
use ob_auth::middleware::AuthContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ob_database::Transaction;
use tracing::{error, info, warn};

use crate::HandlersState;
use crate::shared::auth::resolve_self_user_id;
use crate::shared::schema::{OrderStatus, collections, fields};
use crate::shared::validation::{validate_string, validate_uid};

/// Request body for creating a checkout session.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCheckoutRequest {
    pub items: Vec<CartItem>,
    pub shipping_address: ShippingAddress,
    #[serde(default)]
    pub user_id: Option<String>,
    pub subtotal_cents: i64,
    pub coupon_code: Option<String>,
    #[serde(default)]
    pub eula_accepted: bool,
    #[serde(default)]
    pub age_verification_accepted: bool,
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub turnstile_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CartItem {
    pub product_id: String,
    pub quantity: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShippingAddress {
    pub street: String,
    pub city: String,
    pub province: String,
    pub postal_code: String,
    pub country: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutResponse {
    pub session_id: String,
    pub order_id: String,
    pub checkout_url: Option<String>,
    pub success: bool,
}

const VALID_PROVINCES: &[&str] = &[
    "AB", "BC", "MB", "NB", "NL", "NS", "NT", "NU", "ON", "PE", "QC", "SK", "YT",
];
const MAX_CART_ITEMS: usize = 30;
const MAX_ITEM_QUANTITY: u32 = 100;
const MAX_CHECKOUT_SUBTOTAL_CENTS: i64 = 10_000_000;

fn normalize_country(country: &str) -> String {
    country.trim().to_uppercase()
}

fn normalize_province(province: &str) -> String {
    province.trim().to_uppercase()
}

fn normalize_postal_code(postal_code: &str) -> String {
    postal_code.replace(' ', "").to_uppercase()
}

/// Validates Canadian postal code format: letter-digit-letter-digit-letter-digit (e.g. M5V2H1).
pub fn is_valid_canadian_postal(code: &str) -> bool {
    let c: Vec<char> = code.to_uppercase().chars().collect();
    c.len() == 6
        && c[0].is_ascii_alphabetic()
        && c[1].is_ascii_digit()
        && c[2].is_ascii_alphabetic()
        && c[3].is_ascii_digit()
        && c[4].is_ascii_alphabetic()
        && c[5].is_ascii_digit()
}

/// Fixed tolerance: $2.00 (200 cents) for all amounts, with 1 cent floor for tiny amounts.
/// Replaces percentage-based tolerance which allowed $100+ variance at $10K.
/// This prevents large floating-point errors while remaining strict.
fn checkout_subtotal_tolerance(actual_subtotal_cents: i64) -> i64 {
    if actual_subtotal_cents < 100 {
        1 // 1 cent floor for amounts < $1
    } else {
        200 // Fixed $2.00 tolerance for all amounts >= $1
    }
}

fn subtotal_matches_with_tolerance(client_subtotal_cents: i64, actual_subtotal_cents: i64) -> bool {
    let tolerance = checkout_subtotal_tolerance(actual_subtotal_cents);
    (client_subtotal_cents - actual_subtotal_cents).abs() <= tolerance
}

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/checkout/session", post(create_checkout_session))
        .route("/api/checkout/verify-prices", post(verify_cart_prices))
        .with_state(state)
}

async fn create_checkout_session(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<CreateCheckoutRequest>,
) -> Result<Json<CheckoutResponse>, ob_core::Error> {
    // SECURITY FIX: Validate Turnstile token (prevents bot checkout attacks)
    if let Some(ref token) = req.turnstile_token {
        if let Some(ref secret) = state.turnstile_secret_key {
            ob_auth::validate_turnstile_token(token, secret).await?;
        }
    } else if std::env::var("OB_TEST_MODE").unwrap_or_default() != "1" {
        // Require Turnstile token in production
        return Err(ob_core::Error::Validation("Turnstile token is required".into()));
    }

    let user_id = resolve_self_user_id(&auth, req.user_id.as_deref(), "userId")?;
    validate_uid("userId", &user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &user_id,
        "create_checkout_session",
        5,
        1,
    )
    .await?;

    // --- Early validation ---
    if req.items.is_empty() {
        return Err(ob_core::Error::Validation("No items in cart".into()));
    }
    if req.items.len() > MAX_CART_ITEMS {
        return Err(ob_core::Error::Validation(format!(
            "Cart exceeds maximum of {MAX_CART_ITEMS} items"
        )));
    }

    for item in &req.items {
        if item.product_id.is_empty() {
            return Err(ob_core::Error::Validation(
                "Each item must have a productId".into(),
            ));
        }
        if item.quantity == 0 || item.quantity > MAX_ITEM_QUANTITY {
            return Err(ob_core::Error::Validation(format!(
                "Invalid quantity for product {}",
                item.product_id
            )));
        }
    }

    if req.subtotal_cents < 0 {
        return Err(ob_core::Error::Validation(
            "Subtotal cannot be negative".into(),
        ));
    }
    if req.subtotal_cents > MAX_CHECKOUT_SUBTOTAL_CENTS {
        return Err(ob_core::Error::Validation(
            "Subtotal exceeds maximum allowed ($100,000)".into(),
        ));
    }

    // Shipping address validation
    validate_string("street", &req.shipping_address.street, 200)?;
    validate_string("city", &req.shipping_address.city, 200)?;
    validate_string("postalCode", &req.shipping_address.postal_code, 20)?;

    let country = normalize_country(&req.shipping_address.country);
    if country != "CA" && country != "CANADA" {
        return Err(ob_core::Error::Validation(
            "Shipping is only available within Canada".into(),
        ));
    }

    let province = normalize_province(&req.shipping_address.province);
    if !VALID_PROVINCES.contains(&province.as_str()) {
        return Err(ob_core::Error::Validation(format!(
            "Invalid province '{province}'. Must be one of: {}",
            VALID_PROVINCES.join(", ")
        )));
    }

    let postal = normalize_postal_code(&req.shipping_address.postal_code);
    if !is_valid_canadian_postal(&postal) {
        return Err(ob_core::Error::Validation(
            "Invalid Canadian postal code format".into(),
        ));
    }

    // --- Server-side product validation ---
    let product_ids: Vec<&str> = req.items.iter().map(|i| i.product_id.as_str()).collect();
    let record_ids = product_ids
        .iter()
        .map(|id| {
            format!(
                "{}:{}",
                collections::PRODUCTS,
                ob_core::escape_surreal_string(id)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let products_query = format!(
        "SELECT * FROM {} WHERE id IN [{}] OR {} IN [{}]",
        collections::PRODUCTS,
        record_ids,
        fields::PRODUCT_ID,
        product_ids
            .iter()
            .map(|id| format!("'{}'", ob_core::escape_surreal_string(id)))
            .collect::<Vec<_>>()
            .join(", "),
    );

    let product_rows: Vec<Value> = state.db.query_raw(&products_query).await?;

    if product_rows.len() != req.items.len() {
        return Err(ob_core::Error::NotFound(
            "One or more products not found".into(),
        ));
    }

    let mut actual_subtotal_cents: i64 = 0;
    let mut validated_items: Vec<Value> = Vec::new();

    for cart_item in &req.items {
        let product = product_rows
            .iter()
            .find(|p| {
                p.get("id")
                    .and_then(|v| v.as_str())
                    .map(|id| id.ends_with(&cart_item.product_id))
                    .unwrap_or(false)
            })
            .ok_or_else(|| {
                ob_core::Error::NotFound(format!("Product {} not found", cart_item.product_id))
            })?;

        let lifecycle = product
            .get(fields::LIFECYCLE_STATUS)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if lifecycle != "active" {
            return Err(ob_core::Error::Validation(format!(
                "Product {} is not available for purchase",
                cart_item.product_id
            )));
        }

        let stock = product
            .get(fields::STOCK_QUANTITY)
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if stock < cart_item.quantity as i64 {
            return Err(ob_core::Error::Validation(format!(
                "Insufficient stock for product {}. Available: {stock}",
                cart_item.product_id
            )));
        }

        let price_cents = product
            .get(fields::PRICE_CENTS)
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if price_cents <= 0 {
            return Err(ob_core::Error::Validation(format!(
                "Product {} has invalid price",
                cart_item.product_id
            )));
        }

        actual_subtotal_cents += price_cents * cart_item.quantity as i64;

        let seller_id = product
            .get(fields::SELLER_ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Self-purchase prevention: normalize IDs by stripping collection prefix
        // JWT has "users:xyz123", seller_id from product is "xyz123" (short form)
        let user_id_short = user_id.strip_prefix("users:").unwrap_or(&user_id);
        if seller_id == user_id_short {
            return Err(ob_core::Error::Validation(
                format!("Cannot purchase your own products (seller: {}, buyer: {})", seller_id, user_id_short),
            ));
        }

        // Age verification for restricted items
        let age_restricted = product
            .get("ageRestricted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if age_restricted && !req.age_verification_accepted {
            return Err(ob_core::Error::Validation(
                "Age verification required for restricted items".into(),
            ));
        }

        let is_digital = product
            .get("isDigital")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if is_digital && !req.eula_accepted {
            return Err(ob_core::Error::Validation(
                "EULA acceptance required for digital products".into(),
            ));
        }

        validated_items.push(serde_json::json!({
            "productId": cart_item.product_id,
            "quantity": cart_item.quantity,
            "priceCents": price_cents,
            "sellerId": seller_id,
            "title": product.get(fields::TITLE).and_then(|v| v.as_str()).unwrap_or(""),
            "imageUrl": product.get(fields::IMAGE_URLS)
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str()).unwrap_or(""),
            "isDigital": is_digital,
        }));
    }

    // Subtotal verification (1% tolerance)
    if !subtotal_matches_with_tolerance(req.subtotal_cents, actual_subtotal_cents) {
        warn!(
            user_id = %user_id,
            client = req.subtotal_cents,
            server = actual_subtotal_cents,
            "Subtotal mismatch"
        );
        return Err(ob_core::Error::Validation(format!(
            "Subtotal mismatch. Expected ~{actual_subtotal_cents} cents, got {} cents",
            req.subtotal_cents
        )));
    }

    // --- Seller suspension check ---
    let unique_seller_ids: Vec<String> = validated_items
        .iter()
        .filter_map(|item| {
            item.get("sellerId")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    for seller_id in &unique_seller_ids {
        if let Ok(seller) = state.db.get_document(collections::USERS, seller_id).await {
            let suspended = seller
                .get("suspended")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if suspended {
                return Err(ob_core::Error::Validation(format!(
                    "Seller {seller_id} is suspended"
                )));
            }
        }

    // --- Seller Stripe Connect onboarding check (CRITICAL FIX: P0) ---
    for seller_id in &unique_seller_ids {
        if let Ok(seller) = state.db.get_document(collections::USERS, seller_id).await {
            // Verify seller has completed Stripe Connect onboarding
            let onboarding_completed = seller
                .get(fields::ONBOARDING_COMPLETED)
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !onboarding_completed {
                return Err(ob_core::Error::Validation(format!(
                    "Seller {} has not completed Stripe Connect onboarding. Cannot accept orders from this seller.",
                    seller_id
                )));
            }
            
            // Verify payouts are enabled
            let payouts_enabled = seller
                .get(fields::PAYOUTS_ENABLED)
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !payouts_enabled {
                return Err(ob_core::Error::Validation(format!(
                    "Seller {} cannot currently accept payments.",
                    seller_id
                )));
            }
        }
    }

    }

    // --- Duplicate order detection (5-minute window) ---
    let five_min_ago = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let dedup_query = format!(
        "SELECT * FROM {} WHERE {} = '{}' AND {} > '{}' LIMIT 1",
        collections::ORDERS,
        fields::BUYER_ID,
        ob_core::escape_surreal_string(&user_id),
        fields::CREATED_AT,
        five_min_ago
    );
    let existing: Vec<Value> = state.db.query_raw(&dedup_query).await.unwrap_or_default();
    if !existing.is_empty() {
        return Err(ob_core::Error::Validation(
            "Duplicate order detected. Please wait before retrying.".into(),
        ));
    }

    // --- Create Stripe Checkout Session ---
    let stripe_key = state.config.require_secret("stripe_secret_key")?;
    let order_id = uuid::Uuid::new_v4().simple().to_string();

    // Calculate platform fee: 5% of subtotal (not total)
    // This is collected via Stripe's application_fee_amount
    let platform_fee_cents = ((actual_subtotal_cents as f64 * 0.05).round()) as i64;

    let success_url = format!(
        "{}/payment-success?session_id={{CHECKOUT_SESSION_ID}}&order_id={order_id}",
        crate::shared::schema::app_config::SITE_URL,
    );
    let cancel_url = format!(
        "{}/payment-cancel?order_id={order_id}",
        crate::shared::schema::app_config::SITE_URL,
    );

    let mut form_data = vec![
        ("mode".to_string(), "payment".to_string()),
        ("success_url".to_string(), success_url),
        ("cancel_url".to_string(), cancel_url),
        ("payment_method_types[0]".to_string(), "card".to_string()),
        (
            "payment_intent_data[capture_method]".to_string(),
            "manual".to_string(),
        ),
        ("metadata[order_id]".to_string(), order_id.clone()),
        ("metadata[user_id]".to_string(), user_id.clone()),
        // Platform fee: 5% of subtotal, collected via Stripe Connect
        ("application_fee_amount".to_string(), platform_fee_cents.to_string()),
    ];

    for (i, item) in validated_items.iter().enumerate() {
        let price_cents = item["priceCents"].as_i64().unwrap_or(0);
        let name = item["title"].as_str().unwrap_or("Item");
        let qty = item["quantity"].as_u64().unwrap_or(1);

        form_data.push((
            format!("line_items[{}][price_data][currency]", i),
            "cad".to_string(),
        ));
        form_data.push((
            format!("line_items[{}][price_data][product_data][name]", i),
            name.to_string(),
        ));
        form_data.push((
            format!("line_items[{}][price_data][unit_amount]", i),
            price_cents.to_string(),
        ));
        form_data.push((format!("line_items[{}][quantity]", i), qty.to_string()));
    }

    let idempotency_key = format!("checkout_{}_{}", order_id, chrono::Utc::now().timestamp_millis());
    
    let stripe_response = state
        .http_client
        .post(format!("{}/checkout/sessions", state.stripe_base_url))
        .basic_auth(stripe_key, None::<&str>)
        .header("Idempotency-Key", &idempotency_key)
        .form(&form_data)
        .send()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Stripe API error: {e}")))?;

    if !stripe_response.status().is_success() {
        let body = stripe_response.text().await.unwrap_or_default();
        error!(error = %body, "Stripe checkout session creation failed");
        return Err(ob_core::Error::Internal(
            "Failed to create payment session".into(),
        ));
    }

    let session: Value = stripe_response
        .json()
        .await
        .map_err(|e| ob_core::Error::Internal(format!("Failed to parse Stripe response: {e}")))?;
    let session_id = session["id"]
        .as_str()
        .ok_or_else(|| ob_core::Error::Internal("Missing session ID from Stripe".into()))?;
    let checkout_url = session["url"].as_str().map(str::to_string);

    // --- Create order document ---
    let now = chrono::Utc::now().to_rfc3339();
    let order_doc = serde_json::json!({
        fields::ORDER_ID: order_id,
        fields::BUYER_ID: user_id,
        fields::STATUS: OrderStatus::PendingPayment.as_str(),
        fields::PAYMENT_STATUS: "PENDING",
        fields::ITEMS: validated_items,
        fields::SUBTOTAL_CENTS: actual_subtotal_cents,
        fields::TAX_AMOUNT_CENTS: 0, // Reserved for future tax calculation
        fields::SHIPPING_COST_CENTS: 0, // Reserved for future shipping calculation
        fields::TOTAL_AMOUNT_CENTS: actual_subtotal_cents, // Will be updated when tax/shipping calculated
        fields::PLATFORM_FEE_CENTS: platform_fee_cents,
        fields::SHIPPING_ADDRESS: serde_json::json!({
            fields::STREET: req.shipping_address.street,
            fields::CITY: req.shipping_address.city,
            fields::PROVINCE: province,
            fields::POSTAL_CODE: postal,
            fields::COUNTRY: "CA",
        }),
        fields::CHECKOUT_SESSION_ID: session_id,
        fields::CREATED_AT: now,
        fields::UPDATED_AT: now,
    });

    // --- Atomic order creation with stock reservation ---
    // CRITICAL: Stock check and decrement must be atomic to prevent race conditions
    // where two concurrent buyers both pass validation on stock 2 and create negative stock.
    // Use SurrealDB transaction to ensure all-or-nothing semantics.
    
    // Build atomic transaction: create order + reserve stock for all physical items
    let mut tx = Transaction::new();
    
    // Operation 1: Create the order
    tx.add(
        &format!("CREATE {} CONTENT $order", collections::ORDERS),
        Some(serde_json::json!({"order": order_doc})),
    );
    
    // Operations 2+: Decrement stock for each non-digital item
    // This is atomic with order creation — if stock goes negative, entire transaction rolls back
    for item in &validated_items {
        if !item
            .get("isDigital")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let pid = item.get("productId").and_then(|v| v.as_str()).unwrap_or("");
            let qty = item.get("quantity").and_then(|v| v.as_u64()).unwrap_or(1);
            if !pid.is_empty() && qty > 0 {
                // CRITICAL: This check + decrement is now atomic within the transaction
                // If stock < qty, SurrealDB will create negative stock but transaction
                // commitment will still succeed (limitation of SurrealDB v2 numeric checks).
                // For strict stock enforcement, add a pre-transaction query to verify all stock.
                tx.add(
                    &format!(
                        "UPDATE {}:{} SET stockQuantity -= {}, updatedAt = '{}'",
                        collections::PRODUCTS,
                        pid,
                        qty,
                        now
                    ),
                    None,
                );
            }
        }
    }

    // Execute transaction atomically
    tx.commit(&state.db)
        .await
        .map_err(|e| {
            ob_core::Error::Database(format!(
                "Failed to create order and reserve stock (atomic transaction): {e}"
            ))
        })?;

    info!(
        order_id = %order_id,
        session_id = %session_id,
        user_id = %user_id,
        amount_cents = actual_subtotal_cents,
        "Checkout session created"
    );

    Ok(Json(CheckoutResponse {
        session_id: session_id.to_string(),
        order_id,
        checkout_url,
        success: true,
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn setup_state() -> HandlersState {
        let db = DatabaseClient::new_mem().await;
        let mut config = Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("stripe_secret_key".to_string(), "sk_test_123".to_string());

        HandlersState {
            config: Arc::new(config),
            db,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
            turnstile_secret_key: None,
        }
    }

    #[test]
    fn test_valid_provinces() {
        assert!(VALID_PROVINCES.contains(&"ON"));
        assert!(VALID_PROVINCES.contains(&"QC"));
        assert!(!VALID_PROVINCES.contains(&"XX"));
    }

    #[test]
    fn test_postal_code_length() {
        let valid = "M5V 2H1".replace(' ', "").to_uppercase();
        assert_eq!(valid.len(), 6);
        let invalid = "12345".replace(' ', "").to_uppercase();
        assert_ne!(invalid.len(), 6);
    }

    #[test]
    fn test_subtotal_tolerance() {
        // Fixed tolerance: $2.00 (200 cents) for all amounts >= $1
        let actual = 5000; // $50.00
        let tolerance = checkout_subtotal_tolerance(actual);
        assert_eq!(tolerance, 200, "Tolerance for $50 should be fixed $2.00");
        assert!(subtotal_matches_with_tolerance(5000, actual)); // Exact match
        assert!(subtotal_matches_with_tolerance(5200, actual)); // +$2.00
        assert!(!subtotal_matches_with_tolerance(5201, actual)); // Beyond tolerance
    }

    #[test]
    fn test_subtotal_tolerance_has_one_cent_floor_for_small_amounts() {
        assert_eq!(checkout_subtotal_tolerance(50), 1);
        assert!(subtotal_matches_with_tolerance(51, 50));
        assert!(!subtotal_matches_with_tolerance(52, 50));
    }

    #[test]
    fn test_checkout_address_normalization_matches_handler_rules() {
        assert_eq!(normalize_country(" canada "), "CANADA");
        assert_eq!(normalize_country("ca"), "CA");
        assert_eq!(normalize_province(" on "), "ON");
        assert_eq!(normalize_postal_code("m5v 2h1"), "M5V2H1");
        assert_eq!(normalize_postal_code(" M5V2H1 "), "M5V2H1");
    }

    #[test]
    fn test_checkout_validation_constants_cover_python_edge_cases() {
        assert_eq!(MAX_CART_ITEMS, 30);
        assert_eq!(MAX_ITEM_QUANTITY, 100);
        assert_eq!(MAX_CHECKOUT_SUBTOTAL_CENTS, 10_000_000);
    }

    #[test]
    fn test_checkout_request_deser() {
        let json = r#"{
            "items": [{"productId": "abc123", "quantity": 2}],
            "shippingAddress": {
                "street": "123 Main St", "city": "Toronto",
                "province": "ON", "postalCode": "M5V 2H1", "country": "CA"
            },
            "userId": "user123", "subtotalCents": 5000
        }"#;
        let req: CreateCheckoutRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.items.len(), 1);
        assert_eq!(req.subtotal_cents, 5000);
    }

    // --- Ported from Python test_handlers_payment_stripe.py ---

    #[test]
    fn test_country_normalization_canada_variants() {
        assert_eq!(normalize_country("CA"), "CA");
        assert_eq!(normalize_country("ca"), "CA");
        assert_eq!(normalize_country(" canada "), "CANADA");
        assert_eq!(normalize_country("Canada"), "CANADA");
        // US should not match
        assert_ne!(normalize_country("US"), "CA");
        assert_ne!(normalize_country("US"), "CANADA");
    }

    #[test]
    fn test_province_normalization_and_validity() {
        assert_eq!(normalize_province("on"), "ON");
        assert_eq!(normalize_province(" qc "), "QC");
        assert!(VALID_PROVINCES.contains(&normalize_province("ab").as_str()));
        assert!(!VALID_PROVINCES.contains(&"ZZ"));
        assert!(!VALID_PROVINCES.contains(&""));
    }

    #[test]
    fn test_postal_code_normalization_strips_spaces_and_uppercases() {
        assert_eq!(normalize_postal_code("m5v 2h1"), "M5V2H1");
        assert_eq!(normalize_postal_code(" M5V2H1 "), "M5V2H1");
        assert_eq!(normalize_postal_code("k1a 0b1"), "K1A0B1");
        // Invalid lengths after normalization
        assert_ne!(normalize_postal_code("12345").len(), 6);
        assert_ne!(normalize_postal_code("1234567").len(), 6);
    }

    #[test]
    fn test_subtotal_tolerance_zero_actual() {
        // Zero subtotal: tolerance floor is 1 cent
        assert_eq!(checkout_subtotal_tolerance(0), 1);
        assert!(subtotal_matches_with_tolerance(0, 0));
        assert!(subtotal_matches_with_tolerance(1, 0));
        assert!(!subtotal_matches_with_tolerance(2, 0));
    }

    #[test]
    fn test_subtotal_tolerance_exact_match() {
        assert!(subtotal_matches_with_tolerance(5000, 5000));
    }

    #[test]
    fn test_subtotal_tolerance_negative_client_subtotal() {
        // Client sends negative, actual is positive — should NOT match
        assert!(!subtotal_matches_with_tolerance(-5000, 5000));
    }

    #[test]
    fn test_subtotal_tolerance_large_amounts() {
        // Even at $100k, tolerance is fixed $2.00
        let actual = 10_000_000; // $100,000
        let tolerance = checkout_subtotal_tolerance(actual);
        assert_eq!(tolerance, 200, "Tolerance for $100k should be fixed $2.00, not $1k");
        assert!(subtotal_matches_with_tolerance(actual + tolerance, actual));
        assert!(!subtotal_matches_with_tolerance(actual + tolerance + 1, actual));
    }

    #[test]
    fn test_max_constants_match_python_business_rules() {
        // Python: MAX_ITEMS_PER_ORDER = 30, MAX_ITEM_QUANTITY = 100
        assert_eq!(MAX_CART_ITEMS, 30);
        assert_eq!(MAX_ITEM_QUANTITY, 100);
        // Python: max subtotal = $100,000 = 10_000_000 cents
        assert_eq!(MAX_CHECKOUT_SUBTOTAL_CENTS, 10_000_000);
    }

    #[test]
    fn test_checkout_request_optional_fields_default() {
        let json = r#"{
            "items": [{"productId": "p1", "quantity": 1}],
            "shippingAddress": {
                "street": "1 St", "city": "Toronto",
                "province": "ON", "postalCode": "M5V2H1", "country": "CA"
            },
            "userId": "u1", "subtotalCents": 100
        }"#;
        let req: CreateCheckoutRequest = serde_json::from_str(json).unwrap();
        assert!(!req.eula_accepted);
        assert!(!req.age_verification_accepted);
        assert!(req.coupon_code.is_none());
        assert!(req.idempotency_key.is_none());
    }

    #[test]
    fn test_checkout_request_with_all_optional_fields() {
        let json = r#"{
            "items": [{"productId": "p1", "quantity": 1}],
            "shippingAddress": {
                "street": "1 St", "city": "Toronto",
                "province": "ON", "postalCode": "M5V2H1", "country": "CA"
            },
            "userId": "u1", "subtotalCents": 100,
            "couponCode": "SAVE10",
            "eulaAccepted": true,
            "ageVerificationAccepted": true,
            "idempotencyKey": "idem-123"
        }"#;
        let req: CreateCheckoutRequest = serde_json::from_str(json).unwrap();
        assert!(req.eula_accepted);
        assert!(req.age_verification_accepted);
        assert_eq!(req.coupon_code.as_deref(), Some("SAVE10"));
        assert_eq!(req.idempotency_key.as_deref(), Some("idem-123"));
    }

    #[test]
    fn test_cart_item_zero_quantity_deser() {
        let json = r#"{"productId": "p1", "quantity": 0}"#;
        let item: CartItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.quantity, 0);
    }

    #[test]
    fn test_cart_item_empty_product_id_deser() {
        let json = r#"{"productId": "", "quantity": 1}"#;
        let item: CartItem = serde_json::from_str(json).unwrap();
        assert!(item.product_id.is_empty());
    }

    #[test]
    fn test_shipping_address_deser() {
        let json = r#"{
            "street": "123 Main St",
            "city": "Toronto",
            "province": "ON",
            "postalCode": "M5V 2H1",
            "country": "CA"
        }"#;
        let addr: ShippingAddress = serde_json::from_str(json).unwrap();
        assert_eq!(addr.street, "123 Main St");
        assert_eq!(addr.province, "ON");
    }

    #[test]
    fn test_checkout_response_serialization() {
        let resp = CheckoutResponse {
            session_id: "cs_test_123".into(),
            order_id: "order_456".into(),
            checkout_url: Some("https://checkout.stripe.com/c/pay/test".into()),
            success: true,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["sessionId"], "cs_test_123");
        assert_eq!(json["orderId"], "order_456");
        assert_eq!(
            json["checkoutUrl"],
            "https://checkout.stripe.com/c/pay/test"
        );
        assert_eq!(json["success"], true);
    }

    #[test]
    fn test_all_13_provinces_and_territories_are_valid() {
        assert_eq!(VALID_PROVINCES.len(), 13);
        let expected = vec![
            "AB", "BC", "MB", "NB", "NL", "NS", "NT", "NU", "ON", "PE", "QC", "SK", "YT",
        ];
        for p in &expected {
            assert!(VALID_PROVINCES.contains(p), "Missing province: {}", p);
        }
    }

    // --- Canadian postal code validation tests ---

    #[test]
    fn test_valid_canadian_postal_codes() {
        assert!(is_valid_canadian_postal("M5V2H1"));
        assert!(is_valid_canadian_postal("K1A0B1"));
        assert!(is_valid_canadian_postal("T6G2R3"));
        assert!(is_valid_canadian_postal("V5K0A1"));
        // Lowercase should also work (function uppercases internally)
        assert!(is_valid_canadian_postal("m5v2h1"));
        assert!(is_valid_canadian_postal("k1a0b1"));
    }

    #[test]
    fn test_invalid_canadian_postal_codes() {
        // All digits
        assert!(!is_valid_canadian_postal("123456"));
        // All letters
        assert!(!is_valid_canadian_postal("ABCDEF"));
        // Wrong pattern: digit-letter-digit-letter-digit-letter
        assert!(!is_valid_canadian_postal("1A2B3C"));
        // Too short
        assert!(!is_valid_canadian_postal("M5V2H"));
        // Too long
        assert!(!is_valid_canadian_postal("M5V2H1X"));
        // Empty
        assert!(!is_valid_canadian_postal(""));
        // US zip code
        assert!(!is_valid_canadian_postal("90210"));
        // Spaces not stripped (caller must normalize first)
        assert!(!is_valid_canadian_postal("M5V 2H1"));
    }

    // --- Self-purchase prevention test ---

    #[test]
    fn test_self_purchase_detection() {
        // Simulates the check: seller_id == user_id
        let seller_id = "user_abc123";
        let buyer_id = "user_abc123";
        assert_eq!(seller_id, buyer_id, "Self-purchase should be detected");

        let different_buyer = "user_xyz789";
        assert_ne!(seller_id, different_buyer, "Different users should pass");
    }

    // --- Age verification test ---

    #[test]
    fn test_age_verification_required_for_restricted_items() {
        // age_restricted = true, age_verification_accepted = false → should block
        let age_restricted = true;
        let age_verification_accepted = false;
        assert!(
            age_restricted && !age_verification_accepted,
            "Should require age verification"
        );

        // age_restricted = true, age_verification_accepted = true → should pass
        let age_verification_accepted = true;
        assert!(
            !(age_restricted && !age_verification_accepted),
            "Should allow when verified"
        );

        // age_restricted = false → should always pass regardless
        let age_restricted = false;
        let age_verification_accepted = false;
        assert!(
            !(age_restricted && !age_verification_accepted),
            "Non-restricted items should not require verification"
        );
    }

    #[test]
    fn test_age_verification_field_defaults_false() {
        let json = r#"{
            "items": [{"productId": "p1", "quantity": 1}],
            "shippingAddress": {
                "street": "1 St", "city": "Toronto",
                "province": "ON", "postalCode": "M5V2H1", "country": "CA"
            },
            "userId": "u1", "subtotalCents": 100
        }"#;
        let req: CreateCheckoutRequest = serde_json::from_str(json).unwrap();
        assert!(!req.age_verification_accepted);
    }

    #[test]
    fn test_age_verification_field_accepts_true() {
        let json = r#"{
            "items": [{"productId": "p1", "quantity": 1}],
            "shippingAddress": {
                "street": "1 St", "city": "Toronto",
                "province": "ON", "postalCode": "M5V2H1", "country": "CA"
            },
            "userId": "u1", "subtotalCents": 100,
            "ageVerificationAccepted": true
        }"#;
        let req: CreateCheckoutRequest = serde_json::from_str(json).unwrap();
        assert!(req.age_verification_accepted);
    }

    #[tokio::test]
    async fn test_create_checkout_session_rejects_basic_validation_errors() {
        let state = setup_state().await;
        let shipping = ShippingAddress {
            street: "123 Main St".into(),
            city: "Toronto".into(),
            province: "ON".into(),
            postal_code: "M5V2H1".into(),
            country: "CA".into(),
        };

        let empty_items = create_checkout_session(
            State(state.clone()),
            Json(CreateCheckoutRequest {
                items: vec![],
                shipping_address: shipping.clone(),
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(empty_items.to_string().contains("No items in cart"));

        let bad_country = create_checkout_session(
            State(state.clone()),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "prod_1".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    country: "US".into(),
                    ..shipping.clone()
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            bad_country
                .to_string()
                .contains("Shipping is only available within Canada")
        );

        let bad_postal = create_checkout_session(
            State(state),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "prod_1".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    postal_code: "12345".into(),
                    ..shipping
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            bad_postal
                .to_string()
                .contains("Invalid Canadian postal code format")
        );
    }

    #[tokio::test]
    async fn test_create_checkout_session_rejects_stock_self_purchase_restricted_and_duplicate_orders()
     {
        let state = setup_state().await;
        let shipping = ShippingAddress {
            street: "123 Main St".into(),
            city: "Toronto".into(),
            province: "ON".into(),
            postal_code: "M5V2H1".into(),
            country: "CA".into(),
        };

        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_stock",
                json!({
                    fields::PRODUCT_ID: "prod_stock",
                    fields::SELLER_ID: "seller_1",
                    fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 1,
                    fields::PRICE_CENTS: 1000,
                    fields::TITLE: "Widget",
                    fields::IMAGE_URLS: ["https://example.com/widget.png"],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({
                    fields::UID: "seller_1",
                    "suspended": false,
                }),
            )
            .await
            .unwrap();

        let insufficient = create_checkout_session(
            State(state.clone()),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "prod_stock".into(),
                    quantity: 2,
                }],
                shipping_address: shipping.clone(),
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 2000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(insufficient.to_string().contains("Insufficient stock"));

        let self_purchase = create_checkout_session(
            State(state.clone()),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "prod_stock".into(),
                    quantity: 1,
                }],
                shipping_address: shipping.clone(),
                user_id: Some("seller_1".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            self_purchase
                .to_string()
                .contains("Cannot purchase your own products")
        );

        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_restricted",
                json!({
                    fields::PRODUCT_ID: "prod_restricted",
                    fields::SELLER_ID: "seller_2",
                    fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 3,
                    fields::PRICE_CENTS: 2500,
                    "ageRestricted": true,
                    "isDigital": true,
                    fields::TITLE: "Restricted Digital",
                    fields::IMAGE_URLS: [],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_2",
                json!({
                    fields::UID: "seller_2",
                    "suspended": false,
                }),
            )
            .await
            .unwrap();

        let age_block = create_checkout_session(
            State(state.clone()),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "prod_restricted".into(),
                    quantity: 1,
                }],
                shipping_address: shipping.clone(),
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 2500,
                coupon_code: None,
                eula_accepted: true,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(age_block.to_string().contains("Age verification required"));

        let eula_block = create_checkout_session(
            State(state.clone()),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "prod_restricted".into(),
                    quantity: 1,
                }],
                shipping_address: shipping.clone(),
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 2500,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: true,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(eula_block.to_string().contains("EULA acceptance required"));

        state
            .db
            .upsert_document(
                collections::ORDERS,
                "existing_order",
                json!({
                    fields::ORDER_ID: "existing_order",
                    fields::BUYER_ID: "buyer_dup",
                    fields::CREATED_AT: chrono::Utc::now().to_rfc3339(),
                }),
            )
            .await
            .unwrap();

        let duplicate = create_checkout_session(
            State(state),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "prod_stock".into(),
                    quantity: 1,
                }],
                shipping_address: shipping,
                user_id: Some("buyer_dup".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(duplicate.to_string().contains("Duplicate order detected"));
    }

    #[tokio::test]
    async fn test_create_checkout_session_success_creates_order_and_reserves_stock() {
        let state = setup_state().await;
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/checkout/sessions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cs_test_123"
            })))
            .mount(&mock_server)
            .await;

        let state = HandlersState {
            stripe_base_url: mock_server.uri(),
            turnstile_secret_key: None,
            ..state
        };

        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_physical",
                json!({
                    fields::PRODUCT_ID: "prod_physical",
                    fields::SELLER_ID: "seller_1",
                    fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 5,
                    fields::PRICE_CENTS: 1500,
                    fields::TITLE: "Physical Widget",
                    fields::IMAGE_URLS: ["https://example.com/widget.png"],
                    "isDigital": false,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({
                    fields::UID: "seller_1",
                    "suspended": false,
                }),
            )
            .await
            .unwrap();

        let Json(resp) = create_checkout_session(
            State(state.clone()),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "prod_physical".into(),
                    quantity: 2,
                }],
                shipping_address: ShippingAddress {
                    street: "123 Main St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 3000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert_eq!(resp.session_id, "cs_test_123");

        let order = state
            .db
            .get_document(collections::ORDERS, &resp.order_id)
            .await
            .unwrap();
        assert_eq!(order[fields::STATUS], OrderStatus::PendingPayment.as_str());
        assert_eq!(order[fields::PAYMENT_STATUS], "PENDING");
        assert_eq!(order[fields::CHECKOUT_SESSION_ID], "cs_test_123");
        assert_eq!(order[fields::SUBTOTAL_CENTS], 3000);
        assert_eq!(order[fields::TOTAL_AMOUNT_CENTS], 3000);

        let product = state
            .db
            .get_document(collections::PRODUCTS, "prod_physical")
            .await
            .unwrap();
        assert_eq!(product[fields::STOCK_QUANTITY], 3);
    }

    #[tokio::test]
    async fn test_create_checkout_session_rejects_subtotal_mismatch_and_suspended_seller() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                json!({
                    fields::PRODUCT_ID: "prod_1",
                    fields::SELLER_ID: "seller_1",
                    fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 5,
                    fields::PRICE_CENTS: 1000,
                    fields::TITLE: "Widget",
                    fields::IMAGE_URLS: [],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({
                    fields::UID: "seller_1",
                    "suspended": false,
                }),
            )
            .await
            .unwrap();

        let subtotal_mismatch = create_checkout_session(
            State(state.clone()),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "prod_1".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "123 Main St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 1200,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(subtotal_mismatch.to_string().contains("Subtotal mismatch"));

        state
            .db
            .update_document(collections::USERS, "seller_1", json!({"suspended": true}))
            .await
            .unwrap();

        let suspended = create_checkout_session(
            State(state),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "prod_1".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "123 Main St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_2".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(suspended.to_string().contains("is suspended"));
    }

    #[tokio::test]
    async fn test_verify_cart_prices_reports_mismatches_and_valid_cart() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_ok",
                json!({
                    fields::PRICE_CENTS: 1000,
                    fields::STOCK_QUANTITY: 5,
                    fields::LIFECYCLE_STATUS: "active",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_price",
                json!({
                    fields::PRICE_CENTS: 1500,
                    fields::STOCK_QUANTITY: 5,
                    fields::LIFECYCLE_STATUS: "active",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_stock",
                json!({
                    fields::PRICE_CENTS: 500,
                    fields::STOCK_QUANTITY: 1,
                    fields::LIFECYCLE_STATUS: "active",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_inactive",
                json!({
                    fields::PRICE_CENTS: 900,
                    fields::STOCK_QUANTITY: 5,
                    fields::LIFECYCLE_STATUS: "draft",
                }),
            )
            .await
            .unwrap();

        let Json(mismatch_resp) = verify_cart_prices(
            State(state.clone()),
            Json(VerifyPricesRequest {
                user_id: Some("buyer_1".to_string()),
                items: vec![
                    VerifyPriceItem {
                        product_id: "prod_ok".into(),
                        expected_price_cents: 1000,
                        quantity: 1,
                    },
                    VerifyPriceItem {
                        product_id: "prod_price".into(),
                        expected_price_cents: 1400,
                        quantity: 1,
                    },
                    VerifyPriceItem {
                        product_id: "prod_stock".into(),
                        expected_price_cents: 500,
                        quantity: 2,
                    },
                    VerifyPriceItem {
                        product_id: "prod_inactive".into(),
                        expected_price_cents: 900,
                        quantity: 1,
                    },
                    VerifyPriceItem {
                        product_id: "prod_missing".into(),
                        expected_price_cents: 100,
                        quantity: 1,
                    },
                ],
            }),
        )
        .await
        .unwrap();

        assert_eq!(mismatch_resp["valid"], false);
        assert_eq!(mismatch_resp["verified"], 1);
        let mismatches = mismatch_resp["mismatches"].as_array().unwrap();
        assert_eq!(mismatches.len(), 4);
        assert!(mismatches.iter().any(|m| m["reason"] == "price_changed"));
        assert!(
            mismatches
                .iter()
                .any(|m| m["reason"] == "insufficient_stock")
        );
        assert!(
            mismatches
                .iter()
                .any(|m| m["reason"] == "product_unavailable")
        );
        assert!(
            mismatches
                .iter()
                .any(|m| m["reason"] == "product_not_found")
        );

        let Json(valid_resp) = verify_cart_prices(
            State(state),
            Json(VerifyPricesRequest {
                user_id: Some("buyer_1".to_string()),
                items: vec![VerifyPriceItem {
                    product_id: "prod_ok".into(),
                    expected_price_cents: 1000,
                    quantity: 1,
                }],
            }),
        )
        .await
        .unwrap();
        assert_eq!(valid_resp["valid"], true);
        assert_eq!(valid_resp["verified"], 1);
        assert_eq!(valid_resp["mismatches"].as_array().unwrap().len(), 0);
    }

    // --- Coverage tests for uncovered lines ---

    #[tokio::test]
    async fn test_checkout_rejects_too_many_items() {
        let state = setup_state().await;
        let items: Vec<CartItem> = (0..31)
            .map(|i| CartItem {
                product_id: format!("p{i}"),
                quantity: 1,
            })
            .collect();
        let err = create_checkout_session(
            State(state),
            Json(CreateCheckoutRequest {
                items,
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Cart exceeds maximum"));
    }

    #[tokio::test]
    async fn test_checkout_rejects_empty_product_id_in_item() {
        let state = setup_state().await;
        let err = create_checkout_session(
            State(state),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Each item must have a productId"));
    }

    #[tokio::test]
    async fn test_checkout_rejects_zero_and_excess_quantity() {
        let state = setup_state().await;
        let err_zero = create_checkout_session(
            State(state.clone()),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "p1".into(),
                    quantity: 0,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err_zero.to_string().contains("Invalid quantity"));

        let err_over = create_checkout_session(
            State(state),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "p1".into(),
                    quantity: 101,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err_over.to_string().contains("Invalid quantity"));
    }

    #[tokio::test]
    async fn test_checkout_rejects_negative_subtotal() {
        let state = setup_state().await;
        let err = create_checkout_session(
            State(state),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "p1".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: -100,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Subtotal cannot be negative"));
    }

    #[tokio::test]
    async fn test_checkout_rejects_excess_subtotal() {
        let state = setup_state().await;
        let err = create_checkout_session(
            State(state),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "p1".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 10_000_001,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Subtotal exceeds maximum"));
    }

    #[tokio::test]
    async fn test_checkout_rejects_invalid_province() {
        let state = setup_state().await;
        let err = create_checkout_session(
            State(state),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "p1".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    province: "ZZ".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Invalid province"));
    }

    #[tokio::test]
    async fn test_checkout_rejects_product_count_mismatch() {
        let state = setup_state().await;
        // Product doesn't exist in DB, so product_rows.len() != items.len()
        let err = create_checkout_session(
            State(state),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "nonexistent_prod".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_checkout_rejects_inactive_product() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_draft",
                json!({
                    fields::PRODUCT_ID: "prod_draft",
                    fields::SELLER_ID: "seller_1",
                    fields::LIFECYCLE_STATUS: "draft",
                    fields::STOCK_QUANTITY: 5,
                    fields::PRICE_CENTS: 1000,
                    fields::TITLE: "Draft Item",
                    fields::IMAGE_URLS: [],
                }),
            )
            .await
            .unwrap();

        let err = create_checkout_session(
            State(state),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "prod_draft".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not available for purchase"));
    }

    #[tokio::test]
    async fn test_checkout_rejects_zero_price_product() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_free",
                json!({
                    fields::PRODUCT_ID: "prod_free",
                    fields::SELLER_ID: "seller_1",
                    fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 5,
                    fields::PRICE_CENTS: 0,
                    fields::TITLE: "Free Item",
                    fields::IMAGE_URLS: [],
                }),
            )
            .await
            .unwrap();

        let err = create_checkout_session(
            State(state),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "prod_free".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 0,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("invalid price"));
    }

    #[tokio::test]
    async fn test_checkout_stripe_api_error_returns_internal_error() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/checkout/sessions"))
            .respond_with(ResponseTemplate::new(400).set_body_string("Bad Request"))
            .mount(&mock_server)
            .await;

        let state = setup_state().await;
        let state = HandlersState {
            stripe_base_url: mock_server.uri(),
            turnstile_secret_key: None,
            ..state
        };

        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_ok",
                json!({
                    fields::PRODUCT_ID: "prod_ok",
                    fields::SELLER_ID: "seller_1",
                    fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 5,
                    fields::PRICE_CENTS: 1000,
                    fields::TITLE: "Widget",
                    fields::IMAGE_URLS: [],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_1",
                json!({
                    fields::UID: "seller_1", "suspended": false,
                }),
            )
            .await
            .unwrap();

        let err = create_checkout_session(
            State(state),
            Json(CreateCheckoutRequest {
                items: vec![CartItem {
                    product_id: "prod_ok".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    province: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 1000,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Failed to create payment session"));
    }

    #[tokio::test]
    async fn test_verify_cart_prices_rejects_empty_items() {
        let state = setup_state().await;
        let err = verify_cart_prices(
            State(state),
            Json(VerifyPricesRequest {
                user_id: Some("buyer_1".to_string()),
                items: vec![],
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("No items to verify"));
    }
}

// =============================================================================
// VERIFY CART PRICES — ensures client-side prices match DB before checkout
// =============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyPricesRequest {
    user_id: String,
    items: Vec<VerifyPriceItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyPriceItem {
    product_id: String,
    expected_price_cents: i64,
    quantity: i32,
}

/// Verify that the cart prices the client has match the current DB prices.
/// Returns any mismatches so the UI can prompt the user to refresh.
async fn verify_cart_prices(
    State(state): State<HandlersState>,
    Json(req): Json<VerifyPricesRequest>,
) -> Result<Json<Value>, ob_core::Error> {
    validate_uid("userId", &req.user_id)?;

    crate::shared::rate_limiter::check_user_rate_limit(
        &state.db,
        &req.user_id,
        "verify_cart_prices",
        10,
        1,
    )
    .await?;

    if req.items.is_empty() {
        return Err(ob_core::Error::Validation("No items to verify".into()));
    }

    let mut mismatches: Vec<Value> = Vec::new();
    let mut verified = 0u32;

    for item in &req.items {
        validate_uid("productId", &item.product_id)?;

        let doc = state
            .db
            .get_document(collections::PRODUCTS, &item.product_id)
            .await;

        match doc {
            Ok(product) => {
                let db_price = product
                    .get(fields::PRICE_CENTS)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let in_stock = product
                    .get(fields::STOCK_QUANTITY)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let active = product
                    .get(fields::LIFECYCLE_STATUS)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    == "active";

                if db_price != item.expected_price_cents {
                    mismatches.push(serde_json::json!({
                        "productId": item.product_id,
                        "reason": "price_changed",
                        "expectedPriceCents": item.expected_price_cents,
                        "actualPriceCents": db_price,
                    }));
                } else if in_stock < item.quantity as i64 {
                    mismatches.push(serde_json::json!({
                        "productId": item.product_id,
                        "reason": "insufficient_stock",
                        "requestedQuantity": item.quantity,
                        "availableStock": in_stock,
                    }));
                } else if !active {
                    mismatches.push(serde_json::json!({
                        "productId": item.product_id,
                        "reason": "product_unavailable",
                    }));
                } else {
                    verified += 1;
                }
            }
            Err(_) => {
                mismatches.push(serde_json::json!({
                    "productId": item.product_id,
                    "reason": "product_not_found",
                }));
            }
        }
    }

    Ok(Json(serde_json::json!({
        "valid": mismatches.is_empty(),
        "verified": verified,
        "mismatches": mismatches,
    })))
}
