/// Resource provider registry — routes resource requests to providers.
///
/// Supports multiple providers per resource type. The built-in file provider
/// is always present as a fallback.
///
/// ## Multi-provider semantics
///
/// - **List:** Results from all providers are merged (first-found-wins per ID).
/// - **Get by ID:** Providers are polled in order; the first `Ok(Some(...))` wins.
/// - **Create:** Routed to the configured default provider (or file provider).
/// - **Update / Delete by ID:** Providers are polled in order; the first non-NotFound wins.
/// - **`?provider=X`:** Any operation can target a specific provider by plugin ID.
use signalk_plugin_api::{PluginError, ResourceProvider};
use signalk_types::v2::ResourceQueryParams;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

type ProviderList = Vec<(String, Arc<dyn ResourceProvider>)>;

pub struct ResourceProviderRegistry {
    /// resource_type → ordered list of (plugin_id, provider).
    providers: RwLock<HashMap<String, ProviderList>>,
    /// resource_type → plugin_id used for POST (create) by default.
    default_create_provider: RwLock<HashMap<String, String>>,
    /// Built-in file provider (always present as fallback).
    file_provider: Arc<dyn ResourceProvider>,
}

impl ResourceProviderRegistry {
    pub fn new(file_provider: Arc<dyn ResourceProvider>) -> Self {
        ResourceProviderRegistry {
            providers: RwLock::new(HashMap::new()),
            default_create_provider: RwLock::new(HashMap::new()),
            file_provider,
        }
    }

    /// Register a plugin as a provider for a resource type.
    ///
    /// If the plugin is already registered for this type, its entry is replaced.
    pub async fn register(
        &self,
        resource_type: &str,
        plugin_id: &str,
        provider: Arc<dyn ResourceProvider>,
    ) {
        let mut providers = self.providers.write().await;
        let list = providers.entry(resource_type.to_string()).or_default();
        // Replace if already present (no duplicates per plugin_id).
        if let Some(entry) = list.iter_mut().find(|(id, _)| id == plugin_id) {
            entry.1 = provider;
        } else {
            list.push((plugin_id.to_string(), provider));
        }
    }

    /// Remove all provider registrations for a given plugin.
    pub async fn unregister(&self, plugin_id: &str) {
        let mut providers = self.providers.write().await;
        for list in providers.values_mut() {
            list.retain(|(id, _)| id != plugin_id);
        }
        // Also clean up default_create_provider entries pointing to this plugin.
        let mut defaults = self.default_create_provider.write().await;
        defaults.retain(|_, v| v != plugin_id);
    }

    /// Set the default provider for POST (create) on a resource type.
    ///
    /// The plugin_id must be registered or "file-provider".
    pub async fn set_default_provider(
        &self,
        resource_type: &str,
        plugin_id: &str,
    ) -> Result<(), PluginError> {
        if plugin_id == "file-provider" {
            self.default_create_provider
                .write()
                .await
                .remove(resource_type);
            return Ok(());
        }
        let providers = self.providers.read().await;
        let registered = providers
            .get(resource_type)
            .is_some_and(|list| list.iter().any(|(id, _)| id == plugin_id));
        if !registered {
            return Err(PluginError::not_found(format!(
                "Provider '{plugin_id}' not registered for '{resource_type}'"
            )));
        }
        self.default_create_provider
            .write()
            .await
            .insert(resource_type.to_string(), plugin_id.to_string());
        Ok(())
    }

    /// List all plugin IDs registered as providers for a resource type.
    /// The file provider is always appended as fallback.
    pub async fn list_provider_ids(&self, resource_type: &str) -> Vec<String> {
        let providers = self.providers.read().await;
        let mut ids: Vec<String> = providers
            .get(resource_type)
            .map(|list| list.iter().map(|(id, _)| id.clone()).collect())
            .unwrap_or_default();
        ids.push("file-provider".to_string());
        ids
    }

    /// Get the plugin ID of the default provider for creates.
    pub async fn get_active_provider_id(&self, resource_type: &str) -> String {
        let defaults = self.default_create_provider.read().await;
        if let Some(id) = defaults.get(resource_type) {
            return id.clone();
        }
        "file-provider".to_string()
    }

    // ── CRUD routing ────────────────────────────────────────────────────────

