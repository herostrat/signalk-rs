/// PluginManager — orchestrates the lifecycle of all Tier 1 (Rust) plugins.
///
/// Responsibilities:
/// - Register plugin instances
/// - Start all plugins with their configuration and context
/// - Stop plugins individually or all at once
/// - Track plugin status
/// - Provide access to shared resources (route table, PUT handler registry)
use signalk_plugin_api::{Plugin, PluginError, PluginMetadata, PluginStatus};
use signalk_store::store::SignalKStore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use super::delta_filter::DeltaFilterChain;
use super::host::{PutHandlerRegistry, RustPluginContext, cleanup_plugin};
use super::isolation::guarded;
use super::routes::PluginRouteTable;

/// A registered plugin with its runtime state.
struct PluginEntry {
    plugin: Box<dyn Plugin>,
    status: PluginStatus,
    /// The context is created on start, dropped on stop.
    context: Option<Arc<RustPluginContext>>,
}

/// Manages all Tier 1 (in-process Rust) plugins.
pub struct PluginManager {
    plugins: HashMap<String, PluginEntry>,
    store: Arc<RwLock<SignalKStore>>,
    route_table: Arc<PluginRouteTable>,
    put_handler_registry: Arc<PutHandlerRegistry>,
    /// Shared PUT handler map (also used by bridge via InternalState)
    put_handlers: Arc<RwLock<HashMap<String, String>>>,
    /// Shared plugin routes map (also used by bridge via InternalState)
    plugin_routes: Arc<RwLock<HashMap<String, String>>>,
    /// Shared delta input filter chain (pre-store)
    delta_filter: Arc<DeltaFilterChain>,
    config_dir: PathBuf,
    data_dir: PathBuf,
}

impl PluginManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<RwLock<SignalKStore>>,
        route_table: Arc<PluginRouteTable>,
        put_handler_registry: Arc<PutHandlerRegistry>,
        put_handlers: Arc<RwLock<HashMap<String, String>>>,
        plugin_routes: Arc<RwLock<HashMap<String, String>>>,
        delta_filter: Arc<DeltaFilterChain>,
        config_dir: PathBuf,
        data_dir: PathBuf,
    ) -> Self {
        PluginManager {
            plugins: HashMap::new(),
            store,
            route_table,
            put_handler_registry,
            put_handlers,
            plugin_routes,
            delta_filter,
            config_dir,
            data_dir,
        }
    }

    /// Register a plugin instance. Must be called before `start_all`.
    pub fn register(&mut self, plugin: Box<dyn Plugin>) {
        let meta = plugin.metadata();
        info!(plugin = %meta.id, name = %meta.name, version = %meta.version, "Plugin registered");
        self.plugins.insert(
            meta.id.clone(),
            PluginEntry {
                plugin,
                status: PluginStatus::Stopped,
                context: None,
            },
        );
    }

    /// Start all registered plugins with their configurations.
    ///
    /// `configs` maps plugin_id → config JSON. Plugins not in the map
    /// receive an empty object `{}` as config.
    pub async fn start_all(&mut self, configs: &HashMap<String, serde_json::Value>) {
        let plugin_ids: Vec<String> = self.plugins.keys().cloned().collect();

        for plugin_id in plugin_ids {
            let config = configs
                .get(&plugin_id)
                .cloned()
                .unwrap_or(serde_json::json!({}));

            if let Err(e) = self.start_plugin(&plugin_id, config).await {
                error!(plugin = %plugin_id, error = %e, "Failed to start plugin");
            }
        }
    }

    /// Start a single plugin.
    pub async fn start_plugin(
        &mut self,
        plugin_id: &str,
        config: serde_json::Value,
    ) -> Result<(), PluginError> {
        let entry = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| PluginError::runtime(format!("Unknown plugin: {plugin_id}")))?;

        if matches!(entry.status, PluginStatus::Running(_)) {
            warn!(plugin = %plugin_id, "Plugin already running, skipping start");
            return Ok(());
        }

        entry.status = PluginStatus::Starting;

        let plugin_data_dir = self.data_dir.join(plugin_id);
        let ctx = Arc::new(RustPluginContext::new(
            plugin_id.to_string(),
            self.store.clone(),
            self.route_table.clone(),
            self.put_handler_registry.clone(),
            self.put_handlers.clone(),
            self.plugin_routes.clone(),
            self.config_dir.clone(),
            plugin_data_dir,
            self.delta_filter.clone(),
        ));

        entry.context = Some(ctx.clone());

        // Start with panic isolation
        let mut plugin = std::mem::replace(
            &mut entry.plugin,
            Box::new(PlaceholderPlugin(plugin_id.to_string())),
        );

        let ctx_for_start = ctx.clone();
        let pid = plugin_id.to_string();

        match guarded(&pid, async move {
            plugin.start(config, ctx_for_start).await?;
            Ok(plugin)
        })
        .await
        {
            Ok(plugin) => {
                let entry = self.plugins.get_mut(plugin_id).unwrap();
                entry.plugin = plugin;
                entry.status = PluginStatus::Running("Started".to_string());
                info!(plugin = %plugin_id, "Plugin started");
                Ok(())
            }
            Err(e) => {
                let entry = self.plugins.get_mut(plugin_id).unwrap();
                entry.status = PluginStatus::Error(e.to_string());
                entry.context = None;
                Err(e)
            }
        }
    }

    /// Stop all running plugins.
    pub async fn stop_all(&mut self) {
        let plugin_ids: Vec<String> = self.plugins.keys().cloned().collect();
        for plugin_id in plugin_ids {
            if let Err(e) = self.stop_plugin(&plugin_id).await {
                error!(plugin = %plugin_id, error = %e, "Failed to stop plugin");
            }
        }
    }

    /// Stop a single plugin and clean up its resources.
    pub async fn stop_plugin(&mut self, plugin_id: &str) -> Result<(), PluginError> {
        let entry = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| PluginError::runtime(format!("Unknown plugin: {plugin_id}")))?;

        if matches!(entry.status, PluginStatus::Stopped) {
            return Ok(());
        }

        entry.status = PluginStatus::Stopping;

        // Stop the plugin
        if let Err(e) = entry.plugin.stop().await {
            warn!(plugin = %plugin_id, error = %e, "Plugin stop returned error");
        }

        // Clean up subscriptions and handlers
        if let Some(ctx) = entry.context.take() {
            cleanup_plugin(&ctx);
        }
        self.route_table.remove(plugin_id).await;
        self.put_handler_registry.remove_plugin(plugin_id).await;
        self.delta_filter.remove_plugin(plugin_id);

        // Remove from shared maps
        self.put_handlers
            .write()
            .await
            .retain(|_, pid| pid != plugin_id);
        self.plugin_routes.write().await.remove(plugin_id);

        entry.status = PluginStatus::Stopped;
        info!(plugin = %plugin_id, "Plugin stopped");
        Ok(())
    }

    /// Get the status of all registered plugins.
    pub fn statuses(&self) -> Vec<(PluginMetadata, PluginStatus)> {
        self.plugins
            .values()
            .map(|entry| (entry.plugin.metadata(), entry.status.clone()))
            .collect()
    }

    /// Get the route table (shared with the server for request dispatch).
    pub fn route_table(&self) -> &Arc<PluginRouteTable> {
        &self.route_table
    }

    /// Get the PUT handler registry (shared with the server for PUT dispatch).
    pub fn put_handler_registry(&self) -> &Arc<PutHandlerRegistry> {
        &self.put_handler_registry
    }

    /// Get the delta filter chain (shared with the server for delta pre-filtering).
    pub fn delta_filter(&self) -> &Arc<DeltaFilterChain> {
        &self.delta_filter
    }
}

