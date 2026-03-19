pub mod email;
pub mod jwt;
pub mod key_rotation;
pub mod login_tracking;
pub mod middleware;
pub mod oauth;
pub mod password;
pub mod rate_limit;
pub mod routes;
pub mod totp;
pub mod turnstile;

pub use email::{EmailConfig, EmailService, EmailTemplate};
pub use jwt::{
    Claims, JwtKeys, generate_rsa_keys, issue_access_token_with_claims, issue_challenge_token,
    rotate_keys,
};
pub use key_rotation::{KeyRotationManager, fingerprint_public_key};
pub use middleware::AuthContext;
pub use oauth::{OAuthProvider, OAuthUserInfo};
pub use rate_limit::RateLimiter;
pub use turnstile::validate_turnstile_token;
