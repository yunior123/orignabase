use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{MultiPart, SinglePart, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use ob_core::{Error, Result};
use ob_database::fields;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Email Template System ──────────────────────────────────────────
//
// Admins can customize all auth email templates via:
//   GET  /admin/email-templates           — list all templates
//   GET  /admin/email-templates/:name     — get one template
//   PUT  /admin/email-templates/:name     — update a template
//   POST /admin/email-templates/:name/reset — reset to default
//
// Template variables (Supabase-compatible where possible):
//   {{ .ActionURL }}    — full verification/reset/magic-link URL
//   {{ .Token }}        — raw token string
//   {{ .Email }}        — recipient email address
//   {{ .NewEmail }}     — new email (for email change)
//   {{ .AppName }}      — application name (from config)
//   {{ .SiteURL }}      — site base URL
//   {{ .UserName }}     — display name (if available)
//   {{ .ExpiresIn }}    — human-readable expiry (e.g. "24 hours")

/// Well-known template names (English).
pub const TEMPLATE_VERIFY_EMAIL: &str = "verify_email";
pub const TEMPLATE_RESET_PASSWORD: &str = "reset_password";
pub const TEMPLATE_MFA_ALERT: &str = "mfa_alert";
pub const TEMPLATE_MAGIC_LINK: &str = "magic_link";
pub const TEMPLATE_EMAIL_CHANGE: &str = "email_change";
pub const TEMPLATE_WELCOME: &str = "welcome";

/// Well-known template names (French — Bill 96 / Quebec compliance).
pub const TEMPLATE_VERIFY_EMAIL_FR: &str = "verify_email_fr";
pub const TEMPLATE_RESET_PASSWORD_FR: &str = "reset_password_fr";
pub const TEMPLATE_MFA_ALERT_FR: &str = "mfa_alert_fr";
pub const TEMPLATE_MAGIC_LINK_FR: &str = "magic_link_fr";
pub const TEMPLATE_EMAIL_CHANGE_FR: &str = "email_change_fr";
pub const TEMPLATE_WELCOME_FR: &str = "welcome_fr";

/// CASL-compliant footer appended to all auth emails (bilingual).
const CASL_FOOTER_HTML: &str = r#"
<div style="margin-top:32px;padding-top:16px;border-top:1px solid #eee;font-size:12px;color:#666;text-align:center;">
  <p>Origna Ventures Inc. | Montréal, QC, Canada</p>
  <p><a href="https://orignagta.ca/unsubscribe" style="color:#2563eb;">Unsubscribe / Se désabonner</a> |
     <a href="mailto:support@orignagta.ca" style="color:#2563eb;">support@orignagta.ca</a></p>
  <p>This message was sent in compliance with CASL (Canada's Anti-Spam Legislation).<br>
     Ce message a été envoyé conformément à la LCAP (Loi canadienne anti-pourriel).</p>
</div>"#;

const CASL_FOOTER_TEXT: &str = "\n\n---\nOrigna Ventures Inc. | Montréal, QC, Canada\nUnsubscribe / Se désabonner: https://orignagta.ca/unsubscribe\nsupport@orignagta.ca\nThis message was sent in compliance with CASL. / Ce message a été envoyé conformément à la LCAP.";

/// An email template with subject, HTML body, and plain text body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailTemplate {
    pub name: String,
    pub subject: String,
    pub html: String,
    pub text: String,
}

impl EmailTemplate {
    /// Render the template by replacing `{{ .VarName }}` placeholders.
    pub fn render(&self, vars: &HashMap<String, String>) -> RenderedEmail {
        RenderedEmail {
            subject: replace_vars(&self.subject, vars),
            html: replace_vars(&self.html, vars),
            text: replace_vars(&self.text, vars),
        }
    }
}

pub struct RenderedEmail {
    pub subject: String,
    pub html: String,
    pub text: String,
}

/// Replace `{{ .Key }}` patterns in a string. Also supports `{{.Key}}` (no spaces).
fn replace_vars(template: &str, vars: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        // Match both {{ .Key }} and {{.Key}}
        result = result.replace(&format!("{{{{ .{key} }}}}"), value);
        result = result.replace(&format!("{{{{.{key}}}}}",), value);
    }
    result
}

/// Insert the CASL footer just before the closing `</body>` tag.
/// Falls back to appending at the end if no `</body>` is found.
fn append_casl_footer_html(html: &str) -> String {
    if let Some(pos) = html.rfind("</body>") {
        let mut result = String::with_capacity(html.len() + CASL_FOOTER_HTML.len());
        result.push_str(&html[..pos]);
        result.push_str(CASL_FOOTER_HTML);
        result.push_str(&html[pos..]);
        result
    } else {
        [html, CASL_FOOTER_HTML].concat()
    }
}

/// Resolve the template name for the user's preferred language.
/// Returns the French variant if `lang == "fr"` and a French template exists.
pub fn template_for_lang<'a>(base_name: &'a str, lang: &str) -> &'a str {
    if lang == "fr" {
        match base_name {
            TEMPLATE_VERIFY_EMAIL => TEMPLATE_VERIFY_EMAIL_FR,
            TEMPLATE_RESET_PASSWORD => TEMPLATE_RESET_PASSWORD_FR,
            TEMPLATE_MFA_ALERT => TEMPLATE_MFA_ALERT_FR,
            TEMPLATE_MAGIC_LINK => TEMPLATE_MAGIC_LINK_FR,
            TEMPLATE_EMAIL_CHANGE => TEMPLATE_EMAIL_CHANGE_FR,
            TEMPLATE_WELCOME => TEMPLATE_WELCOME_FR,
            _ => base_name,
        }
    } else {
        match base_name {
            TEMPLATE_VERIFY_EMAIL => TEMPLATE_VERIFY_EMAIL,
            TEMPLATE_RESET_PASSWORD => TEMPLATE_RESET_PASSWORD,
            TEMPLATE_MFA_ALERT => TEMPLATE_MFA_ALERT,
            TEMPLATE_MAGIC_LINK => TEMPLATE_MAGIC_LINK,
            TEMPLATE_EMAIL_CHANGE => TEMPLATE_EMAIL_CHANGE,
            TEMPLATE_WELCOME => TEMPLATE_WELCOME,
            _ => base_name,
        }
    }
}

// ── Default Templates ──────────────────────────────────────────────

pub fn default_templates() -> Vec<EmailTemplate> {
    vec![
        default_verify_email(),
        default_reset_password(),
        default_mfa_alert(),
        default_magic_link(),
        default_email_change(),
        default_welcome(),
        // French (Bill 96 / Quebec compliance)
        default_verify_email_fr(),
        default_reset_password_fr(),
        default_mfa_alert_fr(),
        default_magic_link_fr(),
        default_email_change_fr(),
        default_welcome_fr(),
    ]
}

