//! Food product nutrition structs and validation.
//!
//! Mirrors the Dart `NutritionFacts` and `FoodMetadata` Freezed models.
//! All nutrient amounts stored as integers (mg/mcg/kcal) for precision.

use serde::{Deserialize, Serialize};

use super::schema::{allergen_values, dietary_badge_values, fop_thresholds};

/// Nutrition facts per serving (Health Canada NFT format).
///
/// All macro/mineral values in milligrams, vitamins A/D in micrograms.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NutritionFacts {
    pub serving_size_amount: i64,
    pub serving_size_unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub servings_per_container: Option<i64>,
    pub calories_kcal: i64,
    pub total_fat_mg: i64,
    pub saturated_fat_mg: i64,
    pub trans_fat_mg: i64,
    pub cholesterol_mg: i64,
    pub sodium_mg: i64,
    pub total_carbohydrate_mg: i64,
    pub fibre_mg: i64,
    pub sugars_mg: i64,
    pub protein_mg: i64,
    pub vitamin_a_mcg: i64,
    pub vitamin_c_mg: i64,
    pub calcium_mg: i64,
    pub iron_mg: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_sugars_mg: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub potassium_mg: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vitamin_d_mcg: Option<i64>,
}

/// Food metadata: ingredients, allergens, storage, dietary badges.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FoodMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingredients_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingredients_fr: Option<String>,
    #[serde(default)]
    pub allergens: Vec<String>,
    #[serde(default)]
    pub may_contain_allergens: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_instructions_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_instructions_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_before_days: Option<i64>,
    #[serde(default)]
    pub dietary_badges: Vec<String>,
    #[serde(default)]
    pub fop_high_sodium: bool,
    #[serde(default)]
    pub fop_high_sugars: bool,
    #[serde(default)]
    pub fop_high_saturated_fat: bool,
}

/// Validate all nutrient values in a NutritionFacts struct.
pub fn validate_nutrition_facts(nf: &NutritionFacts) -> ob_core::Result<()> {
    validate_nutrient("servingSizeAmount", nf.serving_size_amount)?;
    if nf.serving_size_amount <= 0 {
        return Err(ob_core::Error::Validation(
            "servingSizeAmount must be positive".into(),
        ));
    }
    let unit = nf.serving_size_unit.as_str();
    if unit != "g" && unit != "mL" {
        return Err(ob_core::Error::Validation(
            "servingSizeUnit must be 'g' or 'mL'".into(),
        ));
    }
    validate_nutrient("caloriesKcal", nf.calories_kcal)?;
    validate_nutrient("totalFatMg", nf.total_fat_mg)?;
    validate_nutrient("saturatedFatMg", nf.saturated_fat_mg)?;
    validate_nutrient("transFatMg", nf.trans_fat_mg)?;
    validate_nutrient("cholesterolMg", nf.cholesterol_mg)?;
    validate_nutrient("sodiumMg", nf.sodium_mg)?;
    validate_nutrient("totalCarbohydrateMg", nf.total_carbohydrate_mg)?;
    validate_nutrient("fibreMg", nf.fibre_mg)?;
    validate_nutrient("sugarsMg", nf.sugars_mg)?;
    validate_nutrient("proteinMg", nf.protein_mg)?;
    validate_nutrient("vitaminAMcg", nf.vitamin_a_mcg)?;
    validate_nutrient("vitaminCMg", nf.vitamin_c_mg)?;
    validate_nutrient("calciumMg", nf.calcium_mg)?;
    validate_nutrient("ironMg", nf.iron_mg)?;
    if let Some(v) = nf.added_sugars_mg {
        validate_nutrient("addedSugarsMg", v)?;
    }
    if let Some(v) = nf.potassium_mg {
        validate_nutrient("potassiumMg", v)?;
    }
    if let Some(v) = nf.vitamin_d_mcg {
        validate_nutrient("vitaminDMcg", v)?;
    }
    if let Some(v) = nf.servings_per_container
        && v <= 0
    {
        return Err(ob_core::Error::Validation(
            "servingsPerContainer must be positive".into(),
        ));
    }
    Ok(())
}

/// Validate food metadata: allergens and dietary badges from allowed sets.
pub fn validate_food_metadata(fm: &FoodMetadata) -> ob_core::Result<()> {
    for a in &fm.allergens {
        if !allergen_values::ALL.contains(&a.as_str()) {
            return Err(ob_core::Error::Validation(format!(
                "Unknown allergen: '{a}'. Must be one of: {:?}",
                allergen_values::ALL
            )));
        }
    }
    for a in &fm.may_contain_allergens {
        if !allergen_values::ALL.contains(&a.as_str()) {
            return Err(ob_core::Error::Validation(format!(
                "Unknown allergen in mayContain: '{a}'"
            )));
        }
    }
    for b in &fm.dietary_badges {
        if !dietary_badge_values::ALL.contains(&b.as_str()) {
            return Err(ob_core::Error::Validation(format!(
                "Unknown dietary badge: '{b}'. Must be one of: {:?}",
                dietary_badge_values::ALL
            )));
        }
    }
    Ok(())
}

/// Compute Health Canada FOP warnings from nutrition facts.
///
/// Returns (high_sodium, high_sugars, high_saturated_fat).
pub fn compute_fop_warnings(nf: &NutritionFacts) -> (bool, bool, bool) {
    (
        nf.sodium_mg >= fop_thresholds::SODIUM_MG_PER_SERVING,
        nf.sugars_mg >= fop_thresholds::SUGARS_MG_PER_SERVING,
        nf.saturated_fat_mg >= fop_thresholds::SATURATED_FAT_MG_PER_SERVING,
    )
}

