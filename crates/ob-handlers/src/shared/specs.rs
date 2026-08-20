//! Product specifications structs and validation.
//! Mirrors the Dart ProductSpec and ProductSpecs Freezed models.

use super::schema::spec_value_types;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductSpec {
    pub key: String,
    pub value: String,
    #[serde(default = "default_text")]
    pub value_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

fn default_text() -> String {
    "text".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProductSpecs {
    #[serde(default)]
    pub specs: Vec<ProductSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
}

pub fn validate_product_specs(specs: &ProductSpecs) -> ob_core::Result<()> {
    if specs.specs.len() > 50 {
        return Err(ob_core::Error::Validation(
            "Maximum 50 specifications allowed".into(),
        ));
    }
    for (i, spec) in specs.specs.iter().enumerate() {
        if spec.key.is_empty() || spec.key.len() > 64 {
            return Err(ob_core::Error::Validation(format!(
                "specs[{i}].key must be 1-64 characters"
            )));
        }
        if spec.value.is_empty() || spec.value.len() > 500 {
            return Err(ob_core::Error::Validation(format!(
                "specs[{i}].value must be 1-500 characters"
            )));
        }
        if !spec_value_types::ALL.contains(&spec.value_type.as_str()) {
            return Err(ob_core::Error::Validation(format!(
                "specs[{i}].valueType '{}' invalid. Must be one of: {:?}",
                spec.value_type,
                spec_value_types::ALL
            )));
        }
        if let Some(ref unit) = spec.unit
            && unit.len() > 20
        {
            return Err(ob_core::Error::Validation(format!(
                "specs[{i}].unit exceeds 20 characters"
            )));
        }
        if let Some(ref group) = spec.group
            && group.len() > 50
        {
            return Err(ob_core::Error::Validation(format!(
                "specs[{i}].group exceeds 50 characters"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_valid_specs() {
        let specs = ProductSpecs {
            specs: vec![
                ProductSpec {
                    key: "ram".into(),
                    value: "16 GB".into(),
                    value_type: "text".into(),
                    unit: Some("GB".into()),
                    group: Some("Performance".into()),
                },
                ProductSpec {
                    key: "storage".into(),
                    value: "512".into(),
                    value_type: "number".into(),
                    unit: Some("GB".into()),
                    group: Some("Performance".into()),
                },
            ],
            brand: Some("Samsung".into()),
            color: Some("Black".into()),
            material: None,
        };
        assert!(validate_product_specs(&specs).is_ok());
    }

    #[test]
    fn validate_empty_key_rejected() {
        let specs = ProductSpecs {
            specs: vec![ProductSpec {
                key: "".into(),
                value: "test".into(),
                value_type: "text".into(),
                unit: None,
                group: None,
            }],
            ..Default::default()
        };
        assert!(validate_product_specs(&specs).is_err());
    }

    #[test]
    fn validate_too_many_specs_rejected() {
        let specs = ProductSpecs {
            specs: (0..51)
                .map(|i| ProductSpec {
                    key: format!("key{i}"),
                    value: "val".into(),
                    value_type: "text".into(),
                    unit: None,
                    group: None,
                })
                .collect(),
            ..Default::default()
        };
        assert!(validate_product_specs(&specs).is_err());
    }

    #[test]
    fn validate_invalid_value_type_rejected() {
        let specs = ProductSpecs {
            specs: vec![ProductSpec {
                key: "test".into(),
                value: "val".into(),
                value_type: "invalid".into(),
                unit: None,
                group: None,
            }],
            ..Default::default()
        };
        assert!(validate_product_specs(&specs).is_err());
    }

    #[test]
    fn validate_empty_specs_ok() {
        let specs = ProductSpecs::default();
        assert!(validate_product_specs(&specs).is_ok());
    }

    #[test]
    fn serde_roundtrip() {
        let specs = ProductSpecs {
            specs: vec![ProductSpec {
                key: "ram".into(),
                value: "16 GB".into(),
                value_type: "text".into(),
                unit: Some("GB".into()),
                group: Some("Performance".into()),
            }],
            brand: Some("Apple".into()),
            color: None,
            material: None,
        };
        let json = serde_json::to_string(&specs).unwrap();
        let restored: ProductSpecs = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.specs.len(), 1);
        assert_eq!(restored.specs[0].key, "ram");
        assert_eq!(restored.brand, Some("Apple".into()));
    }
}