fn default_verify_email() -> EmailTemplate {
    EmailTemplate {
        name: TEMPLATE_VERIFY_EMAIL.into(),
        subject: "Verify your email — {{ .AppName }}".into(),
        text: "Welcome to {{ .AppName }}!\n\n\
               Please verify your email by clicking the link below:\n\n\
               {{ .ActionURL }}\n\n\
               This link expires in {{ .ExpiresIn }}.\n\n\
               If you didn't create an account, you can safely ignore this email.".into(),
        html: r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"></head>
<body style="font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;margin:0;padding:0;background:#f5f5f5">
<table width="100%" cellpadding="0" cellspacing="0" style="padding:40px 0">
<tr><td align="center">
<table width="600" cellpadding="0" cellspacing="0" style="background:#fff;border-radius:8px;padding:40px;border:1px solid #e0e0e0">
<tr><td>
<h2 style="margin:0 0 20px;color:#1a1a1a">Welcome to {{ .AppName }}</h2>
<p style="color:#333;line-height:1.6;margin:0 0 24px">Please verify your email address to get started.</p>
<p style="margin:0 0 24px">
<a href="{{ .ActionURL }}" style="display:inline-block;background:#2563eb;color:#fff;padding:12px 32px;border-radius:6px;text-decoration:none;font-weight:600">Verify Email</a>
</p>
<p style="color:#666;font-size:14px;line-height:1.5;margin:0 0 16px">Or copy and paste this link:<br>
<a href="{{ .ActionURL }}" style="color:#2563eb;word-break:break-all">{{ .ActionURL }}</a></p>
<p style="color:#999;font-size:13px;margin:16px 0 0">This link expires in {{ .ExpiresIn }}. If you didn't create an account, ignore this email.</p>
</td></tr>
</table>
</td></tr>
</table>
</body></html>"#.into(),
    }
}

fn default_reset_password() -> EmailTemplate {
    EmailTemplate {
        name: TEMPLATE_RESET_PASSWORD.into(),
        subject: "Reset your password — {{ .AppName }}".into(),
        text: "You requested a password reset for your {{ .AppName }} account.\n\n\
               Click the link below to reset your password:\n\n\
               {{ .ActionURL }}\n\n\
               This link expires in {{ .ExpiresIn }}.\n\n\
               If you didn't request this, you can safely ignore this email.".into(),
        html: r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"></head>
<body style="font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;margin:0;padding:0;background:#f5f5f5">
<table width="100%" cellpadding="0" cellspacing="0" style="padding:40px 0">
<tr><td align="center">
<table width="600" cellpadding="0" cellspacing="0" style="background:#fff;border-radius:8px;padding:40px;border:1px solid #e0e0e0">
<tr><td>
<h2 style="margin:0 0 20px;color:#1a1a1a">Reset Your Password</h2>
<p style="color:#333;line-height:1.6;margin:0 0 24px">You requested a password reset. Click the button below to choose a new password.</p>
<p style="margin:0 0 24px">
<a href="{{ .ActionURL }}" style="display:inline-block;background:#2563eb;color:#fff;padding:12px 32px;border-radius:6px;text-decoration:none;font-weight:600">Reset Password</a>
</p>
<p style="color:#666;font-size:14px;line-height:1.5;margin:0 0 16px">Or copy and paste this link:<br>
<a href="{{ .ActionURL }}" style="color:#2563eb;word-break:break-all">{{ .ActionURL }}</a></p>
<p style="color:#999;font-size:13px;margin:16px 0 0">This link expires in {{ .ExpiresIn }}. If you didn't request this, ignore this email.</p>
</td></tr>
</table>
</td></tr>
</table>
</body></html>"#.into(),
    }
}

fn default_mfa_alert() -> EmailTemplate {
    EmailTemplate {
        name: TEMPLATE_MFA_ALERT.into(),
        subject: "Security alert: MFA {{ .Action }} — {{ .AppName }}".into(),
        text: "Two-factor authentication has been {{ .Action }} on your {{ .AppName }} account.\n\n\
               If you didn't make this change, please contact support immediately.".into(),
        html: r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"></head>
<body style="font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;margin:0;padding:0;background:#f5f5f5">
<table width="100%" cellpadding="0" cellspacing="0" style="padding:40px 0">
<tr><td align="center">
<table width="600" cellpadding="0" cellspacing="0" style="background:#fff;border-radius:8px;padding:40px;border:1px solid #e0e0e0">
<tr><td>
<h2 style="margin:0 0 20px;color:#1a1a1a">Security Alert</h2>
<p style="color:#333;line-height:1.6;margin:0 0 16px">Two-factor authentication has been <strong>{{ .Action }}</strong> on your {{ .AppName }} account.</p>
<p style="color:#cc0000;font-size:14px;margin:0">If you didn't make this change, please contact support immediately.</p>
</td></tr>
</table>
</td></tr>
</table>
</body></html>"#.into(),
    }
}

fn default_magic_link() -> EmailTemplate {
    EmailTemplate {
        name: TEMPLATE_MAGIC_LINK.into(),
        subject: "Sign in to {{ .AppName }}".into(),
        text: "Click the link below to sign in to your {{ .AppName }} account:\n\n\
               {{ .ActionURL }}\n\n\
               This link expires in {{ .ExpiresIn }}.\n\n\
               If you didn't request this, you can safely ignore this email.".into(),
        html: r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"></head>
<body style="font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;margin:0;padding:0;background:#f5f5f5">
<table width="100%" cellpadding="0" cellspacing="0" style="padding:40px 0">
<tr><td align="center">
<table width="600" cellpadding="0" cellspacing="0" style="background:#fff;border-radius:8px;padding:40px;border:1px solid #e0e0e0">
<tr><td>
<h2 style="margin:0 0 20px;color:#1a1a1a">Sign In to {{ .AppName }}</h2>
<p style="color:#333;line-height:1.6;margin:0 0 24px">Click the button below to sign in. No password needed.</p>
<p style="margin:0 0 24px">
<a href="{{ .ActionURL }}" style="display:inline-block;background:#2563eb;color:#fff;padding:12px 32px;border-radius:6px;text-decoration:none;font-weight:600">Sign In</a>
</p>
<p style="color:#666;font-size:14px;line-height:1.5;margin:0 0 16px">Or copy and paste this link:<br>
<a href="{{ .ActionURL }}" style="color:#2563eb;word-break:break-all">{{ .ActionURL }}</a></p>
<p style="color:#999;font-size:13px;margin:16px 0 0">This link expires in {{ .ExpiresIn }}. If you didn't request this, ignore this email.</p>
</td></tr>
</table>
</td></tr>
</table>
</body></html>"#.into(),
    }
}

fn default_email_change() -> EmailTemplate {
    EmailTemplate {
        name: TEMPLATE_EMAIL_CHANGE.into(),
        subject: "Confirm your new email — {{ .AppName }}".into(),
        text: "You requested to change your email to {{ .NewEmail }}.\n\n\
               Please confirm by clicking the link below:\n\n\
               {{ .ActionURL }}\n\n\
               This link expires in {{ .ExpiresIn }}.\n\n\
               If you didn't request this, you can safely ignore this email.".into(),
        html: r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"></head>
<body style="font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;margin:0;padding:0;background:#f5f5f5">
<table width="100%" cellpadding="0" cellspacing="0" style="padding:40px 0">
<tr><td align="center">
<table width="600" cellpadding="0" cellspacing="0" style="background:#fff;border-radius:8px;padding:40px;border:1px solid #e0e0e0">
<tr><td>
<h2 style="margin:0 0 20px;color:#1a1a1a">Confirm Your New Email</h2>
<p style="color:#333;line-height:1.6;margin:0 0 24px">You requested to change your email to <strong>{{ .NewEmail }}</strong>. Click below to confirm.</p>
<p style="margin:0 0 24px">
<a href="{{ .ActionURL }}" style="display:inline-block;background:#2563eb;color:#fff;padding:12px 32px;border-radius:6px;text-decoration:none;font-weight:600">Confirm Email Change</a>
</p>
<p style="color:#666;font-size:14px;line-height:1.5;margin:0 0 16px">Or copy and paste this link:<br>
<a href="{{ .ActionURL }}" style="color:#2563eb;word-break:break-all">{{ .ActionURL }}</a></p>
<p style="color:#999;font-size:13px;margin:16px 0 0">This link expires in {{ .ExpiresIn }}. If you didn't request this, ignore this email.</p>
</td></tr>
</table>
</td></tr>
</table>
</body></html>"#.into(),
    }
}

