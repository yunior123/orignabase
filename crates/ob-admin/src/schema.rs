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
}
