use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Application environment, parsed from `ENVIRONMENT` env var at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Development,
    Staging,
    Production,
}

impl Environment {
    /// Parse from the `ENVIRONMENT` env var. Defaults to Development if unset.
    pub fn from_env() -> Self {
        match std::env::var("ENVIRONMENT")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "production" | "prod" => Self::Production,
            "staging" => Self::Staging,
            _ => Self::Development,
        }
    }

    pub fn is_production(&self) -> bool {
        matches!(self, Self::Production)
    }

    pub fn is_dev(&self) -> bool {
        matches!(self, Self::Development)
    }

    pub fn is_staging(&self) -> bool {
        matches!(self, Self::Staging)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Staging => "staging",
            Self::Production => "production",
        }
    }
}

impl std::fmt::Display for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

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
    #[serde(default)]
    pub tenant: TenantConfig,
    #[serde(default)]
    pub search: Option<SearchConfig>,
    #[serde(default)]
    pub cors: CorsConfig,
    #[serde(default)]
    pub secrets: SecretsConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SecretsConfig {
    #[serde(flatten)]
    pub values: HashMap<String, String>,
}

impl SecretsConfig {
    /// Get a secret by key name.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|s| s.as_str())
    }

    /// Get a secret or return an error.
    pub fn require(&self, key: &str) -> crate::Result<&str> {
        self.get(key)
            .ok_or_else(|| crate::Error::Config(format!("Missing required secret: {key}")))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// PostgreSQL connection URL (e.g., REDACTED_SECRET/dbname)
    #[serde(default = "default_db_url")]
    pub url: String,
    /// Maximum connections in the pool
    #[serde(default = "default_db_max_connections")]
    pub max_connections: u32,
}

fn default_db_url() -> String {
    "postgres://orignabase:orignabase_dev@localhost:5432/orignabase".to_string()
}