fn default_welcome() -> EmailTemplate {
    EmailTemplate {
        name: TEMPLATE_WELCOME.into(),
        subject: "Welcome to {{ .AppName }}!".into(),
        text: "Welcome to {{ .AppName }}, {{ .UserName }}!\n\n\
               Your account has been created successfully.\n\n\
               Get started at {{ .SiteURL }}".into(),
        html: r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"></head>
<body style="font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;margin:0;padding:0;background:#f5f5f5">
<table width="100%" cellpadding="0" cellspacing="0" style="padding:40px 0">
<tr><td align="center">
<table width="600" cellpadding="0" cellspacing="0" style="background:#fff;border-radius:8px;padding:40px;border:1px solid #e0e0e0">
<tr><td>
<h2 style="margin:0 0 20px;color:#1a1a1a">Welcome to {{ .AppName }}!</h2>
<p style="color:#333;line-height:1.6;margin:0 0 24px">Hi {{ .UserName }}, your account has been created successfully.</p>
<p style="margin:0 0 24px">
<a href="{{ .SiteURL }}" style="display:inline-block;background:#2563eb;color:#fff;padding:12px 32px;border-radius:6px;text-decoration:none;font-weight:600">Get Started</a>
</p>
</td></tr>
</table>
</td></tr>
</table>
</body></html>"#.into(),
    }
}

// ── French Templates (Bill 96 / Quebec compliance) ─────────────────

fn default_verify_email_fr() -> EmailTemplate {
    EmailTemplate {
        name: TEMPLATE_VERIFY_EMAIL_FR.into(),
        subject: "Vérifiez votre courriel — {{ .AppName }}".into(),
        text: "Bienvenue sur {{ .AppName }} !\n\nVeuillez vérifier votre courriel :\n\n{{ .ActionURL }}\n\nCe lien expire dans {{ .ExpiresIn }}.".into(),
        html: r#"<!DOCTYPE html><html lang="fr"><head><meta charset="utf-8"></head><body style="font-family:sans-serif;margin:0;padding:0;background:#f5f5f5"><table width="100%" cellpadding="0" cellspacing="0" style="padding:40px 0"><tr><td align="center"><table width="600" cellpadding="0" cellspacing="0" style="background:#fff;border-radius:8px;padding:40px;border:1px solid #e0e0e0"><tr><td><h2 style="margin:0 0 20px;color:#1a1a1a">Vérifiez votre courriel</h2><p style="color:#333;line-height:1.6;margin:0 0 24px">Cliquez ci-dessous pour vérifier votre adresse courriel.</p><p style="margin:0 0 24px"><a href="{{ .ActionURL }}" style="display:inline-block;background:#2563eb;color:#fff;padding:12px 32px;border-radius:6px;text-decoration:none;font-weight:600">Vérifier</a></p><p style="color:#999;font-size:13px">Ce lien expire dans {{ .ExpiresIn }}.</p></td></tr></table></td></tr></table></body></html>"#.into(),
    }
}

fn default_reset_password_fr() -> EmailTemplate {
    EmailTemplate {
        name: TEMPLATE_RESET_PASSWORD_FR.into(),
        subject: "Réinitialiser votre mot de passe — {{ .AppName }}".into(),
        text: "Réinitialisez votre mot de passe :\n\n{{ .ActionURL }}\n\nCe lien expire dans {{ .ExpiresIn }}.".into(),
        html: r#"<!DOCTYPE html><html lang="fr"><head><meta charset="utf-8"></head><body style="font-family:sans-serif;margin:0;padding:0;background:#f5f5f5"><table width="100%" cellpadding="0" cellspacing="0" style="padding:40px 0"><tr><td align="center"><table width="600" cellpadding="0" cellspacing="0" style="background:#fff;border-radius:8px;padding:40px;border:1px solid #e0e0e0"><tr><td><h2 style="margin:0 0 20px;color:#1a1a1a">Réinitialiser le mot de passe</h2><p style="color:#333;line-height:1.6;margin:0 0 24px">Cliquez ci-dessous pour réinitialiser votre mot de passe.</p><p style="margin:0 0 24px"><a href="{{ .ActionURL }}" style="display:inline-block;background:#2563eb;color:#fff;padding:12px 32px;border-radius:6px;text-decoration:none;font-weight:600">Réinitialiser</a></p><p style="color:#999;font-size:13px">Ce lien expire dans {{ .ExpiresIn }}.</p></td></tr></table></td></tr></table></body></html>"#.into(),
    }
}

fn default_mfa_alert_fr() -> EmailTemplate {
    EmailTemplate {
        name: TEMPLATE_MFA_ALERT_FR.into(),
        subject: "Alerte de sécurité — {{ .AppName }}".into(),
        text: "Un changement MFA a été détecté sur votre compte {{ .AppName }}.\n\nSi ce n'était pas vous, sécurisez votre compte immédiatement.".into(),
        html: r#"<!DOCTYPE html><html lang="fr"><head><meta charset="utf-8"></head><body style="font-family:sans-serif;margin:0;padding:0;background:#f5f5f5"><table width="100%" cellpadding="0" cellspacing="0" style="padding:40px 0"><tr><td align="center"><table width="600" cellpadding="0" cellspacing="0" style="background:#fff;border-radius:8px;padding:40px;border:1px solid #e0e0e0"><tr><td><h2 style="margin:0 0 20px;color:#c0392b">Alerte de sécurité</h2><p style="color:#333;line-height:1.6;margin:0 0 24px">Un changement MFA a été détecté sur votre compte.</p><p style="color:#999;font-size:13px">Si ce n'était pas vous, sécurisez votre compte immédiatement.</p></td></tr></table></td></tr></table></body></html>"#.into(),
    }
}

fn default_magic_link_fr() -> EmailTemplate {
    EmailTemplate {
        name: TEMPLATE_MAGIC_LINK_FR.into(),
        subject: "Votre lien de connexion — {{ .AppName }}".into(),
        text: "Connectez-vous à {{ .AppName }} :\n\n{{ .ActionURL }}\n\nCe lien expire dans {{ .ExpiresIn }}.".into(),
        html: r#"<!DOCTYPE html><html lang="fr"><head><meta charset="utf-8"></head><body style="font-family:sans-serif;margin:0;padding:0;background:#f5f5f5"><table width="100%" cellpadding="0" cellspacing="0" style="padding:40px 0"><tr><td align="center"><table width="600" cellpadding="0" cellspacing="0" style="background:#fff;border-radius:8px;padding:40px;border:1px solid #e0e0e0"><tr><td><h2 style="margin:0 0 20px;color:#1a1a1a">Lien de connexion</h2><p style="color:#333;line-height:1.6;margin:0 0 24px">Cliquez ci-dessous pour vous connecter.</p><p style="margin:0 0 24px"><a href="{{ .ActionURL }}" style="display:inline-block;background:#2563eb;color:#fff;padding:12px 32px;border-radius:6px;text-decoration:none;font-weight:600">Se connecter</a></p><p style="color:#999;font-size:13px">Ce lien expire dans {{ .ExpiresIn }}.</p></td></tr></table></td></tr></table></body></html>"#.into(),
    }
}

