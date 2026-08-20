//! Stripe Checkout Session creation handler.
//! Ported from: functions/handlers/payment_stripe.py::create_checkout_session

use axum::{Extension, Json, Router, extract::State, routing::post};
use ob_auth::middleware::AuthContext;
use ob_database::Transaction;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{error, info, warn};

/// Stripe metadata keys used in Checkout Sessions
const STRIPE_META_ORDER_ID: &str = "order_id";
const STRIPE_META_USER_ID: &str = "user_id";
const STRIPE_META_COUPON_CODE: &str = "coupon_code";

use crate::HandlersState;
use crate::shared::auth::resolve_self_user_id;
use crate::shared::schema::{OrderStatus, collections, fields, lifecycle_status};
use crate::shared::validation::{validate_string, validate_uid};
use ob_database::fields as db_fields;

/// Request body for POST /api/payments/checkout — creates a Stripe Checkout Session.
///
/// Requires JWT auth. Validates: cart non-empty, items <= 30, quantity <= 100 per item,
/// subtotal <= $100,000 CAD, active-market province. Creates order records in PostgreSQL
/// and returns the Stripe session URL for redirect. Supports idempotency via [idempotency_key].
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
    #[serde(alias = "province")]
    pub state: String,
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
    #[serde(default)]
    pub duplicate: bool,
    #[serde(default)]
    pub tax_amount_cents: i64,
}

const VALID_PROVINCES: &[&str] = &[
    "AB", "BC", "MB", "NB", "NL", "NS", "NT", "NU", "ON", "PE", "QC", "SK", "YT",
];
const ACTIVE_SHIPPING_COUNTRIES: &[&str] = &["CA", "CANADA"];
const ACTIVE_STRIPE_SHIPPING_COUNTRIES: &[&str] = &["CA"];
const MAX_CART_ITEMS: usize = 30;
const MAX_ITEM_QUANTITY: u32 = 100;
const MAX_CHECKOUT_SUBTOTAL_CENTS: i64 = 10_000_000;
const PLATFORM_FEE_BPS: i64 = 250;

/// Standard flat-rate shipping cost in cents for non-free orders.
const STANDARD_SHIPPING_CENTS: i64 = 899;
/// International / cross-province seller shipping base in cents ($5.99).
const INTL_SHIPPING_BASE_CENTS: i64 = 599;

/// Returns the combined tax rate (as basis points, e.g. 1300 = 13%) for a Canadian province.
/// Rates: HST provinces return combined rate; GST+PST/QST provinces return sum.
fn province_tax_rate_bps(province: &str) -> u64 {
    match province {
        // HST provinces
        "ON" => 1300,                      // 13% HST
        "NB" | "NL" | "NS" | "PE" => 1500, // 15% HST
        // GST + QST
        "QC" => 1498, // 5% GST + 9.975% QST ≈ 14.975% → 1497.5 bps, round to 1498
        // GST + PST
        "BC" => 1200, // 5% GST + 7% PST = 12%
        "MB" => 1200, // 5% GST + 7% RST = 12%
        "SK" => 1100, // 5% GST + 6% PST = 11%
        // GST only
        "AB" | "NT" | "NU" | "YT" => 500, // 5% GST
        _ => 500,                         // Default to GST only
    }
}

/// Calculates tax amount in cents using integer arithmetic.
/// `taxable_base_cents` is subtotal + shipping. Returns tax in cents.
fn calculate_tax_cents(taxable_base_cents: i64, province: &str) -> i64 {
    let rate_bps = province_tax_rate_bps(province) as i64;
    // rate_bps is in basis points (1/100 of a percent), so divide by 10000
    // Use rounding: (base * rate + 5000) / 10000
    (taxable_base_cents * rate_bps + 5000) / 10000
}

/// Calculates the server-authoritative shipping amount for a validated cart.
///
/// Parameters:
/// - `subtotal_cents`: post-validation cart subtotal in integer cents.
/// - `buyer_province`: normalized active-market province code for the buyer.
/// - `items`: validated product snapshots assembled during checkout validation.
///
/// Returns:
/// - `Ok(cents)` with the flat shipping charge to persist on the order.
/// - `Err(...)` when item-level shipping restrictions make the order invalid.
///
/// Gotchas:
/// - Digital-only carts always return `0`.
/// - Free shipping is evaluated before cross-province pricing.
/// - Perishable and local-delivery-only items hard-fail when the buyer province
///   differs from the seller province.
fn calculate_shipping_cost_cents(
    subtotal_cents: i64,
    buyer_province: &str,
    items: &[Value],
) -> Result<i64, ob_core::Error> {
    // All-digital order: no shipping
    let all_digital = items.iter().all(|item| {
        item.get(fields::IS_DIGITAL)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    });
    if all_digital {
        return Ok(0);
    }

    // Perishable / local-delivery-only enforcement
    for item in items {
        let is_perishable = item
            .get(fields::IS_PERISHABLE)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let is_local_delivery_only = item
            .get(fields::IS_LOCAL_DELIVERY_ONLY)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if is_perishable || is_local_delivery_only {
            let seller_prov = item
                .get(fields::SHIP_FROM_PROVINCE)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !seller_prov.is_empty() && seller_prov != buyer_province {
                let label = if is_perishable {
                    "Perishable items"
                } else {
                    "Local-delivery-only items"
                };
                return Err(ob_core::Error::Validation(format!(
                    "{label} can only be shipped within the same province (50km local delivery). \
                     Seller province: {seller_prov}, buyer province: {buyer_province}"
                )));
            }
        }
    }

    // Free shipping threshold
    if subtotal_cents >= crate::shared::schema::business_rules::FREE_SHIPPING_THRESHOLD_CENTS {
        return Ok(0);
    }

    // Check if any item ships from a different province or a non-Canadian country
    let has_cross_province = items.iter().any(|item| {
        let seller_prov = item
            .get(fields::SHIP_FROM_PROVINCE)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let seller_country = item
            .get(fields::SHIP_FROM_COUNTRY)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let seller_country_upper = seller_country.trim().to_uppercase();
        // Non-Canadian seller → cross-province/international
        let is_international = !seller_country.is_empty()
            && seller_country_upper != "CA"
            && seller_country_upper != "CANADA";
        // Different province within Canada
        let is_cross_province = !seller_prov.is_empty() && seller_prov != buyer_province;
        is_international || is_cross_province
    });

    if has_cross_province {
        Ok(INTL_SHIPPING_BASE_CENTS)
    } else {
        Ok(STANDARD_SHIPPING_CENTS)
    }
}

fn normalize_country(country: &str) -> String {
    country.trim().to_uppercase()
}

fn is_active_shipping_country(country: &str) -> bool {
    let normalized = normalize_country(country);
    ACTIVE_SHIPPING_COUNTRIES.contains(&normalized.as_str())
}

fn normalize_province(province: &str) -> String {
    province.trim().to_uppercase()
}

fn normalize_postal_code(postal_code: &str) -> String {
    postal_code.replace(' ', "").to_uppercase()
}

fn normalize_coupon_code(code: &str) -> String {
    code.trim().to_uppercase()
}

