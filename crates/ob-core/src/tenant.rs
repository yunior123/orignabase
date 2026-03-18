use axum::{extract::Request, middleware::Next, response::Response};

use crate::config::TenantConfig;

/// Resolved tenant context, available to downstream handlers.
#[derive(Debug, Clone)]
pub struct TenantContext {
    /// The resolved tenant ID (namespace).
    pub tenant_id: Option<String>,
}

/// Axum middleware that extracts tenant ID from the configured header
/// and injects a `TenantContext` into request extensions.
///
/// When multi-tenant is disabled, `TenantContext.tenant_id` is always `None`.
pub async fn tenant_middleware(request: Request, next: Next) -> Response {
    let tenant_config = request.extensions().get::<TenantConfig>().cloned();

    let tenant_id = tenant_config.and_then(|cfg| {
        if !cfg.multi_tenant {
            return None;
        }

        // Try header first
        if let Some(header_val) = request.headers().get(&cfg.header_name)
            && let Ok(val) = header_val.to_str()
        {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }

        // Try subdomain extraction from Host header
        if let Some(host) = request.headers().get("host")
            && let Ok(host_str) = host.to_str()
        {
            // e.g., "tenant1.api.orignabase.com" → "tenant1"
            let parts: Vec<&str> = host_str.split('.').collect();
            if parts.len() > 2 {
                let subdomain = parts[0];
                // Skip common non-tenant subdomains
                if subdomain != "www" && subdomain != "api" {
                    return Some(subdomain.to_string());
                }
            }
        }

        None
    });

    let mut request = request;
    request.extensions_mut().insert(TenantContext { tenant_id });
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenant_context_default() {
        let ctx = TenantContext { tenant_id: None };
        assert!(ctx.tenant_id.is_none());
    }

    #[test]
    fn test_tenant_context_with_id() {
        let ctx = TenantContext {
            tenant_id: Some("acme_corp".to_string()),
        };
        assert_eq!(ctx.tenant_id.as_deref(), Some("acme_corp"));
    }

    #[test]
    fn test_tenant_context_clone() {
        let ctx = TenantContext {
            tenant_id: Some("tenant_1".to_string()),
        };
        let cloned = ctx.clone();
        assert_eq!(cloned.tenant_id, ctx.tenant_id);
    }

    #[test]
    fn test_tenant_context_debug() {
        let ctx = TenantContext { tenant_id: None };
        let debug = format!("{:?}", ctx);
        assert!(debug.contains("TenantContext"));
    }
}
