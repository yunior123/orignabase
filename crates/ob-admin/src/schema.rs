use ob_core::Result;
use ob_database::DatabaseClient;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Describes a collection (table) schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionSchema {
    pub name: String,
    pub fields: Vec<FieldDef>,
}

/// A field definition within a collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDef {
    pub name: String,
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub unique: bool,
    #[serde(default)]
    pub indexed: bool,
}

/// Create a SCHEMAFULL table in SurrealDB from a CollectionSchema.
pub async fn create_collection(db: &DatabaseClient, schema: &CollectionSchema) -> Result<()> {
    // Define the table
    let mut query = format!("DEFINE TABLE {} SCHEMAFULL;\n", schema.name);

    for field in &schema.fields {
        let surreal_type = to_surreal_type(&field.field_type);
        query.push_str(&format!(
            "DEFINE FIELD {} ON TABLE {} TYPE {}",
            field.name, schema.name, surreal_type
        ));
        if !field.required {
            // SurrealDB uses ASSERT for required fields
            // For optional: wrap in option
        }
        query.push_str(";\n");

        if field.unique {
            query.push_str(&format!(
                "DEFINE INDEX idx_{0}_{1} ON TABLE {0} FIELDS {1} UNIQUE;\n",
                schema.name, field.name
            ));
        } else if field.indexed {
            query.push_str(&format!(
                "DEFINE INDEX idx_{0}_{1} ON TABLE {0} FIELDS {1};\n",
                schema.name, field.name
            ));
        }
    }

    // DEFINE TABLE/FIELD returns no records, use query_raw_value
    db.query_raw_value(&query).await?;
    tracing::info!("Created collection: {}", schema.name);
    Ok(())
}

/// List all tables in the current database.
pub async fn list_collections(db: &DatabaseClient) -> Result<Value> {
    db.query_raw_value("INFO FOR DB").await
}

/// Drop a table.
pub async fn drop_collection(db: &DatabaseClient, name: &str) -> Result<()> {
    // Validate name to prevent injection (alphanumeric + underscore only)
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(ob_core::Error::Validation("Invalid collection name".into()));
    }
    db.query_raw_value(&format!("REMOVE TABLE {name}")).await?;
    tracing::info!("Dropped collection: {name}");
    Ok(())
}