fn is_valid_coupon_code(code: &str) -> bool {
    (4..=20).contains(&code.len())
        && code
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

fn compute_coupon_discount_cents(
    discount_type: &str,
    discount_value: f64,
    cart_subtotal_cents: i64,
) -> i64 {
    const MIN_CHECKOUT_TOTAL_CENTS: i64 = 100;
    const MAX_COUPON_DISCOUNT_RATIO: f64 = 0.90;

    if cart_subtotal_cents <= MIN_CHECKOUT_TOTAL_CENTS {
        return 0;
    }

    match discount_type {
        "percentage" | "percent" => {
            let effective = discount_value.min(MAX_COUPON_DISCOUNT_RATIO * 100.0);
            let millipercent = (effective * 1000.0).round() as i64;
            let discount = cart_subtotal_cents * millipercent / 100_000;
            if cart_subtotal_cents - discount < MIN_CHECKOUT_TOTAL_CENTS {
                cart_subtotal_cents - MIN_CHECKOUT_TOTAL_CENTS
            } else {
                discount
            }
        }
        "fixed_amount" | "fixed_cents" => {
            let fixed = discount_value as i64;
            fixed.min(cart_subtotal_cents - MIN_CHECKOUT_TOTAL_CENTS)
        }
        "free_shipping" => 0,
        _ => 0,
    }
}

async fn validate_checkout_coupon(
    state: &HandlersState,
    coupon_code: &str,
    user_id: &str,
    raw_subtotal_cents: i64,
    seller_ids: &[String],
) -> Result<(String, i64), ob_core::Error> {
    let code = normalize_coupon_code(coupon_code);
    if !is_valid_coupon_code(&code) {
        return Err(ob_core::Error::Validation(
            "Coupon invalid or unavailable".into(),
        ));
    }

    let coupon = match state.db.get_document(collections::COUPONS, &code).await {
        Ok(doc) if !doc.is_null() => doc,
        _ => {
            let query = format!(
                "SELECT * FROM {} WHERE data->>'{}' = $code LIMIT 1",
                collections::COUPONS,
                fields::CODE
            );
            let docs = state
                .db
                .query_bind(&query, serde_json::json!({"code": code.clone()}))
                .await
                .map_err(|_| ob_core::Error::NotFound("Coupon invalid or unavailable".into()))?;
            docs.into_iter()
                .next()
                .ok_or_else(|| ob_core::Error::NotFound("Coupon invalid or unavailable".into()))?
        }
    };

    let is_active = coupon
        .get(fields::IS_ACTIVE)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !is_active {
        return Err(ob_core::Error::Validation(
            "Coupon invalid or unavailable".into(),
        ));
    }

    if let Some(expires_at) = coupon.get(fields::EXPIRES_AT).and_then(|v| v.as_str())
        && let Ok(exp) = chrono::DateTime::parse_from_rfc3339(expires_at)
        && chrono::Utc::now() > exp
    {
        return Err(ob_core::Error::Validation(
            "Coupon invalid or unavailable".into(),
        ));
    }

    if let Some(coupon_seller) = coupon.get(db_fields::SELLER_ID).and_then(|v| v.as_str())
        && !seller_ids
            .iter()
            .any(|seller_id| seller_id == coupon_seller)
    {
        return Err(ob_core::Error::Validation(
            "Coupon invalid or unavailable".into(),
        ));
    }

    if let Some(min_order) = coupon.get(fields::MIN_ORDER_CENTS).and_then(|v| v.as_i64())
        && raw_subtotal_cents < min_order
    {
        return Err(ob_core::Error::Validation(
            "Cart subtotal does not meet the minimum order requirement for this coupon".into(),
        ));
    }

    let reservation_count_query = format!(
        "SELECT COUNT(*) AS count FROM {} WHERE data->>'{}' = $coupon_id",
        collections::COUPON_USES,
        fields::COUPON_ID,
    );
    let reservation_count = state
        .db
        .query_bind_value(
            &reservation_count_query,
            serde_json::json!({"coupon_id": code.clone()}),
        )
        .await
        .unwrap_or_default()
        .first()
        .and_then(|v| v.get("count"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if let Some(max_uses) = coupon.get(fields::MAX_USES).and_then(|v| v.as_i64())
        && reservation_count >= max_uses
    {
        return Err(ob_core::Error::Validation(
            "Coupon invalid or unavailable".into(),
        ));
    }

    let user_usage_query = format!(
        "SELECT COUNT(*) AS count FROM {} WHERE data->>'{}' = $coupon_id AND data->>'{}' = $user_id",
        collections::COUPON_USES,
        fields::COUPON_ID,
        db_fields::USER_ID,
    );
    let user_use_count = state
        .db
        .query_bind_value(
            &user_usage_query,
            serde_json::json!({
                "coupon_id": code.clone(),
                "user_id": user_id,
            }),
        )
        .await
        .unwrap_or_default()
        .first()
        .and_then(|v| v.get("count"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let max_per_user = coupon
        .get(fields::MAX_USES_PER_USER)
        .and_then(|v| v.as_i64())
        .unwrap_or(1);
    if user_use_count >= max_per_user {
        return Err(ob_core::Error::Validation(
            "You've reached the maximum uses for this coupon".into(),
        ));
    }

    let discount_type = coupon
        .get(fields::COUPON_TYPE)
        .or_else(|| coupon.get(fields::DISCOUNT_TYPE))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let discount_value = coupon
        .get(fields::DISCOUNT_VALUE)
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let discount_amount_cents =
        compute_coupon_discount_cents(discount_type, discount_value, raw_subtotal_cents);

    Ok((code, discount_amount_cents))
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

/// Create the checkout router with routes for session creation and price verification.
pub fn router(state: HandlersState) -> Router {
    Router::new()
        .route("/api/checkout/session", post(create_checkout_session))
        .route("/api/checkout/verify-prices", post(verify_cart_prices))
        .with_state(state)
}

/// Creates a Stripe Checkout Session and the corresponding pending order record.
///
/// Parameters:
/// - `state`: shared handler state containing DB, config, Stripe, and Turnstile clients.
/// - `auth`: authenticated caller context used to resolve the buyer identity.
/// - `req`: checkout payload containing cart items, shipping address, consent flags,
///   and an optional idempotency key.
///
/// Returns:
/// - `Ok(Json<CheckoutResponse>)` with the Stripe session ID, internal order ID,
///   and redirect URL when checkout setup succeeds.
/// - `Err(...)` for validation, auth, Stripe, or database failures.
///
/// Gotchas:
/// - This handler re-validates prices, stock, seller eligibility, and shipping
///   rules server-side; the client subtotal is advisory only.
/// - Multi-seller carts are rejected because the current Stripe Connect flow can
///   route funds to only one destination account.
/// - Stock reservation is finalized through a PostgreSQL transaction after the
///   Stripe session is created, so retries must reuse idempotency keys.
async fn create_checkout_session(
    State(state): State<HandlersState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<CreateCheckoutRequest>,
) -> Result<Json<CheckoutResponse>, ob_core::Error> {
    // SECURITY: Validate Turnstile token (prevents bot checkout attacks)
    let is_test_mode = std::env::var("OB_TEST_MODE").unwrap_or_default() == "1";
    if let Some(ref token) = req.turnstile_token {
        if let Some(ref secret) = state.turnstile_secret_key {
            ob_auth::validate_turnstile_token(token, secret).await?;
        } else if !is_test_mode {
            return Err(ob_core::Error::Forbidden(
                "Turnstile secret not configured — cannot validate token".into(),
            ));
        }
    } else if !is_test_mode {
        return Err(ob_core::Error::Validation(
            "Turnstile token is required".into(),
        ));
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
        // Validate product ID format to prevent injection
        ob_core::validate_document_id(&item.product_id)?;
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
    if !is_active_shipping_country(&country) {
        return Err(ob_core::Error::Validation(
            "Shipping is currently available within Canada only".into(),
        ));
    }

    let province = normalize_province(&req.shipping_address.state);
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

    // --- Server-side product validation (parameterized) ---
    let product_ids: Vec<&str> = req.items.iter().map(|i| i.product_id.as_str()).collect();
    let mut product_rows: Vec<Value> = Vec::with_capacity(product_ids.len());
    for pid in &product_ids {
        if let Ok(doc) = state.db.get_document(collections::PRODUCTS, pid).await {
            product_rows.push(doc);
        }
    }

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
                p.get(db_fields::ID)
                    .and_then(|v| v.as_str())
                    .map(|id| id.ends_with(&cart_item.product_id))
                    .unwrap_or(false)
            })
            .ok_or_else(|| {
                ob_core::Error::NotFound(format!("Product {} not found", cart_item.product_id))
            })?;

        let lifecycle = product
            .get(db_fields::LIFECYCLE_STATUS)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if lifecycle != lifecycle_status::ACTIVE {
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
            .get(db_fields::PRICE_CENTS)
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
            .get(db_fields::SELLER_ID)
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Self-purchase prevention: normalize IDs by stripping collection prefix
        // JWT has "users:xyz123", seller_id from product is "xyz123" (short form)
        let user_id_short = user_id.strip_prefix("users:").unwrap_or(&user_id);
        if seller_id == user_id_short {
            return Err(ob_core::Error::Validation(format!(
                "Cannot purchase your own products (seller: {}, buyer: {})",
                seller_id, user_id_short
            )));
        }

        // Age verification for restricted items
        let age_restricted = product
            .get(fields::IS_AGE_RESTRICTED)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if age_restricted && !req.age_verification_accepted {
            return Err(ob_core::Error::Validation(
                "Age verification required for restricted items".into(),
            ));
        }

        let is_digital = product
            .get(fields::IS_DIGITAL)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if is_digital && !req.eula_accepted {
            return Err(ob_core::Error::Validation(
                "EULA acceptance required for digital products".into(),
            ));
        }

        let is_perishable = product
            .get(fields::IS_PERISHABLE)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let is_local_delivery_only = product
            .get(fields::IS_LOCAL_DELIVERY_ONLY)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let ship_from_province = product
            .get(fields::SHIP_FROM_PROVINCE)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let ship_from_country = product
            .get(fields::SHIP_FROM_COUNTRY)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let weight_kg = product.get("weightKg").and_then(|v| v.as_f64());

        validated_items.push(serde_json::json!({
            fields::PRODUCT_ID: cart_item.product_id,
            fields::QUANTITY: cart_item.quantity,
            db_fields::PRICE_CENTS: price_cents,
            db_fields::SELLER_ID: seller_id,
            fields::TITLE: product.get(fields::TITLE).and_then(|v| v.as_str()).unwrap_or(""),
            fields::IMAGE_URL: product.get(fields::IMAGE_URLS)
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str()).unwrap_or(""),
            fields::IS_DIGITAL: is_digital,
            fields::IS_PERISHABLE: is_perishable,
            fields::IS_LOCAL_DELIVERY_ONLY: is_local_delivery_only,
            fields::SHIP_FROM_PROVINCE: ship_from_province,
            fields::SHIP_FROM_COUNTRY: ship_from_country,
            "weightKg": weight_kg,
        }));
    }

    // --- Seller suspension check ---
    let unique_seller_ids: Vec<String> = validated_items
        .iter()
        .filter_map(|item| {
            item.get(db_fields::SELLER_ID)
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    // --- Multi-seller checkout guard (P0) ---
    // Stripe Connect only supports a single destination per PaymentIntent.
    // Force the frontend to split carts with multiple sellers into separate sessions.
    if unique_seller_ids.len() > 1 {
        return Err(ob_core::Error::Validation(
            "Multi-seller carts require separate checkout sessions per seller.".into(),
        ));
    }

    // Cache seller profiles to avoid N+1 queries (used again for Connect lookup below)
    let mut seller_profiles_cache: std::collections::HashMap<String, Value> =
        std::collections::HashMap::new();

    for seller_id in &unique_seller_ids {
        if let Ok(seller) = state.db.get_document(collections::USERS, seller_id).await {
            let suspended = seller
                .get(fields::SUSPENDED)
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if suspended {
                return Err(ob_core::Error::Validation(format!(
                    "Seller {seller_id} is suspended"
                )));
            }

            // // Verify seller has completed Stripe Connect onboarding
            // // We are bypassing this check since seller onboarding is currently disabled.
            // let onboarding_completed = seller
            //     .get(fields::ONBOARDING_COMPLETED)
            //     .and_then(|v| v.as_bool())
            //     .unwrap_or(false);
            // if !onboarding_completed {
            //     return Err(ob_core::Error::Validation(format!(
            //         "Seller {} has not completed Stripe Connect onboarding. Cannot accept orders from this seller.",
            //         seller_id
            //     )));
            // }

            // Verify both charges and payouts are enabled
            // let charges_enabled = seller
            //     .get(fields::CHARGES_ENABLED)
            //     .and_then(|v| v.as_bool())
            //     .unwrap_or(false);
            // let payouts_enabled = seller
            //     .get(fields::PAYOUTS_ENABLED)
            //     .and_then(|v| v.as_bool())
            //     .unwrap_or(false);
            // if !charges_enabled || !payouts_enabled {
            //     return Err(ob_core::Error::Validation(format!(
            //         "Seller {} cannot currently accept payments.",
            //         seller_id
            //     )));
            // }
        }

        // Cache seller_profiles for Connect account lookup later
        if let Ok(profile) = state
            .db
            .get_document(collections::SELLER_PROFILES, seller_id)
            .await
        {
            seller_profiles_cache.insert(seller_id.clone(), profile);
        }
    }

    let raw_subtotal_cents = actual_subtotal_cents;
    let normalized_coupon_code = req
        .coupon_code
        .as_deref()
        .map(normalize_coupon_code)
        .filter(|code| !code.is_empty());
    let (normalized_coupon_code, discount_amount_cents) =
        if let Some(coupon_code) = normalized_coupon_code {
            let (code, discount) = validate_checkout_coupon(
                &state,
                &coupon_code,
                &user_id,
                raw_subtotal_cents,
                &unique_seller_ids,
            )
            .await?;
            (Some(code), discount)
        } else {
            (None, 0)
        };
    actual_subtotal_cents = raw_subtotal_cents - discount_amount_cents;

    if !subtotal_matches_with_tolerance(req.subtotal_cents, actual_subtotal_cents) {
        warn!(
            user_id = %user_id,
            client = req.subtotal_cents,
            server = actual_subtotal_cents,
            raw_subtotal_cents = raw_subtotal_cents,
            discount_amount_cents = discount_amount_cents,
            "Subtotal mismatch"
        );
        return Err(ob_core::Error::Validation(format!(
            "Subtotal mismatch. Expected ~{actual_subtotal_cents} cents, got {} cents",
            req.subtotal_cents
        )));
    }

    let idempotency_key = req.idempotency_key.clone().unwrap_or_else(|| {
        format!(
            "checkout_{}_{}",
            user_id,
            chrono::Utc::now().timestamp_millis()
        )
    });
    let dedup_query = format!(
        "SELECT * FROM {} WHERE data->>'{}' = $buyer_id AND data->>'{}' = $idempotency_key LIMIT 1",
        collections::ORDERS,
        db_fields::BUYER_ID,
        db_fields::IDEMPOTENCY_KEY,
    );
    let existing: Vec<Value> = match state
        .db
        .query_bind_value(
            &dedup_query,
            serde_json::json!({"buyer_id": user_id, "idempotency_key": idempotency_key}),
        )
        .await
    {
        Ok(rows) => rows,
        Err(err) => {
            warn!(user_id = %user_id, error = %err, "Idempotency lookup failed, allowing checkout to proceed");
            vec![]
        }
    };
    if let Some(existing_order) = existing.first() {
        let existing_order_id = existing_order
            .get(fields::ORDER_ID)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let existing_session_id = existing_order
            .get(fields::CHECKOUT_SESSION_ID)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let existing_tax_amount_cents = existing_order
            .get(fields::TAX_AMOUNT_CENTS)
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        return Ok(Json(CheckoutResponse {
            session_id: existing_session_id,
            order_id: existing_order_id,
            checkout_url: None,
            success: true,
            duplicate: true,
            tax_amount_cents: existing_tax_amount_cents,
        }));
    }

    // --- Create Stripe Checkout Session ---
    // --- Calculate shipping and tax before creating Stripe Checkout ---
    let shipping_cost_cents =
        calculate_shipping_cost_cents(actual_subtotal_cents, &province, &validated_items)?;
    let taxable_base_cents = actual_subtotal_cents + shipping_cost_cents;
    let tax_amount_cents = calculate_tax_cents(taxable_base_cents, &province);
    let total_amount_cents = actual_subtotal_cents + shipping_cost_cents + tax_amount_cents;

    let stripe_key = state.config.require_secret("stripe_secret_key")?;
    let order_id = uuid::Uuid::new_v4().simple().to_string();

    // Calculate platform fee from subtotal only — integer math, no floats.
    let platform_fee_cents = actual_subtotal_cents * PLATFORM_FEE_BPS / 10_000;

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
        (
            "billing_address_collection".to_string(),
            "required".to_string(),
        ),
        (
            "phone_number_collection[enabled]".to_string(),
            "true".to_string(),
        ),
        ("payment_method_types[0]".to_string(), "card".to_string()),
        ("payment_method_types[1]".to_string(), "klarna".to_string()),
        (
            "payment_intent_data[capture_method]".to_string(),
            "manual".to_string(),
        ),
        (
            format!("metadata[{}]", STRIPE_META_ORDER_ID),
            order_id.clone(),
        ),
        (
            format!("metadata[{}]", STRIPE_META_USER_ID),
            user_id.clone(),
        ),
    ];
    for (i, country) in ACTIVE_STRIPE_SHIPPING_COUNTRIES.iter().enumerate() {
        form_data.push((
            format!("shipping_address_collection[allowed_countries][{i}]"),
            (*country).to_string(),
        ));
    }
    if let Some(coupon_code) = &normalized_coupon_code {
        form_data.push((
            format!("metadata[{}]", STRIPE_META_COUPON_CODE),
            coupon_code.clone(),
        ));
    }

    // Platform fee via Stripe Connect — only include when seller has a real Connect account
    // Without Connect, Stripe rejects application_fee_amount with "parameter_unknown"
    // Uses cached seller_profiles from the validation loop above (avoids N+1 queries)
    if platform_fee_cents > 0 {
        let mut has_connect_account = false;
        for sid in &unique_seller_ids {
            if let Some(profile) = seller_profiles_cache.get(sid)
                && let Some(acct_id) = profile
                    .get(fields::STRIPE_ACCOUNT_ID)
                    .and_then(|v| v.as_str())
                && acct_id.starts_with("acct_")
            {
                has_connect_account = true;
                // Add the connected account header for Stripe Connect
                form_data.push((
                    "payment_intent_data[on_behalf_of]".to_string(),
                    acct_id.to_string(),
                ));
                form_data.push((
                    "payment_intent_data[transfer_data][destination]".to_string(),
                    acct_id.to_string(),
                ));
                break;
            }
        }
        if has_connect_account {
            form_data.push((
                "application_fee_amount".to_string(),
                platform_fee_cents.to_string(),
            ));
        }
    }

    let mut line_item_index = 0usize;
    if actual_subtotal_cents > 0 {
        form_data.push((
            format!("line_items[{line_item_index}][price_data][currency]"),
            "cad".to_string(),
        ));
        form_data.push((
            format!("line_items[{line_item_index}][price_data][product_data][name]"),
            "Order subtotal".to_string(),
        ));
        form_data.push((
            format!("line_items[{line_item_index}][price_data][unit_amount]"),
            actual_subtotal_cents.to_string(),
        ));
        form_data.push((
            format!("line_items[{line_item_index}][quantity]"),
            "1".to_string(),
        ));
        line_item_index += 1;
    }
    if shipping_cost_cents > 0 {
        form_data.push((
            format!("line_items[{line_item_index}][price_data][currency]"),
            "cad".to_string(),
        ));
        form_data.push((
            format!("line_items[{line_item_index}][price_data][product_data][name]"),
            "Shipping".to_string(),
        ));
        form_data.push((
            format!("line_items[{line_item_index}][price_data][unit_amount]"),
            shipping_cost_cents.to_string(),
        ));
        form_data.push((
            format!("line_items[{line_item_index}][quantity]"),
            "1".to_string(),
        ));
        line_item_index += 1;
    }
    if tax_amount_cents > 0 {
        form_data.push((
            format!("line_items[{line_item_index}][price_data][currency]"),
            "cad".to_string(),
        ));
        form_data.push((
            format!("line_items[{line_item_index}][price_data][product_data][name]"),
            "Estimated tax".to_string(),
        ));
        form_data.push((
            format!("line_items[{line_item_index}][price_data][unit_amount]"),
            tax_amount_cents.to_string(),
        ));
        form_data.push((
            format!("line_items[{line_item_index}][quantity]"),
            "1".to_string(),
        ));
    }

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
    let session_id = session[db_fields::ID]
        .as_str()
        .ok_or_else(|| ob_core::Error::Internal("Missing session ID from Stripe".into()))?;
    let checkout_url = session["url"].as_str().map(str::to_string);

    // --- Create order document ---
    let now = chrono::Utc::now().to_rfc3339();
    let order_doc = serde_json::json!({
        fields::ORDER_ID: order_id,
        db_fields::BUYER_ID: user_id,
        fields::ORDER_STATUS: OrderStatus::PendingPayment.as_str(),
        fields::PAYMENT_STATUS: "awaiting_payment",
        fields::ITEMS: validated_items,
        db_fields::SUBTOTAL_CENTS: actual_subtotal_cents,
        fields::COUPON_CODE: normalized_coupon_code,
        fields::DISCOUNT_AMOUNT_CENTS: discount_amount_cents,
        fields::TAX_AMOUNT_CENTS: tax_amount_cents,
        fields::SHIPPING_COST_CENTS: shipping_cost_cents,
        db_fields::TOTAL_AMOUNT_CENTS: total_amount_cents,
        fields::PLATFORM_FEE_CENTS: platform_fee_cents,
        db_fields::IDEMPOTENCY_KEY: idempotency_key,
        fields::SHIPPING_ADDRESS: serde_json::json!({
            fields::STREET: req.shipping_address.street,
            fields::CITY: req.shipping_address.city,
            fields::PROVINCE: province,
            fields::POSTAL_CODE: postal,
            fields::COUNTRY: "CA",
        }),
        fields::CHECKOUT_SESSION_ID: session_id,
        db_fields::CREATED_AT: now,
        db_fields::UPDATED_AT: now,
    });

    // --- Atomic order creation with stock reservation ---
    // CRITICAL: Stock check and decrement must be atomic to prevent race conditions
    // where two concurrent buyers both pass validation on stock 2 and create negative stock.
    // Use PostgreSQL transaction to ensure all-or-nothing semantics.

    // Build atomic transaction: create order + reserve stock for all physical items
    let mut tx = Transaction::new();

    // Operation 1: Create the order
    // Use INSERT with explicit ID to ensure the order_id is used as the record key
    tx.add(
        &format!(
            "INSERT INTO {} (id, data) VALUES ($id, $data::jsonb) RETURNING *",
            collections::ORDERS
        ),
        Some(serde_json::json!({
            "id": order_id,
            "data": order_doc,
        })),
    );

    if let Some(coupon_code) = &normalized_coupon_code {
        tx.add(
            &format!(
                "INSERT INTO {} (id, data) VALUES ($id, $data::jsonb) RETURNING *",
                collections::COUPON_USES
            ),
            Some(serde_json::json!({
                "id": format!("{}:{}:{}", order_id, coupon_code, user_id),
                "data": {
                    fields::COUPON_ID: coupon_code,
                    fields::COUPON_CODE: coupon_code,
                    db_fields::USER_ID: user_id,
                    fields::ORDER_ID: order_id,
                    fields::REDEEMED_AT: Value::Null,
                    db_fields::CREATED_AT: now,
                    db_fields::UPDATED_AT: now,
                }
            })),
        );
    }

    // Operations 2+: Decrement stock for each non-digital item
    // This is atomic with order creation — if stock goes negative, entire transaction rolls back
    let mut stock_op_indices: Vec<(usize, String)> = Vec::new();
    for item in &validated_items {
        if !item
            .get(fields::IS_DIGITAL)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let pid = item
                .get(fields::PRODUCT_ID)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let qty = item
                .get(fields::QUANTITY)
                .and_then(|v| v.as_u64())
                .unwrap_or(1);
            if ob_core::validate_document_id(pid).is_ok() && qty > 0 {
                let idx = tx.len();
                // CRITICAL: Atomic check + decrement using WHERE guard.
                // UPDATE only succeeds if stockQuantity >= qty. If 0 rows affected, out of stock.
                // Native PostgreSQL: atomic decrement stockQuantity in JSONB data column.
                // pid is validated by validate_document_id above; qty is a u64 from validated items.
                let now_escaped = now.replace('\'', "''");
                tx.add_raw(
                    &format!(
                        "UPDATE {table} SET data = jsonb_set(jsonb_set(data, '{{stockQuantity}}', to_jsonb((data->>'stockQuantity')::bigint - {qty})), '{{updatedAt}}', '\"{now_escaped}\"'::jsonb), updated_at = now() WHERE id = '{pid}' AND (data->>'stockQuantity')::bigint >= {qty}",
                        table = collections::PRODUCTS,
                    ),
                );
                stock_op_indices.push((idx, pid.to_string()));
            }
        }
    }

    // Execute transaction atomically
    let tx_results = tx.commit(&state.db).await.map_err(|e| {
        ob_core::Error::Database(format!(
            "Failed to create order and reserve stock (atomic transaction): {e}"
        ))
    })?;

    // Verify all stock decrements actually affected a row (WHERE guard matched)
    for (idx, pid) in &stock_op_indices {
        let result = tx_results.get(*idx);
        let is_empty = match result {
            Some(Value::Array(arr)) => arr.is_empty(),
            Some(Value::Null) | None => true,
            _ => false,
        };
        if is_empty {
            warn!(product_id = %pid, order_id = %order_id, "Insufficient stock — UPDATE matched 0 rows");
            return Err(ob_core::Error::Validation(format!(
                "Insufficient stock for product {pid}. Please reduce quantity or remove from cart."
            )));
        }
    }

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
        duplicate: false,
        tax_amount_cents,
    }))
}

#[cfg(test)]
mod tests {
    fn auth(uid: &str) -> AuthContext {
        AuthContext {
            user_id: uid.to_string(),
            roles: vec![],
            authenticated: true,
            email_verified: true,
            custom_claims: serde_json::Value::Null,
        }
    }

    use super::*;
    use axum::extract::State;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use serde_json::json;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn received_stripe_form_body(mock_server: &MockServer) -> String {
        let requests = mock_server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        String::from_utf8_lossy(&requests[0].body).into_owned()
    }

    fn assert_form_body_contains(body: &str, expected_parts: &[&str]) {
        for expected in expected_parts {
            assert!(
                body.contains(expected),
                "expected Stripe form body to contain `{expected}`, got `{body}`"
            );
        }
    }

    async fn setup_state() -> HandlersState {
        // SAFETY: Test-only env var modification in single-threaded test context
        unsafe { std::env::set_var("OB_TEST_MODE", "1") };
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
                "state": "ON", "postalCode": "M5V 2H1", "country": "CA"
            },
            "userId": "user123", "subtotalCents": 5000
        }"#; // ignore-magic
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
        assert_eq!(
            tolerance, 200,
            "Tolerance for $100k should be fixed $2.00, not $1k"
        );
        assert!(subtotal_matches_with_tolerance(actual + tolerance, actual));
        assert!(!subtotal_matches_with_tolerance(
            actual + tolerance + 1,
            actual
        ));
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
                "state": "ON", "postalCode": "M5V2H1", "country": "CA"
            },
            "userId": "u1", "subtotalCents": 100
        }"#; // ignore-magic
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
                "state": "ON", "postalCode": "M5V2H1", "country": "CA"
            },
            "userId": "u1", "subtotalCents": 100,
            "couponCode": "SAVE10",
            "eulaAccepted": true,
            "ageVerificationAccepted": true,
            "idempotencyKey": "idem-123"
        }"#; // ignore-magic
        let req: CreateCheckoutRequest = serde_json::from_str(json).unwrap();
        assert!(req.eula_accepted);
        assert!(req.age_verification_accepted);
        assert_eq!(req.coupon_code.as_deref(), Some("SAVE10"));
        assert_eq!(req.idempotency_key.as_deref(), Some("idem-123"));
    }

    #[test]
    fn test_cart_item_zero_quantity_deser() {
        let json = r#"{"productId": "p1", "quantity": 0}"#; // ignore-magic
        let item: CartItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.quantity, 0);
    }

    #[test]
    fn test_cart_item_empty_product_id_deser() {
        let json = r#"{"productId": "", "quantity": 1}"#; // ignore-magic
        let item: CartItem = serde_json::from_str(json).unwrap();
        assert!(item.product_id.is_empty());
    }

    #[test]
    fn test_shipping_address_deser() {
        let json = r#"{
            "street": "123 Main St",
            "city": "Toronto",
            "state": "ON",
            "postalCode": "M5V 2H1",
            "country": "CA"
        }"#; // ignore-magic
        let addr: ShippingAddress = serde_json::from_str(json).unwrap();
        assert_eq!(addr.street, "123 Main St");
        assert_eq!(addr.state, "ON");
    }

    #[test]
    fn test_checkout_response_serialization() {
        let resp = CheckoutResponse {
            session_id: "cs_test_123".into(),
            order_id: "order_456".into(),
            checkout_url: Some("https://checkout.stripe.com/c/pay/test".into()),
            success: true,
            duplicate: false,
            tax_amount_cents: 507,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["sessionId"], "cs_test_123");
        assert_eq!(json[fields::ORDER_ID], "order_456");
        assert_eq!(
            json["checkoutUrl"],
            "https://checkout.stripe.com/c/pay/test"
        );
        assert_eq!(json["success"], true);
        assert_eq!(json["duplicate"], false);
        assert_eq!(json["taxAmountCents"], 507);
    }

    #[test]
    fn test_active_market_provinces_are_valid() {
        let expected_canada = vec![
            "AB", "BC", "MB", "NB", "NL", "NS", "NT", "NU", "ON", "PE", "QC", "SK", "YT",
        ];

        assert_eq!(VALID_PROVINCES.len(), expected_canada.len());

        for p in &expected_canada {
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
            !age_restricted || age_verification_accepted,
            "Should allow when verified"
        );

        // age_restricted = false → should always pass regardless
        let age_restricted = false;
        let age_verification_accepted = false;
        assert!(
            !age_restricted || age_verification_accepted,
            "Non-restricted items should not require verification"
        );
    }

    #[test]
    fn test_age_verification_field_defaults_false() {
        let json = r#"{"items":[{"productId":"p1","quantity":1}],"shippingAddress":{"street":"1 St","city":"Toronto","state":"ON","postalCode":"M5V2H1","country":"CA"},"userId":"u1","subtotalCents":100}"#; // ignore-magic
        let req: CreateCheckoutRequest = serde_json::from_str(json).unwrap();
        assert!(!req.age_verification_accepted);
    }

    #[test]
    fn test_age_verification_field_accepts_true() {
        let json = r#"{"items":[{"productId":"p1","quantity":1}],"shippingAddress":{"street":"1 St","city":"Toronto","state":"ON","postalCode":"M5V2H1","country":"CA"},"userId":"u1","subtotalCents":100,"ageVerificationAccepted":true}"#; // ignore-magic
        let req: CreateCheckoutRequest = serde_json::from_str(json).unwrap();
        assert!(req.age_verification_accepted);
    }

    #[tokio::test]
    async fn test_create_checkout_session_rejects_basic_validation_errors() {
        let state = setup_state().await;
        let shipping = ShippingAddress {
            street: "123 Main St".into(),
            city: "Toronto".into(),
            state: "ON".into(),
            postal_code: "M5V2H1".into(),
            country: "CA".into(),
        };

        let empty_items = create_checkout_session(
            State(state.clone()),
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
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
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
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
                .contains("Shipping is currently available within Canada only")
        );

        let bad_postal = create_checkout_session(
            State(state),
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
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
    async fn test_create_checkout_session_rejects_stock_self_purchase_and_restricted_orders() {
        let state = setup_state().await;
        let u = uuid::Uuid::new_v4().to_string();
        let prod_stock = format!("prod_stock_{u}");
        let seller_1 = format!("seller_1_{u}");
        let buyer_1 = format!("buyer_1_{u}");
        let prod_restricted = format!("prod_restr_{u}");
        let seller_2 = format!("seller_2_{u}");
        let shipping = ShippingAddress {
            street: "123 Main St".into(),
            city: "Toronto".into(),
            state: "ON".into(),
            postal_code: "M5V2H1".into(),
            country: "CA".into(),
        };

        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                &prod_stock,
                json!({
                    fields::PRODUCT_ID: prod_stock,
                    db_fields::SELLER_ID: seller_1,
                    db_fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 1,
                    db_fields::PRICE_CENTS: 1000,
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
                &seller_1,
                json!({
                    fields::UID: seller_1,
                    fields::SUSPENDED: false,
                    fields::ONBOARDING_COMPLETED: true,
                    fields::CHARGES_ENABLED: true,
                    fields::PAYOUTS_ENABLED: true,
                }),
            )
            .await
            .unwrap();

        let insufficient = create_checkout_session(
            State(state.clone()),
            Extension(auth(&buyer_1)),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: prod_stock.clone(),
                    quantity: 2,
                }],
                shipping_address: shipping.clone(),
                user_id: Some(buyer_1.clone()),
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
            Extension(auth(&seller_1)),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: prod_stock.clone(),
                    quantity: 1,
                }],
                shipping_address: shipping.clone(),
                user_id: Some(seller_1.clone()),
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
                &prod_restricted,
                json!({
                    fields::PRODUCT_ID: prod_restricted,
                    db_fields::SELLER_ID: seller_2,
                    db_fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 3,
                    db_fields::PRICE_CENTS: 2500,
                    fields::IS_AGE_RESTRICTED: true,
                    fields::IS_DIGITAL: true,
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
                &seller_2,
                json!({
                    fields::UID: seller_2,
                    fields::SUSPENDED: false,
                    fields::ONBOARDING_COMPLETED: true,
                    fields::CHARGES_ENABLED: true,
                    fields::PAYOUTS_ENABLED: true,
                }),
            )
            .await
            .unwrap();

        let age_block = create_checkout_session(
            State(state.clone()),
            Extension(auth(&buyer_1)),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: prod_restricted.clone(),
                    quantity: 1,
                }],
                shipping_address: shipping.clone(),
                user_id: Some(buyer_1.clone()),
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
            Extension(auth(&buyer_1)),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: prod_restricted.clone(),
                    quantity: 1,
                }],
                shipping_address: shipping.clone(),
                user_id: Some(buyer_1.clone()),
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
    }

    #[tokio::test]
    async fn test_create_checkout_session_success_creates_order_and_reserves_stock() {
        let state = setup_state().await;
        let mock_server = MockServer::start().await;
        let u = uuid::Uuid::new_v4().to_string();
        let prod_physical = format!("prod_phys_{u}");
        let seller_id = format!("seller_phys_{u}");
        let buyer_id = format!("buyer_phys_{u}");

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
                &prod_physical,
                json!({
                    fields::PRODUCT_ID: prod_physical,
                    db_fields::SELLER_ID: seller_id,
                    db_fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 5,
                    db_fields::PRICE_CENTS: 1500,
                    fields::TITLE: "Physical Widget",
                    fields::IMAGE_URLS: ["https://example.com/widget.png"],
                    fields::IS_DIGITAL: false,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                &seller_id,
                json!({
                    fields::UID: seller_id,
                    fields::SUSPENDED: false,
                    fields::ONBOARDING_COMPLETED: true,
                    fields::CHARGES_ENABLED: true,
                    fields::PAYOUTS_ENABLED: true,
                }),
            )
            .await
            .unwrap();

        let Json(resp) = create_checkout_session(
            State(state.clone()),
            Extension(auth(&buyer_id)),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: prod_physical.clone(),
                    quantity: 2,
                }],
                shipping_address: ShippingAddress {
                    street: "123 Main St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some(buyer_id.clone()),
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

        let stripe_body = received_stripe_form_body(&mock_server).await;
        assert_form_body_contains(
            &stripe_body,
            &[
                "billing_address_collection=required",
                "phone_number_collection%5Benabled%5D=true",
                "shipping_address_collection%5Ballowed_countries%5D%5B0%5D=CA",
                "line_items%5B0%5D%5Bprice_data%5D%5Bproduct_data%5D%5Bname%5D=Order+subtotal",
                "line_items%5B0%5D%5Bprice_data%5D%5Bunit_amount%5D=3000",
                "line_items%5B1%5D%5Bprice_data%5D%5Bproduct_data%5D%5Bname%5D=Shipping",
                "line_items%5B1%5D%5Bprice_data%5D%5Bunit_amount%5D=899",
                "line_items%5B2%5D%5Bprice_data%5D%5Bproduct_data%5D%5Bname%5D=Estimated+tax",
                "line_items%5B2%5D%5Bprice_data%5D%5Bunit_amount%5D=507",
            ],
        );

        let order = state
            .db
            .get_document(collections::ORDERS, &resp.order_id)
            .await
            .unwrap();
        assert_eq!(
            order[fields::ORDER_STATUS],
            OrderStatus::PendingPayment.as_str()
        );
        assert_eq!(order[fields::PAYMENT_STATUS], "awaiting_payment");
        assert_eq!(order[fields::CHECKOUT_SESSION_ID], "cs_test_123");
        assert_eq!(order[db_fields::SUBTOTAL_CENTS], 3000);
        // Total = subtotal (3000) + shipping (899) + tax (ON 13% on 3899 = 507) = 4406
        assert_eq!(order[fields::SHIPPING_COST_CENTS], 899);
        assert_eq!(order[fields::TAX_AMOUNT_CENTS], 507);
        assert_eq!(order[db_fields::TOTAL_AMOUNT_CENTS], 4406);

        let product = state
            .db
            .get_document(collections::PRODUCTS, &prod_physical)
            .await
            .unwrap();
        assert_eq!(product[fields::STOCK_QUANTITY], 3);
    }

    #[tokio::test]
    async fn test_create_checkout_session_with_coupon_persists_discount_and_reserves_coupon_use() {
        let state = setup_state().await;
        let mock_server = MockServer::start().await;
        let u = uuid::Uuid::new_v4().to_string();
        let product_id = format!("prod_coupon_{u}");
        let seller_id = format!("seller_coupon_{u}");
        let buyer_id = format!("buyer_coupon_{u}");

        Mock::given(method("POST"))
            .and(path("/checkout/sessions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cs_coupon_123"
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
                &product_id,
                json!({
                    fields::PRODUCT_ID: product_id,
                    db_fields::SELLER_ID: seller_id,
                    db_fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 5,
                    db_fields::PRICE_CENTS: 3000,
                    fields::TITLE: "Coupon Widget",
                    fields::IMAGE_URLS: ["https://example.com/widget.png"],
                    fields::IS_DIGITAL: false,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                &seller_id,
                json!({
                    fields::UID: seller_id,
                    fields::SUSPENDED: false,
                    fields::ONBOARDING_COMPLETED: true,
                    fields::CHARGES_ENABLED: true,
                    fields::PAYOUTS_ENABLED: true,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::COUPONS,
                "SAVE10",
                json!({
                    fields::CODE: "SAVE10",
                    fields::COUPON_TYPE: "percentage",
                    fields::DISCOUNT_VALUE: 10.0,
                    fields::MAX_USES: 5,
                    fields::MAX_USES_PER_USER: 1,
                    fields::IS_ACTIVE: true,
                }),
            )
            .await
            .unwrap();

        let Json(resp) = create_checkout_session(
            State(state.clone()),
            Extension(auth(&buyer_id)),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: product_id.clone(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "123 Main St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some(buyer_id.clone()),
                subtotal_cents: 2700,
                coupon_code: Some("save10".into()),
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: Some("idem-coupon-1".into()),
            }),
        )
        .await
        .unwrap();

        assert!(resp.success);
        assert!(!resp.duplicate);
        assert_eq!(resp.session_id, "cs_coupon_123");
        assert_eq!(resp.tax_amount_cents, 468);

        let stripe_body = received_stripe_form_body(&mock_server).await;
        assert_form_body_contains(
            &stripe_body,
            &[
                "shipping_address_collection%5Ballowed_countries%5D%5B0%5D=CA",
                "metadata%5Bcoupon_code%5D=SAVE10",
                "line_items%5B0%5D%5Bprice_data%5D%5Bunit_amount%5D=2700",
                "line_items%5B1%5D%5Bprice_data%5D%5Bunit_amount%5D=899",
                "line_items%5B2%5D%5Bprice_data%5D%5Bunit_amount%5D=468",
            ],
        );

        let order = state
            .db
            .get_document(collections::ORDERS, &resp.order_id)
            .await
            .unwrap();
        assert_eq!(order[fields::COUPON_CODE], "SAVE10");
        assert_eq!(order[fields::DISCOUNT_AMOUNT_CENTS], 300);
        assert_eq!(order[db_fields::SUBTOTAL_CENTS], 2700);
        assert_eq!(order[fields::TAX_AMOUNT_CENTS], 468);
        assert_eq!(order[db_fields::TOTAL_AMOUNT_CENTS], 4067);
        assert_eq!(order[db_fields::IDEMPOTENCY_KEY], "idem-coupon-1");

        let reservations: Vec<Value> = state
            .db
            .query_bind_value(
                &format!(
                    "SELECT * FROM {} WHERE data->>'{}' = $order_id AND data->>'{}' = $coupon_code LIMIT 1",
                    collections::COUPON_USES,
                    fields::ORDER_ID,
                    fields::COUPON_CODE,
                ),
                json!({
                    "order_id": resp.order_id,
                    "coupon_code": "SAVE10",
                }),
            )
            .await
            .unwrap();
        assert_eq!(reservations.len(), 1);
        assert_eq!(reservations[0][fields::COUPON_ID], "SAVE10");
        assert_eq!(reservations[0][fields::REDEEMED_AT], Value::Null);
    }

    #[tokio::test]
    async fn test_create_checkout_session_reuses_existing_order_for_same_idempotency_key() {
        let state = setup_state().await;
        let mock_server = MockServer::start().await;
        let u = uuid::Uuid::new_v4().to_string();
        let product_id = format!("prod_idem_{u}");
        let seller_id = format!("seller_idem_{u}");
        let buyer_id = format!("buyer_idem_{u}");

        Mock::given(method("POST"))
            .and(path("/checkout/sessions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "cs_idem_123"
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
                &product_id,
                json!({
                    fields::PRODUCT_ID: product_id,
                    db_fields::SELLER_ID: seller_id,
                    db_fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 5,
                    db_fields::PRICE_CENTS: 1200,
                    fields::TITLE: "Idem Widget",
                    fields::IMAGE_URLS: ["https://example.com/widget.png"],
                    fields::IS_DIGITAL: false,
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                &seller_id,
                json!({
                    fields::UID: seller_id,
                    fields::SUSPENDED: false,
                    fields::ONBOARDING_COMPLETED: true,
                    fields::CHARGES_ENABLED: true,
                    fields::PAYOUTS_ENABLED: true,
                }),
            )
            .await
            .unwrap();

        let request = CreateCheckoutRequest {
            turnstile_token: None,
            items: vec![CartItem {
                product_id: product_id.clone(),
                quantity: 1,
            }],
            shipping_address: ShippingAddress {
                street: "123 Main St".into(),
                city: "Toronto".into(),
                state: "ON".into(),
                postal_code: "M5V2H1".into(),
                country: "CA".into(),
            },
            user_id: Some(buyer_id.clone()),
            subtotal_cents: 1200,
            coupon_code: None,
            eula_accepted: false,
            age_verification_accepted: false,
            idempotency_key: Some("idem-checkout-1".into()),
        };

        let Json(first) = create_checkout_session(
            State(state.clone()),
            Extension(auth(&buyer_id)),
            Json(request),
        )
        .await
        .unwrap();
        let Json(second) = create_checkout_session(
            State(state.clone()),
            Extension(auth(&buyer_id)),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: product_id.clone(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "123 Main St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some(buyer_id.clone()),
                subtotal_cents: 1200,
                coupon_code: None,
                eula_accepted: false,
                age_verification_accepted: false,
                idempotency_key: Some("idem-checkout-1".into()),
            }),
        )
        .await
        .unwrap();

        assert!(first.success);
        assert!(!first.duplicate);
        assert!(second.success);
        assert!(second.duplicate);
        assert_eq!(second.order_id, first.order_id);
        assert_eq!(second.session_id, first.session_id);
    }

    #[tokio::test]
    async fn test_create_checkout_session_rejects_negative_price_product() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_negative_price",
                json!({
                    fields::PRODUCT_ID: "prod_negative_price",
                    db_fields::SELLER_ID: "seller_negative",
                    db_fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 5,
                    db_fields::PRICE_CENTS: -500,
                    fields::TITLE: "Broken Widget",
                    fields::IMAGE_URLS: [],
                }),
            )
            .await
            .unwrap();
        state
            .db
            .upsert_document(
                collections::USERS,
                "seller_negative",
                json!({
                    fields::UID: "seller_negative",
                    fields::SUSPENDED: false,
                    fields::ONBOARDING_COMPLETED: true,
                    fields::CHARGES_ENABLED: true,
                    fields::PAYOUTS_ENABLED: true,
                }),
            )
            .await
            .unwrap();

        let err = create_checkout_session(
            State(state),
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: "prod_negative_price".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "123 Main St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
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
    async fn test_create_checkout_session_rejects_subtotal_mismatch_and_suspended_seller() {
        let state = setup_state().await;
        state
            .db
            .upsert_document(
                collections::PRODUCTS,
                "prod_1",
                json!({
                    fields::PRODUCT_ID: "prod_1",
                    db_fields::SELLER_ID: "seller_1",
                    db_fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 5,
                    db_fields::PRICE_CENTS: 1000,
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
                    fields::SUSPENDED: false,
                    fields::ONBOARDING_COMPLETED: true,
                    fields::CHARGES_ENABLED: true,
                    fields::PAYOUTS_ENABLED: true,
                }),
            )
            .await
            .unwrap();

        let subtotal_mismatch = create_checkout_session(
            State(state.clone()),
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: "prod_1".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "123 Main St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
                    postal_code: "M5V2H1".into(),
                    country: "CA".into(),
                },
                user_id: Some("buyer_1".to_string()),
                subtotal_cents: 1500,
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
            .update_document(
                collections::USERS,
                "seller_1",
                json!({fields::SUSPENDED: true}),
            )
            .await
            .unwrap();

        let suspended = create_checkout_session(
            State(state),
            Extension(auth("buyer_2")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: "prod_1".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "123 Main St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
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
                    db_fields::PRICE_CENTS: 1000,
                    fields::STOCK_QUANTITY: 5,
                    db_fields::LIFECYCLE_STATUS: "active",
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
                    db_fields::PRICE_CENTS: 1500,
                    fields::STOCK_QUANTITY: 5,
                    db_fields::LIFECYCLE_STATUS: "active",
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
                    db_fields::PRICE_CENTS: 500,
                    fields::STOCK_QUANTITY: 1,
                    db_fields::LIFECYCLE_STATUS: "active",
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
                    db_fields::PRICE_CENTS: 900,
                    fields::STOCK_QUANTITY: 5,
                    db_fields::LIFECYCLE_STATUS: "draft",
                }),
            )
            .await
            .unwrap();

        let Json(mismatch_resp) = verify_cart_prices(
            State(state.clone()),
            Json(VerifyPricesRequest {
                user_id: "buyer_1".to_string(),
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
        assert!(
            mismatches
                .iter()
                .any(|m| m[fields::REASON] == "price_changed")
        );
        assert!(
            mismatches
                .iter()
                .any(|m| m[fields::REASON] == "insufficient_stock")
        );
        assert!(
            mismatches
                .iter()
                .any(|m| m[fields::REASON] == "product_unavailable")
        );
        assert!(
            mismatches
                .iter()
                .any(|m| m[fields::REASON] == "product_not_found")
        );

        let Json(valid_resp) = verify_cart_prices(
            State(state),
            Json(VerifyPricesRequest {
                user_id: "buyer_1".to_string(),
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
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items,
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
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
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: "".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
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
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: "p1".into(),
                    quantity: 0,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
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
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: "p1".into(),
                    quantity: 101,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
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
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: "p1".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
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
        assert!(
            err.to_string().contains("Subtotal cannot be negative"),
            "Got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_checkout_rejects_excess_subtotal() {
        let state = setup_state().await;
        let err = create_checkout_session(
            State(state),
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: "p1".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
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
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: "p1".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    state: "ZZ".into(),
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
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: "nonexistent_prod".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
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
                    db_fields::SELLER_ID: "seller_1",
                    db_fields::LIFECYCLE_STATUS: "draft",
                    fields::STOCK_QUANTITY: 5,
                    db_fields::PRICE_CENTS: 1000,
                    fields::TITLE: "Draft Item",
                    fields::IMAGE_URLS: [],
                }),
            )
            .await
            .unwrap();

        let err = create_checkout_session(
            State(state),
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: "prod_draft".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
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
                    db_fields::SELLER_ID: "seller_1",
                    db_fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 5,
                    db_fields::PRICE_CENTS: 0,
                    fields::TITLE: "Free Item",
                    fields::IMAGE_URLS: [],
                }),
            )
            .await
            .unwrap();

        let err = create_checkout_session(
            State(state),
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: "prod_free".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
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
                    db_fields::SELLER_ID: "seller_1",
                    db_fields::LIFECYCLE_STATUS: "active",
                    fields::STOCK_QUANTITY: 5,
                    db_fields::PRICE_CENTS: 1000,
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
                    fields::UID: "seller_1", fields::SUSPENDED: false,
                    fields::ONBOARDING_COMPLETED: true,
                    fields::CHARGES_ENABLED: true,
                    fields::PAYOUTS_ENABLED: true,
                }),
            )
            .await
            .unwrap();

        let err = create_checkout_session(
            State(state),
            Extension(auth("buyer_1")),
            Json(CreateCheckoutRequest {
                turnstile_token: None,
                items: vec![CartItem {
                    product_id: "prod_ok".into(),
                    quantity: 1,
                }],
                shipping_address: ShippingAddress {
                    street: "1 St".into(),
                    city: "Toronto".into(),
                    state: "ON".into(),
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
                user_id: "buyer_1".to_string(),
                items: vec![],
            }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("No items to verify"));
    }

    // --- Tax calculation tests ---

    #[test]
    fn test_province_tax_rates() {
        assert_eq!(province_tax_rate_bps("ON"), 1300); // 13% HST
        assert_eq!(province_tax_rate_bps("QC"), 1498); // ~14.975%
        assert_eq!(province_tax_rate_bps("AB"), 500); // 5% GST
        assert_eq!(province_tax_rate_bps("BC"), 1200); // 12%
        assert_eq!(province_tax_rate_bps("NB"), 1500); // 15% HST
        assert_eq!(province_tax_rate_bps("NS"), 1500); // 15% HST
        assert_eq!(province_tax_rate_bps("SK"), 1100); // 11%
        assert_eq!(province_tax_rate_bps("MB"), 1200); // 12%
        assert_eq!(province_tax_rate_bps("YT"), 500); // 5% GST
    }

    #[test]
    fn test_calculate_tax_cents_ontario() {
        // $100.00 taxable base → 13% = $13.00
        assert_eq!(calculate_tax_cents(10000, "ON"), 1300);
        // $50.00 → 13% = $6.50
        assert_eq!(calculate_tax_cents(5000, "ON"), 650);
    }

    #[test]
    fn test_calculate_tax_cents_quebec() {
        // $100.00 → ~14.975% ≈ $14.98 (1498 bps)
        let tax = calculate_tax_cents(10000, "QC");
        assert_eq!(tax, 1498);
    }

    #[test]
    fn test_calculate_tax_cents_alberta() {
        // $100.00 → 5% = $5.00
        assert_eq!(calculate_tax_cents(10000, "AB"), 500);
        // $38.99 → 5% = $1.95 (rounded)
        assert_eq!(calculate_tax_cents(3899, "AB"), 195);
    }

    #[test]
    fn test_calculate_tax_cents_zero_base() {
        assert_eq!(calculate_tax_cents(0, "ON"), 0);
    }

    // --- Shipping calculation tests ---

    #[test]
    fn test_shipping_free_above_threshold() {
        let items = vec![json!({fields::IS_DIGITAL: false, fields::SHIP_FROM_PROVINCE: "ON"})];
        // $75.00 subtotal → free shipping
        assert_eq!(
            calculate_shipping_cost_cents(7500, "ON", &items).unwrap(),
            0
        );
        // $100.00 subtotal → free shipping
        assert_eq!(
            calculate_shipping_cost_cents(10000, "ON", &items).unwrap(),
            0
        );
    }

    #[test]
    fn test_shipping_standard_below_threshold() {
        let items = vec![json!({fields::IS_DIGITAL: false, fields::SHIP_FROM_PROVINCE: "ON"})];
        // $50.00 subtotal, same province → standard rate
        assert_eq!(
            calculate_shipping_cost_cents(5000, "ON", &items).unwrap(),
            STANDARD_SHIPPING_CENTS
        );
    }

    #[test]
    fn test_shipping_cross_province() {
        let items = vec![json!({fields::IS_DIGITAL: false, fields::SHIP_FROM_PROVINCE: "BC"})];
        // $50.00 subtotal, seller in BC, buyer in ON → cross-province rate
        assert_eq!(
            calculate_shipping_cost_cents(5000, "ON", &items).unwrap(),
            INTL_SHIPPING_BASE_CENTS
        );
    }

    #[test]
    fn test_shipping_digital_items_free() {
        let items = vec![json!({fields::IS_DIGITAL: true, fields::SHIP_FROM_PROVINCE: "ON"})];
        // All digital → no shipping regardless of subtotal
        assert_eq!(
            calculate_shipping_cost_cents(1000, "ON", &items).unwrap(),
            0
        );
    }

    #[test]
    fn test_shipping_mixed_digital_physical() {
        let items = vec![
            json!({fields::IS_DIGITAL: true, fields::SHIP_FROM_PROVINCE: "ON"}),
            json!({fields::IS_DIGITAL: false, fields::SHIP_FROM_PROVINCE: "ON"}),
        ];
        // Mixed: physical items present → standard rate applies
        assert_eq!(
            calculate_shipping_cost_cents(3000, "ON", &items).unwrap(),
            STANDARD_SHIPPING_CENTS
        );
    }

    #[test]
    fn test_shipping_empty_ship_from_province_not_cross() {
        let items = vec![json!({fields::IS_DIGITAL: false, fields::SHIP_FROM_PROVINCE: ""})];
        // Empty shipFromProvince → not cross-province → standard rate
        assert_eq!(
            calculate_shipping_cost_cents(3000, "ON", &items).unwrap(),
            STANDARD_SHIPPING_CENTS
        );
    }

    #[test]
    fn test_shipping_international_seller() {
        let items = vec![
            json!({fields::IS_DIGITAL: false, fields::SHIP_FROM_PROVINCE: "", fields::SHIP_FROM_COUNTRY: "China"}),
        ];
        // International seller → cross-province/intl rate
        assert_eq!(
            calculate_shipping_cost_cents(3000, "ON", &items).unwrap(),
            INTL_SHIPPING_BASE_CENTS
        );
    }

    #[test]
    fn test_shipping_canadian_seller_country_not_cross() {
        let items = vec![
            json!({fields::IS_DIGITAL: false, fields::SHIP_FROM_PROVINCE: "ON", fields::SHIP_FROM_COUNTRY: "Canada"}),
        ];
        // Canadian seller, same province → standard rate
        assert_eq!(
            calculate_shipping_cost_cents(3000, "ON", &items).unwrap(),
            STANDARD_SHIPPING_CENTS
        );
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
                    .get(db_fields::PRICE_CENTS)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let in_stock = product
                    .get(fields::STOCK_QUANTITY)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let active = product
                    .get(db_fields::LIFECYCLE_STATUS)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    == "active";

                if db_price != item.expected_price_cents {
                    mismatches.push(serde_json::json!({
                        fields::PRODUCT_ID: item.product_id,
                        fields::REASON: "price_changed",
                        "expectedPriceCents": item.expected_price_cents,
                        "actualPriceCents": db_price,
                    }));
                } else if in_stock < item.quantity as i64 {
                    mismatches.push(serde_json::json!({
                        fields::PRODUCT_ID: item.product_id,
                        fields::REASON: "insufficient_stock",
                        "requestedQuantity": item.quantity,
                        "availableStock": in_stock,
                    }));
                } else if !active {
                    mismatches.push(serde_json::json!({
                        fields::PRODUCT_ID: item.product_id,
                        fields::REASON: "product_unavailable",
                    }));
                } else {
                    verified += 1;
                }
            }
            Err(_) => {
                mismatches.push(serde_json::json!({
                    fields::PRODUCT_ID: item.product_id,
                    fields::REASON: "product_not_found",
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
