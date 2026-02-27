pub mod api;
pub mod auth;
pub mod config;
pub mod plugins;
pub mod ws;

use signalk_store::store::SignalKStore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::ServerConfig;
use crate::plugins::host::PutHandlerRegistry;
use crate::plugins::routes::PluginRouteTable;

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
}

impl ServerState {
    pub fn new(config: ServerConfig, store: Arc<RwLock<SignalKStore>>) -> Arc<Self> {
        let data_dir = PathBuf::from(&config.data_dir);
        Arc::new(ServerState {
            config,
            store,
            put_handlers: Arc::new(RwLock::new(HashMap::new())),
            plugin_routes: Arc::new(RwLock::new(HashMap::new())),
            put_handler_registry: Arc::new(PutHandlerRegistry::new()),
            route_table: Arc::new(PluginRouteTable::new()),
            data_dir,
        })
    }

    /// Create with externally-provided maps and plugin infrastructure.
    pub fn new_shared(
        config: ServerConfig,
        store: Arc<RwLock<SignalKStore>>,
        put_handlers: Arc<RwLock<HashMap<String, String>>>,
        plugin_routes: Arc<RwLock<HashMap<String, String>>>,
        put_handler_registry: Arc<PutHandlerRegistry>,
        route_table: Arc<PluginRouteTable>,
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
        })
    }
}

/// Build the axum router with all public API routes.
pub fn build_router(state: Arc<ServerState>) -> axum::Router {
    use axum::routing::{any, get, post, put};
    use tower_http::cors::CorsLayer;

    let router = axum::Router::new()
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
        // Plugin routes — proxied to the bridge
        .route("/plugins/{plugin_id}", any(api::proxy_plugin_route))
        .route("/plugins/{plugin_id}/{*rest}", any(api::proxy_plugin_route));

    // Test-only delta injection endpoint (simulator feature)
    #[cfg(feature = "simulator")]
    let router = router.route("/test/inject", post(api::test_inject_delta));

    router.layer(CorsLayer::permissive()).with_state(state)
}
