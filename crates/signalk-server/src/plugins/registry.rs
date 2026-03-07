/// PluginRegistry — tier-agnostic registry of all plugins for the admin API.
///
/// Aggregates plugin info from all tiers:
/// - **Tier 1 (Rust):** Populated from `PluginManager::statuses()`
/// - **Tier 2 (Bridge):** Populated via `POST /internal/v1/bridge/plugins`
/// - **Tier 3 (Standalone):** Populated via registration endpoint (future)
///
/// The admin API reads from this registry to present a unified view.
use chrono::{DateTime, Utc};
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
    /// Timestamp of the last error status (ISO 8601), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_time: Option<DateTime<Utc>>,
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

/// Lightweight provider info for the admin UI provider list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub id: String,
    /// Type label: "signalk-rs", "node-bridge", or "standalone"
    pub type_label: String,
    pub enabled: bool,
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
        let last_error_time = if status.starts_with("error") {
            Some(Utc::now())
        } else {
            // Preserve existing error timestamp when transitioning away from error
            self.plugins
                .get(id)
                .and_then(|p| p.last_error_time)
        };
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
                last_error_time,
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
                last_error_time: None,
            },
        );
    }

    /// Update the status of a plugin by ID.
    pub fn update_status(&mut self, id: &str, status: &str) {
        if let Some(info) = self.plugins.get_mut(id) {
            if status.starts_with("error") {
                info.last_error_time = Some(Utc::now());
            }
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

    /// Provider info for the admin UI — plugin ID, type label, and enabled flag.
    pub fn providers(&self) -> Vec<ProviderInfo> {
        let mut providers: Vec<ProviderInfo> = self
            .plugins
            .values()
            .map(|p| ProviderInfo {
                id: p.id.clone(),
                type_label: match p.tier {
                    PluginTier::Rust => "signalk-rs".to_string(),
                    PluginTier::Bridge => "node-bridge".to_string(),
                    PluginTier::Standalone => "standalone".to_string(),
                },
                enabled: p.enabled,
            })
            .collect();
        providers.sort_by(|a, b| a.id.cmp(&b.id));
        providers
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

    #[test]
    fn last_error_time_tracked() {
        let mut reg = PluginRegistry::new();
        reg.register_tier1("test", "Test", "desc", "0.1.0", "running", true);
        assert!(reg.get("test").unwrap().last_error_time.is_none());

        reg.update_status("test", "error: connection failed");
        assert!(reg.get("test").unwrap().last_error_time.is_some());

        // When transitioning back to running, timestamp is preserved
        let error_time = reg.get("test").unwrap().last_error_time;
        reg.update_status("test", "running: OK");
        assert_eq!(reg.get("test").unwrap().last_error_time, error_time);
    }

    #[test]
    fn providers_returns_sorted_list() {
        let mut reg = PluginRegistry::new();
        reg.register_tier1("zzz-plugin", "ZZZ", "desc", "0.1.0", "running", true);
        reg.register_tier2(BridgePluginInfo {
            id: "aaa-plugin".to_string(),
            name: "AAA".to_string(),
            version: "1.0.0".to_string(),
            description: "desc".to_string(),
            has_webapp: false,
        });

        let providers = reg.providers();
        assert_eq!(providers.len(), 2);
        // Sorted by ID
        assert_eq!(providers[0].id, "aaa-plugin");
        assert_eq!(providers[0].type_label, "node-bridge");
        assert!(providers[0].enabled);
        assert_eq!(providers[1].id, "zzz-plugin");
        assert_eq!(providers[1].type_label, "signalk-rs");
    }
}
