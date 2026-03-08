use axum::{extract::Request, http::header::AUTHORIZATION, middleware::Next, response::Response};
use ob_core::Error;

use crate::jwt::{Claims, verify_token};

/// Extracted auth context available to handlers.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user_id: String,
    pub roles: Vec<String>,
    pub authenticated: bool,
}

impl AuthContext {
    pub fn anonymous() -> Self {
        Self {
            user_id: String::new(),
            roles: vec![],
            authenticated: false,
        }
    }

    pub fn from_claims(claims: Claims) -> Self {
        Self {
            user_id: claims.sub,
            roles: claims.roles,
            authenticated: true,
        }
    }

    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }
}

/// Middleware that extracts JWT from the Authorization header.
/// Does NOT reject unauthenticated requests — that's the security rules' job.
pub async fn auth_extractor(mut request: Request, next: Next) -> Result<Response, Error> {
    let jwt_secret = request
        .extensions()
        .get::<String>()
        .cloned()
        .unwrap_or_default();

    let auth_context = if let Some(auth_header) = request.headers().get(AUTHORIZATION) {
        let header_str = auth_header
            .to_str()
            .map_err(|_| Error::Auth("Invalid Authorization header".into()))?;

        if let Some(token) = header_str.strip_prefix("Bearer ") {
            match verify_token(token, &jwt_secret) {
                Ok(claims) if claims.typ == "access" => AuthContext::from_claims(claims),
                Ok(_) => return Err(Error::Auth("Invalid token type".into())),
                Err(_) => AuthContext::anonymous(),
            }
        } else {
            AuthContext::anonymous()
        }
    } else {
        AuthContext::anonymous()
    };

    request.extensions_mut().insert(auth_context);
    Ok(next.run(request).await)
}
