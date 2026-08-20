use serde_json::{Value, json};
use std::collections::HashSet;
use tracing::warn;

use crate::HandlersState;
use crate::shared::schema::{collections, fields};
use ob_database::fields as db_fields;

use super::{
    OrderItem, OrderSummary, order_confirmation_html, seller_notification_html, send_email,
    shipping_notification_html,
};

pub fn str_field<'a>(value: &'a Value, field: &str) -> &'a str {
    value.get(field).and_then(|v| v.as_str()).unwrap_or("")
}

pub fn record_key(record_id: &str) -> &str {
    record_id.rsplit(':').next().unwrap_or(record_id)
}

pub fn normalize_lang(lang: &str) -> &str {
    if lang.eq_ignore_ascii_case("fr") {
        "fr"
    } else {
        "en"
    }
}

pub fn order_items(order: &Value) -> Vec<Value> {
    order
        .get(fields::ITEMS)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

pub fn item_quantity(item: &Value) -> u32 {
    item.get(fields::QUANTITY)
        .and_then(|v| v.as_u64())
        .map(|v| v.min(u32::MAX as u64) as u32)
        .or_else(|| {
            item.get(fields::QUANTITY)
                .and_then(|v| v.as_i64())
                .map(|v| v.max(0).min(u32::MAX as i64) as u32)
        })
        .unwrap_or(1)
}

pub fn item_price_cents(item: &Value) -> i64 {
    item.get(db_fields::PRICE_CENTS)
        .and_then(|v| v.as_i64())
        .or_else(|| item.get(fields::PRICE).and_then(|v| v.as_i64()))
        .unwrap_or(0)
}

pub fn item_name(item: &Value) -> String {
    item.get(db_fields::NAME)
        .or_else(|| item.get(fields::TITLE))
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or("Item")
        .to_string()
}

pub fn build_order_summary_from_items(order: &Value, items: &[Value]) -> OrderSummary {
    let order_id = order
        .get(db_fields::ID)
        .and_then(|v| v.as_str())
        .map(record_key)
        .unwrap_or_else(|| record_key(str_field(order, fields::ORDER_ID)))
        .to_string();

    let fallback_subtotal = items
        .iter()
        .map(|item| item_price_cents(item) * i64::from(item_quantity(item)))
        .sum::<i64>();

    let summary_items = items
        .iter()
        .map(|item| OrderItem {
            name: item_name(item),
            quantity: item_quantity(item),
            price_cents: item_price_cents(item),
        })
        .collect();

    OrderSummary {
        order_id,
        items: summary_items,
        subtotal_cents: order
            .get(db_fields::SUBTOTAL_CENTS)
            .and_then(|v| v.as_i64())
            .unwrap_or(fallback_subtotal),
        shipping_cost_cents: order
            .get(fields::SHIPPING_COST_CENTS)
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        tax_amount_cents: order
            .get(fields::TAX_AMOUNT_CENTS)
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        total_amount_cents: order
            .get(db_fields::TOTAL_AMOUNT_CENTS)
            .and_then(|v| v.as_i64())
            .unwrap_or(fallback_subtotal),
    }
}

pub fn build_order_summary(order: &Value) -> OrderSummary {
    let items = order_items(order);
    build_order_summary_from_items(order, &items)
}

pub fn build_seller_order_summary(order: &Value, seller_id: &str) -> Option<OrderSummary> {
    let seller_items: Vec<Value> = order_items(order)
        .into_iter()
        .filter(|item| str_field(item, db_fields::SELLER_ID) == seller_id)
        .collect();
    if seller_items.is_empty() {
        return None;
    }

    let subtotal = seller_items
        .iter()
        .map(|item| item_price_cents(item) * i64::from(item_quantity(item)))
        .sum::<i64>();
    let mut summary = build_order_summary_from_items(order, &seller_items);
    summary.subtotal_cents = subtotal;
    summary.shipping_cost_cents = 0;
    summary.tax_amount_cents = 0;
    summary.total_amount_cents = subtotal;
    Some(summary)
}

fn order_buyer_id(order: &Value) -> &str {
    order
        .get(db_fields::BUYER_ID)
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .or_else(|| order.get(db_fields::USER_ID).and_then(|v| v.as_str()))
        .unwrap_or("")
}

pub async fn load_user_document(state: &HandlersState, user_id: &str) -> Option<Value> {
    if user_id.is_empty() {
        return None;
    }
    state
        .db
        .get_document(collections::USERS, user_id)
        .await
        .ok()
}

pub async fn resolve_buyer_contact(
    state: &HandlersState,
    order: &Value,
) -> Option<(String, String, String)> {
    let buyer_doc = load_user_document(state, order_buyer_id(order)).await;
    let email = order
        .get(fields::CUSTOMER_EMAIL)
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            buyer_doc.as_ref().and_then(|doc| {
                doc.get(db_fields::EMAIL)
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .map(ToString::to_string)
            })
        })?;

    let buyer_name = buyer_doc
        .as_ref()
        .and_then(|doc| doc.get(db_fields::NAME).and_then(|v| v.as_str()))
        .filter(|v| !v.trim().is_empty())
        .unwrap_or("Customer")
        .to_string();
    let lang = order
        .get(fields::PREFERRED_LANGUAGE)
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            buyer_doc.as_ref().and_then(|doc| {
                doc.get(fields::LANGUAGE)
                    .or_else(|| doc.get(fields::PREFERRED_LANGUAGE))
                    .and_then(|v| v.as_str())
            })
        })
        .map(normalize_lang)
        .unwrap_or("en")
        .to_string();

    Some((email, buyer_name, lang))
}

