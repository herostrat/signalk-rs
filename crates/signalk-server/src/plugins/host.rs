/// RustPluginContext — the Tier 1 implementation of `PluginContext`.
///
/// Provides direct, in-process access to the SignalK store with zero IPC overhead.
/// Each Rust plugin receives its own `Arc<RustPluginContext>` on start.
use async_trait::async_trait;
use signalk_plugin_api::{
    AutopilotProvider, DeltaCallback, DeltaInputHandler, PluginContext, PluginError, PutCommand,
    PutHandler, PutHandlerResult, RouteCollector, RouterSetup, SubscriptionHandle,
    SubscriptionSpec,
};
use signalk_store::store::SignalKStore;
use signalk_types::Delta;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use tracing::warn;

use super::delta_filter::DeltaFilterChain;
use super::routes::PluginRouteTable;
use crate::autopilot::AutopilotManager;
use crate::webapps::{WebAppInfo, WebappRegistry, WebappSource};

/// Shared PUT handler function — Arc-wrapped for cloning across threads.
pub type SharedPutHandler = Arc<
    dyn Fn(
            PutCommand,
        ) -> Pin<Box<dyn Future<Output = Result<PutHandlerResult, PluginError>> + Send>>
        + Send
        + Sync
        + 'static,
>;

/// Registry of Tier 1 PUT handlers, shared between RustPluginContext and the server.
pub struct PutHandlerRegistry {
    /// path → (plugin_id, handler)
    handlers: RwLock<HashMap<String, (String, SharedPutHandler)>>,
}

impl PutHandlerRegistry {
    pub fn new() -> Self {
        PutHandlerRegistry {
            handlers: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register(&self, path: &str, plugin_id: &str, handler: SharedPutHandler) {
        self.handlers
            .write()
            .await
            .insert(path.to_string(), (plugin_id.to_string(), handler));
    }

    /// Look up a Tier 1 PUT handler for the given path.
    pub async fn find(&self, path: &str) -> Option<(String, SharedPutHandler)> {
        let handlers = self.handlers.read().await;
        for (pattern, (plugin_id, handler)) in handlers.iter() {
            if signalk_types::matches_pattern(pattern, path) {
                return Some((plugin_id.clone(), handler.clone()));
            }
        }
        None
    }

    pub async fn remove_plugin(&self, plugin_id: &str) {
        self.handlers
            .write()
            .await
            .retain(|_, (pid, _)| pid != plugin_id);
    }
}

impl Default for PutHandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Tier 1 plugin context — direct store access, zero IPC.
pub struct RustPluginContext {
    plugin_id: String,
    store: Arc<RwLock<SignalKStore>>,
    route_table: Arc<PluginRouteTable>,
    put_handler_registry: Arc<PutHandlerRegistry>,
    /// Shared map for bridge interop (PUT handlers visible to bridge too)
    put_handlers_map: Arc<RwLock<HashMap<String, String>>>,
    /// Shared map for plugin route discovery
    plugin_routes_map: Arc<RwLock<HashMap<String, String>>>,
    config_dir: PathBuf,
    data_dir: PathBuf,
    status: Arc<Mutex<String>>,
    error_msg: Arc<Mutex<Option<String>>>,
    /// Active subscription abort handles
    sub_handles: Arc<Mutex<HashMap<u64, tokio::task::AbortHandle>>>,
    next_sub_id: Arc<Mutex<u64>>,
    /// Shared delta input filter chain (pre-store)
    delta_filter: Arc<DeltaFilterChain>,
    /// Shared webapp registry for register_webapp()
    webapp_registry: Arc<RwLock<WebappRegistry>>,
    /// Autopilot provider registry (optional — only present in full server context)
    autopilot_manager: Option<Arc<AutopilotManager>>,
    /// Shared SQLite database connection.
    database: Option<Arc<Mutex<signalk_sqlite::rusqlite::Connection>>>,
    /// Resource provider registry (optional — set via PluginManager)
    resource_providers: Option<Arc<crate::resources::ResourceProviderRegistry>>,
}

impl RustPluginContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plugin_id: String,
        store: Arc<RwLock<SignalKStore>>,
        route_table: Arc<PluginRouteTable>,
        put_handler_registry: Arc<PutHandlerRegistry>,
        put_handlers_map: Arc<RwLock<HashMap<String, String>>>,
        plugin_routes_map: Arc<RwLock<HashMap<String, String>>>,
        config_dir: PathBuf,
        data_dir: PathBuf,
        delta_filter: Arc<DeltaFilterChain>,
        webapp_registry: Arc<RwLock<WebappRegistry>>,
        autopilot_manager: Option<Arc<AutopilotManager>>,
        database: Option<Arc<Mutex<signalk_sqlite::rusqlite::Connection>>>,
        resource_providers: Option<Arc<crate::resources::ResourceProviderRegistry>>,
    ) -> Self {
        RustPluginContext {
            plugin_id,
            store,
            route_table,
            put_handler_registry,
            put_handlers_map,
            plugin_routes_map,
            config_dir,
            data_dir,
            status: Arc::new(Mutex::new(String::new())),
            error_msg: Arc::new(Mutex::new(None)),
            sub_handles: Arc::new(Mutex::new(HashMap::new())),
            next_sub_id: Arc::new(Mutex::new(1)),
            delta_filter,
            webapp_registry,
            autopilot_manager,
            database,
            resource_providers,
        }
    }

