use signalk_types::DisplayUnits;

use super::types::*;
use std::collections::HashMap;

/// Resolve the category for a path, checking custom overrides first, then default-categories.
///
/// Supports wildcard matching: `propulsion.*.temperature` matches `propulsion.main.temperature`.
pub fn resolve_category(
    path: &str,
    default_categories: &DefaultCategoriesFile,
    custom_categories: &CustomCategories,
) -> Option<String> {
    // Custom override (exact match)
    if let Some(cat) = custom_categories.get(path) {
        return Some(cat.clone());
    }

    // Default categories: exact match first, then wildcard
    for (category, entry) in &default_categories.categories {
        for pattern in &entry.paths {
            if pattern == path {
                return Some(category.clone());
            }
        }
    }

    // Wildcard matching: `*` matches a single path segment
    for (category, entry) in &default_categories.categories {
        for pattern in &entry.paths {
            if pattern.contains('*') && matches_wildcard(pattern, path) {
                return Some(category.clone());
            }
        }
    }

    None
}

/// Check if a path matches a wildcard pattern where `*` matches exactly one segment.
fn matches_wildcard(pattern: &str, path: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('.').collect();
    let path_parts: Vec<&str> = path.split('.').collect();

    if pattern_parts.len() != path_parts.len() {
        return false;
    }

    pattern_parts
        .iter()
        .zip(path_parts.iter())
        .all(|(p, s)| *p == "*" || p == s)
}

/// Resolve DisplayUnits for a given path using the active preset.
pub fn resolve_display_units(
    path: &str,
    default_categories: &DefaultCategoriesFile,
    custom_categories: &CustomCategories,
    categories: &CategoriesFile,
    active_preset: &Preset,
    definitions: &UnitDefinitions,
) -> Option<DisplayUnits> {
    // 1. Path → Category
    let category = resolve_category(path, default_categories, custom_categories)?;

    // 2. Category → base unit (from categories.json)
    let base_unit = categories.category_to_base_unit.get(&category)?;

    // 3. Preset → target unit for this category
    let preset_cat = active_preset.categories.get(&category)?;
    let target_unit = &preset_cat.target_unit;

    // 4. Definition → conversion formula
    let unit_def = definitions.get(base_unit)?;
    let conversion = if let Some(conv) = unit_def.conversions.get(target_unit) {
        conv.clone()
    } else if target_unit == base_unit {
        // Identity conversion: target equals base unit (e.g. depth "m" → "m")
        super::types::ConversionDef {
            formula: "value * 1".to_string(),
            inverse_formula: "value * 1".to_string(),
            symbol: base_unit.clone(),
            long_name: unit_def.long_name.clone(),
            key: None,
        }
    } else {
        return None;
    };

    Some(DisplayUnits {
        category,
        target_unit: target_unit.clone(),
        formula: conversion.formula.clone(),
        inverse_formula: conversion.inverse_formula.clone(),
        symbol: conversion.symbol.clone(),
        display_format: preset_cat.display_format.clone(),
    })
}

/// Get the SI base unit for a path (used by default_metadata refactoring).
pub fn resolve_base_unit(
    path: &str,
    default_categories: &DefaultCategoriesFile,
    custom_categories: &CustomCategories,
    categories: &CategoriesFile,
) -> Option<String> {
    let category = resolve_category(path, default_categories, custom_categories)?;
    categories.category_to_base_unit.get(&category).cloned()
}

