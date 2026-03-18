pub mod refunds;
pub mod returns;
pub mod shipping;
pub mod status;

use crate::HandlersState;
use axum::Router;

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .merge(status::router(state.clone()))
        .merge(refunds::router(state.clone()))
        .merge(shipping::router(state.clone()))
        .merge(returns::router(state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_orders_router_builds() {
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
