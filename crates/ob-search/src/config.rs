use serde::Deserialize;
use std::collections::HashMap;

/// Search engine configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Meilisearch URL (e.g., http://localhost:7700)
    pub url: String,
    /// Meilisearch API key
    pub api_key: Option<String>,
    /// Index configurations per collection
    #[serde(default)]
    pub indexes: HashMap<String, IndexConfig>,
}

/// Per-index configuration defining which fields are searchable, filterable, sortable.
#[derive(Debug, Clone, Deserialize)]
pub struct IndexConfig {
    /// Fields included in full-text search
    #[serde(default)]
    pub searchable: Vec<String>,
    /// Fields available for filtering
    #[serde(default)]
    pub filterable: Vec<String>,
    /// Fields available for sorting
    #[serde(default)]
    pub sortable: Vec<String>,
    /// Field to use as the primary key (defaults to "id")
    #[serde(default = "default_pk")]
    pub primary_key: String,
}

fn default_pk() -> String {
    "id".to_string()
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "http://localhost:7700".to_string(),
            api_key: None,
            indexes: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SearchConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.url, "http://localhost:7700");
        assert!(config.indexes.is_empty());
    }

    #[test]
    fn test_parse_config() {
        let toml_str = r#"
            enabled = true
            url = "http://meili:7700"
            api_key = "masterkey"

            [indexes.products]
            searchable = ["title", "description"]
            filterable = ["category", "price"]
            sortable = ["price", "created_at"]
        "#;
        let config: SearchConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.url, "http://meili:7700");
        assert_eq!(config.api_key, Some("masterkey".to_string()));
        assert!(config.indexes.contains_key("products"));
        assert_eq!(
            config.indexes["products"].searchable,
            vec!["title", "description"]
        );
    }
}