pub async fn resolve_seller_contact(
    state: &HandlersState,
    seller_id: &str,
) -> Option<(String, String, String)> {
    if seller_id.is_empty() {
        return None;
    }

    let seller_profiles: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "SELECT * FROM {} WHERE data->>'uid' = $seller_id OR data->>'user_id' = $seller_id LIMIT 1",
                collections::SELLER_PROFILES
            ),
            json!({ "seller_id": seller_id }),
        )
        .await
        .unwrap_or_default();
    let seller_profile = seller_profiles.first();
    let user_doc = load_user_document(state, seller_id).await;

    let email = seller_profile
        .and_then(|profile| profile.get(db_fields::EMAIL).and_then(|v| v.as_str()))
        .filter(|v| !v.trim().is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            user_doc.as_ref().and_then(|doc| {
                doc.get(db_fields::EMAIL)
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .map(ToString::to_string)
            })
        })?;

    let seller_name = seller_profile
        .and_then(|profile| {
            profile
                .get(db_fields::NAME)
                .or_else(|| profile.get(fields::BUSINESS_NAME))
                .or_else(|| profile.get(fields::STORE_NAME))
                .or_else(|| profile.get(fields::DISPLAY_NAME))
                .and_then(|v| v.as_str())
        })
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            user_doc.as_ref().and_then(|doc| {
                doc.get(db_fields::NAME)
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
            })
        })
        .unwrap_or(seller_id)
        .to_string();

    let lang = user_doc
        .as_ref()
        .and_then(|doc| doc.get(fields::PREFERRED_LANGUAGE).and_then(|v| v.as_str()))
        .unwrap_or("en")
        .to_string();

    Some((email, seller_name, lang))
}

fn postal_api_key(state: &HandlersState, order_id: &str, log_message: &str) -> Option<String> {
    match state.config.require_secret("postal_api_key") {
        Ok(api_key) => Some(api_key.to_string()),
        Err(err) => {
            warn!(order_id = %order_id, error = %err, context = log_message, "Postal API key unavailable");
            None
        }
    }
}

pub async fn send_order_confirmation_emails(
    state: &HandlersState,
    order: &Value,
) -> Result<(), ob_core::Error> {
    let order_id = order
        .get(db_fields::ID)
        .and_then(|v| v.as_str())
        .map(record_key)
        .unwrap_or("");
    let Some(api_key) = postal_api_key(
        state,
        order_id,
        "Postal API key unavailable; skipping payment success emails",
    ) else {
        return Ok(());
    };

    let order_summary = build_order_summary(order);
    if let Some((buyer_email, buyer_name, lang)) = resolve_buyer_contact(state, order).await {
        let subject = if lang == "fr" {
            "Votre commande est confirmée — Origna"
        } else {
            "Your order is confirmed — Origna"
        };
        let html = order_confirmation_html(&order_summary, &buyer_name, &lang);
        if let Err(err) =
            send_email(&state.http_client, &api_key, &buyer_email, subject, &html).await
        {
            warn!(order_id = %order_id, to = %buyer_email, error = %err, "Failed to send buyer order confirmation email");
        }
    } else {
        warn!(order_id = %order_id, "Buyer email unavailable; skipping order confirmation email");
    }

    let mut seller_ids = HashSet::new();
    for item in order_items(order) {
        let seller_id = str_field(&item, db_fields::SELLER_ID);
        if !seller_id.is_empty() {
            seller_ids.insert(seller_id.to_string());
        }
    }

    for seller_id in seller_ids {
        let Some((seller_email, seller_name, seller_lang)) =
            resolve_seller_contact(state, &seller_id).await
        else {
            warn!(order_id = %order_id, seller_id = %seller_id, "Seller email unavailable; skipping seller notification email");
            continue;
        };
        let Some(seller_summary) = build_seller_order_summary(order, &seller_id) else {
            continue;
        };
        let subject = if seller_lang == "fr" {
            format!("[Origna] Nouvelle commande reçue #{order_id}")
        } else {
            format!("[Origna] New order received #{order_id}")
        };
        let html = seller_notification_html(&seller_summary, &seller_name, &seller_lang);
        if let Err(err) =
            send_email(&state.http_client, &api_key, &seller_email, &subject, &html).await
        {
            warn!(order_id = %order_id, seller_id = %seller_id, to = %seller_email, error = %err, "Failed to send seller order notification email");
        }
    }

    Ok(())
}

