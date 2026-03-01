/// Resource provider registry — routes resource requests to the appropriate provider.
///
/// Each resource type can have a plugin-provided override. If none is registered,
/// the default file-based provider is used.
///
/// Plugin IDs are tracked for the `_providers` API endpoints.
use signalk_plugin_api::{PluginError, ResourceProvider};
use signalk_types::v2::ResourceQueryParams;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ResourceProviderRegistry {
    overrides: RwLock<HashMap<String, Arc<dyn ResourceProvider>>>,
    /// Plugin IDs that have registered as provider for each resource type.
    provider_ids: RwLock<HashMap<String, Vec<String>>>,
    default: Arc<dyn ResourceProvider>,
}

impl ResourceProviderRegistry {
    pub fn new(default: Arc<dyn ResourceProvider>) -> Self {
        ResourceProviderRegistry {
            overrides: RwLock::new(HashMap::new()),
            provider_ids: RwLock::new(HashMap::new()),
            default,
        }
    }

    /// Register a plugin-provided override for a resource type.
    pub async fn register(
        &self,
        resource_type: &str,
        plugin_id: &str,
        provider: Arc<dyn ResourceProvider>,
    ) {
        self.overrides
            .write()
            .await
            .insert(resource_type.to_string(), provider);
        let mut ids = self.provider_ids.write().await;
        let list = ids.entry(resource_type.to_string()).or_default();
        if !list.contains(&plugin_id.to_string()) {
            list.push(plugin_id.to_string());
        }
    }

    /// List all plugin IDs registered as providers for a resource type.
    /// The default file provider is always appended as the fallback.
    pub async fn list_provider_ids(&self, resource_type: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .provider_ids
            .read()
            .await
            .get(resource_type)
            .cloned()
            .unwrap_or_default();
        ids.push("file-provider".to_string());
        ids
    }

    /// Get the plugin ID of the active (highest-priority) provider for a resource type.
    /// Returns the last-registered plugin override, or "file-provider" if none.
    pub async fn get_active_provider_id(&self, resource_type: &str) -> String {
        let ids = self.provider_ids.read().await;
        if let Some(last) = ids.get(resource_type).and_then(|l| l.last()) {
            return last.clone();
        }
        "file-provider".to_string()
    }

    /// Get the provider for a resource type (override or default).
    async fn provider_for(&self, resource_type: &str) -> Arc<dyn ResourceProvider> {
        let overrides = self.overrides.read().await;
        overrides
            .get(resource_type)
            .cloned()
            .unwrap_or_else(|| self.default.clone())
    }

    pub async fn list(
        &self,
        resource_type: &str,
        query: &ResourceQueryParams,
    ) -> Result<serde_json::Value, PluginError> {
        self.provider_for(resource_type)
            .await
            .list(resource_type, query)
            .await
    }

    pub async fn get(
        &self,
        resource_type: &str,
        id: &str,
    ) -> Result<Option<serde_json::Value>, PluginError> {
        self.provider_for(resource_type)
            .await
            .get(resource_type, id)
            .await
    }

    pub async fn create(
        &self,
        resource_type: &str,
        value: serde_json::Value,
    ) -> Result<String, PluginError> {
        self.provider_for(resource_type)
            .await
            .create(resource_type, value)
            .await
    }

    pub async fn update(
        &self,
        resource_type: &str,
        id: &str,
        value: serde_json::Value,
    ) -> Result<(), PluginError> {
        self.provider_for(resource_type)
            .await
            .update(resource_type, id, value)
            .await
    }

    pub async fn delete(&self, resource_type: &str, id: &str) -> Result<(), PluginError> {
        self.provider_for(resource_type)
            .await
            .delete(resource_type, id)
            .await
    }
}
