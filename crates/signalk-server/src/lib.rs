pub mod api;
pub mod auth;
pub mod config;
pub mod ws;

use signalk_store::store::SignalKStore;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::ServerConfig;

/// Shared application state — passed as axum State to all handlers.
#[derive(Debug)]
pub struct ServerState {
    pub config: ServerConfig,
    pub store: Arc<RwLock<SignalKStore>>,
    /// Registered PUT handlers: path_pattern → bridge callback info
    pub put_handlers: Arc<RwLock<HashMap<String, String>>>,
    /// Registered plugin routes: plugin_id → path_prefix
    pub plugin_routes: Arc<RwLock<HashMap<String, String>>>,
}

impl ServerState {
    pub fn new(config: ServerConfig, store: Arc<RwLock<SignalKStore>>) -> Arc<Self> {
        Arc::new(ServerState {
            config,
            store,
            put_handlers: Arc::new(RwLock::new(HashMap::new())),
            plugin_routes: Arc::new(RwLock::new(HashMap::new())),
        })
    }
}

/// Build the axum router with all public API routes.
pub fn build_router(state: Arc<ServerState>) -> axum::Router {
    use axum::routing::{get, post, put};
    use tower_http::cors::CorsLayer;

    axum::Router::new()
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
        // CORS for browser clients
        .layer(CorsLayer::permissive())
        .with_state(state)
}