    /// List resources, merging results from all providers.
    pub async fn list(
        &self,
        resource_type: &str,
        query: &ResourceQueryParams,
    ) -> Result<serde_json::Value, PluginError> {
        // Single-provider shortcut
        if let Some(ref target) = query.provider {
            let provider = self.find_provider(resource_type, target).await?;
            return provider.list(resource_type, query).await;
        }

        // Merge from all providers + file provider
        let mut merged = serde_json::Map::new();

        let providers = self.providers.read().await;
        if let Some(list) = providers.get(resource_type) {
            for (pid, provider) in list {
                match provider.list(resource_type, query).await {
                    Ok(serde_json::Value::Object(map)) => {
                        for (k, v) in map {
                            // First-found wins — don't overwrite existing entries.
                            merged.entry(k).or_insert(v);
                        }
                    }
                    Ok(_) => warn!(provider = %pid, "list() returned non-object"),
                    Err(e) => warn!(provider = %pid, "list() error: {e}"),
                }
            }
        }
        drop(providers);

        // File provider as fallback
        match self.file_provider.list(resource_type, query).await {
            Ok(serde_json::Value::Object(map)) => {
                for (k, v) in map {
                    merged.entry(k).or_insert(v);
                }
            }
            Ok(_) => {}
            Err(e) => warn!("file-provider list() error: {e}"),
        }

        // Apply limit after merging (the individual providers may have their own limits,
        // but the final result needs to respect the requested limit too).
        if query.limit.is_some_and(|limit| merged.len() > limit) {
            let limit = query.limit.unwrap();
            let keys: Vec<String> = merged.keys().skip(limit).cloned().collect();
            for k in keys {
                merged.remove(&k);
            }
        }

        Ok(serde_json::Value::Object(merged))
    }