fn default_email_change_fr() -> EmailTemplate {
    EmailTemplate {
        name: TEMPLATE_EMAIL_CHANGE_FR.into(),
        subject: "Confirmez le changement de courriel — {{ .AppName }}".into(),
        text: "Confirmez le changement de courriel :\n\n{{ .ActionURL }}\n\nCe lien expire dans {{ .ExpiresIn }}.".into(),
        html: r#"<!DOCTYPE html><html lang="fr"><head><meta charset="utf-8"></head><body style="font-family:sans-serif;margin:0;padding:0;background:#f5f5f5"><table width="100%" cellpadding="0" cellspacing="0" style="padding:40px 0"><tr><td align="center"><table width="600" cellpadding="0" cellspacing="0" style="background:#fff;border-radius:8px;padding:40px;border:1px solid #e0e0e0"><tr><td><h2 style="margin:0 0 20px;color:#1a1a1a">Changement de courriel</h2><p style="color:#333;line-height:1.6;margin:0 0 24px">Confirmez votre nouvelle adresse courriel.</p><p style="margin:0 0 24px"><a href="{{ .ActionURL }}" style="display:inline-block;background:#2563eb;color:#fff;padding:12px 32px;border-radius:6px;text-decoration:none;font-weight:600">Confirmer</a></p><p style="color:#999;font-size:13px">Ce lien expire dans {{ .ExpiresIn }}.</p></td></tr></table></td></tr></table></body></html>"#.into(),
    }
}

fn default_welcome_fr() -> EmailTemplate {
    EmailTemplate {
        name: TEMPLATE_WELCOME_FR.into(),
        subject: "Bienvenue sur {{ .AppName }} !".into(),
        text: "Bienvenue sur {{ .AppName }}, {{ .UserName }} !\n\nVotre compte a été créé.\n\nCommencez : {{ .SiteURL }}".into(),
        html: r#"<!DOCTYPE html><html lang="fr"><head><meta charset="utf-8"></head><body style="font-family:sans-serif;margin:0;padding:0;background:#f5f5f5"><table width="100%" cellpadding="0" cellspacing="0" style="padding:40px 0"><tr><td align="center"><table width="600" cellpadding="0" cellspacing="0" style="background:#fff;border-radius:8px;padding:40px;border:1px solid #e0e0e0"><tr><td><h2 style="margin:0 0 20px;color:#1a1a1a">Bienvenue sur {{ .AppName }} !</h2><p style="color:#333;line-height:1.6;margin:0 0 24px">Bonjour {{ .UserName }}, votre compte a été créé.</p><p style="margin:0 0 24px"><a href="{{ .SiteURL }}" style="display:inline-block;background:#2563eb;color:#fff;padding:12px 32px;border-radius:6px;text-decoration:none;font-weight:600">Commencer</a></p></td></tr></table></td></tr></table></body></html>"#.into(),
    }
}

// ── Email Config ───────────────────────────────────────────────────

/// Configuration for the email service.
///
/// ## Deliverability Checklist (DNS — not handled in code)
///
/// To avoid spam folders, configure these DNS records for your sending domain:
/// 1. **SPF**: `v=spf1 include:spf.email.com ~all` (adjust for your provider)
/// 2. **DKIM**: Add the TXT record your SMTP provider gives you
/// 3. **DMARC**: `v=DMARC1; p=quarantine; rua=mailto:dmarc@yourdomain.com`
/// 4. **Return-Path**: Must match the From domain (automatic with most providers)
///
/// Use a subdomain like `mail.yourdomain.com` for transactional email to isolate reputation.
///
/// ## Provider Examples
///
/// ### Postal (recommended)
/// ```env
/// OB_EMAIL__SMTP_HOST=in-v3.email.com
/// OB_EMAIL__SMTP_PORT=587
/// OB_EMAIL__SMTP_USER=<your-api-key>
/// OB_EMAIL__SMTP_PASSWORD=<your-secret-key>
/// ```
///
/// ### SendGrid
/// ```env
/// OB_EMAIL__SMTP_HOST=smtp.sendgrid.net
/// OB_EMAIL__SMTP_PORT=587
/// OB_EMAIL__SMTP_USER=apikey
/// OB_EMAIL__SMTP_PASSWORD=<your-sendgrid-api-key>
/// ```
///
/// ### Amazon SES
/// ```env
/// OB_EMAIL__SMTP_HOST=email-smtp.us-east-1.amazonaws.com
/// OB_EMAIL__SMTP_PORT=587
/// OB_EMAIL__SMTP_USER=<ses-smtp-user>
/// OB_EMAIL__SMTP_PASSWORD=<ses-smtp-password>
/// ```
///
/// ### Resend
/// ```env
/// OB_EMAIL__SMTP_HOST=smtp.resend.com
/// OB_EMAIL__SMTP_PORT=465
/// OB_EMAIL__SMTP_USER=resend
/// OB_EMAIL__SMTP_PASSWORD=<your-resend-api-key>
/// ```
#[derive(Debug, Clone)]
pub struct EmailConfig {
    /// From address with display name, e.g. `"MyApp <noreply@myapp.com>"`
    pub from: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    pub smtp_password: String,
    /// Optional Reply-To address (e.g. `"support@myapp.com"`)
    pub reply_to: Option<String>,
    /// Application name used in templates (default: "OrignaBase")
    pub app_name: String,
    /// Site URL used in templates (default: base_url from AuthState)
    pub site_url: Option<String>,
}

impl EmailConfig {
    /// Create from environment variables (OB_EMAIL__*).
    ///
    /// Required:
    ///   OB_EMAIL__FROM, OB_EMAIL__SMTP_HOST, OB_EMAIL__SMTP_PORT,
    ///   OB_EMAIL__SMTP_USER, OB_EMAIL__SMTP_PASSWORD
    ///
    /// Optional:
    ///   OB_EMAIL__FROM_NAME (default: "OrignaBase")
    ///   OB_EMAIL__REPLY_TO
    ///   OB_EMAIL__APP_NAME (default: "OrignaBase")
    ///   OB_EMAIL__SITE_URL
    pub fn from_env() -> Option<Self> {
        let from_addr = std::env::var("OB_EMAIL__FROM").ok()?;
        let from_name =
            std::env::var("OB_EMAIL__FROM_NAME").unwrap_or_else(|_| "OrignaBase".into());

        Some(Self {
            from: format!("{from_name} <{from_addr}>"),
            smtp_host: std::env::var("OB_EMAIL__SMTP_HOST").ok()?,
            smtp_port: std::env::var("OB_EMAIL__SMTP_PORT").ok()?.parse().ok()?,
            smtp_user: std::env::var("OB_EMAIL__SMTP_USER").ok()?,
            smtp_password: std::env::var("OB_EMAIL__SMTP_PASSWORD").ok()?,
            reply_to: std::env::var("OB_EMAIL__REPLY_TO").ok(),
            app_name: std::env::var("OB_EMAIL__APP_NAME").unwrap_or_else(|_| "OrignaBase".into()),
            site_url: std::env::var("OB_EMAIL__SITE_URL").ok(),
        })
    }
}

// ── Email Service ──────────────────────────────────────────────────