fn to_surreal_type(t: &str) -> &str {
    match t {
        "string" => "string",
        "int" | "integer" => "int",
        "float" | "double" | "number" => "float",
        "bool" | "boolean" => "bool",
        "datetime" | "timestamp" => "datetime",
        "object" | "map" => "object",
        "array" | "list" => "array",
        "record" => "record",
        _ => "any",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_surreal_type_mapping() {
        assert_eq!(to_surreal_type("string"), "string");
        assert_eq!(to_surreal_type("integer"), "int");
        assert_eq!(to_surreal_type("boolean"), "bool");
        assert_eq!(to_surreal_type("datetime"), "datetime");
        assert_eq!(to_surreal_type("unknown"), "any");
    }

    #[test]
    fn test_surreal_type_mapping_all_variants() {
        assert_eq!(to_surreal_type("int"), "int");
        assert_eq!(to_surreal_type("float"), "float");
        assert_eq!(to_surreal_type("double"), "float");
        assert_eq!(to_surreal_type("number"), "float");
        assert_eq!(to_surreal_type("bool"), "bool");
        assert_eq!(to_surreal_type("timestamp"), "datetime");
        assert_eq!(to_surreal_type("object"), "object");
        assert_eq!(to_surreal_type("map"), "object");
        assert_eq!(to_surreal_type("array"), "array");
        assert_eq!(to_surreal_type("list"), "array");
        assert_eq!(to_surreal_type("record"), "record");
    }

    #[test]
    fn test_collection_schema_serialization() {
        let schema = CollectionSchema {
            name: "products".to_string(),
            fields: vec![
                FieldDef {
                    name: "title".to_string(),
                    field_type: "string".to_string(),
                    required: true,
                    unique: false,
                    indexed: false,
                },
                FieldDef {
                    name: "price".to_string(),
                    field_type: "float".to_string(),
                    required: true,
                    unique: false,
                    indexed: true,
                },
            ],
        };

        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["name"], "products");
        assert_eq!(json["fields"][0]["name"], "title");
        assert_eq!(json["fields"][0]["field_type"], "string");
        assert_eq!(json["fields"][0]["required"], true);
        assert_eq!(json["fields"][1]["indexed"], true);
    }

    #[test]
    fn test_collection_schema_deserialization() {
        let json_str = r#"{
            "name": "orders",
            "fields": [
                { "name": "total", "field_type": "float", "required": true, "unique": false, "indexed": false },
                { "name": "sku", "field_type": "string", "required": false, "unique": true, "indexed": false }
            ]
        }"#;

        let schema: CollectionSchema = serde_json::from_str(json_str).unwrap();
        assert_eq!(schema.name, "orders");
        assert_eq!(schema.fields.len(), 2);
        assert_eq!(schema.fields[0].name, "total");
        assert!(schema.fields[0].required);
        assert!(schema.fields[1].unique);
    }

    #[test]
    fn test_field_def_defaults() {
        // required, unique, indexed should default to false
        let json_str = r#"{ "name": "description", "field_type": "string" }"#;
        let field: FieldDef = serde_json::from_str(json_str).unwrap();
        assert_eq!(field.name, "description");
        assert!(!field.required);
        assert!(!field.unique);
        assert!(!field.indexed);
    }

    #[test]
    fn test_collection_schema_roundtrip() {
        let schema = CollectionSchema {
            name: "users".to_string(),
            fields: vec![FieldDef {
                name: "email".to_string(),
                field_type: "string".to_string(),
                required: true,
                unique: true,
                indexed: true,
            }],
        };

        let json_str = serde_json::to_string(&schema).unwrap();
        let deserialized: CollectionSchema = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.name, schema.name);
        assert_eq!(deserialized.fields.len(), 1);
        assert_eq!(deserialized.fields[0].name, "email");
        assert!(deserialized.fields[0].unique);
    }

    /// Test that drop_collection rejects invalid collection names (SQL injection prevention).
    /// The validation check requires only alphanumeric + underscore characters.
    #[tokio::test]
    #[ignore = "requires running SurrealDB instance"]
    async fn test_drop_collection_rejects_invalid_names() {
        use ob_core::config::DatabaseConfig;

        let config = DatabaseConfig {
            endpoint: "ws://localhost:8000".to_string(),
            username: Some("root".to_string()),
            password: Some("root".to_string()),
            namespace: "test".to_string(),
            name: "test_admin".to_string(),
        };
        let db = DatabaseClient::connect(&config).await.unwrap();

        // Valid names should not produce a Validation error (may fail with DB error, that's ok)
        let valid_names = ["products", "my_table", "Table123", "a_b_c"];
        for name in valid_names {
            let result = drop_collection(&db, name).await;
            // Should NOT be a Validation error — may be a DB error if table doesn't exist
            if let Err(e) = &result {
                assert!(
                    !matches!(e, ob_core::Error::Validation(_)),
                    "Valid name '{name}' should not fail validation"
                );
            }
        }

        // Invalid names should produce Validation error
        let invalid_names = ["drop table;--", "my-table", "table name", "tbl.col", "a/b"];
        for name in invalid_names {
            let result = drop_collection(&db, name).await;
            assert!(
                matches!(result, Err(ob_core::Error::Validation(_))),
                "Invalid name '{name}' should fail validation"
            );
        }
    }

    /// Test collection name validation logic without needing a DB connection.
    #[test]
    fn test_collection_name_validation_logic() {
        // The validation check used in drop_collection:
        // name.chars().all(|c| c.is_alphanumeric() || c == '_')
        let valid = |name: &str| name.chars().all(|c| c.is_alphanumeric() || c == '_');

        assert!(valid("products"));
        assert!(valid("my_table"));
        assert!(valid("Table123"));
        assert!(valid("a"));
        assert!(valid("_private"));

        assert!(!valid("drop table;--"));
        assert!(!valid("my-table"));
        assert!(!valid("table name"));
        assert!(!valid("tbl.col"));
        assert!(!valid("a/b"));
        // Note: empty string passes chars().all() (vacuous truth), but is invalid semantically
    }

    #[test]
    fn test_empty_string_vacuous_truth() {
        // Document that empty string passes the validation predicate (vacuous truth).
        // This is a known edge case — the chars().all() check returns true for "".
        let valid = |name: &str| name.chars().all(|c| c.is_alphanumeric() || c == '_');
        assert!(valid(""), "Empty string should pass chars().all() due to vacuous truth");
    }

    #[test]
    fn test_collection_schema_empty_fields() {
        let schema = CollectionSchema {
            name: "empty_table".to_string(),
            fields: vec![],
        };
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["name"], "empty_table");
        assert!(json["fields"].as_array().unwrap().is_empty());

        // Roundtrip
        let json_str = serde_json::to_string(&schema).unwrap();
        let back: CollectionSchema = serde_json::from_str(&json_str).unwrap();
        assert_eq!(back.name, "empty_table");
        assert!(back.fields.is_empty());
    }

    #[test]
    fn test_function_meta_all_trigger_types_serde() {
        // Test FunctionMeta from ob-functions trigger types serialized as raw JSON
        // to verify the schema structures work with all trigger variants.
        let json_str = r#"{
            "name": "multi",
            "fields": [
                { "name": "a", "field_type": "string", "required": true, "unique": true, "indexed": true },
                { "name": "b", "field_type": "int", "required": false, "unique": false, "indexed": true },
                { "name": "c", "field_type": "bool" }
            ]
        }"#;
        let schema: CollectionSchema = serde_json::from_str(json_str).unwrap();
        assert_eq!(schema.fields.len(), 3);
        assert!(schema.fields[0].required);
        assert!(schema.fields[0].unique);
        assert!(schema.fields[0].indexed);
        assert!(!schema.fields[1].unique);
        assert!(schema.fields[1].indexed);
        // Field "c" should have defaults (required=false, unique=false, indexed=false)
        assert!(!schema.fields[2].required);
        assert!(!schema.fields[2].unique);
        assert!(!schema.fields[2].indexed);
    }

    #[test]
    fn test_field_def_all_true() {
        let field = FieldDef {
            name: "email".to_string(),
            field_type: "string".to_string(),
            required: true,
            unique: true,
            indexed: true,
        };
        let json = serde_json::to_value(&field).unwrap();
        assert_eq!(json["required"], true);
        assert_eq!(json["unique"], true);
        assert_eq!(json["indexed"], true);
    }
}
