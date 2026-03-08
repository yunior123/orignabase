use crate::Config;
use std::sync::Arc;

/// Shared application state accessible from all request handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> Config {
        toml::from_str(
            r#"
            [database]
            endpoint = "localhost:8000"
            "#,
        )
        .unwrap()
    }

    #[test]
    fn test_new_state_stores_config() {
        let config = make_config();
        let state = AppState::new(config);
        assert_eq!(state.config.host, "0.0.0.0");
        assert_eq!(state.config.port, 8080);
        assert_eq!(state.config.database.endpoint, "localhost:8000");
    }

    #[test]
    fn test_state_config_accessible_via_arc() {
        let config = make_config();
        let state = AppState::new(config);
        // Arc strong count should be 1
        assert_eq!(Arc::strong_count(&state.config), 1);
    }

    #[test]
    fn test_state_clone_shares_config() {
        let config = make_config();
        let state = AppState::new(config);
        let cloned = state.clone();
        // Both should point to the same Arc allocation
        assert_eq!(Arc::strong_count(&state.config), 2);
        assert_eq!(Arc::strong_count(&cloned.config), 2);
        assert_eq!(state.config.port, cloned.config.port);
    }
}
