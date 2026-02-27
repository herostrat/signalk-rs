/// Resource provider registry — routes resource requests to the appropriate provider.
///
/// Each resource type can have a plugin-provided override. If none is registered,
/// the default file-based provider is used.
use signalk_plugin_api::{PluginError, ResourceProvider};
use signalk_types::v2::ResourceQueryParams;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ResourceProviderRegistry {
    overrides: RwLock<HashMap<String, Arc<dyn ResourceProvider>>>,
    default: Arc<dyn ResourceProvider>,
}

impl ResourceProviderRegistry {
    pub fn new(default: Arc<dyn ResourceProvider>) -> Self {
        ResourceProviderRegistry {
            overrides: RwLock::new(HashMap::new()),
            default,
        }
    }

    /// Register a plugin-provided override for a resource type.
    pub async fn register(&self, resource_type: &str, provider: Arc<dyn ResourceProvider>) {
        self.overrides
            .write()
            .await
            .insert(resource_type.to_string(), provider);
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
