use crate::email::{OrderSummary, email_wrapper, html_escape, t};
use crate::shared::schema::email_config;

/// Generate buyer shipping notification HTML with tracking details.
pub fn shipping_notification_html(
    order: &OrderSummary,
    buyer_name: &str,
    tracking_number: &str,
    carrier: Option<&str>,
    lang: &str,
) -> String {
    let l = if lang == "fr" { "fr" } else { "en" };
    let short_id = if order.order_id.len() > 8 {
        &order.order_id[..8]
    } else {
        &order.order_id
    };
    let safe_buyer_name = if buyer_name.trim().is_empty() {
        if l == "fr" { "Bonjour" } else { "Hello" }.to_string()
    } else if l == "fr" {
        format!("Bonjour {}", html_escape(buyer_name))
    } else {
        format!("Hello {}", html_escape(buyer_name))
    };
    let tracking_line = if let Some(carrier) = carrier.filter(|value| !value.trim().is_empty()) {
        if l == "fr" {
            format!(
                "Votre commande #{short_id} est en route via {}. Suivi: {}.",
                html_escape(carrier),
                html_escape(tracking_number),
            )
        } else {
            format!(
                "Your order #{short_id} is on the way via {}. Tracking: {}.",
                html_escape(carrier),
                html_escape(tracking_number),
            )
        }
    } else if l == "fr" {
        format!(
            "Votre commande #{short_id} est en route. Suivi: {}.",
            html_escape(tracking_number),
        )
    } else {
        format!(
            "Your order #{short_id} is on the way. Tracking: {}.",
            html_escape(tracking_number),
        )
    };

    let cta = t("cta.track_order", l);
    let title = if l == "fr" {
        format!("Commande #{short_id} expédiée")
    } else {
        format!("Order #{short_id} shipped")
    };
    let hero = if l == "fr" {
        "Votre commande est en route"
    } else {
        "Your order is on the way"
    };
    let tracking_label = if l == "fr" {
        "Numéro de suivi"
    } else {
        "Tracking number"
    };
    let carrier_label = if l == "fr" { "Transporteur" } else { "Carrier" };
    let content = format!(
        r##"<tr><td style="padding:32px 40px 24px 40px;">
            <h1 style="color:#1a1a2e;margin-top:0;font-size:22px;">{hero}</h1>
            <p style="color:#555;font-size:15px;">{buyer},</p>
            <p style="color:#555;font-size:15px;">{tracking_line}</p>
            <table width="100%" cellpadding="0" cellspacing="0" style="margin:24px 0;border-collapse:collapse;background:#f8f9ff;border-radius:12px;overflow:hidden;">
                <tr>
                    <td style="padding:14px 16px;font-size:13px;font-weight:700;color:#667EEA;">{order_label}</td>
                    <td style="padding:14px 16px;font-size:14px;color:#1a1a2e;text-align:right;">#{short_id}</td>
                </tr>
                <tr>
                    <td style="padding:14px 16px;font-size:13px;font-weight:700;color:#667EEA;">{tracking_label}</td>
                    <td style="padding:14px 16px;font-size:14px;color:#1a1a2e;text-align:right;">{tracking_number}</td>
                </tr>
                {carrier_row}
            </table>
            <div style="margin-top:24px;text-align:center;">
                <a href="{url}/orders" style="background:linear-gradient(135deg,#667EEA,#764BA2);color:#fff;padding:12px 28px;border-radius:8px;text-decoration:none;font-weight:700;display:inline-block;">
                    {cta}
                </a>
            </div>
        </td></tr>"##,
        hero = hero,
        buyer = safe_buyer_name,
        tracking_line = tracking_line,
        order_label = t("label.order_id", l),
        short_id = short_id,
        tracking_label = tracking_label,
        tracking_number = html_escape(tracking_number),
        carrier_row = carrier
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                format!(
                    r#"<tr>
                    <td style="padding:14px 16px;font-size:13px;font-weight:700;color:#667EEA;">{carrier_label}</td>
                    <td style="padding:14px 16px;font-size:14px;color:#1a1a2e;text-align:right;">{carrier}</td>
                </tr>"#,
                    carrier_label = carrier_label,
                    carrier = html_escape(value),
                )
            })
            .unwrap_or_default(),
        url = email_config::URL_PROD,
        cta = cta,
    );

    email_wrapper(&title, &content, true, l)
}
