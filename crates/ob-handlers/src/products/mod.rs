pub mod crud;
pub mod questions;
pub mod ratings;
pub mod stock;
pub mod triggers;

use crate::HandlersState;
use axum::Router;

pub fn router(state: HandlersState) -> Router {
    Router::new()
        .merge(crud::router(state.clone()))
        .merge(ratings::router(state.clone()))
        .merge(stock::router(state.clone()))
        .merge(questions::router(state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ob_core::Config;
    use ob_database::DatabaseClient;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_products_router_builds() {
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
