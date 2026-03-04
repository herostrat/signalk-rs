use std::collections::HashMap;
use std::path::Path;

use super::types::*;

/// Load and parse a JSON file, returning an error message on failure.
pub fn load_json_public<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    load_json(path)
}

fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse {}: {e}", path.display()))
}

/// Load unit definitions from standard + custom files, merging custom on top.
pub fn load_definitions(static_dir: &Path, data_dir: &Path) -> Result<UnitDefinitions, String> {
    let mut defs: UnitDefinitions = load_json(&static_dir.join("standard-units-definitions.json"))?;

    let custom_path = data_dir.join("custom-units-definitions.json");
    if custom_path.exists() {
        let custom: UnitDefinitions = load_json(&custom_path)?;
        for (base_unit, custom_def) in custom {
            let entry = defs.entry(base_unit).or_insert_with(|| UnitDefinition {
                long_name: custom_def.long_name.clone(),
                conversions: HashMap::new(),
            });
            for (target, conv) in custom_def.conversions {
                entry.conversions.insert(target, conv);
            }
        }
    }

    Ok(defs)
}

/// Load categories mapping (category → base unit).
pub fn load_categories(static_dir: &Path) -> Result<CategoriesFile, String> {
    load_json(&static_dir.join("categories.json"))
}

/// Load default path→category assignments.
pub fn load_default_categories(static_dir: &Path) -> Result<DefaultCategoriesFile, String> {
    load_json(&static_dir.join("default-categories.json"))
}

/// Load custom path→category overrides.
pub fn load_custom_categories(data_dir: &Path) -> CustomCategories {
    let path = data_dir.join("custom-categories.json");
    if path.exists() {
        load_json(&path).unwrap_or_default()
    } else {
        HashMap::new()
    }
}

/// Load the config file (activePreset).
pub fn load_config(data_dir: &Path, static_dir: &Path) -> UnitPrefsConfig {
    let data_path = data_dir.join("config.json");
    if data_path.exists()
        && let Ok(cfg) = load_json::<UnitPrefsConfig>(&data_path)
    {
        return cfg;
    }
    load_json(&static_dir.join("config.json")).unwrap_or(UnitPrefsConfig {
        active_preset: "nautical-metric".to_string(),
    })
}

/// Load a preset by name. Checks custom presets (data_dir) first, then built-in (static_dir/presets/).
pub fn load_preset(name: &str, static_dir: &Path, data_dir: &Path) -> Result<Preset, String> {
    let custom_path = data_dir.join(format!("presets/{name}.json"));
    if custom_path.exists() {
        return load_json(&custom_path);
    }
    load_json(&static_dir.join(format!("presets/{name}.json")))
}

/// List all built-in preset names.
pub fn list_builtin_presets(static_dir: &Path) -> Vec<String> {
    let presets_dir = static_dir.join("presets");
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(presets_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.path().file_stem() {
                names.push(name.to_string_lossy().to_string());
            }
        }
    }
    names.sort();
    names
}

/// List all custom preset names.
pub fn list_custom_presets(data_dir: &Path) -> Vec<String> {
    let presets_dir = data_dir.join("presets");
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(presets_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.path().file_stem() {
                names.push(name.to_string_lossy().to_string());
            }
        }
    }
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn static_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("unitpreferences")
    }

    #[test]
    fn load_standard_definitions() {
        let defs = load_definitions(&static_dir(), &static_dir()).unwrap();
        assert!(defs.contains_key("m/s"), "should have m/s");
        assert!(defs.contains_key("K"), "should have K");
        let ms = &defs["m/s"];
        assert!(
            ms.conversions.contains_key("kn"),
            "m/s should have kn conversion"
        );
    }

    #[test]
    fn load_categories_file() {
        let cats = load_categories(&static_dir()).unwrap();
        assert_eq!(cats.category_to_base_unit.get("speed").unwrap(), "m/s");
        assert_eq!(cats.category_to_base_unit.get("temperature").unwrap(), "K");
    }

    #[test]
    fn load_default_categories_file() {
        let dc = load_default_categories(&static_dir()).unwrap();
        assert!(dc.categories.contains_key("speed"));
        let speed = &dc.categories["speed"];
        assert!(
            speed
                .paths
                .contains(&"navigation.speedOverGround".to_string())
        );
    }

    #[test]
    fn load_config_file() {
        let cfg = load_config(&static_dir(), &static_dir());
        assert_eq!(cfg.active_preset, "nautical-metric");
    }

    #[test]
    fn load_preset_nautical_metric() {
        let preset = load_preset("nautical-metric", &static_dir(), &static_dir()).unwrap();
        assert_eq!(preset.categories["speed"].target_unit, "kn");
        assert_eq!(preset.categories["temperature"].target_unit, "C");
    }

    #[test]
    fn list_builtin_presets_finds_all() {
        let presets = list_builtin_presets(&static_dir());
        assert!(
            presets.len() >= 6,
            "should have at least 6 built-in presets"
        );
        assert!(presets.contains(&"nautical-metric".to_string()));
    }
}
