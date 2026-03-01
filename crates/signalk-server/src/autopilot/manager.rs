/// AutopilotManager — central registry for autopilot provider plugins.
///
/// The server's V2 autopilot API (`/signalk/v2/api/vessels/self/autopilots/`)
/// delegates all commands to providers registered here.
///
/// Multiple providers can be registered (e.g. software autopilot + hardware
/// driver). The "default" pointer tracks which one the API uses when the
/// request targets `_default` rather than a specific device ID.
use signalk_plugin_api::{AutopilotProvider, PluginError};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct AutopilotManager {
    providers: RwLock<HashMap<String, Arc<dyn AutopilotProvider>>>,
    default_id: RwLock<Option<String>>,
}

impl AutopilotManager {
    pub fn new() -> Arc<Self> {
        Arc::new(AutopilotManager {
            providers: RwLock::new(HashMap::new()),
            default_id: RwLock::new(None),
        })
    }

    /// Register a provider. The first registration automatically becomes the default.
    pub async fn register(&self, provider: Arc<dyn AutopilotProvider>) {
        let id = provider.device_id().to_string();
        let mut map = self.providers.write().await;
        let mut def = self.default_id.write().await;
        if map.is_empty() {
            *def = Some(id.clone());
        }
        map.insert(id, provider);
    }

    /// Look up a provider by device ID.
    pub async fn get(&self, id: &str) -> Option<Arc<dyn AutopilotProvider>> {
        self.providers.read().await.get(id).cloned()
    }

    /// Return the default provider, if any.
    pub async fn get_default(&self) -> Option<Arc<dyn AutopilotProvider>> {
        let id = self.default_id.read().await.clone()?;
        self.get(&id).await
    }

    /// Return the ID of the current default provider.
    pub async fn default_id(&self) -> Option<String> {
        self.default_id.read().await.clone()
    }

    /// List all registered providers as `(device_id, is_default)` pairs.
    pub async fn list(&self) -> Vec<(String, bool)> {
        let map = self.providers.read().await;
        let def = self.default_id.read().await.clone();
        map.keys()
            .map(|id| (id.clone(), def.as_deref() == Some(id)))
            .collect()
    }

    /// Set the default provider by device ID.
    ///
    /// Returns `Err(NotFound)` if the ID is not registered.
    pub async fn set_default(&self, id: &str) -> Result<(), PluginError> {
        if self.providers.read().await.contains_key(id) {
            *self.default_id.write().await = Some(id.to_string());
            Ok(())
        } else {
            Err(PluginError::not_found(format!(
                "Autopilot device not found: {id}"
            )))
        }
    }

    /// Resolve `"_default"` to the actual device ID, or return `id` unchanged.
    pub async fn resolve_id(&self, id: &str) -> Option<String> {
        if id == "_default" {
            self.default_id.read().await.clone()
        } else if self.providers.read().await.contains_key(id) {
            Some(id.to_string())
        } else {
            None
        }
    }
}

impl Default for AutopilotManager {
    fn default() -> Self {
        AutopilotManager {
            providers: RwLock::new(HashMap::new()),
            default_id: RwLock::new(None),
        }
    }
}
