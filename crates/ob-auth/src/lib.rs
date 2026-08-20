//! Authentication and authorization for OrignaBase.
//!
//! Handles JWT issuance/validation (RS256), email/password auth, Google/Apple OAuth,
//! MFA via TOTP, Cloudflare Turnstile bot protection, email verification/password reset,
//! and rate limiting. Keys are auto-rotated via [`KeyRotationManager`].

pub mod email;
pub mod jwt;
pub mod key_rotation;
pub mod login_tracking;
pub mod middleware;
pub mod oauth;
pub mod password;
pub mod rate_limit;
pub mod revocation;
pub mod routes;
pub mod totp;
pub mod turnstile;

pub use email::{EmailConfig, EmailService, EmailTemplate};
pub use jwt::{
    Claims, JwtKeys, generate_rsa_keys, issue_access_token_with_claims, issue_challenge_token,
    rotate_keys,
};
pub use key_rotation::{KeyRotationManager, fingerprint_public_key};
pub use middleware::{
    AuthContext, assert_jwt_secret_configured, assert_no_live_stripe_in_dev,
    assert_test_mode_not_in_production,
};
pub use oauth::{OAuthProvider, OAuthUserInfo};
pub use rate_limit::RateLimiter;
pub use revocation::{cleanup_revoked_tokens, is_token_revoked, revoke_token};
pub use turnstile::validate_turnstile_token;