/// Email service with customizable templates.
///
/// Templates are loaded from DB on first use, with embedded defaults as fallback.
/// Admins can customize templates via the admin API.
#[derive(Clone)]
pub struct EmailService {
    config: EmailConfig,
    db: Option<ob_database::DatabaseClient>,
}

impl EmailService {
    pub fn new(config: EmailConfig) -> Self {
        Self { config, db: None }
    }

    /// Create with database access for custom template storage.
    pub fn with_db(config: EmailConfig, db: ob_database::DatabaseClient) -> Self {
        Self {
            config,
            db: Some(db),
        }
    }

    /// Build the SMTP transport.
    fn transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
        let creds = Credentials::new(
            self.config.smtp_user.clone(),
            self.config.smtp_password.clone(),
        );

        Ok(
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.config.smtp_host)
                .map_err(|e| Error::Internal(format!("SMTP relay error: {e}")))?
                .port(self.config.smtp_port)
                .credentials(creds)
                .build(),
        )
    }

    /// Get a template by name. Checks DB first, falls back to embedded default.
    async fn get_template(&self, name: &str) -> EmailTemplate {
        // Try DB first
        if let Some(ref db) = self.db
            && let Ok(doc) = db.get_document("_email_templates", name).await
            && !doc.is_null()
            && let Ok(template) = serde_json::from_value::<EmailTemplate>(doc)
        {
            return template;
        }

        if let Some(ref db) = self.db
            && let Ok(results) = db.list_documents("_email_templates", None, None).await
            && let Some(doc) = results
                .into_iter()
                .find(|row| row.get(fields::NAME).and_then(|value| value.as_str()) == Some(name))
            && let Ok(template) = serde_json::from_value::<EmailTemplate>(doc)
        {
            return template;
        }

        // Fall back to embedded default
        default_templates()
            .into_iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| EmailTemplate {
                name: name.into(),
                subject: name.into(),
                text: String::new(),
                html: String::new(),
            })
    }

    /// Build standard template variables.
    fn base_vars(&self) -> HashMap<String, String> {
        let mut vars = HashMap::new();
        vars.insert("AppName".into(), self.config.app_name.clone());
        if let Some(ref url) = self.config.site_url {
            vars.insert("SiteURL".into(), url.clone());
        }
        vars
    }

    /// Send a verification email with a token link.
    /// Uses French template if `lang == "fr"` (Bill 96 compliance).
    pub async fn send_verification_email(
        &self,
        to: &str,
        token: &str,
        base_url: &str,
        lang: &str,
    ) -> Result<()> {
        let tpl_name = template_for_lang(TEMPLATE_VERIFY_EMAIL, lang);
        let template = self.get_template(tpl_name).await;
        let mut vars = self.base_vars();
        vars.insert(
            "ActionURL".into(),
            format!("{base_url}/auth/verify-email?token={token}"),
        );
        vars.insert("Token".into(), token.into());
        vars.insert("Email".into(), to.into());
        let expires = if lang == "fr" {
            "24 heures"
        } else {
            "24 hours"
        };
        vars.insert("ExpiresIn".into(), expires.into());
        vars.insert("SiteURL".into(), base_url.into());

        let rendered = template.render(&vars);
        self.send_multipart(to, &rendered.subject, &rendered.text, &rendered.html)
            .await
    }

    /// Send a password reset email.
    /// Uses French template if `lang == "fr"`.
    pub async fn send_reset_email(
        &self,
        to: &str,
        token: &str,
        base_url: &str,
        lang: &str,
    ) -> Result<()> {
        let tpl_name = template_for_lang(TEMPLATE_RESET_PASSWORD, lang);
        let template = self.get_template(tpl_name).await;
        let mut vars = self.base_vars();
        vars.insert(
            "ActionURL".into(),
            format!("{base_url}/auth/reset-password?token={token}"),
        );
        vars.insert("Token".into(), token.into());
        vars.insert("Email".into(), to.into());
        let expires = if lang == "fr" { "1 heure" } else { "1 hour" };
        vars.insert("ExpiresIn".into(), expires.into());
        vars.insert("SiteURL".into(), base_url.into());

        let rendered = template.render(&vars);
        self.send_multipart(to, &rendered.subject, &rendered.text, &rendered.html)
            .await
    }

    /// Send an MFA change alert.
    /// Uses French template if `lang == "fr"`.
    pub async fn send_mfa_alert(&self, to: &str, action: &str, lang: &str) -> Result<()> {
        let tpl_name = template_for_lang(TEMPLATE_MFA_ALERT, lang);
        let template = self.get_template(tpl_name).await;
        let mut vars = self.base_vars();
        let action_label = if lang == "fr" {
            match action {
                "enabled" => "activée",
                "disabled" => "désactivée",
                _ => action,
            }
        } else {
            action
        };
        vars.insert("Action".into(), action_label.into());
        vars.insert("Email".into(), to.into());

        let rendered = template.render(&vars);
        self.send_multipart(to, &rendered.subject, &rendered.text, &rendered.html)
            .await
    }

    /// Send a magic link sign-in email.
    /// Uses French template if `lang == "fr"`.
    pub async fn send_magic_link_email(
        &self,
        to: &str,
        token: &str,
        base_url: &str,
        lang: &str,
    ) -> Result<()> {
        let tpl_name = template_for_lang(TEMPLATE_MAGIC_LINK, lang);
        let template = self.get_template(tpl_name).await;
        let mut vars = self.base_vars();
        vars.insert(
            "ActionURL".into(),
            format!("{base_url}/auth/verify-magic-link?token={token}"),
        );
        vars.insert("Token".into(), token.into());
        vars.insert("Email".into(), to.into());
        vars.insert("ExpiresIn".into(), "15 minutes".into());
        vars.insert("SiteURL".into(), base_url.into());

        let rendered = template.render(&vars);
        self.send_multipart(to, &rendered.subject, &rendered.text, &rendered.html)
            .await
    }

    /// Send a welcome email to a newly created user.
    /// Uses French template if `lang == "fr"`.
    pub async fn send_welcome_email(&self, to: &str, lang: &str) -> Result<()> {
        let tpl_name = template_for_lang(TEMPLATE_WELCOME, lang);
        let template = self.get_template(tpl_name).await;
        let mut vars = self.base_vars();
        vars.insert("Email".into(), to.to_string());
        vars.insert(
            "UserName".into(),
            to.split('@').next().unwrap_or("User").to_string(),
        );

        let rendered = template.render(&vars);
        self.send_multipart(to, &rendered.subject, &rendered.text, &rendered.html)
            .await
    }

    /// Send a multipart HTML + plain text email with proper deliverability headers.
    /// CASL footer is automatically appended to all auth emails.
    async fn send_multipart(&self, to: &str, subject: &str, plain: &str, html: &str) -> Result<()> {
        // Append CASL footer for compliance
        let plain = [plain, CASL_FOOTER_TEXT].concat();
        let html = append_casl_footer_html(html);
        let mut builder = Message::builder()
            .from(
                self.config
                    .from
                    .parse()
                    .map_err(|e| Error::Internal(format!("Invalid from address: {e}")))?,
            )
            .to(to
                .parse()
                .map_err(|e| Error::Internal(format!("Invalid recipient address: {e}")))?)
            .subject(subject);

        // Reply-To header — important for deliverability
        if let Some(ref reply_to) = self.config.reply_to {
            builder = builder.reply_to(
                reply_to
                    .parse()
                    .map_err(|e| Error::Internal(format!("Invalid reply-to address: {e}")))?,
            );
        }

        let email = builder
            .multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(plain.to_string()),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(html.to_string()),
                    ),
            )
            .map_err(|e| Error::Internal(format!("Email build failed: {e}")))?;

        let transport = self.transport()?;
        transport
            .send(email)
            .await
            .map_err(|e| Error::Internal(format!("Email send failed: {e}")))?;

        tracing::info!(to = to, subject = subject, "Email sent successfully");
        Ok(())
    }

    // ── Admin Template Management ──────────────────────────────────

    /// List all templates (custom + defaults for any not customized).
    pub async fn list_templates(&self) -> Result<Vec<EmailTemplate>> {
        let defaults = default_templates();

        let Some(ref db) = self.db else {
            return Ok(defaults);
        };

        let raw = db.list_documents("_email_templates", None, None).await?;
        let custom: Vec<EmailTemplate> = raw
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();

        // Merge: custom overrides defaults by name
        let custom_names: std::collections::HashSet<_> =
            custom.iter().map(|t| t.name.clone()).collect();

        let mut result = custom;
        for d in defaults {
            if !custom_names.contains(&d.name) {
                result.push(d);
            }
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }

    /// Get a single template (custom or default).
    pub async fn get_template_by_name(&self, name: &str) -> Result<EmailTemplate> {
        Ok(self.get_template(name).await)
    }

    /// Save a custom template to the database.
    pub async fn save_template(&self, template: EmailTemplate) -> Result<()> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| Error::Internal("Database not configured for email templates".into()))?;

        let template_name = template.name.clone();
        db.upsert_document(
            "_email_templates",
            &template_name,
            serde_json::to_value(&template)
                .map_err(|e| Error::Internal(format!("Template serialization failed: {e}")))?,
        )
        .await?;

        Ok(())
    }

    /// Reset a template to its default.
    pub async fn reset_template(&self, name: &str) -> Result<EmailTemplate> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| Error::Internal("Database not configured for email templates".into()))?;

        let _ = db.delete_document("_email_templates", name).await;

        default_templates()
            .into_iter()
            .find(|t| t.name == name)
            .ok_or_else(|| Error::NotFound(format!("No default template named '{name}'")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_replace_vars_spaced() {
        let mut vars = HashMap::new();
        vars.insert("AppName".into(), "TestApp".into());
        vars.insert("ActionURL".into(), "https://example.com/verify".into());

        let result = replace_vars("Welcome to {{ .AppName }}! Click {{ .ActionURL }}", &vars);
        assert_eq!(
            result,
            "Welcome to TestApp! Click https://example.com/verify"
        );
    }

    #[test]
    fn test_replace_vars_no_spaces() {
        let mut vars = HashMap::new();
        vars.insert("AppName".into(), "TestApp".into());

        let result = replace_vars("Hello {{.AppName}}", &vars);
        assert_eq!(result, "Hello TestApp");
    }

    #[test]
    fn test_replace_vars_missing_key_unchanged() {
        let vars = HashMap::new();
        let result = replace_vars("Hello {{ .Unknown }}", &vars);
        assert_eq!(result, "Hello {{ .Unknown }}");
    }

    #[test]
    fn test_replace_vars_empty_template() {
        let vars = HashMap::new();
        let result = replace_vars("", &vars);
        assert_eq!(result, "");
    }

    #[test]
    fn test_replace_vars_no_placeholders() {
        let mut vars = HashMap::new();
        vars.insert("Key".into(), "val".into());
        let result = replace_vars("No placeholders here", &vars);
        assert_eq!(result, "No placeholders here");
    }

    #[test]
    fn test_replace_vars_multiple_same_key() {
        let mut vars = HashMap::new();
        vars.insert("Name".into(), "Alice".into());
        let result = replace_vars("{{ .Name }} and {{ .Name }}", &vars);
        assert_eq!(result, "Alice and Alice");
    }

    #[test]
    fn test_template_render() {
        let template = EmailTemplate {
            name: "test".into(),
            subject: "Welcome {{ .UserName }}".into(),
            html: "<h1>Hi {{ .UserName }}</h1>".into(),
            text: "Hi {{ .UserName }}".into(),
        };
        let mut vars = HashMap::new();
        vars.insert("UserName".into(), "Alice".into());
        let rendered = template.render(&vars);
        assert_eq!(rendered.subject, "Welcome Alice");
        assert_eq!(rendered.html, "<h1>Hi Alice</h1>");
        assert_eq!(rendered.text, "Hi Alice");
    }

    #[test]
    fn test_default_templates_count() {
        let templates = default_templates();
        assert_eq!(templates.len(), 12); // 6 English + 6 French
    }

    #[test]
    fn test_default_template_names() {
        let templates = default_templates();
        let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&TEMPLATE_VERIFY_EMAIL));
        assert!(names.contains(&TEMPLATE_RESET_PASSWORD));
        assert!(names.contains(&TEMPLATE_MFA_ALERT));
        assert!(names.contains(&TEMPLATE_MAGIC_LINK));
        assert!(names.contains(&TEMPLATE_EMAIL_CHANGE));
        assert!(names.contains(&TEMPLATE_WELCOME));
    }

    #[test]
    fn test_default_templates_have_variables() {
        for template in default_templates() {
            assert!(
                template.subject.contains("{{ .AppName }}")
                    || template.subject.contains("{{.AppName}}"),
                "Template '{}' subject should contain AppName variable",
                template.name
            );
        }
    }

    #[test]
    fn test_verify_email_template_render() {
        let template = default_verify_email();
        let mut vars = HashMap::new();
        vars.insert("AppName".into(), "MyApp".into());
        vars.insert(
            "ActionURL".into(),
            "https://myapp.com/verify?token=abc".into(),
        );
        vars.insert("ExpiresIn".into(), "24 hours".into());

        let rendered = template.render(&vars);
        assert!(rendered.subject.contains("MyApp"));
        assert!(rendered.html.contains("https://myapp.com/verify?token=abc"));
        assert!(rendered.text.contains("24 hours"));
    }

    #[test]
    fn test_email_config_from_parts() {
        let config = EmailConfig {
            from: "MyApp <noreply@myapp.com>".into(),
            smtp_host: "in-v3.email.com".into(),
            smtp_port: 587,
            smtp_user: "api-key".into(),
            smtp_password: "secret-key".into(),
            reply_to: Some("support@myapp.com".into()),
            app_name: "MyApp".into(),
            site_url: Some("https://myapp.com".into()),
        };
        assert_eq!(config.smtp_port, 587);
        assert!(config.from.contains("MyApp"));
        assert_eq!(config.app_name, "MyApp");
    }

    #[test]
    fn test_email_service_new() {
        let config = EmailConfig {
            from: "Test <test@test.com>".into(),
            smtp_host: "localhost".into(),
            smtp_port: 25,
            smtp_user: "u".into(),
            smtp_password: "p".into(),
            reply_to: None,
            app_name: "Test".into(),
            site_url: None,
        };
        let service = EmailService::new(config);
        assert!(service.db.is_none());
    }

    #[test]
    fn test_email_service_with_db() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let db = rt.block_on(ob_database::DatabaseClient::new_mem());
        let config = EmailConfig {
            from: "Test <test@test.com>".into(),
            smtp_host: "localhost".into(),
            smtp_port: 25,
            smtp_user: "u".into(),
            smtp_password: "p".into(),
            reply_to: None,
            app_name: "Test".into(),
            site_url: None,
        };
        let service = EmailService::with_db(config, db);
        assert!(service.db.is_some());
    }

    #[test]
    fn test_base_vars() {
        let config = EmailConfig {
            from: "Test <test@test.com>".into(),
            smtp_host: "localhost".into(),
            smtp_port: 25,
            smtp_user: "u".into(),
            smtp_password: "p".into(),
            reply_to: None,
            app_name: "CoolApp".into(),
            site_url: Some("https://cool.app".into()),
        };
        let service = EmailService::new(config);
        let vars = service.base_vars();
        assert_eq!(vars.get("AppName").unwrap(), "CoolApp");
        assert_eq!(vars.get("SiteURL").unwrap(), "https://cool.app");
    }

    #[test]
    fn test_base_vars_without_site_url() {
        let config = EmailConfig {
            from: "Test <test@test.com>".into(),
            smtp_host: "localhost".into(),
            smtp_port: 25,
            smtp_user: "u".into(),
            smtp_password: "p".into(),
            reply_to: None,
            app_name: "TestApp".into(),
            site_url: None,
        };
        let service = EmailService::new(config);
        let vars = service.base_vars();
        assert_eq!(vars.get("AppName").unwrap(), "TestApp");
        assert!(!vars.contains_key("SiteURL"));
    }

    #[test]
    fn test_mfa_alert_template_variables() {
        let template = default_mfa_alert();
        let mut vars = HashMap::new();
        vars.insert("AppName".into(), "MyApp".into());
        vars.insert("Action".into(), "enabled".into());

        let rendered = template.render(&vars);
        assert!(rendered.subject.contains("enabled"));
        assert!(rendered.subject.contains("MyApp"));
        assert!(rendered.html.contains("enabled"));
    }

    #[test]
    fn test_email_template_serialization() {
        let template = default_verify_email();
        let json = serde_json::to_value(&template).unwrap();
        assert_eq!(json[fields::NAME], TEMPLATE_VERIFY_EMAIL);
        assert!(json["subject"].as_str().unwrap().contains("{{ .AppName }}"));

        let deserialized: EmailTemplate = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.name, template.name);
    }

    #[test]
    fn test_reset_password_template_render() {
        let template = default_reset_password();
        let mut vars = HashMap::new();
        vars.insert("AppName".into(), "MyApp".into());
        vars.insert(
            "ActionURL".into(),
            "https://myapp.com/reset?token=xyz".into(),
        );
        vars.insert("ExpiresIn".into(), "1 hour".into());

        let rendered = template.render(&vars);
        assert!(rendered.subject.contains("MyApp"));
        assert!(rendered.html.contains("Reset Password"));
        assert!(rendered.text.contains("1 hour"));
    }

    #[test]
    fn test_magic_link_template_render() {
        let template = default_magic_link();
        let mut vars = HashMap::new();
        vars.insert("AppName".into(), "MyApp".into());
        vars.insert(
            "ActionURL".into(),
            "https://myapp.com/magic?token=abc".into(),
        );
        vars.insert("ExpiresIn".into(), "15 minutes".into());

        let rendered = template.render(&vars);
        assert!(rendered.subject.contains("MyApp"));
        assert!(rendered.html.contains("Sign In"));
        assert!(rendered.text.contains("15 minutes"));
    }

    #[test]
    fn test_email_change_template_render() {
        let template = default_email_change();
        let mut vars = HashMap::new();
        vars.insert("AppName".into(), "MyApp".into());
        vars.insert("NewEmail".into(), "new@example.com".into());
        vars.insert("ActionURL".into(), "https://myapp.com/confirm".into());
        vars.insert("ExpiresIn".into(), "24 hours".into());

        let rendered = template.render(&vars);
        assert!(rendered.html.contains("new@example.com"));
        assert!(rendered.text.contains("new@example.com"));
    }

    #[test]
    fn test_welcome_template_render() {
        let template = default_welcome();
        let mut vars = HashMap::new();
        vars.insert("AppName".into(), "MyApp".into());
        vars.insert("UserName".into(), "Alice".into());
        vars.insert("SiteURL".into(), "https://myapp.com".into());

        let rendered = template.render(&vars);
        assert!(rendered.subject.contains("MyApp"));
        assert!(rendered.html.contains("Alice"));
        assert!(rendered.text.contains("Alice"));
    }

    #[test]
    fn test_default_templates_all_have_text_and_html() {
        for template in default_templates() {
            assert!(
                !template.text.is_empty(),
                "Template '{}' has empty text",
                template.name
            );
            assert!(
                !template.html.is_empty(),
                "Template '{}' has empty html",
                template.name
            );
            assert!(
                !template.subject.is_empty(),
                "Template '{}' has empty subject",
                template.name
            );
        }
    }

    #[test]
    fn test_email_config_from_env_missing_required() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::remove_var("OB_EMAIL__FROM");
            std::env::remove_var("OB_EMAIL__SMTP_HOST");
            std::env::remove_var("OB_EMAIL__SMTP_PORT");
            std::env::remove_var("OB_EMAIL__SMTP_USER");
            std::env::remove_var("OB_EMAIL__SMTP_PASSWORD");
        }
        let config = EmailConfig::from_env();
        assert!(config.is_none());
    }

    #[test]
    fn test_email_config_from_env_all_set() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var("OB_EMAIL__FROM", "noreply@myapp.com");
            std::env::set_var("OB_EMAIL__SMTP_HOST", "smtp.myapp.com");
            std::env::set_var("OB_EMAIL__SMTP_PORT", "587");
            std::env::set_var("OB_EMAIL__SMTP_USER", "user");
            std::env::set_var("OB_EMAIL__SMTP_PASSWORD", "pass");
            std::env::set_var("OB_EMAIL__APP_NAME", "TestApp");
        }
        let config = EmailConfig::from_env().unwrap();
        assert_eq!(config.smtp_host, "smtp.myapp.com");
        assert_eq!(config.smtp_port, 587);
        assert_eq!(config.app_name, "TestApp");
        assert!(config.from.contains("noreply@myapp.com"));

        unsafe {
            std::env::remove_var("OB_EMAIL__FROM");
            std::env::remove_var("OB_EMAIL__SMTP_HOST");
            std::env::remove_var("OB_EMAIL__SMTP_PORT");
            std::env::remove_var("OB_EMAIL__SMTP_USER");
            std::env::remove_var("OB_EMAIL__SMTP_PASSWORD");
            std::env::remove_var("OB_EMAIL__APP_NAME");
        }
    }

    #[test]
    fn test_email_config_from_env_with_optional() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var("OB_EMAIL__FROM", "noreply@test.com");
            std::env::set_var("OB_EMAIL__SMTP_HOST", "smtp.test.com");
            std::env::set_var("OB_EMAIL__SMTP_PORT", "465");
            std::env::set_var("OB_EMAIL__SMTP_USER", "u");
            std::env::set_var("OB_EMAIL__SMTP_PASSWORD", "p");
            std::env::set_var("OB_EMAIL__REPLY_TO", "support@test.com");
            std::env::set_var("OB_EMAIL__SITE_URL", "https://test.com");
            std::env::set_var("OB_EMAIL__FROM_NAME", "TestSender");
        }
        let config = EmailConfig::from_env().unwrap();
        assert_eq!(config.reply_to, Some("support@test.com".into()));
        assert_eq!(config.site_url, Some("https://test.com".into()));
        assert!(config.from.contains("TestSender"));

        unsafe {
            std::env::remove_var("OB_EMAIL__FROM");
            std::env::remove_var("OB_EMAIL__SMTP_HOST");
            std::env::remove_var("OB_EMAIL__SMTP_PORT");
            std::env::remove_var("OB_EMAIL__SMTP_USER");
            std::env::remove_var("OB_EMAIL__SMTP_PASSWORD");
            std::env::remove_var("OB_EMAIL__REPLY_TO");
            std::env::remove_var("OB_EMAIL__SITE_URL");
            std::env::remove_var("OB_EMAIL__FROM_NAME");
        }
    }

    #[tokio::test]
    async fn test_list_templates_without_db() {
        let config = EmailConfig {
            from: "Test <test@test.com>".into(),
            smtp_host: "localhost".into(),
            smtp_port: 25,
            smtp_user: "u".into(),
            smtp_password: "p".into(),
            reply_to: None,
            app_name: "Test".into(),
            site_url: None,
        };
        let service = EmailService::new(config);
        let templates = service.list_templates().await.unwrap();
        assert_eq!(templates.len(), 12); // 6 English + 6 French
    }

    #[tokio::test]
    async fn test_get_template_by_name_default() {
        let config = EmailConfig {
            from: "Test <test@test.com>".into(),
            smtp_host: "localhost".into(),
            smtp_port: 25,
            smtp_user: "u".into(),
            smtp_password: "p".into(),
            reply_to: None,
            app_name: "Test".into(),
            site_url: None,
        };
        let service = EmailService::new(config);
        let template = service
            .get_template_by_name(TEMPLATE_VERIFY_EMAIL)
            .await
            .unwrap();
        assert_eq!(template.name, TEMPLATE_VERIFY_EMAIL);
    }

    #[tokio::test]
    async fn test_get_template_by_name_unknown_fallback() {
        let config = EmailConfig {
            from: "Test <test@test.com>".into(),
            smtp_host: "localhost".into(),
            smtp_port: 25,
            smtp_user: "u".into(),
            smtp_password: "p".into(),
            reply_to: None,
            app_name: "Test".into(),
            site_url: None,
        };
        let service = EmailService::new(config);
        let template = service.get_template_by_name("nonexistent").await.unwrap();
        assert_eq!(template.name, "nonexistent");
        assert!(template.text.is_empty());
        assert!(template.html.is_empty());
    }

    #[tokio::test]
    async fn test_save_and_get_template() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let config = EmailConfig {
            from: "Test <test@test.com>".into(),
            smtp_host: "localhost".into(),
            smtp_port: 25,
            smtp_user: "u".into(),
            smtp_password: "p".into(),
            reply_to: None,
            app_name: "Test".into(),
            site_url: None,
        };
        let service = EmailService::with_db(config, db);
        let template_name = format!("custom_verify_{}", uuid::Uuid::new_v4());
        let subject = "Custom verify for {{ .AppName }}".to_string();

        let template = EmailTemplate {
            name: template_name.clone(),
            subject: subject.clone(),
            html: "<p>Custom HTML</p>".into(),
            text: "Custom text".into(),
        };

        service.save_template(template).await.unwrap();
        let fetched = service.get_template_by_name(&template_name).await.unwrap();
        assert_eq!(fetched.name, template_name);
        assert_eq!(fetched.subject, subject);
    }

    #[tokio::test]
    async fn test_save_template_without_db_fails() {
        let config = EmailConfig {
            from: "Test <test@test.com>".into(),
            smtp_host: "localhost".into(),
            smtp_port: 25,
            smtp_user: "u".into(),
            smtp_password: "p".into(),
            reply_to: None,
            app_name: "Test".into(),
            site_url: None,
        };
        let service = EmailService::new(config);
        let template = EmailTemplate {
            name: "test".into(),
            subject: "Test".into(),
            html: "".into(),
            text: "".into(),
        };
        let result = service.save_template(template).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_reset_template_to_default() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let config = EmailConfig {
            from: "Test <test@test.com>".into(),
            smtp_host: "localhost".into(),
            smtp_port: 25,
            smtp_user: "u".into(),
            smtp_password: "p".into(),
            reply_to: None,
            app_name: "Test".into(),
            site_url: None,
        };
        let service = EmailService::with_db(config, db);

        let custom = EmailTemplate {
            name: TEMPLATE_VERIFY_EMAIL.into(),
            subject: "Custom subject".into(),
            html: "Custom HTML".into(),
            text: "Custom text".into(),
        };
        service.save_template(custom).await.unwrap();

        let reset = service.reset_template(TEMPLATE_VERIFY_EMAIL).await.unwrap();
        assert_eq!(reset.name, TEMPLATE_VERIFY_EMAIL);
        assert!(reset.subject.contains("{{ .AppName }}"));
    }

    #[tokio::test]
    async fn test_reset_template_without_db_fails() {
        let config = EmailConfig {
            from: "Test <test@test.com>".into(),
            smtp_host: "localhost".into(),
            smtp_port: 25,
            smtp_user: "u".into(),
            smtp_password: "p".into(),
            reply_to: None,
            app_name: "Test".into(),
            site_url: None,
        };
        let service = EmailService::new(config);
        let result = service.reset_template("any").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_templates_with_custom() {
        let db = ob_database::DatabaseClient::new_mem().await;
        let config = EmailConfig {
            from: "Test <test@test.com>".into(),
            smtp_host: "localhost".into(),
            smtp_port: 25,
            smtp_user: "u".into(),
            smtp_password: "p".into(),
            reply_to: None,
            app_name: "Test".into(),
            site_url: None,
        };
        let service = EmailService::with_db(config, db);

        let custom_name = format!("custom_tpl_{}", uuid::Uuid::new_v4());
        let custom = EmailTemplate {
            name: custom_name.clone(),
            subject: "Custom subject".into(),
            html: "<p>Custom</p>".into(),
            text: "Custom".into(),
        };
        service.save_template(custom).await.unwrap();

        let templates = service.list_templates().await.unwrap();
        // At minimum: 6 defaults + 1 custom (stale rows may add more)
        assert!(
            templates.len() >= 7,
            "Expected at least 7 templates, got {}",
            templates.len()
        );
        let custom_tpl = templates.iter().find(|t| t.name == custom_name).unwrap();
        assert_eq!(custom_tpl.subject, "Custom subject");
    }

    #[test]
    fn test_template_constants() {
        assert_eq!(TEMPLATE_VERIFY_EMAIL, "verify_email");
        assert_eq!(TEMPLATE_RESET_PASSWORD, "reset_password");
        assert_eq!(TEMPLATE_MFA_ALERT, "mfa_alert");
        assert_eq!(TEMPLATE_MAGIC_LINK, "magic_link");
        assert_eq!(TEMPLATE_EMAIL_CHANGE, "email_change");
        assert_eq!(TEMPLATE_WELCOME, "welcome");
    }

    #[test]
    fn test_rendered_email_fields() {
        let template = EmailTemplate {
            name: "t".into(),
            subject: "Subj".into(),
            html: "<p>HTML</p>".into(),
            text: "Plain".into(),
        };
        let rendered = template.render(&HashMap::new());
        assert_eq!(rendered.subject, "Subj");
        assert_eq!(rendered.html, "<p>HTML</p>");
        assert_eq!(rendered.text, "Plain");
    }
}
