/// PluginRegistry — tier-agnostic registry of all plugins for the admin API.
///
/// Aggregates plugin info from all tiers:
/// - **Tier 1 (Rust):** Populated from `PluginManager::statuses()`
/// - **Tier 2 (Bridge):** Populated via `POST /internal/v1/bridge/plugins`
/// - **Tier 3 (Standalone):** Populated via registration endpoint (future)
///
/// The admin API reads from this registry to present a unified view.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Plugin tier — where the plugin runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginTier {
    /// Tier 1: compiled Rust, runs in-process
    Rust,
    /// Tier 2: Node.js, runs in bridge process
    Bridge,
    /// Tier 3: external binary, connects via UDS
    Standalone,
}

/// Tier-agnostic plugin info for the admin API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub tier: PluginTier,
    pub status: String,
    pub enabled: bool,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub has_webapp: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webapp_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
}

/// Plugin info reported by the bridge for Tier 2 plugins.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgePluginInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub has_webapp: bool,
}

/// Central registry that aggregates plugin info across all tiers.
#[derive(Debug, Default)]
pub struct PluginRegistry {
    plugins: HashMap<String, PluginInfo>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register or update a Tier 1 (Rust) plugin.
    pub fn register_tier1(
        &mut self,
        id: &str,
        name: &str,
        description: &str,
        version: &str,
        status: &str,
        enabled: bool,
    ) {
        self.plugins.insert(
            id.to_string(),
            PluginInfo {
                id: id.to_string(),
                name: name.to_string(),
                description: description.to_string(),
                version: version.to_string(),
                tier: PluginTier::Rust,
                status: status.to_string(),
                enabled,
                keywords: vec!["signalk-node-server-plugin".to_string()],
                has_webapp: false,
                webapp_url: None,
                schema: None,
            },
        );
    }

    /// Register or update a Tier 2 (Bridge/Node.js) plugin.
    pub fn register_tier2(&mut self, info: BridgePluginInfo) {
        self.plugins.insert(
            info.id.clone(),
            PluginInfo {
                id: info.id,
                name: info.name,
                description: info.description,
                version: info.version,
                tier: PluginTier::Bridge,
                status: "running".to_string(),
                enabled: true,
                keywords: vec!["signalk-node-server-plugin".to_string()],
                has_webapp: info.has_webapp,
                webapp_url: None,
                schema: None,
            },
        );
    }

    /// Update the status of a plugin by ID.
    pub fn update_status(&mut self, id: &str, status: &str) {
        if let Some(info) = self.plugins.get_mut(id) {
            info.status = status.to_string();
            info.enabled = !status.starts_with("stopped") && !status.starts_with("error");
        }
    }

    /// Set the webapp URL for a plugin (when it also has a webapp).
    pub fn set_webapp_url(&mut self, id: &str, url: &str) {
        if let Some(info) = self.plugins.get_mut(id) {
            info.has_webapp = true;
            info.webapp_url = Some(url.to_string());
        }
    }

    /// Get all registered plugins as a unified list.
    pub fn all(&self) -> Vec<PluginInfo> {
        self.plugins.values().cloned().collect()
    }

    /// Get a single plugin by ID.
    pub fn get(&self, id: &str) -> Option<&PluginInfo> {
        self.plugins.get(id)
    }

    /// Get a mutable reference to a single plugin by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut PluginInfo> {
        self.plugins.get_mut(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_tier1_and_tier2() {
        let mut reg = PluginRegistry::new();
        reg.register_tier1(
            "sensor-data-simulator",
            "Sensor Data Simulator",
            "Test data generator",
            "0.1.0",
            "running",
            true,
        );
        reg.register_tier2(BridgePluginInfo {
            id: "signalk-to-nmea0183".to_string(),
            name: "SignalK to NMEA0183".to_string(),
            version: "3.0.0".to_string(),
            description: "Converts to NMEA".to_string(),
            has_webapp: false,
        });

        let all = reg.all();
        assert_eq!(all.len(), 2);

        let sim = reg.get("sensor-data-simulator").unwrap();
        assert_eq!(sim.tier, PluginTier::Rust);
        assert!(sim.enabled);

        let nmea = reg.get("signalk-to-nmea0183").unwrap();
        assert_eq!(nmea.tier, PluginTier::Bridge);
    }

    #[test]
    fn update_status() {
        let mut reg = PluginRegistry::new();
        reg.register_tier1("test", "Test", "desc", "0.1.0", "running", true);

        reg.update_status("test", "stopped");
        let info = reg.get("test").unwrap();
        assert_eq!(info.status, "stopped");
        assert!(!info.enabled);
    }

    #[test]
    fn set_webapp_url() {
        let mut reg = PluginRegistry::new();
        reg.register_tier2(BridgePluginInfo {
            id: "freeboard-sk".to_string(),
            name: "Freeboard".to_string(),
            version: "1.0.0".to_string(),
            description: "Chart plotter".to_string(),
            has_webapp: true,
        });

        reg.set_webapp_url("freeboard-sk", "/@signalk/freeboard-sk");
        let info = reg.get("freeboard-sk").unwrap();
        assert!(info.has_webapp);
        assert_eq!(info.webapp_url.as_deref(), Some("/@signalk/freeboard-sk"));
    }
}