/// Placeholder plugin used while the real plugin is running inside `guarded()`.
struct PlaceholderPlugin(String);

#[async_trait::async_trait]
impl Plugin for PlaceholderPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(&self.0, "placeholder", "", "0.0.0")
    }

    async fn start(
        &mut self,
        _config: serde_json::Value,
        _ctx: Arc<dyn signalk_plugin_api::PluginContext>,
    ) -> Result<(), PluginError> {
        Err(PluginError::runtime("placeholder plugin cannot be started"))
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signalk_store::store::SignalKStore;

    /// A minimal test plugin that sets its status on start.
    struct TestPlugin {
        id: String,
    }

    impl TestPlugin {
        fn new(id: &str) -> Self {
            TestPlugin { id: id.to_string() }
        }
    }

    #[async_trait::async_trait]
    impl Plugin for TestPlugin {
        fn metadata(&self) -> PluginMetadata {
            PluginMetadata::new(&self.id, "Test Plugin", "A test plugin", "0.1.0")
        }

        async fn start(
            &mut self,
            _config: serde_json::Value,
            ctx: Arc<dyn signalk_plugin_api::PluginContext>,
        ) -> Result<(), PluginError> {
            ctx.set_status("Running");
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), PluginError> {
            Ok(())
        }
    }

    fn make_manager() -> PluginManager {
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        PluginManager::new(
            store,
            Arc::new(PluginRouteTable::new()),
            Arc::new(PutHandlerRegistry::new()),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(DeltaFilterChain::new()),
            PathBuf::from("/tmp/signalk-test/config"),
            PathBuf::from("/tmp/signalk-test/data"),
        )
    }

    #[tokio::test]
    async fn register_and_start_plugin() {
        let mut mgr = make_manager();
        mgr.register(Box::new(TestPlugin::new("test-plugin")));

        let configs = HashMap::new();
        mgr.start_all(&configs).await;

        let statuses = mgr.statuses();
        assert_eq!(statuses.len(), 1);
        assert!(matches!(statuses[0].1, PluginStatus::Running(_)));
    }

    #[tokio::test]
    async fn start_and_stop_plugin() {
        let mut mgr = make_manager();
        mgr.register(Box::new(TestPlugin::new("test-plugin")));

        mgr.start_plugin("test-plugin", serde_json::json!({}))
            .await
            .unwrap();
        assert!(matches!(mgr.statuses()[0].1, PluginStatus::Running(_)));

        mgr.stop_plugin("test-plugin").await.unwrap();
        assert!(matches!(mgr.statuses()[0].1, PluginStatus::Stopped));
    }

    #[tokio::test]
    async fn panicking_plugin_doesnt_crash_manager() {
        struct PanicPlugin;

        #[async_trait::async_trait]
        impl Plugin for PanicPlugin {
            fn metadata(&self) -> PluginMetadata {
                PluginMetadata::new("panic-plugin", "Panic", "Crashes on start", "0.0.1")
            }

            async fn start(
                &mut self,
                _config: serde_json::Value,
                _ctx: Arc<dyn signalk_plugin_api::PluginContext>,
            ) -> Result<(), PluginError> {
                panic!("intentional crash");
            }

            async fn stop(&mut self) -> Result<(), PluginError> {
                Ok(())
            }
        }

        let mut mgr = make_manager();
        mgr.register(Box::new(PanicPlugin));

        let result = mgr
            .start_plugin("panic-plugin", serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert!(matches!(mgr.statuses()[0].1, PluginStatus::Error(_)));
    }
}
