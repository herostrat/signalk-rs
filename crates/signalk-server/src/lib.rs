pub mod api;
pub mod auth;
pub mod autopilot;
pub mod config;
pub mod course;
pub mod history;
pub mod plugins;
pub mod resources;
pub mod webapps;
pub mod ws;

use signalk_store::store::SignalKStore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::RwLock;

use crate::autopilot::AutopilotManager;
use crate::config::ServerConfig;
use crate::course::CourseManager;
use crate::history::HistoryManager;
use crate::plugins::host::PutHandlerRegistry;
use crate::plugins::manager::PluginManager;
use crate::plugins::registry::PluginRegistry;
use crate::plugins::routes::PluginRouteTable;
use crate::resources::ResourceProviderRegistry;
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
    /// Resource provider registry (file-based default, plugin-overridable)
    pub resource_providers: Arc<ResourceProviderRegistry>,
    /// Course/navigation manager
    pub course_manager: Arc<CourseManager>,
    /// Autopilot provider registry — V2 autopilot API delegates here
    pub autopilot_manager: Arc<AutopilotManager>,
    /// History subsystem — time-series storage and query
    pub history_manager: Arc<HistoryManager>,
}

impl ServerState {
    pub fn new(config: ServerConfig, store: Arc<RwLock<SignalKStore>>) -> Arc<Self> {
        let data_dir = PathBuf::from(&config.data_dir);
        let route_table = Arc::new(PluginRouteTable::new());
        let put_handler_registry = Arc::new(PutHandlerRegistry::new());
        let put_handlers = Arc::new(RwLock::new(HashMap::new()));
        let plugin_routes = Arc::new(RwLock::new(HashMap::new()));
        let webapp_registry = Arc::new(RwLock::new(WebappRegistry::new()));
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            resources::FileResourceProvider::new(data_dir.join("resources")),
        )));
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
        let course_manager = Arc::new(CourseManager::new(
            store.clone(),
            data_dir.clone(),
            resource_providers.clone(),
        ));
        let autopilot_manager = AutopilotManager::new();
        let history_config = history::HistoryConfig::default();
        let history_manager =
            HistoryManager::new_in_memory(history_config).expect("History manager init");
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
            resource_providers,
            course_manager,
            autopilot_manager,
            history_manager,
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
        webapp_registry: Arc<RwLock<WebappRegistry>>,
        resource_providers: Arc<ResourceProviderRegistry>,
        autopilot_manager: Arc<AutopilotManager>,
        history_manager: Arc<HistoryManager>,
    ) -> Arc<Self> {
        let data_dir = PathBuf::from(&config.data_dir);
        let course_manager = Arc::new(CourseManager::new(
            store.clone(),
            data_dir.clone(),
            resource_providers.clone(),
        ));
        Arc::new(ServerState {
            config,
            store,
            put_handlers,
            plugin_routes,
            put_handler_registry,
            route_table,
            data_dir,
            plugin_registry,
            webapp_registry,
            plugin_manager,
            resource_providers,
            course_manager,
            autopilot_manager,
            history_manager,
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
        // ════════════════════════════════════════════════════════════════════
        // SignalK v1 Spec  —  https://signalk.org/specification/1.7.0/
        // These routes are defined by the spec. Paths and semantics are mandated.
        // ════════════════════════════════════════════════════════════════════
        // -- Discovery --------------------------------------------------------
        // KIP requests /signalk/ (with trailing slash), so both variants are needed.
        .route("/signalk", get(api::discovery))
        .route("/signalk/", get(api::discovery))
        // -- REST Data API ----------------------------------------------------
        .route("/signalk/v1/api", get(api::full_model))
        .route("/signalk/v1/api/", get(api::full_model))
        .route(
            "/signalk/v1/api/{*path}",
            get(api::get_path).put(api::put_path),
        )
        // -- History ----------------------------------------------------------
        // Spec-defined. Real implementation requires persistent storage (SQLite).
        // See also the v2 History API stubs below.
        .route("/signalk/v1/snapshot", get(api::snapshot))
        // -- Authentication ---------------------------------------------------
        .route("/signalk/v1/auth/login", post(auth::login))
        .route("/signalk/v1/auth/validate", post(auth::validate))
        .route("/signalk/v1/auth/logout", put(auth::logout))
        // -- WebSocket Streaming ----------------------------------------------
        // TODO (spec): /signalk/v1/playback — WS history playback (needs persistent store)
        .route("/signalk/v1/stream", get(ws::handler))
        // -- Application Data -------------------------------------------------
        // scope = "global" or "user"
        .route(
            "/signalk/v1/applicationData/{scope}/{appId}/{version}",
            get(api::get_app_data).post(api::set_app_data),
        )
        .route(
            "/signalk/v1/applicationData/{scope}/{appId}/{version}/{*key}",
            get(api::get_app_data_key).post(api::set_app_data_key),
        )
        // -- Track History ----------------------------------------------------
        // Spec-defined. Delegates to the tracks plugin via PluginRouteTable.
        // The plugin stores position history as a time series (not in the SK store tree).
        .route(
            "/signalk/v1/api/tracks",
            get(api::tracks::get_all_tracks).delete(api::tracks::delete_all_tracks),
        )
        .route(
            "/signalk/v1/api/vessels/{vessel_id}/track",
            get(api::tracks::get_vessel_track).delete(api::tracks::delete_vessel_track),
        )
        // ════════════════════════════════════════════════════════════════════
        // SignalK v2 Spec  —  https://demo.signalk.org/documentation/Developing/REST_APIs/
        // v2 is an evolving extension to v1. Spec and implementations grow in parallel.
        // ════════════════════════════════════════════════════════════════════
        // -- Features Discovery -----------------------------------------------
        .route("/signalk/v2/features", get(api::v2::features::get_features))
        // -- Resources API ----------------------------------------------------
        .route(
            "/signalk/v2/api/resources/{resource_type}",
            get(api::v2::resources::list_resources).post(api::v2::resources::create_resource),
        )
        .route(
            "/signalk/v2/api/resources/{resource_type}/{id}",
            get(api::v2::resources::get_resource)
                .put(api::v2::resources::update_resource)
                .delete(api::v2::resources::delete_resource),
        )
        // Provider discovery for resource types
        .route(
            "/signalk/v2/api/resources/{resource_type}/_providers",
            get(api::v2::resources::list_providers),
        )
        .route(
            "/signalk/v2/api/resources/{resource_type}/_providers/_default",
            get(api::v2::resources::get_default_provider),
        )
        .route(
            "/signalk/v2/api/resources/{resource_type}/_providers/_default/{plugin_id}",
            post(api::v2::resources::set_default_provider),
        )
        // -- Course API -------------------------------------------------------
        .route(
            "/signalk/v2/api/vessels/self/navigation/course",
            get(api::v2::course::get_course).delete(api::v2::course::clear_course),
        )
        .route(
            "/signalk/v2/api/vessels/self/navigation/course/_config",
            get(api::v2::course::get_config),
        )
        .route(
            "/signalk/v2/api/vessels/self/navigation/course/_config/apiOnly",
            post(api::v2::course::enable_api_only).delete(api::v2::course::disable_api_only),
        )
        .route(
            "/signalk/v2/api/vessels/self/navigation/course/destination",
            put(api::v2::course::set_destination),
        )
        .route(
            "/signalk/v2/api/vessels/self/navigation/course/activeRoute",
            put(api::v2::course::set_active_route),
        )
        .route(
            "/signalk/v2/api/vessels/self/navigation/course/activeRoute/nextPoint",
            put(api::v2::course::advance_next_point),
        )
        .route(
            "/signalk/v2/api/vessels/self/navigation/course/activeRoute/pointIndex",
            put(api::v2::course::set_point_index),
        )
        .route(
            "/signalk/v2/api/vessels/self/navigation/course/activeRoute/reverse",
            put(api::v2::course::reverse_route),
        )
        .route(
            "/signalk/v2/api/vessels/self/navigation/course/arrivalCircle",
            put(api::v2::course::set_arrival_circle),
        )
        // calcValues is our extension (not in spec, but spec-compatible computed values)
        .route(
            "/signalk/v2/api/vessels/self/navigation/course/calcValues",
            get(api::v2::course::get_calc_values),
        )
        // -- Autopilot API ----------------------------------------------------
        // Full V2 autopilot API. Routes to registered AutopilotProvider plugins.
        .route(
            "/signalk/v2/api/vessels/self/autopilots",
            get(api::v2::autopilot::list_autopilots),
        )
        .route(
            "/signalk/v2/api/vessels/self/autopilots/_providers/_default",
            get(api::v2::autopilot::get_default_provider),
        )
        .route(
            "/signalk/v2/api/vessels/self/autopilots/_providers/_default/{id}",
            post(api::v2::autopilot::set_default_provider),
        )
        .route(
            "/signalk/v2/api/vessels/self/autopilots/{device_id}",
            get(api::v2::autopilot::get_autopilot),
        )
        .route(
            "/signalk/v2/api/vessels/self/autopilots/{device_id}/state",
            get(api::v2::autopilot::get_state).put(api::v2::autopilot::set_state),
        )
        .route(
            "/signalk/v2/api/vessels/self/autopilots/{device_id}/mode",
            get(api::v2::autopilot::get_mode).put(api::v2::autopilot::set_mode),
        )
        .route(
            "/signalk/v2/api/vessels/self/autopilots/{device_id}/target",
            get(api::v2::autopilot::get_target).put(api::v2::autopilot::set_target),
        )
        .route(
            "/signalk/v2/api/vessels/self/autopilots/{device_id}/target/adjust",
            put(api::v2::autopilot::adjust_target),
        )
        .route(
            "/signalk/v2/api/vessels/self/autopilots/{device_id}/engage",
            post(api::v2::autopilot::engage),
        )
        .route(
            "/signalk/v2/api/vessels/self/autopilots/{device_id}/disengage",
            post(api::v2::autopilot::disengage),
        )
        .route(
            "/signalk/v2/api/vessels/self/autopilots/{device_id}/tack/{direction}",
            post(api::v2::autopilot::tack),
        )
        .route(
            "/signalk/v2/api/vessels/self/autopilots/{device_id}/gybe/{direction}",
            post(api::v2::autopilot::gybe),
        )
        .route(
            "/signalk/v2/api/vessels/self/autopilots/{device_id}/dodge",
            post(api::v2::autopilot::dodge_enter)
                .put(api::v2::autopilot::dodge_adjust)
                .delete(api::v2::autopilot::dodge_exit),
        )
        // -- Notifications API ------------------------------------------------
        // Alarm interaction: silence and acknowledge active notifications.
        .route(
            "/signalk/v2/api/notifications/{notification_id}/silence",
            post(api::v2::notifications::silence),
        )
        .route(
            "/signalk/v2/api/notifications/{notification_id}/acknowledge",
            post(api::v2::notifications::acknowledge),
        )
        // -- History API -------------------------------------------------------
        // SignalK v2 History API — backed by SQLite (HistoryManager).
        .route(
            "/signalk/v2/api/history/values",
            get(api::v2::history::get_values),
        )
        .route(
            "/signalk/v2/api/history/contexts",
            get(api::v2::history::get_contexts),
        )
        .route(
            "/signalk/v2/api/history/paths",
            get(api::v2::history::get_paths),
        )
        // ════════════════════════════════════════════════════════════════════
        // De-facto Standard  —  not in the spec, but expected by all clients
        // ════════════════════════════════════════════════════════════════════
        // -- Webapp Listing ---------------------------------------------------
        // Not in the v1 spec, but expected by InstrumentPanel, KIP, and others
        // for webapp discovery. All known SignalK servers expose this.
        .route("/signalk/v1/webapps", get(api::webapps::list_webapps))
        // -- Plugin Routing ---------------------------------------------------
        // Not defined by the spec, but established convention across all SignalK servers.
        // Plugins register their own routes under /plugins/{plugin_id}/.
        // Tier 1 (Rust): PluginRouteTable. Tier 2 (JS): bridge proxy.
        .route("/plugins/{plugin_id}", any(api::proxy_plugin_route))
        .route("/plugins/{plugin_id}/{*rest}", any(api::proxy_plugin_route))
        // ════════════════════════════════════════════════════════════════════
        // Admin API  —  our own, not part of the SignalK spec
        // Plugin lifecycle management, used by the Admin UI at /admin/.
        // ════════════════════════════════════════════════════════════════════
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
        // ════════════════════════════════════════════════════════════════════
        // /skServer Compatibility  —  not in spec, for the Node.js Admin UI
        // The Node.js reference server exposes /skServer/* routes that its
        // Admin UI calls directly. We mirror them to run the Admin UI unmodified.
        // Can be removed once we ship our own Admin UI or upstream adopts standard endpoints.
        // ════════════════════════════════════════════════════════════════════
        .route(
            "/skServer/loginStatus",
            get(api::server_routes::login_status),
        )
        .route("/skServer/plugins", get(api::admin::list_plugins))
        .route(
            "/skServer/plugins/{plugin_id}/config",
            get(api::admin::get_plugin_config).post(api::admin::update_plugin_config),
        )
        .route("/skServer/webapps", get(api::server_routes::list_webapps))
        .route("/skServer/settings", get(api::server_routes::get_settings))
        .route("/skServer/vessel", get(api::server_routes::get_vessel))
        .route("/skServer/addons", get(api::server_routes::empty_array))
        .route(
            "/skServer/appstore/available",
            get(api::server_routes::appstore_available),
        )
        .route(
            "/skServer/security/access/requests",
            get(api::server_routes::empty_array),
        )
        .route(
            "/skServer/security/config",
            get(api::server_routes::empty_object),
        )
        .route(
            "/skServer/security/users",
            get(api::server_routes::empty_array),
        )
        .route(
            "/skServer/security/devices",
            get(api::server_routes::empty_array),
        )
        .route("/skServer/providers", get(api::server_routes::empty_array))
        .route(
            "/skServer/availablePaths",
            get(api::server_routes::empty_array),
        )
        .route(
            "/skServer/sourcePriorities",
            get(api::server_routes::empty_object),
        )
        .route("/skServer/debugKeys", get(api::server_routes::empty_array))
        .route(
            "/skServer/logfiles/",
            get(api::server_routes::list_logfiles),
        )
        .route(
            "/skServer/runDiscovery",
            put(api::server_routes::run_discovery),
        );

    // Mount static file serving for each discovered webapp
    for webapp in webapps {
        router = router.nest_service(
            &webapp.url,
            tower_http::services::ServeDir::new(&webapp.public_dir)
                .append_index_html_on_directories(true),
        );
    }

    // Admin UI — explicit mount (no signalk-webapp keyword, like original server)
    // The admin UI's index.html contains a `%ADDONSCRIPTS%` placeholder that the
    // original Node.js server replaces with embedded plugin script tags. We serve
    // a processed index.html from memory (the container FS may be read-only) and
    // use ServeDir as fallback for all other static assets.
    let admin_ui_dir =
        std::path::Path::new(&state.config.modules_dir).join("@signalk/server-admin-ui/public");
    if admin_ui_dir.is_dir() {
        // Read and process index.html in memory, stripping the placeholder
        let index_html: Arc<String> = Arc::new(
            std::fs::read_to_string(admin_ui_dir.join("index.html"))
                .unwrap_or_default()
                .replace("%ADDONSCRIPTS%", ""),
        );

        let serve_dir = tower_http::services::ServeDir::new(&admin_ui_dir);
        let admin_service = AdminService {
            index_html,
            serve_dir,
        };
        router = router.nest_service("/admin", admin_service);
    }

    // Test-only delta injection endpoint (simulator feature)
    #[cfg(feature = "simulator")]
    let router = router.route("/test/inject", post(api::test_inject_delta));

    router.layer(CorsLayer::permissive()).with_state(state)
}

// ─── Admin UI Service ─────────────────────────────────────────────────────────
// Wraps ServeDir but intercepts index.html requests to serve a processed
// version with template placeholders stripped (e.g. %ADDONSCRIPTS%).

#[derive(Clone)]
struct AdminService {
    index_html: Arc<String>,
    serve_dir: tower_http::services::ServeDir,
}

impl tower::Service<axum::http::Request<axum::body::Body>> for AdminService {
    type Response = axum::response::Response;
    type Error = std::convert::Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: axum::http::Request<axum::body::Body>) -> Self::Future {
        let path = req.uri().path();
        // Serve processed index.html for root/directory requests
        if path == "/" || path.is_empty() || path == "/index.html" {
            let html = self.index_html.clone();
            return Box::pin(async move {
                Ok(axum::response::Response::builder()
                    .status(200)
                    .header("content-type", "text/html")
                    .body(axum::body::Body::from(html.as_str().to_string()))
                    .unwrap())
            });
        }
        // All other files: delegate to ServeDir
        let fut = self.serve_dir.call(req);
        Box::pin(async move {
            let resp = fut.await.map_err(|e| match e {})?;
            Ok(resp.map(axum::body::Body::new))
        })
    }
}
