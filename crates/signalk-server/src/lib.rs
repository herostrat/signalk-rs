pub mod api;
pub mod auth;
pub mod config;
pub mod plugins;
pub mod webapps;
pub mod ws;

use signalk_store::store::SignalKStore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::ServerConfig;
use crate::plugins::host::PutHandlerRegistry;
use crate::plugins::manager::PluginManager;
use crate::plugins::registry::PluginRegistry;
use crate::plugins::routes::PluginRouteTable;
use crate::webapps::{WebAppInfo, WebappRegistry};

/// Shared application state — passed as axum State to all handlers.
pub struct ServerState {
    pub config: ServerConfig,
    pub store: Arc<RwLock<SignalKStore>>,
    /// Registered PUT handlers: path_pattern → plugin_id (shared with bridge)
    pub put_handlers: Arc<RwLock<HashMap<String, String>>>,
    /// Registered plugin routes: plugin_id → path_prefix (shared with bridge)
    pub plugin_routes: Arc<RwLock<HashMap<String, String>>>,
    /// Tier 1 PUT handler registry (local Rust handlers, checked before bridge)
    pub put_handler_registry: Arc<PutHandlerRegistry>,
    /// Tier 1 route table (local Rust routes, checked before bridge proxy)
    pub route_table: Arc<PluginRouteTable>,
    /// Data directory for persistent storage (applicationData etc.)
    pub data_dir: PathBuf,
    /// Tier-agnostic plugin registry for the admin API
    pub plugin_registry: Arc<RwLock<PluginRegistry>>,
    /// Webapp registry for static file serving
    pub webapp_registry: Arc<RwLock<WebappRegistry>>,
    /// Plugin manager for Tier 1 lifecycle control (admin API)
    pub plugin_manager: Arc<tokio::sync::Mutex<PluginManager>>,
}

impl ServerState {
    pub fn new(config: ServerConfig, store: Arc<RwLock<SignalKStore>>) -> Arc<Self> {
        let data_dir = PathBuf::from(&config.data_dir);
        let route_table = Arc::new(PluginRouteTable::new());
        let put_handler_registry = Arc::new(PutHandlerRegistry::new());
        let put_handlers = Arc::new(RwLock::new(HashMap::new()));
        let plugin_routes = Arc::new(RwLock::new(HashMap::new()));
        let webapp_registry = Arc::new(RwLock::new(WebappRegistry::new()));
        let plugin_manager = PluginManager::new(
            store.clone(),
            route_table.clone(),
            put_handler_registry.clone(),
            put_handlers.clone(),
            plugin_routes.clone(),
            Arc::new(crate::plugins::delta_filter::DeltaFilterChain::new()),
            webapp_registry.clone(),
            data_dir.join("plugin-config"),
            data_dir.join("plugin-data"),
        );
        Arc::new(ServerState {
            config,
            store,
            put_handlers,
            plugin_routes,
            put_handler_registry,
            route_table,
            data_dir,
            plugin_registry: Arc::new(RwLock::new(PluginRegistry::new())),
            webapp_registry,
            plugin_manager: Arc::new(tokio::sync::Mutex::new(plugin_manager)),
        })
    }

    /// Create with externally-provided maps and plugin infrastructure.
    #[allow(clippy::too_many_arguments)]
    pub fn new_shared(
        config: ServerConfig,
        store: Arc<RwLock<SignalKStore>>,
        put_handlers: Arc<RwLock<HashMap<String, String>>>,
        plugin_routes: Arc<RwLock<HashMap<String, String>>>,
        put_handler_registry: Arc<PutHandlerRegistry>,
        route_table: Arc<PluginRouteTable>,
        plugin_manager: Arc<tokio::sync::Mutex<PluginManager>>,
        plugin_registry: Arc<RwLock<PluginRegistry>>,
    ) -> Arc<Self> {
        let data_dir = PathBuf::from(&config.data_dir);
        Arc::new(ServerState {
            config,
            store,
            put_handlers,
            plugin_routes,
            put_handler_registry,
            route_table,
            data_dir,
            plugin_registry,
            webapp_registry: Arc::new(RwLock::new(WebappRegistry::new())),
            plugin_manager,
        })
    }
}

/// Build the axum router with all public API routes.
///
/// `webapps` are mounted as static file services at their URL paths.
/// They must be passed in separately because router construction is sync,
/// but the webapp registry uses an async RwLock.
pub fn build_router(state: Arc<ServerState>, webapps: &[WebAppInfo]) -> axum::Router {
    use axum::routing::{any, get, post, put};
    use tower_http::cors::CorsLayer;

    let mut router = axum::Router::new()
        // Discovery
        .route("/signalk", get(api::discovery))
        // REST data API
        .route("/signalk/v1/api", get(api::full_model))
        .route("/signalk/v1/api/", get(api::full_model))
        .route("/signalk/v1/api/{*path}", get(api::get_path))
        .route("/signalk/v1/api/{*path}", put(api::put_path))
        // History (not implemented)
        .route("/signalk/v1/snapshot", get(api::snapshot))
        // Auth
        .route("/signalk/v1/auth/login", post(auth::login))
        .route("/signalk/v1/auth/validate", post(auth::validate))
        .route("/signalk/v1/auth/logout", put(auth::logout))
        // WebSocket streaming
        .route("/signalk/v1/stream", get(ws::handler))
        // Application data persistence
        .route(
            "/signalk/v1/applicationData/{appId}/{version}",
            get(api::get_app_data).post(api::set_app_data),
        )
        .route(
            "/signalk/v1/applicationData/{appId}/{version}/{*key}",
            get(api::get_app_data_key).post(api::set_app_data_key),
        )
        // Webapp listing
        .route("/signalk/v1/webapps", get(api::webapps::list_webapps))
        // Admin API
        .route("/admin/api/plugins", get(api::admin::list_plugins))
        .route(
            "/admin/api/plugins/{plugin_id}",
            get(api::admin::get_plugin),
        )
        .route(
            "/admin/api/plugins/{plugin_id}/config",
            get(api::admin::get_plugin_config).put(api::admin::update_plugin_config),
        )
        .route(
            "/admin/api/plugins/{plugin_id}/restart",
            post(api::admin::restart_plugin),
        )
        .route(
            "/admin/api/plugins/{plugin_id}/enable",
            post(api::admin::enable_plugin),
        )
        .route(
            "/admin/api/plugins/{plugin_id}/disable",
            post(api::admin::disable_plugin),
        )
        // Plugin routes — proxied to the bridge
        .route("/plugins/{plugin_id}", any(api::proxy_plugin_route))
        .route("/plugins/{plugin_id}/{*rest}", any(api::proxy_plugin_route));

    // Mount static file serving for each discovered webapp
    for webapp in webapps {
        router = router.nest_service(
            &webapp.url,
            tower_http::services::ServeDir::new(&webapp.public_dir)
                .append_index_html_on_directories(true),
        );
    }

    // Test-only delta injection endpoint (simulator feature)
    #[cfg(feature = "simulator")]
    let router = router.route("/test/inject", post(api::test_inject_delta));

    router.layer(CorsLayer::permissive()).with_state(state)
}
