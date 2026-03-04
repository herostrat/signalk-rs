use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single unit conversion definition (from a base SI unit to a target unit).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionDef {
    pub formula: String,
    pub inverse_formula: String,
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// A base unit with all its available conversions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitDefinition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_name: Option<String>,
    pub conversions: HashMap<String, ConversionDef>,
}

/// Root structure for standard-units-definitions.json and custom-units-definitions.json.
pub type UnitDefinitions = HashMap<String, UnitDefinition>;

/// Categories file: maps category names to their base SI unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoriesFile {
    pub category_to_base_unit: HashMap<String, String>,
    pub core_categories: Vec<String>,
}

/// A single category entry in default-categories.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultCategoryEntry {
    pub si_unit: String,
    pub paths: Vec<String>,
}

/// default-categories.json root structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultCategoriesFile {
    pub description: String,
    pub version: String,
    pub categories: HashMap<String, DefaultCategoryEntry>,
}

/// Custom categories: path → category name override.
pub type CustomCategories = HashMap<String, String>;

/// A preset category entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetCategory {
    pub base_unit: String,
    pub target_unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_format: Option<String>,
}

/// A unit preset file (e.g. nautical-metric.json).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub version: String,
    pub date: String,
    pub name: String,
    pub description: String,
    pub categories: HashMap<String, PresetCategory>,
}

/// Unit preferences config.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitPrefsConfig {
    pub active_preset: String,
}

/// Response for GET /presets — built-in and custom presets.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetsResponse {
    pub built_in: HashMap<String, PresetSummary>,
    pub custom: HashMap<String, PresetSummary>,
}

/// Summary of a preset for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetSummary {
    pub name: String,
    pub description: String,
}