    /// Get a single resource by ID, polling all providers.
    pub async fn get(
        &self,
        resource_type: &str,
        id: &str,
        provider: Option<&str>,
    ) -> Result<Option<serde_json::Value>, PluginError> {
        if let Some(target) = provider {
            return self
                .find_provider(resource_type, target)
                .await?
                .get(resource_type, id)
                .await;
        }

        // Poll registered providers first, then file provider.
        let providers = self.providers.read().await;
        if let Some(list) = providers.get(resource_type) {
            for (_pid, p) in list {
                match p.get(resource_type, id).await {
                    Ok(Some(v)) => return Ok(Some(v)),
                    Ok(None) => continue,
                    Err(e) if e.is_not_found() => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        drop(providers);

        self.file_provider.get(resource_type, id).await
    }

    /// Create a new resource, routing to the default provider.
    pub async fn create(
        &self,
        resource_type: &str,
        value: serde_json::Value,
        provider: Option<&str>,
    ) -> Result<String, PluginError> {
        if let Some(target) = provider {
            return self
                .find_provider(resource_type, target)
                .await?
                .create(resource_type, value)
                .await;
        }

        // Use the configured default provider, or file provider.
        let defaults = self.default_create_provider.read().await;
        if let Some(default_id) = defaults.get(resource_type) {
            let default_id = default_id.clone();
            drop(defaults);
            let providers = self.providers.read().await;
            if let Some((_, p)) = providers
                .get(resource_type)
                .and_then(|list| list.iter().find(|(id, _)| id == &default_id))
            {
                return p.create(resource_type, value).await;
            }
        }

        self.file_provider.create(resource_type, value).await
    }

    /// Update a resource by ID, polling providers until one succeeds.
    pub async fn update(
        &self,
        resource_type: &str,
        id: &str,
        value: serde_json::Value,
        provider: Option<&str>,
    ) -> Result<(), PluginError> {
        if let Some(target) = provider {
            return self
                .find_provider(resource_type, target)
                .await?
                .update(resource_type, id, value)
                .await;
        }

        // Poll registered providers first.
        let providers = self.providers.read().await;
        if let Some(list) = providers.get(resource_type) {
            for (_pid, p) in list {
                match p.update(resource_type, id, value.clone()).await {
                    Ok(()) => return Ok(()),
                    Err(e) if e.is_not_found() => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        drop(providers);

        self.file_provider.update(resource_type, id, value).await
    }

    /// Delete a resource by ID, polling providers until one succeeds.
    pub async fn delete(
        &self,
        resource_type: &str,
        id: &str,
        provider: Option<&str>,
    ) -> Result<(), PluginError> {
        if let Some(target) = provider {
            return self
                .find_provider(resource_type, target)
                .await?
                .delete(resource_type, id)
                .await;
        }

        // Poll registered providers first.
        let providers = self.providers.read().await;
        if let Some(list) = providers.get(resource_type) {
            for (_pid, p) in list {
                match p.delete(resource_type, id).await {
                    Ok(()) => return Ok(()),
                    Err(e) if e.is_not_found() => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        drop(providers);

        self.file_provider.delete(resource_type, id).await
    }

    // ── Helpers ─────────────────────────────────────────────────────────────

    /// Find a specific provider by plugin ID.
    async fn find_provider(
        &self,
        resource_type: &str,
        plugin_id: &str,
    ) -> Result<Arc<dyn ResourceProvider>, PluginError> {
        if plugin_id == "file-provider" {
            return Ok(self.file_provider.clone());
        }
        let providers = self.providers.read().await;
        if let Some((_, p)) = providers
            .get(resource_type)
            .and_then(|list| list.iter().find(|(id, _)| id == plugin_id))
        {
            return Ok(p.clone());
        }
        Err(PluginError::not_found(format!(
            "Provider '{plugin_id}' not registered for '{resource_type}'"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    /// A simple in-memory provider for testing.
    struct MemProvider {
        data: tokio::sync::Mutex<HashMap<String, serde_json::Value>>,
    }

    impl MemProvider {
        fn new() -> Self {
            MemProvider {
                data: tokio::sync::Mutex::new(HashMap::new()),
            }
        }

        fn with_entries(entries: Vec<(&str, serde_json::Value)>) -> Self {
            let mut data = HashMap::new();
            for (k, v) in entries {
                data.insert(k.to_string(), v);
            }
            MemProvider {
                data: tokio::sync::Mutex::new(data),
            }
        }
    }

    #[async_trait]
    impl ResourceProvider for MemProvider {
        async fn list(
            &self,
            _resource_type: &str,
            query: &ResourceQueryParams,
        ) -> Result<serde_json::Value, PluginError> {
            let data = self.data.lock().await;
            let mut map = serde_json::Map::new();
            for (k, v) in data.iter() {
                map.insert(k.clone(), v.clone());
                if query.limit.is_some_and(|limit| map.len() >= limit) {
                    break;
                }
            }
            Ok(serde_json::Value::Object(map))
        }

        async fn get(
            &self,
            _resource_type: &str,
            id: &str,
        ) -> Result<Option<serde_json::Value>, PluginError> {
            Ok(self.data.lock().await.get(id).cloned())
        }

        async fn create(
            &self,
            _resource_type: &str,
            value: serde_json::Value,
        ) -> Result<String, PluginError> {
            let id = uuid::Uuid::new_v4().to_string();
            self.data.lock().await.insert(id.clone(), value);
            Ok(id)
        }

        async fn update(
            &self,
            _resource_type: &str,
            id: &str,
            value: serde_json::Value,
        ) -> Result<(), PluginError> {
            let mut data = self.data.lock().await;
            if data.contains_key(id) {
                data.insert(id.to_string(), value);
                Ok(())
            } else {
                Err(PluginError::not_found(format!("Resource '{id}' not found")))
            }
        }

        async fn delete(&self, _resource_type: &str, id: &str) -> Result<(), PluginError> {
            let mut data = self.data.lock().await;
            if data.remove(id).is_some() {
                Ok(())
            } else {
                Err(PluginError::not_found(format!("Resource '{id}' not found")))
            }
        }
    }

    fn default_query() -> ResourceQueryParams {
        ResourceQueryParams::default()
    }

    #[tokio::test]
    async fn list_merges_from_all_providers() {
        let file = Arc::new(MemProvider::with_entries(vec![(
            "f1",
            serde_json::json!({"name": "file-one"}),
        )]));
        let registry = ResourceProviderRegistry::new(file);

        let plugin = Arc::new(MemProvider::with_entries(vec![(
            "p1",
            serde_json::json!({"name": "plugin-one"}),
        )]));
        registry.register("waypoints", "my-plugin", plugin).await;

        let result = registry.list("waypoints", &default_query()).await.unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("f1"));
        assert!(map.contains_key("p1"));
    }

    #[tokio::test]
    async fn list_first_found_wins_on_duplicate_id() {
        let file = Arc::new(MemProvider::with_entries(vec![(
            "shared",
            serde_json::json!({"source": "file"}),
        )]));
        let registry = ResourceProviderRegistry::new(file);

        let plugin = Arc::new(MemProvider::with_entries(vec![(
            "shared",
            serde_json::json!({"source": "plugin"}),
        )]));
        registry.register("waypoints", "my-plugin", plugin).await;

        let result = registry.list("waypoints", &default_query()).await.unwrap();
        // Plugin is queried first, so its value should win.
        assert_eq!(result["shared"]["source"], "plugin");
    }

    #[tokio::test]
    async fn get_polls_providers_in_order() {
        let file = Arc::new(MemProvider::new());
        let registry = ResourceProviderRegistry::new(file);

        let plugin = Arc::new(MemProvider::with_entries(vec![(
            "p1",
            serde_json::json!({"name": "from-plugin"}),
        )]));
        registry.register("routes", "charts-plugin", plugin).await;

        // Found in plugin provider
        let result = registry.get("routes", "p1", None).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap()["name"], "from-plugin");

        // Not found anywhere
        let result = registry.get("routes", "nonexistent", None).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn create_routes_to_default_provider() {
        let file = Arc::new(MemProvider::new());
        let registry = ResourceProviderRegistry::new(file.clone());

        let plugin: Arc<MemProvider> = Arc::new(MemProvider::new());
        registry
            .register("waypoints", "my-plugin", plugin.clone())
            .await;
        registry
            .set_default_provider("waypoints", "my-plugin")
            .await
            .unwrap();

        let id = registry
            .create("waypoints", serde_json::json!({"name": "test"}), None)
            .await
            .unwrap();

        // Should be in the plugin, not the file provider.
        assert!(plugin.data.lock().await.contains_key(&id));
        assert!(file.data.lock().await.is_empty());
    }

    #[tokio::test]
    async fn create_falls_back_to_file_provider() {
        let file: Arc<MemProvider> = Arc::new(MemProvider::new());
        let registry = ResourceProviderRegistry::new(file.clone());

        let id = registry
            .create("waypoints", serde_json::json!({"name": "test"}), None)
            .await
            .unwrap();

        assert!(file.data.lock().await.contains_key(&id));
    }

    #[tokio::test]
    async fn provider_query_targets_specific_provider() {
        let file = Arc::new(MemProvider::with_entries(vec![(
            "f1",
            serde_json::json!({"name": "file-one"}),
        )]));
        let registry = ResourceProviderRegistry::new(file);

        let plugin = Arc::new(MemProvider::with_entries(vec![(
            "p1",
            serde_json::json!({"name": "plugin-one"}),
        )]));
        registry.register("waypoints", "my-plugin", plugin).await;

        // Target only file provider
        let query = ResourceQueryParams {
            provider: Some("file-provider".to_string()),
            ..Default::default()
        };
        let result = registry.list("waypoints", &query).await.unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("f1"));

        // Target only plugin
        let query = ResourceQueryParams {
            provider: Some("my-plugin".to_string()),
            ..Default::default()
        };
        let result = registry.list("waypoints", &query).await.unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("p1"));
    }

    #[tokio::test]
    async fn provider_query_unknown_returns_not_found() {
        let file = Arc::new(MemProvider::new());
        let registry = ResourceProviderRegistry::new(file);

        let query = ResourceQueryParams {
            provider: Some("nonexistent".to_string()),
            ..Default::default()
        };
        let result = registry.list("waypoints", &query).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn set_default_provider_validates() {
        let file = Arc::new(MemProvider::new());
        let registry = ResourceProviderRegistry::new(file);

        // Unknown plugin → error
        let result = registry
            .set_default_provider("waypoints", "nonexistent")
            .await;
        assert!(result.is_err());

        // "file-provider" always works
        let result = registry
            .set_default_provider("waypoints", "file-provider")
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn list_provider_ids_includes_file_provider() {
        let file = Arc::new(MemProvider::new());
        let registry = ResourceProviderRegistry::new(file);

        let plugin = Arc::new(MemProvider::new());
        registry.register("routes", "my-plugin", plugin).await;

        let ids = registry.list_provider_ids("routes").await;
        assert_eq!(ids, vec!["my-plugin", "file-provider"]);

        // Type with no registrations → just file-provider
        let ids = registry.list_provider_ids("waypoints").await;
        assert_eq!(ids, vec!["file-provider"]);
    }

    #[tokio::test]
    async fn get_active_provider_id_returns_default() {
        let file = Arc::new(MemProvider::new());
        let registry = ResourceProviderRegistry::new(file);

        assert_eq!(
            registry.get_active_provider_id("waypoints").await,
            "file-provider"
        );

        let plugin = Arc::new(MemProvider::new());
        registry.register("waypoints", "my-plugin", plugin).await;
        registry
            .set_default_provider("waypoints", "my-plugin")
            .await
            .unwrap();

        assert_eq!(
            registry.get_active_provider_id("waypoints").await,
            "my-plugin"
        );
    }

    #[tokio::test]
    async fn unregister_removes_all_entries() {
        let file = Arc::new(MemProvider::new());
        let registry = ResourceProviderRegistry::new(file);

        let plugin = Arc::new(MemProvider::new());
        registry
            .register("routes", "my-plugin", plugin.clone())
            .await;
        registry.register("waypoints", "my-plugin", plugin).await;
        registry
            .set_default_provider("routes", "my-plugin")
            .await
            .unwrap();

        registry.unregister("my-plugin").await;

        let ids = registry.list_provider_ids("routes").await;
        assert_eq!(ids, vec!["file-provider"]);
        let ids = registry.list_provider_ids("waypoints").await;
        assert_eq!(ids, vec!["file-provider"]);
        assert_eq!(
            registry.get_active_provider_id("routes").await,
            "file-provider"
        );
    }

    #[tokio::test]
    async fn update_polls_providers() {
        let file = Arc::new(MemProvider::with_entries(vec![(
            "f1",
            serde_json::json!({"name": "old"}),
        )]));
        let registry = ResourceProviderRegistry::new(file.clone());

        let plugin = Arc::new(MemProvider::with_entries(vec![(
            "p1",
            serde_json::json!({"name": "old"}),
        )]));
        registry
            .register("waypoints", "my-plugin", plugin.clone())
            .await;

        // Update plugin-owned resource
        registry
            .update("waypoints", "p1", serde_json::json!({"name": "new"}), None)
            .await
            .unwrap();
        assert_eq!(plugin.data.lock().await["p1"]["name"], "new");

        // Update file-owned resource (plugin returns NotFound, falls through)
        registry
            .update("waypoints", "f1", serde_json::json!({"name": "new"}), None)
            .await
            .unwrap();
        assert_eq!(file.data.lock().await["f1"]["name"], "new");

        // Update nonexistent
        let result = registry
            .update("waypoints", "nope", serde_json::json!({"name": "x"}), None)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_polls_providers() {
        let file = Arc::new(MemProvider::with_entries(vec![(
            "f1",
            serde_json::json!({"name": "file"}),
        )]));
        let registry = ResourceProviderRegistry::new(file);

        let plugin = Arc::new(MemProvider::with_entries(vec![(
            "p1",
            serde_json::json!({"name": "plugin"}),
        )]));
        registry
            .register("waypoints", "my-plugin", plugin.clone())
            .await;

        // Delete plugin-owned
        registry.delete("waypoints", "p1", None).await.unwrap();
        assert!(plugin.data.lock().await.is_empty());

        // Delete file-owned
        registry.delete("waypoints", "f1", None).await.unwrap();

        // Delete nonexistent
        let result = registry.delete("waypoints", "nope", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn register_deduplicates() {
        let file = Arc::new(MemProvider::new());
        let registry = ResourceProviderRegistry::new(file);

        let p1 = Arc::new(MemProvider::new());
        let p2 = Arc::new(MemProvider::new());
        registry.register("routes", "my-plugin", p1).await;
        registry.register("routes", "my-plugin", p2).await;

        let ids = registry.list_provider_ids("routes").await;
        assert_eq!(ids, vec!["my-plugin", "file-provider"]);
    }

    #[tokio::test]
    async fn list_with_limit_after_merge() {
        let file = Arc::new(MemProvider::with_entries(vec![
            ("f1", serde_json::json!(1)),
            ("f2", serde_json::json!(2)),
        ]));
        let registry = ResourceProviderRegistry::new(file);

        let plugin = Arc::new(MemProvider::with_entries(vec![(
            "p1",
            serde_json::json!(3),
        )]));
        registry.register("waypoints", "my-plugin", plugin).await;

        let query = ResourceQueryParams {
            limit: Some(2),
            ..Default::default()
        };
        let result = registry.list("waypoints", &query).await.unwrap();
        assert_eq!(result.as_object().unwrap().len(), 2);
    }

    // ── Multi-provider (3+ providers) ────────────────────────────────────────

    /// Helper: registry with file + 2 plugin providers for "waypoints".
    async fn registry_with_three_providers() -> (
        ResourceProviderRegistry,
        Arc<MemProvider>,
        Arc<MemProvider>,
        Arc<MemProvider>,
    ) {
        let file = Arc::new(MemProvider::with_entries(vec![(
            "f1",
            serde_json::json!({"src": "file"}),
        )]));
        let registry = ResourceProviderRegistry::new(file.clone());

        let alpha = Arc::new(MemProvider::with_entries(vec![(
            "a1",
            serde_json::json!({"src": "alpha"}),
        )]));
        registry
            .register("waypoints", "alpha-plugin", alpha.clone())
            .await;

        let beta = Arc::new(MemProvider::with_entries(vec![(
            "b1",
            serde_json::json!({"src": "beta"}),
        )]));
        registry
            .register("waypoints", "beta-plugin", beta.clone())
            .await;

        (registry, file, alpha, beta)
    }

    #[tokio::test]
    async fn three_providers_list_merges_all() {
        let (registry, _, _, _) = registry_with_three_providers().await;
        let result = registry.list("waypoints", &default_query()).await.unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.len(), 3);
        assert!(map.contains_key("f1"));
        assert!(map.contains_key("a1"));
        assert!(map.contains_key("b1"));
    }

    #[tokio::test]
    async fn three_providers_get_polls_all() {
        let (registry, _, _, _) = registry_with_three_providers().await;

        // Each provider's item is findable
        assert_eq!(
            registry
                .get("waypoints", "a1", None)
                .await
                .unwrap()
                .unwrap()["src"],
            "alpha"
        );
        assert_eq!(
            registry
                .get("waypoints", "b1", None)
                .await
                .unwrap()
                .unwrap()["src"],
            "beta"
        );
        assert_eq!(
            registry
                .get("waypoints", "f1", None)
                .await
                .unwrap()
                .unwrap()["src"],
            "file"
        );
        assert!(
            registry
                .get("waypoints", "nope", None)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn three_providers_update_routes_to_owner() {
        let (registry, file, alpha, beta) = registry_with_three_providers().await;

        registry
            .update(
                "waypoints",
                "a1",
                serde_json::json!({"src": "alpha-v2"}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(alpha.data.lock().await["a1"]["src"], "alpha-v2");

        registry
            .update(
                "waypoints",
                "b1",
                serde_json::json!({"src": "beta-v2"}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(beta.data.lock().await["b1"]["src"], "beta-v2");

        registry
            .update(
                "waypoints",
                "f1",
                serde_json::json!({"src": "file-v2"}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(file.data.lock().await["f1"]["src"], "file-v2");
    }

    #[tokio::test]
    async fn three_providers_delete_routes_to_owner() {
        let (registry, _, alpha, beta) = registry_with_three_providers().await;

        registry.delete("waypoints", "b1", None).await.unwrap();
        assert!(beta.data.lock().await.is_empty());
        // alpha still has its entry
        assert!(alpha.data.lock().await.contains_key("a1"));

        registry.delete("waypoints", "a1", None).await.unwrap();
        assert!(alpha.data.lock().await.is_empty());
    }

    #[tokio::test]
    async fn three_providers_list_duplicate_id_first_registered_wins() {
        let file = Arc::new(MemProvider::with_entries(vec![(
            "dup",
            serde_json::json!({"src": "file"}),
        )]));
        let registry = ResourceProviderRegistry::new(file);

        let alpha = Arc::new(MemProvider::with_entries(vec![(
            "dup",
            serde_json::json!({"src": "alpha"}),
        )]));
        registry.register("waypoints", "alpha-plugin", alpha).await;

        let beta = Arc::new(MemProvider::with_entries(vec![(
            "dup",
            serde_json::json!({"src": "beta"}),
        )]));
        registry.register("waypoints", "beta-plugin", beta).await;

        let result = registry.list("waypoints", &default_query()).await.unwrap();
        // alpha was registered first → wins
        assert_eq!(result["dup"]["src"], "alpha");
    }

    // ── ?provider= targeting for get/update/delete ───────────────────────────

    #[tokio::test]
    async fn provider_targeting_get_by_id() {
        let (registry, _, _, _) = registry_with_three_providers().await;

        // Target alpha → finds a1 but not b1
        let result = registry
            .get("waypoints", "a1", Some("alpha-plugin"))
            .await
            .unwrap();
        assert!(result.is_some());

        let result = registry
            .get("waypoints", "b1", Some("alpha-plugin"))
            .await
            .unwrap();
        assert!(result.is_none());

        // Target file-provider
        let result = registry
            .get("waypoints", "f1", Some("file-provider"))
            .await
            .unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn provider_targeting_update() {
        let (registry, file, alpha, _) = registry_with_three_providers().await;

        // Target alpha — update a1
        registry
            .update(
                "waypoints",
                "a1",
                serde_json::json!({"src": "targeted"}),
                Some("alpha-plugin"),
            )
            .await
            .unwrap();
        assert_eq!(alpha.data.lock().await["a1"]["src"], "targeted");

        // Target alpha — a1 not in file → NotFound
        let result = registry
            .update(
                "waypoints",
                "a1",
                serde_json::json!({"src": "x"}),
                Some("file-provider"),
            )
            .await;
        assert!(result.is_err());

        // File-provider's own item unmodified
        assert_eq!(file.data.lock().await["f1"]["src"], "file");
    }

    #[tokio::test]
    async fn provider_targeting_delete() {
        let (registry, _, alpha, beta) = registry_with_three_providers().await;

        // Target beta — delete b1
        registry
            .delete("waypoints", "b1", Some("beta-plugin"))
            .await
            .unwrap();
        assert!(beta.data.lock().await.is_empty());
        // alpha unaffected
        assert!(alpha.data.lock().await.contains_key("a1"));

        // Target beta — a1 not there → error
        let result = registry
            .delete("waypoints", "a1", Some("beta-plugin"))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn provider_targeting_create() {
        let file = Arc::new(MemProvider::new());
        let registry = ResourceProviderRegistry::new(file.clone());

        let plugin = Arc::new(MemProvider::new());
        registry
            .register("waypoints", "my-plugin", plugin.clone())
            .await;

        // Create targeting the plugin (bypasses default/file-provider)
        let id = registry
            .create(
                "waypoints",
                serde_json::json!({"name": "targeted"}),
                Some("my-plugin"),
            )
            .await
            .unwrap();
        assert!(plugin.data.lock().await.contains_key(&id));
        assert!(file.data.lock().await.is_empty());

        // Create targeting file-provider
        let id2 = registry
            .create(
                "waypoints",
                serde_json::json!({"name": "in-file"}),
                Some("file-provider"),
            )
            .await
            .unwrap();
        assert!(file.data.lock().await.contains_key(&id2));
    }

    // ── Unregister with multiple plugins ─────────────────────────────────────

    #[tokio::test]
    async fn unregister_one_of_multiple_plugins() {
        let (registry, _, _, _) = registry_with_three_providers().await;

        registry.unregister("alpha-plugin").await;

        let ids = registry.list_provider_ids("waypoints").await;
        assert_eq!(ids, vec!["beta-plugin", "file-provider"]);

        // beta's items still accessible
        let result = registry.get("waypoints", "b1", None).await.unwrap();
        assert!(result.is_some());

        // alpha's items gone
        let result = registry.get("waypoints", "a1", None).await.unwrap();
        assert!(result.is_none());
    }
}