    /// Read the current status message.
    pub fn status(&self) -> String {
        self.status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Read the current error message, if any.
    pub fn error(&self) -> Option<String> {
        self.error_msg
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

#[async_trait]
impl PluginContext for RustPluginContext {
    async fn get_self_path(&self, path: &str) -> Result<Option<serde_json::Value>, PluginError> {
        let store = self.store.read().await;
        Ok(store.get_self_path(path).map(|v| v.value.clone()))
    }

    async fn get_self_path_sources(
        &self,
        path: &str,
    ) -> Result<Option<std::collections::HashMap<String, serde_json::Value>>, PluginError> {
        let store = self.store.read().await;
        Ok(store.get_self_path_sources(path).map(|sources| {
            sources
                .iter()
                .map(|(src, sv)| (src.clone(), sv.value.clone()))
                .collect()
        }))
    }

    async fn get_path(&self, full_path: &str) -> Result<Option<serde_json::Value>, PluginError> {
        let store = self.store.read().await;
        // Parse "vessels.{uri}.{path}" format
        let parts: Vec<&str> = full_path.splitn(3, '.').collect();
        if parts.len() >= 3 && parts[0] == "vessels" {
            let vessel_uri = parts[1];
            let sk_path = parts[2];
            // Resolve "self" to actual URI
            let uri = if vessel_uri == "self" {
                store.self_uri.clone()
            } else {
                vessel_uri.to_string()
            };
            Ok(store
                .get_vessel_path(&uri, sk_path)
                .map(|v| v.value.clone()))
        } else {
            // Fallback: try as self path
            Ok(store.get_self_path(full_path).map(|v| v.value.clone()))
        }
    }

    async fn handle_message(&self, delta: Delta) -> Result<(), PluginError> {
        let delta = match self.delta_filter.apply(delta) {
            Some(d) => d,
            None => return Ok(()), // dropped by a delta input handler
        };
        self.store.write().await.apply_delta(delta);
        Ok(())
    }

    async fn subscribe(
        &self,
        spec: SubscriptionSpec,
        callback: DeltaCallback,
    ) -> Result<SubscriptionHandle, PluginError> {
        let rx = {
            let store = self.store.read().await;
            store.subscribe()
        };

        let handle_id = {
            let mut id = self.next_sub_id.lock().unwrap_or_else(|p| p.into_inner());
            let current = *id;
            *id += 1;
            current
        };

        let patterns: Vec<String> = spec.subscribe.iter().map(|s| s.path.clone()).collect();
        let context = spec.context;
        let plugin_id = self.plugin_id.clone();

        let abort_handle = tokio::spawn(async move {
            let mut rx = rx;
            loop {
                match rx.recv().await {
                    Ok(delta) => {
                        let delta_ctx = delta.context.as_deref().unwrap_or("vessels.self");
                        if context != "*" && context != delta_ctx {
                            continue;
                        }

                        let matches = delta.updates.iter().any(|u| {
                            u.values.iter().any(|pv| {
                                patterns
                                    .iter()
                                    .any(|pat| signalk_types::matches_pattern(pat, &pv.path))
                            })
                        });

                        if matches {
                            callback(delta);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(plugin = %plugin_id, lagged = n, "Subscription lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .abort_handle();

        self.sub_handles
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(handle_id, abort_handle);

        Ok(SubscriptionHandle::new(handle_id))
    }

    async fn unsubscribe(&self, handle: SubscriptionHandle) -> Result<(), PluginError> {
        if let Some(abort) = self
            .sub_handles
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&handle.id())
        {
            abort.abort();
        }
        Ok(())
    }

    async fn register_put_handler(
        &self,
        _context: &str,
        path: &str,
        handler: PutHandler,
    ) -> Result<(), PluginError> {
        // Convert Box to Arc for shared access
        let shared: SharedPutHandler = Arc::from(handler);

        // Register in the Tier 1 handler registry (for direct invocation)
        self.put_handler_registry
            .register(path, &self.plugin_id, shared)
            .await;

        // Also register in the shared map (makes the path discoverable by put_path)
        self.put_handlers_map
            .write()
            .await
            .insert(path.to_string(), self.plugin_id.clone());

        tracing::info!(
            plugin = %self.plugin_id,
            path = %path,
            "Rust PUT handler registered"
        );
        Ok(())
    }

    async fn register_routes(&self, setup: RouterSetup) -> Result<(), PluginError> {
        let mut collector = RouteCollector::new();
        setup(&mut collector);
        let routes = collector.into_routes();

        let route_count = routes.len();
        self.route_table.register(&self.plugin_id, routes).await;

        // Register in the shared discovery map
        self.plugin_routes_map.write().await.insert(
            self.plugin_id.clone(),
            format!("/plugins/{}", self.plugin_id),
        );

        tracing::info!(
            plugin = %self.plugin_id,
            routes = route_count,
            "Rust plugin routes registered"
        );
        Ok(())
    }

    async fn save_options(&self, opts: serde_json::Value) -> Result<(), PluginError> {
        let path = self.config_dir.join(format!("{}.json", self.plugin_id));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&opts)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    async fn read_options(&self) -> Result<serde_json::Value, PluginError> {
        let path = self.config_dir.join(format!("{}.json", self.plugin_id));
        if !path.exists() {
            return Ok(serde_json::Value::Object(serde_json::Map::new()));
        }
        let content = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&content)?)
    }

    fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    fn database(&self) -> Option<Arc<Mutex<signalk_sqlite::rusqlite::Connection>>> {
        self.database.clone()
    }

    fn set_status(&self, msg: &str) {
        *self.status.lock().unwrap_or_else(|p| p.into_inner()) = msg.to_string();
        *self.error_msg.lock().unwrap_or_else(|p| p.into_inner()) = None;
        tracing::debug!(plugin = %self.plugin_id, status = %msg, "Plugin status");
    }

    fn set_error(&self, msg: &str) {
        *self.error_msg.lock().unwrap_or_else(|p| p.into_inner()) = Some(msg.to_string());
        tracing::warn!(plugin = %self.plugin_id, error = %msg, "Plugin error");
    }

    async fn register_webapp(
        &self,
        info: signalk_plugin_api::WebAppRegistration,
    ) -> Result<(), PluginError> {
        let url = format!("/@signalk/{}", self.plugin_id);
        let webapp = WebAppInfo {
            name: self.plugin_id.clone(),
            version: String::new(),
            display_name: Some(info.display_name),
            description: info.description,
            keywords: vec!["signalk-webapp".to_string()],
            url: url.clone(),
            public_dir: info.public_dir,
            source: Some(WebappSource::RustPlugin {
                plugin_id: self.plugin_id.clone(),
            }),
        };
        self.webapp_registry.write().await.register(webapp);
        tracing::info!(plugin = %self.plugin_id, url = %url, "Webapp registered");
        Ok(())
    }

    async fn register_delta_input_handler(
        &self,
        handler: DeltaInputHandler,
    ) -> Result<(), PluginError> {
        self.delta_filter.register(&self.plugin_id, handler);
        tracing::info!(plugin = %self.plugin_id, "Delta input handler registered");
        Ok(())
    }

    async fn register_autopilot_provider(
        &self,
        provider: Arc<dyn AutopilotProvider>,
    ) -> Result<(), PluginError> {
        let manager = self.autopilot_manager.as_ref().ok_or_else(|| {
            PluginError::runtime("register_autopilot_provider: no AutopilotManager available")
        })?;
        let device_id = provider.device_id().to_string();
        manager.register(provider, &self.plugin_id).await;
        tracing::info!(
            plugin = %self.plugin_id,
            device_id = %device_id,
            "Autopilot provider registered"
        );
        Ok(())
    }

    async fn register_resource_provider(
        &self,
        resource_type: &str,
        provider: Box<dyn signalk_plugin_api::ResourceProvider>,
    ) -> Result<(), PluginError> {
        let registry = self.resource_providers.as_ref().ok_or_else(|| {
            PluginError::runtime(
                "register_resource_provider: no ResourceProviderRegistry available",
            )
        })?;
        registry
            .register(resource_type, &self.plugin_id, Arc::from(provider))
            .await;
        tracing::info!(
            plugin = %self.plugin_id,
            resource_type = %resource_type,
            "Resource provider registered"
        );
        Ok(())
    }
}

/// Clean up all resources for a plugin (called by PluginManager on stop).
pub fn cleanup_plugin(ctx: &RustPluginContext) {
    // Abort all subscription tasks
    let handles: Vec<_> = ctx
        .sub_handles
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .drain()
        .map(|(_, h)| h)
        .collect();
    for handle in handles {
        handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signalk_plugin_api::{delta_callback, put_handler};
    use signalk_store::store::SignalKStore;
    use signalk_types::{PathValue, Source, Subscription, Update};

    fn make_test_context() -> (Arc<RustPluginContext>, Arc<RwLock<SignalKStore>>) {
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let route_table = Arc::new(PluginRouteTable::new());
        let put_registry = Arc::new(PutHandlerRegistry::new());
        let put_handlers = Arc::new(RwLock::new(HashMap::new()));
        let plugin_routes = Arc::new(RwLock::new(HashMap::new()));
        let delta_filter = Arc::new(DeltaFilterChain::new());

        let webapp_registry = Arc::new(RwLock::new(WebappRegistry::new()));
        let ctx = Arc::new(RustPluginContext::new(
            "test-plugin".to_string(),
            store.clone(),
            route_table,
            put_registry,
            put_handlers,
            plugin_routes,
            PathBuf::from("/tmp/signalk-test/config"),
            PathBuf::from("/tmp/signalk-test/data"),
            delta_filter,
            webapp_registry,
            None, // no autopilot manager in tests
            None, // no shared database in tests
            None, // no resource providers in tests
        ));

        (ctx, store)
    }

    #[tokio::test]
    async fn get_self_path_returns_stored_value() {
        let (ctx, store) = make_test_context();

        // Insert data into the store
        store
            .write()
            .await
            .apply_delta(Delta::self_vessel(vec![Update::new(
                Source::plugin("test"),
                vec![PathValue::new(
                    "navigation.speedOverGround",
                    serde_json::json!(3.5),
                )],
            )]));

        let result = ctx
            .get_self_path("navigation.speedOverGround")
            .await
            .unwrap();
        assert_eq!(result, Some(serde_json::json!(3.5)));
    }

    #[tokio::test]
    async fn get_self_path_returns_none_for_missing() {
        let (ctx, _store) = make_test_context();
        let result = ctx.get_self_path("navigation.nonexistent").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn handle_message_writes_to_store() {
        let (ctx, store) = make_test_context();

        let delta = Delta::self_vessel(vec![Update::new(
            Source::plugin("test"),
            vec![PathValue::new(
                "navigation.speedOverGround",
                serde_json::json!(5.0),
            )],
        )]);

        ctx.handle_message(delta).await.unwrap();

        let value = store
            .read()
            .await
            .get_self_path("navigation.speedOverGround")
            .unwrap()
            .value
            .clone();
        assert_eq!(value, serde_json::json!(5.0));
    }

    #[tokio::test]
    async fn subscribe_receives_matching_deltas() {
        let (ctx, store) = make_test_context();

        let received = Arc::new(Mutex::new(Vec::new()));
        let received_clone = received.clone();

        ctx.subscribe(
            SubscriptionSpec::self_vessel(vec![Subscription::path("navigation.*")]),
            delta_callback(move |delta| {
                received_clone.lock().unwrap().push(delta);
            }),
        )
        .await
        .unwrap();

        // Small delay to let the subscription task start
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Apply a delta to the store (triggers broadcast)
        store
            .write()
            .await
            .apply_delta(Delta::self_vessel(vec![Update::new(
                Source::plugin("test"),
                vec![PathValue::new(
                    "navigation.speedOverGround",
                    serde_json::json!(3.5),
                )],
            )]));

        // Small delay for the subscription callback to fire
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let count = received.lock().unwrap().len();
        assert_eq!(count, 1, "Expected 1 delta, got {}", count);
    }

    #[tokio::test]
    async fn unsubscribe_stops_delivery() {
        let (ctx, store) = make_test_context();

        let received = Arc::new(Mutex::new(Vec::new()));
        let received_clone = received.clone();

        let handle = ctx
            .subscribe(
                SubscriptionSpec::self_vessel(vec![Subscription::path("navigation.*")]),
                delta_callback(move |delta| {
                    received_clone.lock().unwrap().push(delta);
                }),
            )
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        ctx.unsubscribe(handle).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // This delta should NOT be received
        store
            .write()
            .await
            .apply_delta(Delta::self_vessel(vec![Update::new(
                Source::plugin("test"),
                vec![PathValue::new(
                    "navigation.speedOverGround",
                    serde_json::json!(5.0),
                )],
            )]));

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let count = received.lock().unwrap().len();
        assert_eq!(
            count, 0,
            "Expected 0 deltas after unsubscribe, got {}",
            count
        );
    }

    #[tokio::test]
    async fn register_put_handler_makes_path_discoverable() {
        let (ctx, _store) = make_test_context();

        ctx.register_put_handler(
            "vessels.self",
            "steering.autopilot.target.headingTrue",
            put_handler(|_cmd| async move { Ok(PutHandlerResult::Completed) }),
        )
        .await
        .unwrap();

        // The handler should be findable
        let result = ctx
            .put_handler_registry
            .find("steering.autopilot.target.headingTrue")
            .await;
        assert!(result.is_some());
        let (pid, _) = result.unwrap();
        assert_eq!(pid, "test-plugin");
    }

    #[tokio::test]
    async fn set_status_and_error() {
        let (ctx, _store) = make_test_context();

        ctx.set_status("Running");
        assert_eq!(ctx.status(), "Running");
        assert_eq!(ctx.error(), None);

        ctx.set_error("Connection lost");
        assert_eq!(ctx.error(), Some("Connection lost".to_string()));
    }
}
