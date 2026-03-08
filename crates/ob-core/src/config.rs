use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub cluster: ClusterConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_db_namespace")]
    pub namespace: String,
    #[serde(default = "default_db_name")]
    pub name: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_jwt_secret")]
    pub jwt_secret: String,
    #[serde(default = "default_access_token_ttl")]
    pub access_token_ttl_secs: u64,
    #[serde(default = "default_refresh_token_ttl")]
    pub refresh_token_ttl_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SecurityConfig {
    #[serde(default = "default_rules_path")]
    pub rules_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClusterConfig {
    /// Enable multi-node clustering via NATS JetStream
    #[serde(default)]
    pub enabled: bool,
    /// NATS server URL
    #[serde(default = "default_nats_url")]
    pub nats_url: String,
    /// Unique node ID (auto-generated if not set)
    pub node_id: Option<String>,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            nats_url: default_nats_url(),
            node_id: None,
        }
    }
}

fn default_nats_url() -> String {
    "nats://localhost:4222".to_string()
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_db_endpoint() -> String {
    "ws://localhost:8000".to_string()
}

fn default_db_namespace() -> String {
    "orignabase".to_string()
}

fn default_db_name() -> String {
    "main".to_string()
}

fn default_jwt_secret() -> String {
    "CHANGE_ME_IN_PRODUCTION".to_string()
}

fn default_access_token_ttl() -> u64 {
    900 // 15 minutes
}

fn default_refresh_token_ttl() -> u64 {
    604800 // 7 days
}

fn default_rules_path() -> String {
    "rules.ob".to_string()
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt_secret: default_jwt_secret(),
            access_token_ttl_secs: default_access_token_ttl(),
            refresh_token_ttl_secs: default_refresh_token_ttl(),
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            rules_path: default_rules_path(),
        }
    }
}

impl Config {
    /// Load configuration from TOML file with environment variable overrides.
    ///
    /// Environment variables use the prefix `OB_` and double underscores for nesting:
    /// - `OB_HOST` → `host`
    /// - `OB_PORT` → `port`
    /// - `OB_DATABASE__ENDPOINT` → `database.endpoint`
    /// - `OB_AUTH__JWT_SECRET` → `auth.jwt_secret`
    pub fn load(path: Option<&Path>) -> crate::Result<Self> {
        let _ = dotenvy::dotenv();

        let mut config_str = String::new();
        if let Some(p) = path {
            if p.exists() {
                config_str =
                    std::fs::read_to_string(p).map_err(|e| crate::Error::Config(e.to_string()))?;
            }
        } else {
            // Try default paths
            for candidate in &["orignabase.toml", "config.toml"] {
                if let Ok(content) = std::fs::read_to_string(candidate) {
                    config_str = content;
                    break;
                }
            }
        }

        let mut config: Config = if config_str.is_empty() {
            // Minimal default config when no file exists
            toml::from_str(
                r#"
                [database]
                endpoint = "ws://localhost:8000"
                "#,
            )
            .map_err(|e| crate::Error::Config(e.to_string()))?
        } else {
            toml::from_str(&config_str).map_err(|e| crate::Error::Config(e.to_string()))?
        };

        // Environment variable overrides
        if let Ok(v) = std::env::var("OB_HOST") {
            config.host = v;
        }
        if let Ok(v) = std::env::var("OB_PORT") {
            config.port = v
                .parse()
                .map_err(|_| crate::Error::Config("OB_PORT must be a number".into()))?;
        }
        if let Ok(v) = std::env::var("OB_DATABASE__ENDPOINT") {
            config.database.endpoint = v;
        }
        if let Ok(v) = std::env::var("OB_DATABASE__NAMESPACE") {
            config.database.namespace = v;
        }
        if let Ok(v) = std::env::var("OB_DATABASE__NAME") {
            config.database.name = v;
        }
        if let Ok(v) = std::env::var("OB_DATABASE__USERNAME") {
            config.database.username = Some(v);
        }
        if let Ok(v) = std::env::var("OB_DATABASE__PASSWORD") {
            config.database.password = Some(v);
        }
        if let Ok(v) = std::env::var("OB_AUTH__JWT_SECRET") {
            config.auth.jwt_secret = v;
        }
        if let Ok(v) = std::env::var("OB_AUTH__ACCESS_TOKEN_TTL_SECS") {
            config.auth.access_token_ttl_secs = v
                .parse()
                .map_err(|_| crate::Error::Config("Invalid access token TTL".into()))?;
        }
        if let Ok(v) = std::env::var("OB_AUTH__REFRESH_TOKEN_TTL_SECS") {
            config.auth.refresh_token_ttl_secs = v
                .parse()
                .map_err(|_| crate::Error::Config("Invalid refresh token TTL".into()))?;
        }
        if let Ok(v) = std::env::var("OB_SECURITY__RULES_PATH") {
            config.security.rules_path = v;
        }
        if let Ok(v) = std::env::var("OB_CLUSTER__ENABLED") {
            config.cluster.enabled = v == "true" || v == "1";
        }
        if let Ok(v) = std::env::var("OB_CLUSTER__NATS_URL") {
            config.cluster.nats_url = v;
        }
        if let Ok(v) = std::env::var("OB_CLUSTER__NODE_ID") {
            config.cluster.node_id = Some(v);
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::load(None).unwrap();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert_eq!(config.database.endpoint, "ws://localhost:8000");
        assert_eq!(config.auth.access_token_ttl_secs, 900);
    }

    #[test]
    fn test_parse_toml() {
        let toml_str = r#"
            host = "127.0.0.1"
            port = 9090
            [database]
            endpoint = "ws://db:8000"
            namespace = "test"
            name = "testdb"
            [auth]
            jwt_secret = "supersecret"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 9090);
        assert_eq!(config.database.namespace, "test");
        assert_eq!(config.auth.jwt_secret, "supersecret");
    }
}
