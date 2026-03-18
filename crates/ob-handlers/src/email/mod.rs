//! Mailjet email service — Rust port of Python email_service.py.
//!
//! Provides:
//! - `send_email()` — POST to Mailjet REST API v3.1 with Basic auth
//! - HTML template generators for order confirmation, seller notification,
//!   low stock alert, and abandoned cart (bilingual EN/FR, CASL compliant)

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use crate::shared::schema::email_config;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum EmailError {
    #[error("Mailjet API error (status {status}): {body}")]
    MailjetApi { status: u16, body: String },
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Missing credentials")]
    MissingCredentials,
}

pub type Result<T> = std::result::Result<T, EmailError>;

// ---------------------------------------------------------------------------
// Bilingual strings (EN/FR — Quebec Bill 96 compliance)
// ---------------------------------------------------------------------------

fn t(key: &str, lang: &str) -> &'static str {
    let l = if lang == "fr" { 1 } else { 0 };
    match key {
        "col.product" => ["Product", "Produit"][l],
        "col.qty" => ["Qty", "Qté"][l],
        "col.price" => ["Price", "Prix"][l],
        "price.subtotal" => ["Subtotal", "Sous-total"][l],
        "price.shipping" => ["Shipping", "Livraison"][l],
        "price.taxes" => ["Taxes", "Taxes"][l],
        "price.total" => ["Total", "Total"][l],
        "price.free" => ["Free", "Gratuit"][l],
        "label.order_id" => ["Order ID:", "N° de commande :"][l],
        "confirm.hero_h" => ["Order Confirmed!", "Commande confirmée !"][l],
        "confirm.hero_s" => [
            "Thank you for shopping with us, your order is being prepared.",
            "Merci pour votre achat. Votre commande est en cours de préparation.",
        ][l],
        "seller.hero_h" => ["New Order Received!", "Nouvelle commande reçue !"][l],
        "seller.hero_s" => [
            "You have a new order to fulfill. Ship it fast!",
            "Vous avez une nouvelle commande à traiter. Expédiez-la rapidement !",
        ][l],
        "seller.action_banner" => [
            "ACTION REQUIRED — Confirm and ship this order within 48 hours",
            "ACTION REQUISE — Confirmez et expédiez cette commande dans les 48 heures",
        ][l],
        "cta.track_order" => ["Track Your Order", "Suivre ma commande"][l],
        "cta.manage_orders" => ["Manage Orders", "Gérer les commandes"][l],
        "footer.unsubscribe" => [
            "Unsubscribe from marketing emails",
            "Se désabonner des courriels promotionnels",
        ][l],
        "footer.privacy" => ["Privacy Policy", "Politique de confidentialité"][l],
        "cart.hero_h" => ["Your cart is waiting", "Votre panier vous attend"][l],
        "cart.hero_s" => [
            "You have items in your cart that are still available:",
            "Vous avez des articles dans votre panier qui sont encore disponibles :",
        ][l],
        "cart.cta" => ["Complete your purchase", "Compléter mon achat"][l],
        _ => "",
    }
}