/// Validate a single nutrient value: non-negative and within bounds.
fn validate_nutrient(field: &str, value: i64) -> ob_core::Result<()> {
    if value < 0 {
        return Err(ob_core::Error::Validation(format!(
            "{field} cannot be negative"
        )));
    }
    if value > 999_999 {
        return Err(ob_core::Error::Validation(format!(
            "{field} exceeds maximum (999999)"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_nutrition_facts() -> NutritionFacts {
        NutritionFacts {
            serving_size_amount: 55,
            serving_size_unit: "g".into(),
            servings_per_container: Some(8),
            calories_kcal: 230,
            total_fat_mg: 12000,
            saturated_fat_mg: 3000,
            trans_fat_mg: 0,
            cholesterol_mg: 0,
            sodium_mg: 160,
            total_carbohydrate_mg: 37000,
            fibre_mg: 4000,
            sugars_mg: 12000,
            protein_mg: 3000,
            vitamin_a_mcg: 100,
            vitamin_c_mg: 10,
            calcium_mg: 260,
            iron_mg: 8,
            added_sugars_mg: None,
            potassium_mg: Some(235),
            vitamin_d_mcg: Some(2),
        }
    }

    #[test]
    fn validate_nutrition_facts_valid() {
        let nf = valid_nutrition_facts();
        assert!(validate_nutrition_facts(&nf).is_ok());
    }

    #[test]
    fn validate_nutrition_facts_negative_rejected() {
        let mut nf = valid_nutrition_facts();
        nf.sodium_mg = -1;
        let err = validate_nutrition_facts(&nf);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("sodiumMg"));
    }

    #[test]
    fn validate_nutrition_facts_exceeds_max_rejected() {
        let mut nf = valid_nutrition_facts();
        nf.total_fat_mg = 1_000_000;
        assert!(validate_nutrition_facts(&nf).is_err());
    }

    #[test]
    fn validate_nutrition_facts_zero_serving_rejected() {
        let mut nf = valid_nutrition_facts();
        nf.serving_size_amount = 0;
        assert!(validate_nutrition_facts(&nf).is_err());
    }

    #[test]
    fn validate_nutrition_facts_invalid_unit_rejected() {
        let mut nf = valid_nutrition_facts();
        nf.serving_size_unit = "oz".into();
        assert!(validate_nutrition_facts(&nf).is_err());
    }

    #[test]
    fn compute_fop_warnings_high_sodium() {
        let mut nf = valid_nutrition_facts();
        nf.sodium_mg = 400; // >= 345 threshold
        nf.sugars_mg = 5000; // below 15000
        nf.saturated_fat_mg = 1000; // below 3000
        let (sodium, sugars, sat_fat) = compute_fop_warnings(&nf);
        assert!(sodium);
        assert!(!sugars);
        assert!(!sat_fat);
    }

    #[test]
    fn compute_fop_warnings_high_sugars() {
        let mut nf = valid_nutrition_facts();
        nf.sodium_mg = 100;
        nf.sugars_mg = 54000; // 54g
        nf.saturated_fat_mg = 1000;
        let (sodium, sugars, sat_fat) = compute_fop_warnings(&nf);
        assert!(!sodium);
        assert!(sugars);
        assert!(!sat_fat);
    }

    #[test]
    fn compute_fop_warnings_none() {
        let mut nf = valid_nutrition_facts();
        nf.sodium_mg = 100;
        nf.sugars_mg = 5000;
        nf.saturated_fat_mg = 1000;
        let (sodium, sugars, sat_fat) = compute_fop_warnings(&nf);
        assert!(!sodium);
        assert!(!sugars);
        assert!(!sat_fat);
    }

    #[test]
    fn validate_allergens_valid() {
        let fm = FoodMetadata {
            allergens: vec!["wheat".into(), "milk".into()],
            may_contain_allergens: vec!["soy".into()],
            ..Default::default()
        };
        assert!(validate_food_metadata(&fm).is_ok());
    }

    #[test]
    fn validate_allergens_invalid_rejected() {
        let fm = FoodMetadata {
            allergens: vec!["kiwi".into()],
            ..Default::default()
        };
        let err = validate_food_metadata(&fm);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("kiwi"));
    }

    #[test]
    fn validate_dietary_badges_valid() {
        let fm = FoodMetadata {
            dietary_badges: vec!["organic".into(), "vegan".into()],
            ..Default::default()
        };
        assert!(validate_food_metadata(&fm).is_ok());
    }

    #[test]
    fn validate_dietary_badges_invalid_rejected() {
        let fm = FoodMetadata {
            dietary_badges: vec!["paleo".into()],
            ..Default::default()
        };
        assert!(validate_food_metadata(&fm).is_err());
    }

    #[test]
    fn serde_roundtrip_nutrition_facts() {
        let nf = valid_nutrition_facts();
        let json = serde_json::to_string(&nf).unwrap();
        let restored: NutritionFacts = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.serving_size_amount, 55);
        assert_eq!(restored.calories_kcal, 230);
        assert_eq!(restored.potassium_mg, Some(235));
    }

    #[test]
    fn serde_roundtrip_food_metadata() {
        let fm = FoodMetadata {
            ingredients_en: Some("Oats, sugar".into()),
            ingredients_fr: Some("Avoine, sucre".into()),
            allergens: vec!["wheat".into()],
            dietary_badges: vec!["organic".into()],
            fop_high_sugars: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&fm).unwrap();
        let restored: FoodMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.ingredients_en, Some("Oats, sugar".into()));
        assert_eq!(restored.allergens, vec!["wheat"]);
        assert!(restored.fop_high_sugars);
        assert!(!restored.fop_high_sodium);
    }
}