/// Resolve display units for all known paths in the active preset.
/// Returns a map from path to DisplayUnits.
pub fn resolve_all(
    default_categories: &DefaultCategoriesFile,
    custom_categories: &CustomCategories,
    categories: &CategoriesFile,
    active_preset: &Preset,
    definitions: &UnitDefinitions,
) -> HashMap<String, DisplayUnits> {
    let mut result = HashMap::new();

    // Iterate all explicit paths in default-categories
    for (category, entry) in &default_categories.categories {
        for pattern in &entry.paths {
            // Skip wildcard patterns — they can't be pre-resolved
            if pattern.contains('*') {
                continue;
            }
            if let Some(du) = resolve_display_units(
                pattern,
                default_categories,
                custom_categories,
                categories,
                active_preset,
                definitions,
            ) {
                result.insert(pattern.clone(), du);
            }
        }
        // Also check custom overrides pointing to this category
        for (path, cat) in custom_categories {
            if cat == category
                && let Some(du) = resolve_display_units(
                    path,
                    default_categories,
                    custom_categories,
                    categories,
                    active_preset,
                    definitions,
                )
            {
                result.insert(path.clone(), du);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unitpreferences::loader;
    use std::path::PathBuf;

    fn static_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("unitpreferences")
    }

    fn test_data() -> (
        DefaultCategoriesFile,
        CustomCategories,
        CategoriesFile,
        Preset,
        UnitDefinitions,
    ) {
        let sd = static_dir();
        let dc = loader::load_default_categories(&sd).unwrap();
        let cc = HashMap::new();
        let cats = loader::load_categories(&sd).unwrap();
        let preset = loader::load_preset("nautical-metric", &sd, &sd).unwrap();
        let defs = loader::load_definitions(&sd, &sd).unwrap();
        (dc, cc, cats, preset, defs)
    }

    #[test]
    fn resolve_sog_category() {
        let (dc, cc, _, _, _) = test_data();
        let cat = resolve_category("navigation.speedOverGround", &dc, &cc);
        assert_eq!(cat.as_deref(), Some("speed"));
    }

    #[test]
    fn resolve_wildcard_category() {
        let (dc, cc, _, _, _) = test_data();
        let cat = resolve_category("propulsion.main.temperature", &dc, &cc);
        assert_eq!(cat.as_deref(), Some("temperature"));
    }

    #[test]
    fn resolve_sog_display_units() {
        let (dc, cc, cats, preset, defs) = test_data();
        let du = resolve_display_units(
            "navigation.speedOverGround",
            &dc,
            &cc,
            &cats,
            &preset,
            &defs,
        )
        .unwrap();
        assert_eq!(du.category, "speed");
        assert_eq!(du.target_unit, "kn");
        assert_eq!(du.symbol, "kn");
        assert!(du.formula.contains("1.94384"));
    }

    #[test]
    fn resolve_temperature_display_units() {
        let (dc, cc, cats, preset, defs) = test_data();
        let du = resolve_display_units(
            "environment.outside.temperature",
            &dc,
            &cc,
            &cats,
            &preset,
            &defs,
        )
        .unwrap();
        assert_eq!(du.category, "temperature");
        assert_eq!(du.target_unit, "C");
        assert_eq!(du.symbol, "°C");
    }

    #[test]
    fn resolve_depth_display_units() {
        let (dc, cc, cats, preset, defs) = test_data();
        let du = resolve_display_units(
            "environment.depth.belowKeel",
            &dc,
            &cc,
            &cats,
            &preset,
            &defs,
        )
        .unwrap();
        assert_eq!(du.category, "depth");
        assert_eq!(du.target_unit, "m");
        assert_eq!(du.symbol, "m");
    }

    #[test]
    fn resolve_unknown_path_returns_none() {
        let (dc, cc, cats, preset, defs) = test_data();
        let du = resolve_display_units("some.unknown.path", &dc, &cc, &cats, &preset, &defs);
        assert!(du.is_none());
    }

    #[test]
    fn wildcard_matching() {
        assert!(matches_wildcard(
            "propulsion.*.temperature",
            "propulsion.main.temperature"
        ));
        assert!(matches_wildcard(
            "propulsion.*.temperature",
            "propulsion.port.temperature"
        ));
        assert!(!matches_wildcard(
            "propulsion.*.temperature",
            "propulsion.main.oilTemperature"
        ));
        assert!(!matches_wildcard(
            "propulsion.*.temperature",
            "propulsion.temperature"
        ));
        assert!(!matches_wildcard(
            "propulsion.*.temperature",
            "propulsion.main.sub.temperature"
        ));
    }

    #[test]
    fn custom_category_override() {
        let (dc, _, cats, preset, defs) = test_data();
        let mut cc = HashMap::new();
        cc.insert("my.custom.path".to_string(), "speed".to_string());
        let du = resolve_display_units("my.custom.path", &dc, &cc, &cats, &preset, &defs).unwrap();
        assert_eq!(du.category, "speed");
        assert_eq!(du.target_unit, "kn");
    }

    #[test]
    fn resolve_pressure_display_units() {
        let (dc, cc, cats, preset, defs) = test_data();
        let du = resolve_display_units(
            "environment.outside.pressure",
            &dc,
            &cc,
            &cats,
            &preset,
            &defs,
        )
        .unwrap();
        assert_eq!(du.category, "pressure");
        assert_eq!(du.target_unit, "mbar");
        assert_eq!(du.symbol, "mbar");
    }
}