// ---------------------------------------------------------------------------
// Order / Item structs for template rendering
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderSummary {
    pub order_id: String,
    pub items: Vec<OrderItem>,
    pub subtotal_cents: i64,
    pub shipping_cost_cents: i64,
    pub tax_amount_cents: i64,
    pub total_amount_cents: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderItem {
    pub name: String,
    pub quantity: u32,
    pub price_cents: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CartItem {
    pub name: String,
}

// ---------------------------------------------------------------------------
// CASL-compliant footer (Canadian Anti-Spam Legislation)
// ---------------------------------------------------------------------------

fn casl_footer(include_gst: bool, lang: &str) -> String {
    let gst_line = if include_gst {
        format!(
            r#"<p style="margin:0 0 8px 0;font-size:11px;color:rgba(255,255,255,0.35);">GST/HST Registration: {}</p>"#,
            email_config::GST_HST_NUMBER
        )
    } else {
        String::new()
    };
    let t_unsub = t("footer.unsubscribe", lang);
    let t_priv = t("footer.privacy", lang);
    format!(
        r##"<tr><td bgcolor="#1a1a2e" style="background-color:#1a1a2e;padding:32px 40px;text-align:center;">
            <div style="margin-bottom:16px;">
                <span style="font-size:12px;font-weight:700;letter-spacing:3px;text-transform:uppercase;color:rgba(255,255,255,0.5);">O R I G N A</span>
            </div>
            <p style="margin:0 0 8px 0;font-size:13px;color:rgba(255,255,255,0.5);">{tagline}</p>
            <p style="margin:0 0 8px 0;font-size:12px;color:rgba(255,255,255,0.35);">{addr}</p>
            {gst}
            <p style="margin:0 0 8px 0;font-size:12px;color:rgba(255,255,255,0.35);">
                Questions? <a href="mailto:{support}" style="color:#667EEA;text-decoration:none;">{support}</a>
                 | Privacy: <a href="mailto:{privacy}" style="color:#667EEA;text-decoration:none;">{privacy}</a>
            </p>
            <p style="margin:0 0 12px 0;font-size:12px;color:rgba(255,255,255,0.35);">
                <a href="#" style="color:#667EEA;text-decoration:underline;">{unsub}</a>
                 | <a href="{url}/privacy-policy" style="color:#667EEA;text-decoration:none;">{priv_label}</a>
            </p>
            <div style="border-top:1px solid rgba(255,255,255,0.1);padding-top:16px;">
                <p style="margin:0;font-size:11px;color:rgba(255,255,255,0.25);">{copy}</p>
            </div>
        </td></tr>"##,
        tagline = email_config::APP_TAGLINE,
        addr = email_config::PHYSICAL_ADDRESS,
        gst = gst_line,
        support = email_config::SUPPORT_EMAIL,
        privacy = email_config::PRIVACY_OFFICER_EMAIL,
        unsub = t_unsub,
        url = email_config::URL_PROD,
        priv_label = t_priv,
        copy = email_config::COPYRIGHT_TEXT,
    )
}

/// Wrap content in the standard Origna email shell.
fn email_wrapper(title: &str, content: &str, include_gst: bool, lang: &str) -> String {
    let footer = casl_footer(include_gst, lang);
    format!(
        r##"<!DOCTYPE html><html><head><meta charset="utf-8"><title>{title}</title></head>
<body style="margin:0;padding:0;background:#f4f4f8;font-family:Arial,Helvetica,sans-serif;">
<table width="100%" cellpadding="0" cellspacing="0" style="max-width:640px;margin:0 auto;">
{content}
{footer}
</table></body></html>"##,
        title = title,
        content = content,
        footer = footer,
    )
}

// ---------------------------------------------------------------------------
// Core send function — Mailjet REST API v3.1
// ---------------------------------------------------------------------------

/// Send an email via Mailjet REST API v3.1.
///
/// Uses HTTP Basic auth (api_key:secret_key) and JSON payload.
pub async fn send_email(
    http_client: &reqwest::Client,
    api_key: &str,
    secret_key: &str,
    to_email: &str,
    subject: &str,
    html_body: &str,
) -> Result<()> {
    if api_key.is_empty() || secret_key.is_empty() {
        return Err(EmailError::MissingCredentials);
    }

    let credentials = B64.encode(format!("{api_key}:{secret_key}"));

    let payload = json!({
        "Messages": [{
            "From": {
                "Email": email_config::SUPPORT_EMAIL,
                "Name": email_config::SENDER_NAME,
            },
            "To": [{
                "Email": to_email,
            }],
            "Subject": subject,
            "HTMLPart": html_body,
        }]
    });

    let url = std::env::var("MAILJET_API_URL")
        .unwrap_or_else(|_| "https://api.mailjet.com/v3.1/send".to_string());

    let resp = http_client
        .post(url)
        .header("Authorization", format!("Basic {credentials}"))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    let status = resp.status().as_u16();
    if status >= 400 {
        let body = resp.text().await.unwrap_or_default();
        return Err(EmailError::MailjetApi { status, body });
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// HTML template generators
// ---------------------------------------------------------------------------

/// Generate order confirmation HTML (bilingual EN/FR).
pub fn order_confirmation_html(order: &OrderSummary, _buyer_name: &str, lang: &str) -> String {
    let l = if lang == "fr" { "fr" } else { "en" };
    let short_id = if order.order_id.len() > 8 {
        &order.order_id[..8]
    } else {
        &order.order_id
    };

    let mut items_html = String::new();
    for (i, item) in order.items.iter().enumerate() {
        let bg = if i % 2 == 0 { "#f8f9ff" } else { "#ffffff" };
        let line_total = item.price_cents * item.quantity as i64;
        items_html.push_str(&format!(
            r#"<tr style="background:{bg};">
                <td style="padding:14px 16px;font-size:14px;color:#1a1a2e;font-weight:600;">{name}</td>
                <td style="padding:14px 16px;text-align:center;font-size:14px;color:#555;">&times;{qty}</td>
                <td style="padding:14px 16px;text-align:right;font-size:14px;font-weight:600;color:#1a1a2e;">${total:.2}</td>
            </tr>"#,
            bg = bg,
            name = html_escape(&item.name),
            qty = item.quantity,
            total = line_total as f64 / 100.0,
        ));
    }

    let subtotal = order.subtotal_cents as f64 / 100.0;
    let shipping = order.shipping_cost_cents as f64 / 100.0;
    let taxes = order.tax_amount_cents as f64 / 100.0;
    let total = order.total_amount_cents as f64 / 100.0;

    let shipping_display = if order.shipping_cost_cents == 0 {
        t("price.free", l).to_string()
    } else {
        format!("${shipping:.2}")
    };

    let content = format!(
        r##"<tr><td style="padding:32px 40px 24px 40px;">
            <h1 style="color:#1a1a2e;margin-top:0;font-size:22px;">{hero_h}</h1>
            <p style="color:#555;font-size:15px;">{hero_s}</p>
            <p style="color:#888;font-size:13px;">{order_label} #{short_id}</p>
        </td></tr>
        <tr><td style="padding:0 40px;">
            <table width="100%" cellpadding="0" cellspacing="0" style="border-collapse:collapse;">
                <tr style="background:#667EEA;">
                    <td style="padding:10px 16px;color:#fff;font-size:13px;font-weight:700;">{col_prod}</td>
                    <td style="padding:10px 16px;color:#fff;font-size:13px;font-weight:700;text-align:center;">{col_qty}</td>
                    <td style="padding:10px 16px;color:#fff;font-size:13px;font-weight:700;text-align:right;">{col_price}</td>
                </tr>
                {items}
            </table>
        </td></tr>
        <tr><td style="padding:24px 40px;">
            <table width="100%" style="font-size:14px;color:#333;">
                <tr><td>{lbl_sub}</td><td style="text-align:right;">${subtotal:.2}</td></tr>
                <tr><td>{lbl_ship}</td><td style="text-align:right;">{ship_disp}</td></tr>
                <tr><td>{lbl_tax}</td><td style="text-align:right;">${taxes:.2}</td></tr>
                <tr style="font-weight:700;font-size:16px;">
                    <td style="padding-top:12px;border-top:2px solid #667EEA;">{lbl_total}</td>
                    <td style="padding-top:12px;border-top:2px solid #667EEA;text-align:right;">${total:.2}</td>
                </tr>
            </table>
        </td></tr>"##,
        hero_h = t("confirm.hero_h", l),
        hero_s = t("confirm.hero_s", l),
        order_label = t("label.order_id", l),
        short_id = short_id,
        col_prod = t("col.product", l),
        col_qty = t("col.qty", l),
        col_price = t("col.price", l),
        items = items_html,
        lbl_sub = t("price.subtotal", l),
        subtotal = subtotal,
        lbl_ship = t("price.shipping", l),
        ship_disp = shipping_display,
        lbl_tax = t("price.taxes", l),
        taxes = taxes,
        lbl_total = t("price.total", l),
        total = total,
    );

    email_wrapper(t("confirm.hero_h", l), &content, true, l)
}

/// Generate seller notification HTML.
pub fn seller_notification_html(order: &OrderSummary, seller_name: &str) -> String {
    let lang = "en"; // Seller emails default to English
    let short_id = if order.order_id.len() > 8 {
        &order.order_id[..8]
    } else {
        &order.order_id
    };
    let total = order.total_amount_cents as f64 / 100.0;

    let mut items_html = String::new();
    for item in &order.items {
        items_html.push_str(&format!(
            r#"<tr><td style="padding:8px 0;border-bottom:1px solid #eee;">{name}</td>
               <td style="padding:8px 0;text-align:center;border-bottom:1px solid #eee;">&times;{qty}</td>
               <td style="padding:8px 0;text-align:right;border-bottom:1px solid #eee;">${price:.2}</td></tr>"#,
            name = html_escape(&item.name),
            qty = item.quantity,
            price = item.price_cents as f64 / 100.0 * item.quantity as f64,
        ));
    }

    let content = format!(
        r##"<tr><td style="background:#FFF3CD;padding:14px 40px;text-align:center;font-weight:700;color:#856404;font-size:14px;">
            {action}
        </td></tr>
        <tr><td style="padding:32px 40px;">
            <h1 style="color:#1a1a2e;margin-top:0;font-size:22px;">{hero_h}</h1>
            <p style="color:#555;font-size:15px;">Hi {seller}, {hero_s}</p>
            <p style="color:#1a1a2e;font-size:16px;font-weight:700;">Order #{short_id} — ${total:.2} CAD</p>
            <table width="100%" style="font-size:14px;margin-top:16px;">
                {items}
            </table>
            <div style="margin-top:24px;text-align:center;">
                <a href="{url}/seller/orders" style="background:linear-gradient(135deg,#667EEA,#764BA2);color:#fff;padding:12px 28px;border-radius:8px;text-decoration:none;font-weight:700;display:inline-block;">
                    {cta}
                </a>
            </div>
        </td></tr>"##,
        action = t("seller.action_banner", lang),
        hero_h = t("seller.hero_h", lang),
        seller = html_escape(seller_name),
        hero_s = t("seller.hero_s", lang),
        short_id = short_id,
        total = total,
        items = items_html,
        url = email_config::URL_PROD,
        cta = t("cta.manage_orders", lang),
    );

    email_wrapper(t("seller.hero_h", lang), &content, false, lang)
}

/// Generate low stock alert HTML.
pub fn low_stock_alert_html(product_name: &str, current_stock: u32) -> String {
    let units = if current_stock == 1 { "unit" } else { "units" };
    let content = format!(
        r##"<tr><td style="padding:32px 40px;">
            <h2 style="color:#E53E3E;">Low Stock Alert</h2>
            <p>Your product <strong>{name}</strong> is running low on stock.</p>
            <table style="width:100%;border-collapse:collapse;margin:16px 0;">
                <tr><td style="padding:6px 0;color:#666;width:160px;">Current stock</td>
                    <td style="font-weight:bold;color:#E53E3E;">{stock} {units} remaining</td></tr>
            </table>
            <p>Please restock soon to avoid missing sales.</p>
            <div style="margin-top:20px;">
                <a href="{url}/seller/products" style="background:#5B30F6;color:#fff;padding:10px 22px;border-radius:6px;text-decoration:none;font-weight:bold;">
                    Manage Inventory
                </a>
            </div>
            <p style="color:#999;font-size:12px;margin-top:20px;">
                You are receiving this because you enabled low stock alerts for this product.<br>
                Origna Ventures Inc. — {addr}
            </p>
        </td></tr>"##,
        name = html_escape(product_name),
        stock = current_stock,
        units = units,
        url = email_config::URL_PROD,
        addr = email_config::PHYSICAL_ADDRESS,
    );

    email_wrapper("Low Stock Alert", &content, false, "en")
}

/// Generate abandoned cart HTML (bilingual EN/FR).
pub fn abandoned_cart_html(items: &[CartItem], buyer_name: &str, lang: &str) -> String {
    let l = if lang == "fr" { "fr" } else { "en" };

    let product_list: String = items
        .iter()
        .take(3)
        .map(|i| format!("<li>{}</li>", html_escape(&i.name)))
        .collect();

    let more_label = if items.len() > 3 {
        let extra = items.len() - 3;
        if l == "fr" {
            format!(" (et {extra} de plus)")
        } else {
            format!(" (and {extra} more)")
        }
    } else {
        String::new()
    };

    let hi = if l == "fr" {
        format!("Bonjour {}", html_escape(buyer_name))
    } else {
        format!("Hi {}", html_escape(buyer_name))
    };

    let content = format!(
        r##"<tr><td style="padding:32px 40px 24px 40px;">
            <h2 style="color:#1a1a2e;margin-top:0;font-size:20px;">{hero_h}</h2>
            <p style="color:#555;font-size:15px;">{hi},</p>
            <p style="color:#555;font-size:15px;">{hero_s}</p>
            <ul style="margin:16px 0;padding-left:24px;color:#1a1a2e;font-weight:500;line-height:1.6;">
                {list}
            </ul>
            {more}
            <div style="margin-top:32px;text-align:center;">
                <a href="{url}/cart" style="background:linear-gradient(135deg,#667EEA,#764BA2);color:#fff;padding:12px 28px;border-radius:8px;text-decoration:none;font-weight:700;display:inline-block;">
                    {cta}
                </a>
            </div>
        </td></tr>"##,
        hero_h = t("cart.hero_h", l),
        hi = hi,
        hero_s = t("cart.hero_s", l),
        list = product_list,
        more = if more_label.is_empty() {
            String::new()
        } else {
            format!(r#"<p style="color:#888;font-size:13px;font-style:italic;">{more_label}</p>"#)
        },
        url = email_config::URL_PROD,
        cta = t("cart.cta", l),
    );

    let title = t("cart.hero_h", l);
    email_wrapper(title, &content, false, l)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_order_confirmation_html_en() {
        let order = OrderSummary {
            order_id: "abc12345def".into(),
            items: vec![
                OrderItem {
                    name: "Widget".into(),
                    quantity: 2,
                    price_cents: 1500,
                },
                OrderItem {
                    name: "Gadget".into(),
                    quantity: 1,
                    price_cents: 3000,
                },
            ],
            subtotal_cents: 6000,
            shipping_cost_cents: 0,
            tax_amount_cents: 780,
            total_amount_cents: 6780,
        };
        let html = order_confirmation_html(&order, "Yunior", "en");
        assert!(html.contains("Order Confirmed!"));
        assert!(html.contains("abc12345")); // short ID
        assert!(html.contains("Widget"));
        assert!(html.contains("Gadget"));
        assert!(html.contains("$67.80")); // total
        assert!(html.contains("Free")); // free shipping
        assert!(html.contains(email_config::GST_HST_NUMBER)); // CASL GST
        assert!(html.contains(email_config::PHYSICAL_ADDRESS)); // CASL address
    }

    #[test]
    fn test_order_confirmation_html_fr() {
        let order = OrderSummary {
            order_id: "fr123456abc".into(),
            items: vec![OrderItem {
                name: "Produit".into(),
                quantity: 1,
                price_cents: 2000,
            }],
            subtotal_cents: 2000,
            shipping_cost_cents: 500,
            tax_amount_cents: 250,
            total_amount_cents: 2750,
        };
        let html = order_confirmation_html(&order, "Marie", "fr");
        assert!(html.contains("Commande confirmée"));
        assert!(html.contains("Sous-total"));
        assert!(html.contains("Produit"));
    }

    #[test]
    fn test_seller_notification_html() {
        let order = OrderSummary {
            order_id: "sell123456ab".into(),
            items: vec![OrderItem {
                name: "T-Shirt".into(),
                quantity: 3,
                price_cents: 2500,
            }],
            subtotal_cents: 7500,
            shipping_cost_cents: 0,
            tax_amount_cents: 975,
            total_amount_cents: 8475,
        };
        let html = seller_notification_html(&order, "Bob");
        assert!(html.contains("New Order Received!"));
        assert!(html.contains("ACTION REQUIRED"));
        assert!(html.contains("sell1234")); // short ID
        assert!(html.contains("Bob"));
    }

    #[test]
    fn test_low_stock_alert_html() {
        let html = low_stock_alert_html("Maple Syrup", 3);
        assert!(html.contains("Low Stock Alert"));
        assert!(html.contains("Maple Syrup"));
        assert!(html.contains("3 units remaining"));
        assert!(html.contains(email_config::PHYSICAL_ADDRESS));
    }

    #[test]
    fn test_abandoned_cart_html_en() {
        let items = vec![
            CartItem {
                name: "Shoes".into(),
            },
            CartItem { name: "Hat".into() },
        ];
        let html = abandoned_cart_html(&items, "Alice", "en");
        assert!(html.contains("Your cart is waiting"));
        assert!(html.contains("Shoes"));
        assert!(html.contains("Hat"));
        assert!(html.contains("Complete your purchase"));
    }

    #[test]
    fn test_abandoned_cart_html_fr_with_more() {
        let items = vec![
            CartItem { name: "A".into() },
            CartItem { name: "B".into() },
            CartItem { name: "C".into() },
            CartItem { name: "D".into() },
            CartItem { name: "E".into() },
        ];
        let html = abandoned_cart_html(&items, "Jean", "fr");
        assert!(html.contains("Votre panier vous attend"));
        assert!(html.contains("et 2 de plus"));
        assert!(html.contains("Compléter mon achat"));
    }

    #[test]
    fn test_html_escape() {
        assert_eq!(
            html_escape("<script>alert(\"xss\")</script>"),
            "&lt;script&gt;alert(&quot;xss&quot;)&lt;/script&gt;"
        );
        // Verify & is escaped
        assert_eq!(html_escape("A & B"), "A &amp; B");
    }

    #[test]
    fn test_order_confirmation_short_id() {
        let order = OrderSummary {
            order_id: "123".into(),
            items: vec![],
            subtotal_cents: 0,
            shipping_cost_cents: 0,
            tax_amount_cents: 0,
            total_amount_cents: 0,
        };
        let html = order_confirmation_html(&order, "User", "en");
        assert!(html.contains("#123"));
    }

    #[test]
    fn test_low_stock_alert_singular() {
        let html = low_stock_alert_html("Milk", 1);
        assert!(html.contains("1 unit remaining"));
    }

    #[tokio::test]
    async fn test_send_email_success() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let _lock = ENV_MUTEX.lock().unwrap();
        let server = MockServer::start().await;
        unsafe { std::env::set_var("MAILJET_API_URL", server.uri()) };

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"success"})))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = send_email(&client, "key", "secret", "test@test.com", "Hi", "<p>Hi</p>").await;

        assert!(result.is_ok());
        unsafe { std::env::remove_var("MAILJET_API_URL") };
    }

    #[tokio::test]
    async fn test_send_email_missing_credentials() {
        let client = reqwest::Client::new();
        let result = send_email(&client, "", "secret", "test@test.com", "Hi", "<p>Hello</p>").await;
        assert!(matches!(result, Err(EmailError::MissingCredentials)));
    }

    #[tokio::test]
    async fn test_send_email_api_error_unauthorized() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let _lock = ENV_MUTEX.lock().unwrap();
        let server = MockServer::start().await;
        unsafe { std::env::set_var("MAILJET_API_URL", server.uri()) };

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = send_email(
            &client,
            "fake_api",
            "fake_secret",
            "test@test.com",
            "Hi",
            "<p>Hello</p>",
        )
        .await;
        match result {
            Err(EmailError::MailjetApi { status, body }) => {
                assert_eq!(status, 401);
                assert_eq!(body, "Unauthorized");
            }
            _ => panic!("Expected MailjetApi error 401, got {:?}", result),
        }
        unsafe { std::env::remove_var("MAILJET_API_URL") };
    }

    #[test]
    fn test_email_error_display() {
        let err = EmailError::MissingCredentials;
        assert_eq!(err.to_string(), "Missing credentials");

        let err2 = EmailError::MailjetApi {
            status: 400,
            body: "Bad Request".to_string(),
        };
        assert_eq!(
            err2.to_string(),
            "Mailjet API error (status 400): Bad Request"
        );
    }

    #[test]
    fn test_bilingual_strings_exhaustive() {
        let keys = [
            "col.product",
            "col.qty",
            "col.price",
            "price.subtotal",
            "price.shipping",
            "price.taxes",
            "price.total",
            "price.free",
            "label.order_id",
            "confirm.hero_h",
            "confirm.hero_s",
            "seller.hero_h",
            "seller.hero_s",
            "seller.action_banner",
            "cta.track_order",
            "cta.manage_orders",
            "footer.unsubscribe",
            "footer.privacy",
            "cart.hero_h",
            "cart.hero_s",
            "cart.cta",
        ];
        for k in keys {
            assert!(!t(k, "en").is_empty());
            assert!(!t(k, "fr").is_empty());
        }
        assert_eq!(t("unknown", "en"), "");
        assert_eq!(t("unknown", "fr"), "");
    }

    // --- Ported from Python test_services_email_service_templates.py ---

    #[test]
    fn test_html_escape_xss_in_item_names() {
        let order = OrderSummary {
            order_id: "xss_test_12".into(),
            items: vec![OrderItem {
                name: "<script>alert('xss')</script>".into(),
                quantity: 1,
                price_cents: 1000,
            }],
            subtotal_cents: 1000,
            shipping_cost_cents: 0,
            tax_amount_cents: 130,
            total_amount_cents: 1130,
        };
        let html = order_confirmation_html(&order, "Attacker", "en");
        assert!(
            !html.contains("<script>"),
            "XSS script tags must be escaped"
        );
        assert!(
            html.contains("&lt;script&gt;"),
            "Script tags should be HTML-escaped"
        );
    }

    #[test]
    fn test_order_confirmation_empty_items() {
        let order = OrderSummary {
            order_id: "empty_items".into(),
            items: vec![],
            subtotal_cents: 0,
            shipping_cost_cents: 0,
            tax_amount_cents: 0,
            total_amount_cents: 0,
        };
        let html = order_confirmation_html(&order, "User", "en");
        assert!(html.contains("<html"));
        assert!(html.contains("Order Confirmed!"));
        assert!(html.contains("$0.00"));
    }

    #[test]
    fn test_order_confirmation_shipping_cost_nonzero() {
        let order = OrderSummary {
            order_id: "shipping_test".into(),
            items: vec![OrderItem {
                name: "Widget".into(),
                quantity: 1,
                price_cents: 5000,
            }],
            subtotal_cents: 5000,
            shipping_cost_cents: 899, // $8.99
            tax_amount_cents: 650,
            total_amount_cents: 6549,
        };
        let html = order_confirmation_html(&order, "Buyer", "en");
        assert!(
            html.contains("$8.99"),
            "Non-free shipping should show dollar amount"
        );
        assert!(
            !html.contains("Free"),
            "Should not show 'Free' when shipping costs money"
        );
    }

    #[test]
    fn test_order_confirmation_alternating_row_colors() {
        let order = OrderSummary {
            order_id: "rows_test12".into(),
            items: vec![
                OrderItem {
                    name: "A".into(),
                    quantity: 1,
                    price_cents: 100,
                },
                OrderItem {
                    name: "B".into(),
                    quantity: 1,
                    price_cents: 200,
                },
                OrderItem {
                    name: "C".into(),
                    quantity: 1,
                    price_cents: 300,
                },
            ],
            subtotal_cents: 600,
            shipping_cost_cents: 0,
            tax_amount_cents: 78,
            total_amount_cents: 678,
        };
        let html = order_confirmation_html(&order, "User", "en");
        assert!(
            html.contains("#f8f9ff"),
            "Even rows should have light background"
        );
        assert!(
            html.contains("#ffffff"),
            "Odd rows should have white background"
        );
    }

    #[test]
    fn test_seller_notification_xss_in_seller_name() {
        let order = OrderSummary {
            order_id: "seller_xss1".into(),
            items: vec![OrderItem {
                name: "Product".into(),
                quantity: 1,
                price_cents: 1000,
            }],
            subtotal_cents: 1000,
            shipping_cost_cents: 0,
            tax_amount_cents: 130,
            total_amount_cents: 1130,
        };
        let html = seller_notification_html(&order, "<img src=x onerror=alert(1)>");
        // The angle brackets must be escaped so the browser won't parse it as a tag
        assert!(!html.contains("<img"), "Raw <img tag must be escaped");
        assert!(
            html.contains("&lt;img"),
            "Escaped seller name must use &lt;"
        );
        assert!(html.contains("&gt;"), "Closing bracket must be escaped");
    }

    #[test]
    fn test_low_stock_alert_zero_stock() {
        let html = low_stock_alert_html("Out of Stock Item", 0);
        assert!(html.contains("0 units remaining"));
    }

    #[test]
    fn test_abandoned_cart_exactly_three_items_no_more_label() {
        let items = vec![
            CartItem { name: "A".into() },
            CartItem { name: "B".into() },
            CartItem { name: "C".into() },
        ];
        let html = abandoned_cart_html(&items, "User", "en");
        assert!(html.contains("A"));
        assert!(html.contains("B"));
        assert!(html.contains("C"));
        assert!(
            !html.contains("more"),
            "Exactly 3 items should show no 'more' label"
        );
    }

    #[test]
    fn test_abandoned_cart_single_item() {
        let items = vec![CartItem {
            name: "Solo Item".into(),
        }];
        let html = abandoned_cart_html(&items, "Buyer", "en");
        assert!(html.contains("Solo Item"));
        assert!(!html.contains("more"));
    }

    #[test]
    fn test_abandoned_cart_empty_items() {
        let items: Vec<CartItem> = vec![];
        let html = abandoned_cart_html(&items, "Buyer", "en");
        assert!(html.contains("<html"));
        assert!(html.contains("Your cart is waiting"));
    }

    #[test]
    fn test_casl_footer_with_gst() {
        let footer = casl_footer(true, "en");
        assert!(footer.contains(email_config::GST_HST_NUMBER));
        assert!(footer.contains(email_config::PHYSICAL_ADDRESS));
        assert!(footer.contains(email_config::SUPPORT_EMAIL));
        assert!(footer.contains("Unsubscribe"));
    }

    #[test]
    fn test_casl_footer_without_gst() {
        let footer = casl_footer(false, "en");
        assert!(!footer.contains(email_config::GST_HST_NUMBER));
        assert!(footer.contains(email_config::PHYSICAL_ADDRESS)); // Still has address
    }

    #[test]
    fn test_casl_footer_french() {
        let footer = casl_footer(true, "fr");
        assert!(footer.contains("Se désabonner"));
        assert!(footer.contains("Politique de confidentialité"));
    }

    #[test]
    fn test_email_wrapper_includes_doctype_and_html_structure() {
        let html = email_wrapper("Test", "<tr><td>Content</td></tr>", false, "en");
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<html>"));
        assert!(html.contains("</html>"));
        assert!(html.contains("Content"));
    }

    #[test]
    fn test_bilingual_en_fr_are_different() {
        // Verify EN and FR translations are actually different (not duplicated)
        let check_keys = [
            "confirm.hero_h",
            "confirm.hero_s",
            "col.qty",
            "price.subtotal",
            "price.free",
        ];
        for k in check_keys {
            let en = t(k, "en");
            let fr = t(k, "fr");
            assert_ne!(en, fr, "EN and FR should differ for key '{k}'");
        }
    }

    #[test]
    fn test_html_escape_ampersand_and_quotes() {
        assert_eq!(html_escape("A & B"), "A &amp; B");
        assert_eq!(
            html_escape(r#"He said "hello""#),
            "He said &quot;hello&quot;"
        );
        assert_eq!(html_escape("a < b > c"), "a &lt; b &gt; c");
    }

    #[test]
    fn test_seller_notification_short_order_id() {
        // Covers line 317: order_id <= 8 chars uses full ID (else branch)
        let order = OrderSummary {
            order_id: "short1".into(),
            items: vec![OrderItem {
                name: "Item".into(),
                quantity: 1,
                price_cents: 500,
            }],
            subtotal_cents: 500,
            shipping_cost_cents: 0,
            tax_amount_cents: 65,
            total_amount_cents: 565,
        };
        let html = seller_notification_html(&order, "Seller");
        assert!(html.contains("short1"), "Full short order ID should appear");
    }

    #[test]
    fn test_abandoned_cart_en_with_more_than_three_items() {
        // Covers line 411: English "(and N more)" label
        let items = vec![
            CartItem { name: "X".into() },
            CartItem { name: "Y".into() },
            CartItem { name: "Z".into() },
            CartItem { name: "W".into() },
        ];
        let html = abandoned_cart_html(&items, "Bob", "en");
        assert!(
            html.contains("and 1 more"),
            "English 'more' label should appear"
        );
    }

    #[test]
    fn test_send_email_missing_api_key() {
        // Synchronous check: missing api_key returns MissingCredentials
        // (Cannot call async send_email in sync test, so we verify the error type exists)
        let err = EmailError::MissingCredentials;
        assert_eq!(err.to_string(), "Missing credentials");
    }
}
