use serde_json::{Value, json};
use std::collections::HashSet;
use tracing::warn;

use crate::HandlersState;
use crate::shared::schema::{collections, fields};

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
    item.get("quantity")
        .and_then(|v| v.as_u64())
        .map(|v| v.min(u32::MAX as u64) as u32)
        .or_else(|| {
            item.get("quantity")
                .and_then(|v| v.as_i64())
                .map(|v| v.max(0).min(u32::MAX as i64) as u32)
        })
        .unwrap_or(1)
}

pub fn item_price_cents(item: &Value) -> i64 {
    item.get(fields::PRICE_CENTS)
        .and_then(|v| v.as_i64())
        .or_else(|| item.get("price").and_then(|v| v.as_i64()))
        .unwrap_or(0)
}

pub fn item_name(item: &Value) -> String {
    item.get(fields::NAME)
        .or_else(|| item.get(fields::TITLE))
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or("Item")
        .to_string()
}

pub fn build_order_summary_from_items(order: &Value, items: &[Value]) -> OrderSummary {
    let order_id = order
        .get("id")
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
            .get(fields::SUBTOTAL_CENTS)
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
            .get(fields::TOTAL_AMOUNT_CENTS)
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
        .filter(|item| str_field(item, fields::SELLER_ID) == seller_id)
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
        .get(fields::BUYER_ID)
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .or_else(|| order.get("userId").and_then(|v| v.as_str()))
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
                doc.get(fields::EMAIL)
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .map(ToString::to_string)
            })
        })?;

    let buyer_name = buyer_doc
        .as_ref()
        .and_then(|doc| doc.get(fields::NAME).and_then(|v| v.as_str()))
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
) -> Option<(String, String)> {
    if seller_id.is_empty() {
        return None;
    }

    let seller_profiles: Vec<Value> = state
        .db
        .query_bind_value(
            &format!(
                "SELECT * FROM {} WHERE uid = $seller_id OR user_id = $seller_id LIMIT 1",
                collections::SELLER_PROFILES
            ),
            json!({ "seller_id": seller_id }),
        )
        .await
        .unwrap_or_default();
    let seller_profile = seller_profiles.first();
    let user_doc = load_user_document(state, seller_id).await;

    let email = seller_profile
        .and_then(|profile| profile.get(fields::EMAIL).and_then(|v| v.as_str()))
        .filter(|v| !v.trim().is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            user_doc.as_ref().and_then(|doc| {
                doc.get(fields::EMAIL)
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .map(ToString::to_string)
            })
        })?;

    let seller_name = seller_profile
        .and_then(|profile| {
            profile
                .get(fields::NAME)
                .or_else(|| profile.get("businessName"))
                .or_else(|| profile.get("storeName"))
                .or_else(|| profile.get("displayName"))
                .and_then(|v| v.as_str())
        })
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            user_doc.as_ref().and_then(|doc| {
                doc.get(fields::NAME)
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
            })
        })
        .unwrap_or(seller_id)
        .to_string();

    Some((email, seller_name))
}

fn mailjet_credentials(
    state: &HandlersState,
    order_id: &str,
    log_message: &str,
) -> Option<(String, String)> {
    match (
        state.config.require_secret("mailjet_api_key"),
        state.config.require_secret("mailjet_secret_key"),
    ) {
        (Ok(api_key), Ok(secret_key)) => Some((api_key, secret_key)),
        (Err(err), _) | (_, Err(err)) => {
            warn!(order_id = %order_id, error = %err, context = log_message, "Mailjet credentials unavailable");
            None
        }
    }
}

pub async fn send_order_confirmation_emails(
    state: &HandlersState,
    order: &Value,
) -> Result<(), ob_core::Error> {
    let order_id = order
        .get("id")
        .and_then(|v| v.as_str())
        .map(record_key)
        .unwrap_or("");
    let Some((api_key, secret_key)) = mailjet_credentials(
        state,
        order_id,
        "Mailjet credentials unavailable; skipping payment success emails",
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
        if let Err(err) = send_email(
            &state.http_client,
            &api_key,
            &secret_key,
            &buyer_email,
            subject,
            &html,
        )
        .await
        {
            warn!(order_id = %order_id, to = %buyer_email, error = %err, "Failed to send buyer order confirmation email");
        }
    } else {
        warn!(order_id = %order_id, "Buyer email unavailable; skipping order confirmation email");
    }

    let mut seller_ids = HashSet::new();
    for item in order_items(order) {
        let seller_id = str_field(&item, fields::SELLER_ID);
        if !seller_id.is_empty() {
            seller_ids.insert(seller_id.to_string());
        }
    }

    for seller_id in seller_ids {
        let Some((seller_email, seller_name)) = resolve_seller_contact(state, &seller_id).await
        else {
            warn!(order_id = %order_id, seller_id = %seller_id, "Seller email unavailable; skipping seller notification email");
            continue;
        };
        let Some(seller_summary) = build_seller_order_summary(order, &seller_id) else {
            continue;
        };
        let subject = format!("[Origna] New order received #{order_id}");
        let html = seller_notification_html(&seller_summary, &seller_name);
        if let Err(err) = send_email(
            &state.http_client,
            &api_key,
            &secret_key,
            &seller_email,
            &subject,
            &html,
        )
        .await
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
        .get("id")
        .and_then(|v| v.as_str())
        .map(record_key)
        .unwrap_or("");
    let Some((api_key, secret_key)) = mailjet_credentials(
        state,
        order_id,
        "Mailjet credentials unavailable; skipping order status email",
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
    if let Err(err) = send_email(
        &state.http_client,
        &api_key,
        &secret_key,
        &buyer_email,
        &subject,
        &html,
    )
    .await
    {
        warn!(order_id = %order_id, to = %buyer_email, error = %err, "Failed to send shipping notification email");
    }
}
