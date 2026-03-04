pub mod api;
pub mod loader;
pub mod resolver;
pub mod types;

use signalk_types::DisplayUnits;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use types::*;

/// Manages unit preferences: definitions, categories, presets, and display unit resolution.
///
/// Thread-safe via interior RwLock. The static_dir contains the shipped JSON files,
/// the data_dir contains user-customizable files (config, custom definitions/categories, custom presets).
pub struct UnitPreferencesManager {
    static_dir: PathBuf,
    data_dir: PathBuf,
    inner: RwLock<Inner>,
}

struct Inner {
    definitions: UnitDefinitions,
    categories: CategoriesFile,
    default_categories: DefaultCategoriesFile,
    custom_categories: CustomCategories,
    config: UnitPrefsConfig,
    active_preset: Preset,
}

impl UnitPreferencesManager {
    /// Create a new manager, loading all JSON data from disk.
    ///
    /// `static_dir` — shipped JSON files (read-only, e.g. /usr/share/signalk-rs/unitpreferences/)
    /// `data_dir` — writable directory for user customizations (e.g. {data_dir}/unitpreferences/)
    pub fn new(static_dir: &Path, data_dir: &Path) -> Result<Self, String> {
        // Ensure data directories exist
        let _ = std::fs::create_dir_all(data_dir.join("presets"));

        let definitions = loader::load_definitions(static_dir, data_dir)?;
        let categories = loader::load_categories(static_dir)?;
        let default_categories = loader::load_default_categories(static_dir)?;
        let custom_categories = loader::load_custom_categories(data_dir);
        let config = loader::load_config(data_dir, static_dir);
        let active_preset = loader::load_preset(&config.active_preset, static_dir, data_dir)?;

        Ok(Self {
            static_dir: static_dir.to_path_buf(),
            data_dir: data_dir.to_path_buf(),
            inner: RwLock::new(Inner {
                definitions,
                categories,
                default_categories,
                custom_categories,
                config,
                active_preset,
            }),
        })
    }

    /// Resolve display units for a path based on the active preset.
    pub fn resolve_display_units(&self, path: &str) -> Option<DisplayUnits> {
        let inner = self.inner.read().unwrap();
        resolver::resolve_display_units(
            path,
            &inner.default_categories,
            &inner.custom_categories,
            &inner.categories,
            &inner.active_preset,
            &inner.definitions,
        )
    }

    /// Get the current config.
    pub fn config(&self) -> UnitPrefsConfig {
        self.inner.read().unwrap().config.clone()
    }

    /// Set the active preset and reload it.
    pub fn set_active_preset(&self, preset_name: &str) -> Result<(), String> {
        let preset = loader::load_preset(preset_name, &self.static_dir, &self.data_dir)?;
        let mut inner = self.inner.write().unwrap();
        inner.config.active_preset = preset_name.to_string();
        inner.active_preset = preset;

        // Persist config
        let config_json = serde_json::to_string_pretty(&inner.config)
            .map_err(|e| format!("Failed to serialize config: {e}"))?;
        std::fs::write(self.data_dir.join("config.json"), config_json)
            .map_err(|e| format!("Failed to write config: {e}"))?;
        Ok(())
    }

    /// Get the active preset.
    pub fn active_preset(&self) -> Preset {
        self.inner.read().unwrap().active_preset.clone()
    }

    /// List all available presets (built-in + custom).
    pub fn list_presets(&self) -> PresetsResponse {
        let mut built_in = std::collections::HashMap::new();
        let mut custom = std::collections::HashMap::new();

        for name in loader::list_builtin_presets(&self.static_dir) {
            if let Ok(preset) = loader::load_preset(&name, &self.static_dir, &self.data_dir) {
                built_in.insert(
                    name,
                    PresetSummary {
                        name: preset.name,
                        description: preset.description,
                    },
                );
            }
        }

        for name in loader::list_custom_presets(&self.data_dir) {
            if let Ok(preset) = loader::load_preset(&name, &self.static_dir, &self.data_dir) {
                custom.insert(
                    name,
                    PresetSummary {
                        name: preset.name,
                        description: preset.description,
                    },
                );
            }
        }

        PresetsResponse { built_in, custom }
    }

    /// Get a specific preset by name.
    pub fn get_preset(&self, name: &str) -> Result<Preset, String> {
        loader::load_preset(name, &self.static_dir, &self.data_dir)
    }

    /// Save a custom preset.
    pub fn save_custom_preset(&self, name: &str, preset: &Preset) -> Result<(), String> {
        let presets_dir = self.data_dir.join("presets");
        let _ = std::fs::create_dir_all(&presets_dir);
        let json = serde_json::to_string_pretty(preset)
            .map_err(|e| format!("Failed to serialize preset: {e}"))?;
        std::fs::write(presets_dir.join(format!("{name}.json")), json)
            .map_err(|e| format!("Failed to write preset: {e}"))?;

        // If this is the active preset, reload it
        let is_active = self.inner.read().unwrap().config.active_preset == name;
        if is_active {
            let mut inner = self.inner.write().unwrap();
            inner.active_preset = preset.clone();
        }

        Ok(())
    }

    /// Delete a custom preset.
    pub fn delete_custom_preset(&self, name: &str) -> Result<(), String> {
        let path = self.data_dir.join(format!("presets/{name}.json"));
        if !path.exists() {
            return Err(format!("Custom preset '{name}' not found"));
        }
        std::fs::remove_file(&path).map_err(|e| format!("Failed to delete preset: {e}"))?;
        Ok(())
    }

    /// Get all unit definitions (merged standard + custom).
    pub fn definitions(&self) -> UnitDefinitions {
        self.inner.read().unwrap().definitions.clone()
    }

    /// Get custom unit definitions.
    pub fn custom_definitions(&self) -> UnitDefinitions {
        let path = self.data_dir.join("custom-units-definitions.json");
        if path.exists() {
            loader::load_json_public(&path).unwrap_or_default()
        } else {
            std::collections::HashMap::new()
        }
    }

    /// Save custom unit definitions and reload.
    pub fn save_custom_definitions(&self, defs: &UnitDefinitions) -> Result<(), String> {
        let json = serde_json::to_string_pretty(defs)
            .map_err(|e| format!("Failed to serialize definitions: {e}"))?;
        std::fs::write(self.data_dir.join("custom-units-definitions.json"), json)
            .map_err(|e| format!("Failed to write definitions: {e}"))?;

        // Reload merged definitions
        let new_defs = loader::load_definitions(&self.static_dir, &self.data_dir)?;
        self.inner.write().unwrap().definitions = new_defs;
        Ok(())
    }

    /// Get custom categories.
    pub fn custom_categories(&self) -> CustomCategories {
        self.inner.read().unwrap().custom_categories.clone()
    }

    /// Save custom categories and reload.
    pub fn save_custom_categories(&self, cats: &CustomCategories) -> Result<(), String> {
        let json = serde_json::to_string_pretty(cats)
            .map_err(|e| format!("Failed to serialize categories: {e}"))?;
        std::fs::write(self.data_dir.join("custom-categories.json"), json)
            .map_err(|e| format!("Failed to write categories: {e}"))?;

        self.inner.write().unwrap().custom_categories = cats.clone();
        Ok(())
    }

    /// Get the categories file (category → base unit).
    pub fn categories(&self) -> CategoriesFile {
        self.inner.read().unwrap().categories.clone()
    }

    /// Get the default categories file (path → category assignments).
    pub fn default_categories(&self) -> DefaultCategoriesFile {
        self.inner.read().unwrap().default_categories.clone()
    }
}