pub async fn send_shipping_notification(
    state: &HandlersState,
    order: &Value,
    tracking_number: &str,
    carrier: Option<&str>,
    buyer_lang: Option<&str>,
) {
    let order_id = order
        .get(db_fields::ID)
        .and_then(|v| v.as_str())
        .map(record_key)
        .unwrap_or("");
    let Some(api_key) = postal_api_key(
        state,
        order_id,
        "Postal API key unavailable; skipping order status email",
    ) else {
        return;
    };
    let Some((buyer_email, buyer_name, resolved_lang)) = resolve_buyer_contact(state, order).await
    else {
        warn!(order_id = %order_id, "Buyer email unavailable; skipping shipping email");
        return;
    };
    if tracking_number.trim().is_empty() {
        return;
    }

    let lang = buyer_lang
        .filter(|value| !value.trim().is_empty())
        .map(normalize_lang)
        .unwrap_or(&resolved_lang);
    let summary = build_order_summary(order);
    let subject = if lang == "fr" {
        format!("Commande #{} expédiée — Origna", summary.order_id)
    } else {
        format!("Order #{} shipped — Origna", summary.order_id)
    };
    let html = shipping_notification_html(&summary, &buyer_name, tracking_number, carrier, lang);
    if let Err(err) = send_email(&state.http_client, &api_key, &buyer_email, &subject, &html).await
    {
        warn!(order_id = %order_id, to = %buyer_email, error = %err, "Failed to send shipping notification email");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::schema::fields;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    static ENV_MUTEX: Mutex<()> = Mutex::const_new(());

    async fn make_test_state() -> HandlersState {
        let mut config = ob_core::Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("postal_api_key".into(), "test_api_key".into());
        let db = ob_database::DatabaseClient::new_mem().await;
        HandlersState::new(Arc::new(config), db)
    }

    async fn make_test_state_no_secrets() -> HandlersState {
        let config = ob_core::Config::load(None).unwrap();
        let db = ob_database::DatabaseClient::new_mem().await;
        HandlersState::new(Arc::new(config), db)
    }

    fn sample_order_with_items() -> Value {
        json!({
            "id": "orders:order1",
            db_fields::BUYER_ID: "buyer1",
            fields::CUSTOMER_EMAIL: "buyer@test.com",
            fields::PREFERRED_LANGUAGE: "en",
            fields::ITEMS: [
                {
                    db_fields::NAME: "Widget",
                    db_fields::PRICE_CENTS: 1500,
                    "quantity": 2,
                    db_fields::SELLER_ID: "seller1"
                },
                {
                    db_fields::NAME: "Gadget",
                    db_fields::PRICE_CENTS: 3000,
                    "quantity": 1,
                    db_fields::SELLER_ID: "seller2"
                }
            ],
            db_fields::SUBTOTAL_CENTS: 6000,
            fields::SHIPPING_COST_CENTS: 500,
            fields::TAX_AMOUNT_CENTS: 780,
            db_fields::TOTAL_AMOUNT_CENTS: 7280,
        })
    }

    fn sample_order_single_seller() -> Value {
        json!({
            "id": "orders:order2",
            db_fields::BUYER_ID: "buyer1",
            fields::CUSTOMER_EMAIL: "buyer@test.com",
            fields::PREFERRED_LANGUAGE: "fr",
            fields::ITEMS: [
                {
                    db_fields::NAME: "Produit",
                    db_fields::PRICE_CENTS: 2000,
                    "quantity": 1,
                    db_fields::SELLER_ID: "seller1"
                }
            ],
            db_fields::SUBTOTAL_CENTS: 2000,
            fields::SHIPPING_COST_CENTS: 0,
            fields::TAX_AMOUNT_CENTS: 260,
            db_fields::TOTAL_AMOUNT_CENTS: 2260,
        })
    }

    // --- str_field tests ---

    #[test]
    fn test_str_field_basic() {
        let val = json!({"name": "Alice"});
        assert_eq!(str_field(&val, "name"), "Alice");
    }

    #[test]
    fn test_str_field_missing() {
        let val = json!({});
        assert_eq!(str_field(&val, "missing"), "");
    }

    #[test]
    fn test_str_field_wrong_type() {
        let val = json!({"count": 42});
        assert_eq!(str_field(&val, "count"), "");
    }

    #[test]
    fn test_str_field_null_value() {
        let val = json!({"name": null});
        assert_eq!(str_field(&val, "name"), "");
    }

    // --- record_key tests ---

    #[test]
    fn test_record_key_with_prefix() {
        assert_eq!(record_key("users:abc123"), "abc123");
    }

    #[test]
    fn test_record_key_without_prefix() {
        assert_eq!(record_key("abc123"), "abc123");
    }

    #[test]
    fn test_record_key_multiple_colons() {
        assert_eq!(record_key("a:b:c"), "c");
    }

    #[test]
    fn test_record_key_empty() {
        assert_eq!(record_key(""), "");
    }

    // --- normalize_lang tests ---

    #[test]
    fn test_normalize_lang_fr() {
        assert_eq!(normalize_lang("fr"), "fr");
    }

    #[test]
    fn test_normalize_lang_fr_uppercase() {
        assert_eq!(normalize_lang("FR"), "fr");
    }

    #[test]
    fn test_normalize_lang_en() {
        assert_eq!(normalize_lang("en"), "en");
    }

    #[test]
    fn test_normalize_lang_unknown() {
        assert_eq!(normalize_lang("de"), "en");
    }

    #[test]
    fn test_normalize_lang_empty() {
        assert_eq!(normalize_lang(""), "en");
    }

    // --- order_items tests ---

    #[test]
    fn test_order_items_present() {
        let order = sample_order_with_items();
        let items = order_items(&order);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn test_order_items_missing() {
        let order = json!({"id": "test"});
        let items = order_items(&order);
        assert!(items.is_empty());
    }

    #[test]
    fn test_order_items_wrong_type() {
        let order = json!({"items": "not_an_array"});
        let items = order_items(&order);
        assert!(items.is_empty());
    }

    // --- item_quantity tests ---

    #[test]
    fn test_item_quantity_u64() {
        let item = json!({"quantity": 5u64});
        assert_eq!(item_quantity(&item), 5);
    }

    #[test]
    fn test_item_quantity_i64_positive() {
        let item = json!({"quantity": 3i64});
        assert_eq!(item_quantity(&item), 3);
    }

    #[test]
    fn test_item_quantity_i64_negative() {
        let item = json!({"quantity": -5i64});
        assert_eq!(item_quantity(&item), 0);
    }

    #[test]
    fn test_item_quantity_missing() {
        let item = json!({});
        assert_eq!(item_quantity(&item), 1);
    }

    #[test]
    fn test_item_quantity_zero() {
        let item = json!({"quantity": 0u64});
        assert_eq!(item_quantity(&item), 0);
    }

    #[test]
    fn test_item_quantity_string_type() {
        let item = json!({"quantity": "three"});
        assert_eq!(item_quantity(&item), 1);
    }

    // --- item_price_cents tests ---

    #[test]
    fn test_item_price_cents_from_price_cents() {
        let item = json!({db_fields::PRICE_CENTS: 2500});
        assert_eq!(item_price_cents(&item), 2500);
    }

    #[test]
    fn test_item_price_cents_from_price() {
        let item = json!({"price": 1500});
        assert_eq!(item_price_cents(&item), 1500);
    }

    #[test]
    fn test_item_price_cents_prefer_price_cents() {
        let item = json!({db_fields::PRICE_CENTS: 2500, "price": 1500});
        assert_eq!(item_price_cents(&item), 2500);
    }

    #[test]
    fn test_item_price_cents_missing() {
        let item = json!({});
        assert_eq!(item_price_cents(&item), 0);
    }

    // --- item_name tests ---

    #[test]
    fn test_item_name_from_name() {
        let item = json!({db_fields::NAME: "Widget"});
        assert_eq!(item_name(&item), "Widget");
    }

    #[test]
    fn test_item_name_from_title() {
        let item = json!({fields::TITLE: "Title Widget"});
        assert_eq!(item_name(&item), "Title Widget");
    }

    #[test]
    fn test_item_name_prefer_name_over_title() {
        // db_fields::NAME and fields::TITLE both map to "name" now
        let item = json!({db_fields::NAME: "Product Name"});
        assert_eq!(item_name(&item), "Product Name");
    }

    #[test]
    fn test_item_name_missing() {
        let item = json!({});
        assert_eq!(item_name(&item), "Item");
    }

    #[test]
    fn test_item_name_empty_string() {
        let item = json!({db_fields::NAME: "  "});
        assert_eq!(item_name(&item), "Item");
    }

    // --- build_order_summary tests ---

    #[test]
    fn test_build_order_summary_full() {
        let order = sample_order_with_items();
        let summary = build_order_summary(&order);
        assert_eq!(summary.order_id, "order1");
        assert_eq!(summary.items.len(), 2);
        assert_eq!(summary.subtotal_cents, 6000);
        assert_eq!(summary.shipping_cost_cents, 500);
        assert_eq!(summary.tax_amount_cents, 780);
        assert_eq!(summary.total_amount_cents, 7280);
    }

    #[test]
    fn test_build_order_summary_no_items() {
        let order = json!({
            "id": "orders:empty",
            db_fields::SUBTOTAL_CENTS: 0,
            db_fields::TOTAL_AMOUNT_CENTS: 0,
        });
        let summary = build_order_summary(&order);
        assert!(summary.items.is_empty());
        assert_eq!(summary.subtotal_cents, 0);
        assert_eq!(summary.total_amount_cents, 0);
    }

    #[test]
    fn test_build_order_summary_fallback_subtotal() {
        let order = json!({
            "id": "orders:fb",
            fields::ITEMS: [
                {db_fields::PRICE_CENTS: 1000, "quantity": 2}
            ]
        });
        let summary = build_order_summary(&order);
        assert_eq!(summary.subtotal_cents, 2000);
        assert_eq!(summary.total_amount_cents, 2000);
    }

    #[test]
    fn test_build_order_summary_order_id_from_field() {
        let order = json!({
            fields::ORDER_ID: "orders:from_field",
        });
        let summary = build_order_summary(&order);
        assert_eq!(summary.order_id, "from_field");
    }

    #[test]
    fn test_build_order_summary_no_id() {
        let order = json!({});
        let summary = build_order_summary(&order);
        assert_eq!(summary.order_id, "");
    }

    // --- build_seller_order_summary tests ---

    #[test]
    fn test_build_seller_order_summary_found() {
        let order = sample_order_with_items();
        let summary = build_seller_order_summary(&order, "seller1");
        assert!(summary.is_some());
        let s = summary.unwrap();
        assert_eq!(s.items.len(), 1);
        assert_eq!(s.items[0].name, "Widget");
        assert_eq!(s.subtotal_cents, 3000);
        assert_eq!(s.total_amount_cents, 3000);
        assert_eq!(s.shipping_cost_cents, 0);
        assert_eq!(s.tax_amount_cents, 0);
    }

    #[test]
    fn test_build_seller_order_summary_not_found() {
        let order = sample_order_with_items();
        let summary = build_seller_order_summary(&order, "nonexistent");
        assert!(summary.is_none());
    }

    #[test]
    fn test_build_seller_order_summary_no_items() {
        let order = json!({"id": "orders:empty"});
        let summary = build_seller_order_summary(&order, "seller1");
        assert!(summary.is_none());
    }

    #[test]
    fn test_build_seller_order_summary_multiple_items_same_seller() {
        let order = json!({
            "id": "orders:multi",
            fields::ITEMS: [
                {db_fields::NAME: "A", db_fields::PRICE_CENTS: 1000, "quantity": 1, db_fields::SELLER_ID: "s1"},
                {db_fields::NAME: "B", db_fields::PRICE_CENTS: 2000, "quantity": 1, db_fields::SELLER_ID: "s1"},
                {db_fields::NAME: "C", db_fields::PRICE_CENTS: 500, "quantity": 1, db_fields::SELLER_ID: "s2"}
            ]
        });
        let summary = build_seller_order_summary(&order, "s1");
        assert!(summary.is_some());
        let s = summary.unwrap();
        assert_eq!(s.items.len(), 2);
        assert_eq!(s.subtotal_cents, 3000);
    }

    // --- build_order_summary_from_items tests ---

    #[test]
    fn test_build_order_summary_from_items_explicit() {
        let order = json!({
            "id": "orders:explicit",
            db_fields::SUBTOTAL_CENTS: 5000,
            fields::SHIPPING_COST_CENTS: 800,
            fields::TAX_AMOUNT_CENTS: 650,
            db_fields::TOTAL_AMOUNT_CENTS: 6450,
        });
        let items =
            vec![json!({db_fields::NAME: "Item1", db_fields::PRICE_CENTS: 5000, "quantity": 1})];
        let summary = build_order_summary_from_items(&order, &items);
        assert_eq!(summary.subtotal_cents, 5000);
        assert_eq!(summary.shipping_cost_cents, 800);
        assert_eq!(summary.tax_amount_cents, 650);
        assert_eq!(summary.total_amount_cents, 6450);
    }

    // --- async: load_user_document ---

    #[tokio::test]
    async fn test_load_user_document_empty_id() {
        let state = make_test_state().await;
        let result = load_user_document(&state, "").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_load_user_document_not_found() {
        let state = make_test_state().await;
        let result = load_user_document(&state, "nonexistent").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_load_user_document_found() {
        let state = make_test_state().await;
        state
            .db
            .create_document(
                "users",
                json!({
                    "id": "found_user",
                    db_fields::EMAIL: "found@test.com",
                    db_fields::NAME: "Found User",
                    fields::LANGUAGE: "en",
                }),
            )
            .await
            .unwrap();

        let result = load_user_document(&state, "found_user").await;
        assert!(result.is_some());
        let doc = result.unwrap();
        assert_eq!(doc[db_fields::EMAIL], "found@test.com");
    }

    #[tokio::test]
    async fn test_load_user_document_with_collection_prefix() {
        let state = make_test_state().await;
        state
            .db
            .create_document(
                "users",
                json!({
                    "id": "prefixed",
                    db_fields::EMAIL: "prefixed@test.com",
                }),
            )
            .await
            .unwrap();

        let result = load_user_document(&state, "users:prefixed").await;
        assert!(result.is_some());
    }

    // --- async: resolve_buyer_contact ---

    #[tokio::test]
    async fn test_resolve_buyer_contact_from_order_email() {
        let state = make_test_state().await;
        let order = json!({
            "id": "orders:oc1",
            db_fields::BUYER_ID: "buyer_nodoc",
            fields::CUSTOMER_EMAIL: "order@test.com",
            fields::PREFERRED_LANGUAGE: "fr",
        });
        let result = resolve_buyer_contact(&state, &order).await;
        assert!(result.is_some());
        let (email, name, lang) = result.unwrap();
        assert_eq!(email, "order@test.com");
        assert_eq!(name, "Customer");
        assert_eq!(lang, "fr");
    }

    #[tokio::test]
    async fn test_resolve_buyer_contact_from_user_doc() {
        let state = make_test_state().await;
        state
            .db
            .create_document(
                "users",
                json!({
                    "id": "buyer2",
                    db_fields::EMAIL: "buyer2@test.com",
                    db_fields::NAME: "Bob",
                    fields::LANGUAGE: "en",
                }),
            )
            .await
            .unwrap();

        let order = json!({
            "id": "orders:oc2",
            db_fields::BUYER_ID: "buyer2",
            fields::CUSTOMER_EMAIL: "",
            fields::PREFERRED_LANGUAGE: "",
        });
        let result = resolve_buyer_contact(&state, &order).await;
        assert!(result.is_some());
        let (email, name, _lang) = result.unwrap();
        assert_eq!(email, "buyer2@test.com");
        assert_eq!(name, "Bob");
    }

    #[tokio::test]
    async fn test_resolve_buyer_contact_no_email() {
        let state = make_test_state().await;
        let order = json!({
            "id": "orders:oc3",
            db_fields::BUYER_ID: "nobody",
            fields::CUSTOMER_EMAIL: "",
        });
        let result = resolve_buyer_contact(&state, &order).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_resolve_buyer_contact_uses_user_id_field() {
        let state = make_test_state().await;
        state
            .db
            .create_document(
                "users",
                json!({
                    "id": "uid_buyer",
                    db_fields::EMAIL: "uid@test.com",
                }),
            )
            .await
            .unwrap();

        let order = json!({
            "id": "orders:oc4",
            "userId": "uid_buyer",
            fields::CUSTOMER_EMAIL: "",
        });
        let result = resolve_buyer_contact(&state, &order).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, "uid@test.com");
    }

    #[tokio::test]
    async fn test_resolve_buyer_contact_lang_from_user_doc() {
        let state = make_test_state().await;
        state
            .db
            .create_document(
                "users",
                json!({
                    "id": "lang_buyer",
                    db_fields::EMAIL: "lang@test.com",
                    fields::PREFERRED_LANGUAGE: "fr",
                }),
            )
            .await
            .unwrap();

        let order = json!({
            "id": "orders:oc5",
            db_fields::BUYER_ID: "lang_buyer",
            fields::CUSTOMER_EMAIL: "lang@test.com",
        });
        let result = resolve_buyer_contact(&state, &order).await;
        assert!(result.is_some());
        let (_, _, lang) = result.unwrap();
        assert_eq!(lang, "fr");
    }

    // --- async: resolve_seller_contact ---

    #[tokio::test]
    async fn test_resolve_seller_contact_empty_id() {
        let state = make_test_state().await;
        let result = resolve_seller_contact(&state, "").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_resolve_seller_contact_from_user_doc() {
        let state = make_test_state().await;
        state
            .db
            .create_document(
                "users",
                json!({
                    "id": "seller2",
                    db_fields::EMAIL: "seller2@test.com",
                    db_fields::NAME: "Seller Two",
                }),
            )
            .await
            .unwrap();

        let result = resolve_seller_contact(&state, "seller2").await;
        assert!(result.is_some());
        let (email, name, lang) = result.unwrap();
        assert_eq!(email, "seller2@test.com");
        assert_eq!(name, "Seller Two");
        assert_eq!(lang, "en"); // default when preferredLanguage not set
    }

    #[tokio::test]
    async fn test_resolve_seller_contact_not_found() {
        let state = make_test_state().await;
        let result = resolve_seller_contact(&state, "nobody").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_resolve_seller_contact_user_no_email() {
        let state = make_test_state().await;
        state
            .db
            .create_document(
                "users",
                json!({
                    "id": "seller_noemail",
                    db_fields::NAME: "No Email Seller",
                }),
            )
            .await
            .unwrap();

        let result = resolve_seller_contact(&state, "seller_noemail").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_resolve_seller_contact_user_fallback_name() {
        let state = make_test_state().await;
        state
            .db
            .create_document(
                "users",
                json!({
                    "id": "seller_fn",
                    db_fields::EMAIL: "fn@test.com",
                }),
            )
            .await
            .unwrap();

        let result = resolve_seller_contact(&state, "seller_fn").await;
        assert!(result.is_some());
        let (_, name, _) = result.unwrap();
        assert_eq!(name, "seller_fn");
    }

    // --- async: send_order_confirmation_emails ---

    #[tokio::test]
    async fn test_send_order_confirmation_emails_no_credentials() {
        let state = make_test_state_no_secrets().await;
        let order = sample_order_with_items();
        let result = send_order_confirmation_emails(&state, &order).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_order_confirmation_emails_success() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let _lock = ENV_MUTEX.lock().await;
        let server = MockServer::start().await;
        unsafe { std::env::set_var("POSTAL_API_URL", server.uri()) };

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"success"})))
            .mount(&server)
            .await;

        let state = make_test_state().await;
        state
            .db
            .create_document(
                "users",
                json!({
                    "id": "buyer1",
                    db_fields::EMAIL: "buyer@test.com",
                    db_fields::NAME: "Test Buyer",
                    fields::LANGUAGE: "en",
                }),
            )
            .await
            .unwrap();
        state
            .db
            .create_document(
                "users",
                json!({
                    "id": "seller1",
                    db_fields::EMAIL: "seller1@test.com",
                    db_fields::NAME: "Seller One",
                }),
            )
            .await
            .unwrap();

        let order = sample_order_single_seller();
        let result = send_order_confirmation_emails(&state, &order).await;
        assert!(result.is_ok());

        unsafe { std::env::remove_var("POSTAL_API_URL") };
    }

    #[tokio::test]
    async fn test_send_order_confirmation_emails_no_buyer_email() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let _lock = ENV_MUTEX.lock().await;
        let server = MockServer::start().await;
        unsafe { std::env::set_var("POSTAL_API_URL", server.uri()) };

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"success"})))
            .mount(&server)
            .await;

        let state = make_test_state().await;
        let order = json!({
            "id": "orders:no_buyer",
            db_fields::BUYER_ID: "nobody",
            fields::CUSTOMER_EMAIL: "",
            fields::ITEMS: [
                {db_fields::NAME: "X", db_fields::PRICE_CENTS: 100, "quantity": 1, db_fields::SELLER_ID: "s1"}
            ],
            db_fields::SUBTOTAL_CENTS: 100,
            db_fields::TOTAL_AMOUNT_CENTS: 100,
        });
        let result = send_order_confirmation_emails(&state, &order).await;
        assert!(result.is_ok());

        unsafe { std::env::remove_var("POSTAL_API_URL") };
    }

    #[tokio::test]
    async fn test_send_order_confirmation_emails_seller_no_email() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let _lock = ENV_MUTEX.lock().await;
        let server = MockServer::start().await;
        unsafe { std::env::set_var("POSTAL_API_URL", server.uri()) };

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"success"})))
            .mount(&server)
            .await;

        let state = make_test_state().await;
        state
            .db
            .create_document(
                "users",
                json!({
                    "id": "buyer1",
                    db_fields::EMAIL: "buyer@test.com",
                    db_fields::NAME: "Test Buyer",
                }),
            )
            .await
            .unwrap();

        let order = json!({
            "id": "orders:no_seller_email",
            db_fields::BUYER_ID: "buyer1",
            fields::CUSTOMER_EMAIL: "buyer@test.com",
            fields::PREFERRED_LANGUAGE: "en",
            fields::ITEMS: [
                {db_fields::NAME: "X", db_fields::PRICE_CENTS: 100, "quantity": 1, db_fields::SELLER_ID: "no_seller"}
            ],
            db_fields::SUBTOTAL_CENTS: 100,
            db_fields::TOTAL_AMOUNT_CENTS: 100,
        });
        let result = send_order_confirmation_emails(&state, &order).await;
        assert!(result.is_ok());

        unsafe { std::env::remove_var("POSTAL_API_URL") };
    }

    #[tokio::test]
    async fn test_send_order_confirmation_emails_no_items() {
        let state = make_test_state().await;
        let order = json!({
            "id": "orders:no_items",
            db_fields::BUYER_ID: "nobody",
            fields::CUSTOMER_EMAIL: "test@test.com",
            fields::ITEMS: [],
            db_fields::SUBTOTAL_CENTS: 0,
            db_fields::TOTAL_AMOUNT_CENTS: 0,
        });
        let result = send_order_confirmation_emails(&state, &order).await;
        assert!(result.is_ok());
    }

    // --- async: send_shipping_notification ---

    #[tokio::test]
    async fn test_send_shipping_notification_no_credentials() {
        let state = make_test_state_no_secrets().await;
        let order = sample_order_with_items();
        send_shipping_notification(&state, &order, "TN123", Some("UPS"), None).await;
    }

    #[tokio::test]
    async fn test_send_shipping_notification_empty_tracking() {
        let state = make_test_state().await;
        let order = sample_order_with_items();
        send_shipping_notification(&state, &order, "  ", Some("UPS"), None).await;
    }

    #[tokio::test]
    async fn test_send_shipping_notification_success() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let _lock = ENV_MUTEX.lock().await;
        let server = MockServer::start().await;
        unsafe { std::env::set_var("POSTAL_API_URL", server.uri()) };

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"success"})))
            .mount(&server)
            .await;

        let state = make_test_state().await;
        state
            .db
            .create_document(
                "users",
                json!({
                    "id": "buyer1",
                    db_fields::EMAIL: "buyer@test.com",
                    db_fields::NAME: "Test Buyer",
                    fields::LANGUAGE: "en",
                }),
            )
            .await
            .unwrap();

        let order = sample_order_with_items();
        send_shipping_notification(&state, &order, "TN123456", Some("UPS"), None).await;

        unsafe { std::env::remove_var("POSTAL_API_URL") };
    }

    #[tokio::test]
    async fn test_send_shipping_notification_no_buyer() {
        let state = make_test_state().await;
        let order = json!({
            "id": "orders:ship_no_buyer",
            db_fields::BUYER_ID: "nobody",
            fields::CUSTOMER_EMAIL: "",
            fields::ITEMS: [],
        });
        send_shipping_notification(&state, &order, "TN123", None, None).await;
    }

    #[tokio::test]
    async fn test_send_shipping_notification_lang_override() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let _lock = ENV_MUTEX.lock().await;
        let server = MockServer::start().await;
        unsafe { std::env::set_var("POSTAL_API_URL", server.uri()) };

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"success"})))
            .mount(&server)
            .await;

        let state = make_test_state().await;
        state
            .db
            .create_document(
                "users",
                json!({
                    "id": "buyer1",
                    db_fields::EMAIL: "buyer@test.com",
                    db_fields::NAME: "Test Buyer",
                }),
            )
            .await
            .unwrap();

        let order = sample_order_with_items();
        send_shipping_notification(&state, &order, "TN789", Some("FedEx"), Some("fr")).await;

        unsafe { std::env::remove_var("POSTAL_API_URL") };
    }

    // --- postal_api_key ---

    #[test]
    fn test_postal_api_key_present() {
        let mut config = ob_core::Config::load(None).unwrap();
        config
            .secrets
            .values
            .insert("postal_api_key".into(), "key".into());
        let db_fut = ob_database::DatabaseClient::new_mem();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let db = rt.block_on(db_fut);
        let state = HandlersState::new(Arc::new(config), db);
        let api_key = postal_api_key(&state, "order1", "test");
        assert_eq!(api_key.as_deref(), Some("key"));
    }

    #[test]
    fn test_postal_api_key_missing() {
        let config = ob_core::Config::load(None).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let db = rt.block_on(ob_database::DatabaseClient::new_mem());
        let state = HandlersState::new(Arc::new(config), db);
        let api_key = postal_api_key(&state, "order1", "test");
        assert!(api_key.is_none());
    }
}
