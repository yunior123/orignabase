use ob_auth::middleware::AuthContext;

pub fn require_authenticated(auth: &AuthContext) -> ob_core::Result<&str> {
    if !auth.authenticated || auth.user_id.is_empty() {
        return Err(ob_core::Error::Auth("Authentication required".into()));
    }
    Ok(auth.user_id.as_str())
}

pub fn require_admin(auth: &AuthContext) -> ob_core::Result<&str> {
    let user_id = require_authenticated(auth)?;
    if !auth.has_role("admin") {
        return Err(ob_core::Error::Forbidden("Admin role required".into()));
    }
    Ok(user_id)
}

pub fn resolve_self_user_id(
    auth: &AuthContext,
    provided: Option<&str>,
    field_name: &str,
) -> ob_core::Result<String> {
    let user_id = require_authenticated(auth)?;
    if let Some(provided) = provided
        && !provided.is_empty()
        && provided != user_id
    {
        return Err(ob_core::Error::Forbidden(format!(
            "{field_name} must match the authenticated user"
        )));
    }
    Ok(user_id.to_string())
}

pub fn resolve_admin_user_id(
    auth: &AuthContext,
    provided: Option<&str>,
    field_name: &str,
) -> ob_core::Result<String> {
    let admin_id = require_admin(auth)?;
    if let Some(provided) = provided
        && !provided.is_empty()
        && provided != admin_id
    {
        return Err(ob_core::Error::Forbidden(format!(
            "{field_name} must match the authenticated admin"
        )));
    }
    Ok(admin_id.to_string())
}