fn default_db_max_connections() -> u32 {
    20
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: default_db_url(),
            max_connections: default_db_max_connections(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_jwt_secret")]
    pub jwt_secret: String,
    #[serde(default = "default_access_token_ttl")]
    pub access_token_ttl_secs: u64,
    #[serde(default = "default_refresh_token_ttl")]
    pub refresh_token_ttl_secs: u64,
    #[serde(default)]
    pub google: Option<GoogleOAuthConfig>,
    #[serde(default)]
    pub apple: Option<AppleOAuthConfig>,
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GoogleOAuthConfig {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppleOAuthConfig {
    pub team_id: String,
    pub key_id: String,
    pub service_id: String,
    /// Path to the .p8 private key file
    pub private_key_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
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

#[derive(Debug, Clone, Deserialize)]
pub struct TenantConfig {
    /// Enable multi-tenant namespace resolution
    #[serde(default)]
    pub multi_tenant: bool,
    /// HTTP header used to identify the tenant
    #[serde(default = "default_tenant_header")]
    pub header_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchConfig {
    pub url: String,
    #[serde(default)]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CorsConfig {
    #[serde(default = "default_cors_allowed_origins")]
    pub allowed_origins: Vec<String>,
}

fn default_cors_allowed_origins() -> Vec<String> {
    vec![
        "https://orignagta.ca".to_string(),
        "https://www.orignagta.ca".to_string(),
        "https://dev.orignagta.ca".to_string(),
        "https://staging.orignagta.ca".to_string(),
    ]
}

fn default_tenant_header() -> String {
    "X-Tenant-ID".to_string()
}

impl Default for TenantConfig {
    fn default() -> Self {
        Self {
            multi_tenant: false,
            header_name: default_tenant_header(),
        }
    }
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

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: default_cors_allowed_origins(),
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
            google: None,
            apple: None,
            oidc: None,
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
    /// - `OB_DATABASE__URL` → `database.url`
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
                url = "postgres://orignabase:orignabase_dev@localhost:5432/orignabase"
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
        if let Ok(v) = std::env::var("OB_DATABASE__URL") {
            config.database.url = v;
        }
        if let Ok(v) = std::env::var("OB_DATABASE__MAX_CONNECTIONS") {
            config.database.max_connections = v.parse().map_err(|_| {
                crate::Error::Config("OB_DATABASE__MAX_CONNECTIONS must be a number".into())
            })?;
        }
        if let Ok(v) = std::env::var("OB_SEARCH__URL") {
            let search = config.search.get_or_insert(SearchConfig {
                url: v.clone(),
                api_key: None,
            });
            search.url = v;
        }
        if let Ok(v) = std::env::var("OB_SEARCH__API_KEY") {
            let search = config.search.get_or_insert(SearchConfig {
                url: "http://localhost:7700".to_string(),
                api_key: Some(v.clone()),
            });
            search.api_key = Some(v);
        }
        if let Ok(v) = std::env::var("OB_CORS__ALLOWED_ORIGINS") {
            config.cors.allowed_origins = v
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(ToOwned::to_owned)
                .collect();
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

        // Google OAuth from env. Client ID is sufficient for the current
        // Google ID token verification flow used by mobile/native SDK login.
        if let Ok(id) = std::env::var("OB_AUTH__GOOGLE_CLIENT_ID") {
            config.auth.google = Some(GoogleOAuthConfig {
                client_id: id,
                client_secret: std::env::var("OB_AUTH__GOOGLE_CLIENT_SECRET").ok(),
            });
        }

        // Apple OAuth from env
        if let (Ok(team), Ok(key), Ok(svc), Ok(pk_path)) = (
            std::env::var("OB_AUTH__APPLE_TEAM_ID"),
            std::env::var("OB_AUTH__APPLE_KEY_ID"),
            std::env::var("OB_AUTH__APPLE_SERVICE_ID"),
            std::env::var("OB_AUTH__APPLE_PRIVATE_KEY_PATH"),
        ) {
            config.auth.apple = Some(AppleOAuthConfig {
                team_id: team,
                key_id: key,
                service_id: svc,
                private_key_path: pk_path,
            });
        }

        // Generic OIDC from env
        if let (Ok(issuer), Ok(id)) = (
            std::env::var("OB_AUTH__OIDC_ISSUER_URL"),
            std::env::var("OB_AUTH__OIDC_CLIENT_ID"),
        ) {
            config.auth.oidc = Some(OidcConfig {
                issuer_url: issuer,
                client_id: id,
                client_secret: std::env::var("OB_AUTH__OIDC_CLIENT_SECRET").ok(),
            });
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

        // Secrets from environment variables (OB_SECRETS__KEY_NAME → secrets["key_name"])
        for (key, val) in std::env::vars() {
            if let Some(secret_key) = key.strip_prefix("OB_SECRETS__") {
                config.secrets.values.insert(secret_key.to_lowercase(), val);
            }
        }

        Ok(config)
    }

    /// Convenience: get a secret by key.
    pub fn secret(&self, key: &str) -> Option<&str> {
        self.secrets.get(key)
    }

    /// Convenience: get a required secret or error.
    pub fn require_secret(&self, key: &str) -> crate::Result<&str> {
        self.secrets.require(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mutex to serialize all tests that call Config::load (which reads env vars).
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Helper: clear all OB_ env vars to ensure a clean slate.
    fn clear_all_ob_env_vars() {
        for key in &[
            "OB_HOST",
            "OB_PORT",
            "OB_DATABASE__URL",
            "OB_DATABASE__MAX_CONNECTIONS",
            "OB_AUTH__JWT_SECRET",
            "OB_AUTH__ACCESS_TOKEN_TTL_SECS",
            "OB_AUTH__REFRESH_TOKEN_TTL_SECS",
            "OB_SECURITY__RULES_PATH",
            "OB_CLUSTER__ENABLED",
            "OB_CLUSTER__NATS_URL",
            "OB_CLUSTER__NODE_ID",
            "OB_SEARCH__URL",
            "OB_SEARCH__API_KEY",
            "OB_CORS__ALLOWED_ORIGINS",
        ] {
            unsafe { std::env::remove_var(key) };
        }
    }

    #[test]
    fn test_default_config() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_all_ob_env_vars();
        let config = Config::load(None).unwrap();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert_eq!(
            config.database.url,
            "postgres://orignabase:orignabase_dev@localhost:5432/orignabase"
        );
        assert_eq!(config.auth.access_token_ttl_secs, 900);
    }

    #[test]
    fn test_parse_toml() {
        let toml_str = r#"
            host = "127.0.0.1"
            port = 9090
            [database]
            url = "REDACTED_SECRET/testdb"
            [auth]
            jwt_secret = "supersecret"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 9090);
        assert_eq!(config.database.url, "REDACTED_SECRET/testdb");
        assert_eq!(config.auth.jwt_secret, "supersecret");
    }

    // ── Default impl tests ──

    #[test]
    fn test_auth_config_default() {
        let auth = AuthConfig::default();
        assert_eq!(auth.jwt_secret, "CHANGE_ME_IN_PRODUCTION");
        assert_eq!(auth.access_token_ttl_secs, 900);
        assert_eq!(auth.refresh_token_ttl_secs, 604800);
    }

    #[test]
    fn test_security_config_default() {
        let sec = SecurityConfig::default();
        assert_eq!(sec.rules_path, "rules.ob");
    }

    #[test]
    fn test_cluster_config_default() {
        let cluster = ClusterConfig::default();
        assert!(!cluster.enabled);
        assert_eq!(cluster.nats_url, "nats://localhost:4222");
        assert!(cluster.node_id.is_none());
    }

    #[test]
    fn test_parse_search_section() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "postgres://localhost:5432/test"
            [search]
            url = "http://meili:7700"
            api_key = "masterKey"
            "#,
        )
        .unwrap();

        let search = config.search.expect("search config should parse");
        assert_eq!(search.url, "http://meili:7700");
        assert_eq!(search.api_key.as_deref(), Some("masterKey"));
    }

    #[test]
    fn test_parse_cors_section() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "postgres://localhost:5432/test"
            [cors]
            allowed_origins = ["https://one.example", "https://two.example"]
            "#,
        )
        .unwrap();

        assert_eq!(
            config.cors.allowed_origins,
            vec![
                "https://one.example".to_string(),
                "https://two.example".to_string()
            ]
        );
    }

    #[test]
    fn test_database_defaults_via_serde() {
        let config: Config = toml::from_str(
            r#"
            [database]
            "#,
        )
        .unwrap();
        assert_eq!(
            config.database.url,
            "postgres://orignabase:orignabase_dev@localhost:5432/orignabase"
        );
        assert_eq!(config.database.max_connections, 20);
    }

    #[test]
    fn test_host_and_port_defaults_via_serde() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "postgres://x:5432/x"
            "#,
        )
        .unwrap();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
    }

    // ── Load from nonexistent file falls back to defaults ──

    #[test]
    fn test_load_nonexistent_file_uses_defaults() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_all_ob_env_vars();
        let config = Config::load(Some(Path::new("/tmp/__nonexistent_orignabase__.toml"))).unwrap();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert_eq!(
            config.database.url,
            "postgres://orignabase:orignabase_dev@localhost:5432/orignabase"
        );
    }

    // ── Cluster section from TOML ──

    #[test]
    fn test_parse_cluster_section() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "postgres://localhost:5432/test"
            [cluster]
            enabled = true
            nats_url = "nats://remote:4222"
            node_id = "node-1"
            "#,
        )
        .unwrap();
        assert!(config.cluster.enabled);
        assert_eq!(config.cluster.nats_url, "nats://remote:4222");
        assert_eq!(config.cluster.node_id.as_deref(), Some("node-1"));
    }

    // ── Environment variable override tests ──
    // All env var tests in one function, holding the ENV_MUTEX,
    // to avoid parallel test pollution (env vars are process-global).

    #[test]
    fn test_env_overrides_all() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_all_ob_env_vars();

        // -- OB_AUTH__GOOGLE_CLIENT_ID without secret --
        unsafe { std::env::set_var("OB_AUTH__GOOGLE_CLIENT_ID", "google-web-client-id") };
        let config = Config::load(None).unwrap();
        let google = config
            .auth
            .google
            .expect("google config should be created from client id");
        assert_eq!(google.client_id, "google-web-client-id");
        assert!(google.client_secret.is_none());
        clear_all_ob_env_vars();

        // -- OB_AUTH__GOOGLE_CLIENT_SECRET with client id --
        unsafe { std::env::set_var("OB_AUTH__GOOGLE_CLIENT_ID", "google-web-client-id") };
        unsafe { std::env::set_var("OB_AUTH__GOOGLE_CLIENT_SECRET", "google-secret") };
        let config = Config::load(None).unwrap();
        let google = config
            .auth
            .google
            .expect("google config should include optional secret");
        assert_eq!(google.client_id, "google-web-client-id");
        assert_eq!(google.client_secret.as_deref(), Some("google-secret"));
        clear_all_ob_env_vars();

        // -- OB_HOST --
        unsafe { std::env::set_var("OB_HOST", "10.0.0.1") };
        let config = Config::load(None).unwrap();
        assert_eq!(config.host, "10.0.0.1");
        clear_all_ob_env_vars();

        // -- OB_PORT (valid) --
        unsafe { std::env::set_var("OB_PORT", "3000") };
        let config = Config::load(None).unwrap();
        assert_eq!(config.port, 3000);
        clear_all_ob_env_vars();

        // -- OB_PORT (invalid) --
        unsafe { std::env::set_var("OB_PORT", "not_a_number") };
        let result = Config::load(None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("OB_PORT must be a number")
        );
        clear_all_ob_env_vars();

        // -- OB_DATABASE__URL --
        unsafe { std::env::set_var("OB_DATABASE__URL", "postgres://remote:5432/mydb") };
        let config = Config::load(None).unwrap();
        assert_eq!(config.database.url, "postgres://remote:5432/mydb");
        clear_all_ob_env_vars();

        // -- OB_DATABASE__MAX_CONNECTIONS --
        unsafe { std::env::set_var("OB_DATABASE__MAX_CONNECTIONS", "50") };
        let config = Config::load(None).unwrap();
        assert_eq!(config.database.max_connections, 50);
        clear_all_ob_env_vars();

        // -- OB_AUTH__JWT_SECRET --
        unsafe { std::env::set_var("OB_AUTH__JWT_SECRET", "my_jwt_secret") };
        let config = Config::load(None).unwrap();
        assert_eq!(config.auth.jwt_secret, "my_jwt_secret");
        clear_all_ob_env_vars();

        // -- OB_AUTH__ACCESS_TOKEN_TTL_SECS (valid) --
        unsafe { std::env::set_var("OB_AUTH__ACCESS_TOKEN_TTL_SECS", "1800") };
        let config = Config::load(None).unwrap();
        assert_eq!(config.auth.access_token_ttl_secs, 1800);
        clear_all_ob_env_vars();

        // -- OB_AUTH__ACCESS_TOKEN_TTL_SECS (invalid) --
        unsafe { std::env::set_var("OB_AUTH__ACCESS_TOKEN_TTL_SECS", "abc") };
        let result = Config::load(None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid access token TTL")
        );
        clear_all_ob_env_vars();

        // -- OB_AUTH__REFRESH_TOKEN_TTL_SECS (valid) --
        unsafe { std::env::set_var("OB_AUTH__REFRESH_TOKEN_TTL_SECS", "86400") };
        let config = Config::load(None).unwrap();
        assert_eq!(config.auth.refresh_token_ttl_secs, 86400);
        clear_all_ob_env_vars();

        // -- OB_AUTH__REFRESH_TOKEN_TTL_SECS (invalid) --
        unsafe { std::env::set_var("OB_AUTH__REFRESH_TOKEN_TTL_SECS", "xyz") };
        let result = Config::load(None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid refresh token TTL")
        );
        clear_all_ob_env_vars();

        // -- OB_SECURITY__RULES_PATH --
        unsafe { std::env::set_var("OB_SECURITY__RULES_PATH", "/etc/rules.ob") };
        let config = Config::load(None).unwrap();
        assert_eq!(config.security.rules_path, "/etc/rules.ob");
        clear_all_ob_env_vars();

        // -- OB_CLUSTER__ENABLED = "true" --
        unsafe { std::env::set_var("OB_CLUSTER__ENABLED", "true") };
        let config = Config::load(None).unwrap();
        assert!(config.cluster.enabled);
        clear_all_ob_env_vars();

        // -- OB_CLUSTER__ENABLED = "1" --
        unsafe { std::env::set_var("OB_CLUSTER__ENABLED", "1") };
        let config = Config::load(None).unwrap();
        assert!(config.cluster.enabled);
        clear_all_ob_env_vars();

        // -- OB_CLUSTER__ENABLED = "false" --
        unsafe { std::env::set_var("OB_CLUSTER__ENABLED", "false") };
        let config = Config::load(None).unwrap();
        assert!(!config.cluster.enabled);
        clear_all_ob_env_vars();

        // -- OB_CLUSTER__NATS_URL --
        unsafe { std::env::set_var("OB_CLUSTER__NATS_URL", "nats://prod:4222") };
        let config = Config::load(None).unwrap();
        assert_eq!(config.cluster.nats_url, "nats://prod:4222");
        clear_all_ob_env_vars();

        // -- OB_CLUSTER__NODE_ID --
        unsafe { std::env::set_var("OB_CLUSTER__NODE_ID", "node-42") };
        let config = Config::load(None).unwrap();
        assert_eq!(config.cluster.node_id.as_deref(), Some("node-42"));
        clear_all_ob_env_vars();

        // -- OB_SEARCH__URL --
        unsafe { std::env::set_var("OB_SEARCH__URL", "http://meili:7700") };
        let config = Config::load(None).unwrap();
        let search = config.search.expect("search config should be created");
        assert_eq!(search.url, "http://meili:7700");
        assert!(search.api_key.is_none());
        clear_all_ob_env_vars();

        // -- OB_SEARCH__API_KEY --
        unsafe { std::env::set_var("OB_SEARCH__API_KEY", "masterKey") };
        let config = Config::load(None).unwrap();
        let search = config
            .search
            .expect("search config should be created from api key");
        assert_eq!(search.url, "http://localhost:7700");
        assert_eq!(search.api_key.as_deref(), Some("masterKey"));
        clear_all_ob_env_vars();

        // -- OB_CORS__ALLOWED_ORIGINS --
        unsafe {
            std::env::set_var(
                "OB_CORS__ALLOWED_ORIGINS",
                "https://api.example.com, https://admin.example.com",
            )
        };
        let config = Config::load(None).unwrap();
        assert_eq!(
            config.cors.allowed_origins,
            vec![
                "https://api.example.com".to_string(),
                "https://admin.example.com".to_string()
            ]
        );
        clear_all_ob_env_vars();
    }
}
