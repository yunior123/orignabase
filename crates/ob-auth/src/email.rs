use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{MultiPart, SinglePart, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use ob_core::{Error, Result};
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

/// Well-known template names.
pub const TEMPLATE_VERIFY_EMAIL: &str = "verify_email";
pub const TEMPLATE_RESET_PASSWORD: &str = "reset_password";
pub const TEMPLATE_MFA_ALERT: &str = "mfa_alert";
pub const TEMPLATE_MAGIC_LINK: &str = "magic_link";
pub const TEMPLATE_EMAIL_CHANGE: &str = "email_change";
pub const TEMPLATE_WELCOME: &str = "welcome";

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

// ── Default Templates ──────────────────────────────────────────────

pub fn default_templates() -> Vec<EmailTemplate> {
    vec![
        default_verify_email(),
        default_reset_password(),
        default_mfa_alert(),
        default_magic_link(),
        default_email_change(),
        default_welcome(),
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

// ── Email Config ───────────────────────────────────────────────────

/// Configuration for the email service.
///
/// ## Deliverability Checklist (DNS — not handled in code)
///
/// To avoid spam folders, configure these DNS records for your sending domain:
/// 1. **SPF**: `v=spf1 include:spf.mailjet.com ~all` (adjust for your provider)
/// 2. **DKIM**: Add the TXT record your SMTP provider gives you
/// 3. **DMARC**: `v=DMARC1; p=quarantine; rua=mailto:dmarc@yourdomain.com`
/// 4. **Return-Path**: Must match the From domain (automatic with most providers)
///
/// Use a subdomain like `mail.yourdomain.com` for transactional email to isolate reputation.
///
/// ## Provider Examples
///
/// ### Mailjet (recommended)
/// ```env
/// OB_EMAIL__SMTP_HOST=in-v3.mailjet.com
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
            && let Ok(results) = db
                .query_bind(
                    "SELECT * FROM _email_templates WHERE name = $name",
                    serde_json::json!({ "name": name }),
                )
                .await
            && let Some(doc) = results.first()
            && let Ok(template) = serde_json::from_value::<EmailTemplate>(doc.clone())
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
    pub async fn send_verification_email(
        &self,
        to: &str,
        token: &str,
        base_url: &str,
    ) -> Result<()> {
        let template = self.get_template(TEMPLATE_VERIFY_EMAIL).await;
        let mut vars = self.base_vars();
        vars.insert(
            "ActionURL".into(),
            format!("{base_url}/auth/verify-email?token={token}"),
        );
        vars.insert("Token".into(), token.into());
        vars.insert("Email".into(), to.into());
        vars.insert("ExpiresIn".into(), "24 hours".into());
        vars.insert("SiteURL".into(), base_url.into());

        let rendered = template.render(&vars);
        self.send_multipart(to, &rendered.subject, &rendered.text, &rendered.html)
            .await
    }

    /// Send a password reset email.
    pub async fn send_reset_email(&self, to: &str, token: &str, base_url: &str) -> Result<()> {
        let template = self.get_template(TEMPLATE_RESET_PASSWORD).await;
        let mut vars = self.base_vars();
        vars.insert(
            "ActionURL".into(),
            format!("{base_url}/auth/reset-password?token={token}"),
        );
        vars.insert("Token".into(), token.into());
        vars.insert("Email".into(), to.into());
        vars.insert("ExpiresIn".into(), "1 hour".into());
        vars.insert("SiteURL".into(), base_url.into());

        let rendered = template.render(&vars);
        self.send_multipart(to, &rendered.subject, &rendered.text, &rendered.html)
            .await
    }

    /// Send an MFA change alert.
    pub async fn send_mfa_alert(&self, to: &str, action: &str) -> Result<()> {
        let template = self.get_template(TEMPLATE_MFA_ALERT).await;
        let mut vars = self.base_vars();
        vars.insert("Action".into(), action.into());
        vars.insert("Email".into(), to.into());

        let rendered = template.render(&vars);
        self.send_multipart(to, &rendered.subject, &rendered.text, &rendered.html)
            .await
    }

    /// Send a magic link sign-in email.
    pub async fn send_magic_link_email(&self, to: &str, token: &str, base_url: &str) -> Result<()> {
        let template = self.get_template(TEMPLATE_MAGIC_LINK).await;
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
    pub async fn send_welcome_email(&self, to: &str) -> Result<()> {
        let template = self.get_template(TEMPLATE_WELCOME).await;
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
    async fn send_multipart(&self, to: &str, subject: &str, plain: &str, html: &str) -> Result<()> {
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

        let custom: Vec<EmailTemplate> = db
            .query_raw("SELECT * FROM _email_templates")
            .await?
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

        // Upsert: delete existing, then create
        let _ = db
            .query_bind(
                "DELETE FROM _email_templates WHERE name = $name",
                serde_json::json!({ "name": template.name }),
            )
            .await;

        db.create_document(
            "_email_templates",
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

        let _ = db
            .query_bind(
                "DELETE FROM _email_templates WHERE name = $name",
                serde_json::json!({ "name": name }),
            )
            .await;

        default_templates()
            .into_iter()
            .find(|t| t.name == name)
            .ok_or_else(|| Error::NotFound(format!("No default template named '{name}'")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(templates.len(), 6);
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
            // Every template should reference AppName
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
            smtp_host: "in-v3.mailjet.com".into(),
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
        assert_eq!(json["name"], TEMPLATE_VERIFY_EMAIL);
        assert!(json["subject"].as_str().unwrap().contains("{{ .AppName }}"));

        // Roundtrip
        let deserialized: EmailTemplate = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.name, template.name);
    }
}
