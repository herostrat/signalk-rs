/// Dynamic route table for Tier 1 (Rust) plugin REST endpoints.
///
/// When a request arrives at `/plugins/{plugin_id}/...`, the server checks
/// this table first. If a local handler exists, it's called directly.
/// Otherwise, the request is proxied to the bridge.
use signalk_plugin_api::{PluginRequest, PluginResponse, RegisteredRoute};
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Stores REST routes registered by in-process Rust plugins.
pub struct PluginRouteTable {
    /// plugin_id → list of registered routes
    routes: RwLock<HashMap<String, Vec<RegisteredRoute>>>,
}

impl PluginRouteTable {
    pub fn new() -> Self {
        PluginRouteTable {
            routes: RwLock::new(HashMap::new()),
        }
    }

    /// Register routes for a plugin, replacing any previous registration.
    pub async fn register(&self, plugin_id: &str, routes: Vec<RegisteredRoute>) {
        self.routes
            .write()
            .await
            .insert(plugin_id.to_string(), routes);
    }

    /// Remove all routes for a plugin (called on stop).
    pub async fn remove(&self, plugin_id: &str) {
        self.routes.write().await.remove(plugin_id);
    }

    /// Check if this plugin has any locally registered routes.
    pub async fn has_routes(&self, plugin_id: &str) -> bool {
        self.routes.read().await.contains_key(plugin_id)
    }

    /// Try to handle a request for a plugin's route.
    ///
    /// `relative_path` is the path after `/plugins/{plugin_id}`, e.g. `/recorded`.
    /// Returns `None` if no matching route is found.
    pub async fn handle(
        &self,
        plugin_id: &str,
        method: &str,
        relative_path: &str,
        request: PluginRequest,
    ) -> Option<PluginResponse> {
        let routes = self.routes.read().await;
        let plugin_routes = routes.get(plugin_id)?;

        for route in plugin_routes {
            if route.method == method && route.path == relative_path {
                // Drop the read lock before calling the handler
                let handler = route.handler.clone();
                drop(routes);
                return Some(handler(request).await);
            }
        }

        None
    }
}

impl Default for PluginRouteTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signalk_plugin_api::route_handler;

    #[tokio::test]
    async fn register_and_handle_route() {
        let table = PluginRouteTable::new();

        let routes = vec![RegisteredRoute {
            method: "GET".into(),
            path: "/status".into(),
            handler: route_handler(|_req| async move {
                PluginResponse::json(200, &serde_json::json!({"ok": true}))
            }),
        }];

        table.register("test-plugin", routes).await;
        assert!(table.has_routes("test-plugin").await);

        let request = PluginRequest {
            method: "GET".into(),
            path: "/plugins/test-plugin/status".into(),
            query: None,
            headers: vec![],
            body: vec![],
        };

        let response = table.handle("test-plugin", "GET", "/status", request).await;
        assert!(response.is_some());
        let resp = response.unwrap();
        assert_eq!(resp.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn handle_returns_none_for_unknown_plugin() {
        let table = PluginRouteTable::new();
        let request = PluginRequest {
            method: "GET".into(),
            path: "/plugins/unknown/status".into(),
            query: None,
            headers: vec![],
            body: vec![],
        };
        assert!(table.handle("unknown", "GET", "/status", request).await.is_none());
    }

    #[tokio::test]
    async fn remove_clears_routes() {
        let table = PluginRouteTable::new();
        table
            .register(
                "test-plugin",
                vec![RegisteredRoute {
                    method: "GET".into(),
                    path: "/".into(),
                    handler: route_handler(|_| async { PluginResponse::empty(200) }),
                }],
            )
            .await;

        assert!(table.has_routes("test-plugin").await);
        table.remove("test-plugin").await;
        assert!(!table.has_routes("test-plugin").await);
    }
}
