//! Admin API integration tests — exercises the plugin management REST endpoints.
//!
//! Uses the real PluginManager + PluginRegistry with test plugins to verify
//! the admin API handlers return correct responses.
mod helpers;

use helpers::{get, post_empty, test_app_with_state};
use signalk_server::plugins::registry::BridgePluginInfo;
use std::sync::Arc;

use signalk_plugin_api::{Plugin, PluginContext, PluginError, PluginMetadata};

/// A minimal test plugin for admin API tests.
struct AdminTestPlugin {
    id: String,
    name: String,
}

impl AdminTestPlugin {
    fn new(id: &str, name: &str) -> Self {
        AdminTestPlugin {
            id: id.to_string(),
            name: name.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl Plugin for AdminTestPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            &self.id,
            &self.name,
            "A test plugin for admin tests",
            "1.0.0",
        )
    }

    async fn start(
        &mut self,
        _config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError> {
        ctx.set_status("Running");
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        Ok(())
    }
}

#[tokio::test]
async fn list_plugins_empty() {
    let (app, _state) = test_app_with_state();
    let (status, body) = get(app, "/admin/api/plugins").await;
    assert_eq!(status, 200);
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn list_plugins_with_tier1() {
    let (app, state) = test_app_with_state();

    // Register and start a Tier 1 plugin
    {
        let mut mgr = state.plugin_manager.lock().await;
        mgr.register(Box::new(AdminTestPlugin::new("test-alpha", "Test Alpha")));
        mgr.start_plugin("test-alpha", serde_json::json!({}))
            .await
            .unwrap();
    }

    // Sync registry
    {
        let mgr = state.plugin_manager.lock().await;
        signalk_server::api::admin::populate_registry_from_manager(&state, &mgr).await;
    }

    let (status, body) = get(app, "/admin/api/plugins").await;
    assert_eq!(status, 200);

    let plugins = body.as_array().unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0]["id"], "test-alpha");
    assert_eq!(plugins[0]["name"], "Test Alpha");
    assert_eq!(plugins[0]["tier"], "rust");
    assert!(plugins[0]["status"].as_str().unwrap().contains("running"));
    assert_eq!(plugins[0]["enabled"], true);
}

#[tokio::test]
async fn list_plugins_with_tier2() {
    let (app, state) = test_app_with_state();

    // Register a Tier 2 (Bridge) plugin directly in registry
    {
        let mut registry = state.plugin_registry.write().await;
        registry.register_tier2(BridgePluginInfo {
            id: "signalk-to-nmea0183".to_string(),
            name: "SignalK to NMEA0183".to_string(),
            version: "3.0.0".to_string(),
            description: "Converts SignalK to NMEA0183".to_string(),
            has_webapp: false,
        });
    }

    let (status, body) = get(app, "/admin/api/plugins").await;
    assert_eq!(status, 200);

    let plugins = body.as_array().unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0]["id"], "signalk-to-nmea0183");
    assert_eq!(plugins[0]["tier"], "bridge");
}

#[tokio::test]
async fn list_plugins_mixed_tiers() {
    let (app, state) = test_app_with_state();

    // Register Tier 1
    {
        let mut mgr = state.plugin_manager.lock().await;
        mgr.register(Box::new(AdminTestPlugin::new("rust-plugin", "Rust Plugin")));
        mgr.start_plugin("rust-plugin", serde_json::json!({}))
            .await
            .unwrap();
    }
    {
        let mgr = state.plugin_manager.lock().await;
        signalk_server::api::admin::populate_registry_from_manager(&state, &mgr).await;
    }

    // Register Tier 2
    {
        let mut registry = state.plugin_registry.write().await;
        registry.register_tier2(BridgePluginInfo {
            id: "bridge-plugin".to_string(),
            name: "Bridge Plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "A bridge plugin".to_string(),
            has_webapp: true,
        });
    }

    let (status, body) = get(app, "/admin/api/plugins").await;
    assert_eq!(status, 200);

    let plugins = body.as_array().unwrap();
    assert_eq!(plugins.len(), 2);

    // Find each by ID
    let rust = plugins.iter().find(|p| p["id"] == "rust-plugin").unwrap();
    let bridge = plugins.iter().find(|p| p["id"] == "bridge-plugin").unwrap();

    assert_eq!(rust["tier"], "rust");
    assert_eq!(bridge["tier"], "bridge");
    assert_eq!(bridge["hasWebapp"], true);
}

#[tokio::test]
async fn get_plugin_found() {
    let (app, state) = test_app_with_state();

    {
        let mut registry = state.plugin_registry.write().await;
        registry.register_tier1("test-get", "Test Get", "desc", "0.1.0", "running", true);
    }

    let (status, body) = get(app, "/admin/api/plugins/test-get").await;
    assert_eq!(status, 200);
    assert_eq!(body["id"], "test-get");
    assert_eq!(body["name"], "Test Get");
}

#[tokio::test]
async fn get_plugin_not_found() {
    let (app, _state) = test_app_with_state();

    let (status, body) = get(app, "/admin/api/plugins/nonexistent").await;
    assert_eq!(status, 404);
    assert!(body["message"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn disable_tier1_plugin() {
    let (app, state) = test_app_with_state();

    // Register and start
    {
        let mut mgr = state.plugin_manager.lock().await;
        mgr.register(Box::new(AdminTestPlugin::new(
            "disable-test",
            "Disable Test",
        )));
        mgr.start_plugin("disable-test", serde_json::json!({}))
            .await
            .unwrap();
    }
    {
        let mgr = state.plugin_manager.lock().await;
        signalk_server::api::admin::populate_registry_from_manager(&state, &mgr).await;
    }

    // Verify running
    assert!(state.plugin_manager.lock().await.is_running("disable-test"));

    // Disable
    let (status, _) = post_empty(app.clone(), "/admin/api/plugins/disable-test/disable").await;
    assert_eq!(status, 204);

    // Verify stopped
    assert!(!state.plugin_manager.lock().await.is_running("disable-test"));

    // Registry should reflect stopped status
    let registry = state.plugin_registry.read().await;
    let info = registry.get("disable-test").unwrap();
    assert!(
        info.status.contains("stopped"),
        "Expected 'stopped', got: {}",
        info.status
    );
    assert!(!info.enabled);
}

#[tokio::test]
async fn enable_tier1_plugin() {
    let (app, state) = test_app_with_state();

    // Register but don't start
    {
        let mut mgr = state.plugin_manager.lock().await;
        mgr.register(Box::new(AdminTestPlugin::new("enable-test", "Enable Test")));
    }
    {
        let mgr = state.plugin_manager.lock().await;
        signalk_server::api::admin::populate_registry_from_manager(&state, &mgr).await;
    }

    // Verify not running
    assert!(!state.plugin_manager.lock().await.is_running("enable-test"));

    // Enable
    let (status, _) = post_empty(app.clone(), "/admin/api/plugins/enable-test/enable").await;
    assert_eq!(status, 204);

    // Verify running
    assert!(state.plugin_manager.lock().await.is_running("enable-test"));
}

#[tokio::test]
async fn disable_nonexistent_returns_404() {
    let (app, _state) = test_app_with_state();

    let (status, body) = post_empty(app, "/admin/api/plugins/ghost/disable").await;
    assert_eq!(status, 404);
    assert!(body["message"].as_str().unwrap().contains("not found"));
}
