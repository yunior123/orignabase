pub mod capture;
pub mod checkout;
pub mod connect;
pub mod providers;
pub mod subscriptions;
pub mod webhooks;

use crate::HandlersState;
use axum::Router;

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .merge(checkout::router(state.clone()))
        .merge(capture::router(state.clone()))
        .merge(webhooks::router(state.clone()))
        .merge(connect::router(state.clone()))
        .merge(subscriptions::router(state.clone()))
        .merge(providers::router(state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_payments_router_builds() {
        let state = HandlersState {
            config: Arc::new(Config::load(None).unwrap()),
            db: DatabaseClient::new_mem().await,
            http_client: reqwest::Client::new(),
            stripe_client: None,
            stripe_base_url: "https://api.stripe.com/v1".into(),
        };
        let _router = router(state);
    }
}
