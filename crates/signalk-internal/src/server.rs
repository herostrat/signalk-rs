/// Internal API server — listens on a Unix Domain Socket for bridge requests.
///
/// Exposes:
///   POST /internal/v1/delta           — bridge injects delta (handleMessage)
///   GET  /internal/v1/api/{*path}     — bridge queries path (getSelfPath)
///   PUT  /internal/v1/api/{*path}     — bridge writes path (putSelfPath)
///   POST /internal/v1/handlers        — bridge registers PUT handler
///   POST /internal/v1/plugin-routes   — bridge registers REST routes
///   POST /internal/v1/bridge/register — bridge registers itself on startup
use axum::{
    body::Body,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;
use tracing::{info, warn};

use crate::protocol::{
    BridgeRegistration, DeltaIngest, HandlerRegistration, PathQuery, PathQueryResponse,
    PluginRouteRegistration,
};
use crate::uds::bind_unix_socket;

/// Callbacks from the main server injected when creating InternalState.
pub struct Callbacks {
    pub on_delta: Box<dyn Fn(DeltaIngest) + Send + Sync>,
    pub on_query: Box<dyn Fn(PathQuery) -> Option<PathQueryResponse> + Send + Sync>,
}

/// State shared across all internal API handlers.
#[derive(Clone)]
pub struct InternalState {
    pub bridge_token: Arc<String>,
    pub bridge_version: Arc<RwLock<Option<String>>>,
    pub put_handlers: Arc<RwLock<HashMap<String, String>>>,
    pub plugin_routes: Arc<RwLock<HashMap<String, String>>>,
    pub callbacks: Arc<Callbacks>,
}

impl InternalState {
    pub fn new(bridge_token: String, callbacks: Callbacks) -> Self {
        InternalState {
            bridge_token: Arc::new(bridge_token),
            bridge_version: Arc::new(RwLock::new(None)),
            put_handlers: Arc::new(RwLock::new(HashMap::new())),
            plugin_routes: Arc::new(RwLock::new(HashMap::new())),
            callbacks: Arc::new(callbacks),
        }
    }
}

/// Start the internal API HTTP server on a Unix Domain Socket.
pub async fn serve_internal_api(
    socket_path: PathBuf,
    state: InternalState,
) -> anyhow::Result<()> {
    let listener = bind_unix_socket(&socket_path)?;
    info!(socket = %socket_path.display(), "Internal API listening");

    let router = build_internal_router(state);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let app = router.clone();

        tokio::spawn(async move {
            // axum Router expects Request<Body>, hyper gives Request<Incoming>
            // .map(Body::new) converts between the two body types
            let service = hyper::service::service_fn(move |req: hyper::Request<Incoming>| {
                let app = app.clone();
                async move { app.oneshot(req.map(Body::new)).await }
            });

            if let Err(e) = auto::Builder::new(TokioExecutor::new())
                .serve_connection(io, service)
                .await
            {
                warn!("Internal connection error: {}", e);
            }
        });
    }
}

fn build_internal_router(state: InternalState) -> Router {
    Router::new()
        .route("/internal/v1/delta", post(ingest_delta))
        .route("/internal/v1/api/{*path}", get(query_path))
        .route("/internal/v1/api/{*path}", put(write_path))
        .route("/internal/v1/handlers", post(register_handler))
        .route("/internal/v1/plugin-routes", post(register_plugin_routes))
        .route("/internal/v1/bridge/register", post(register_bridge))
        .with_state(state)
}

fn check_token(headers: &axum::http::HeaderMap, state: &InternalState) -> bool {
    headers
        .get("X-Bridge-Token")
        .and_then(|v| v.to_str().ok())
        .map(|t| t == state.bridge_token.as_str())
        .unwrap_or(false)
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"message": "Invalid or missing X-Bridge-Token"})),
    )
        .into_response()
}

async fn ingest_delta(
    State(state): State<InternalState>,
    headers: axum::http::HeaderMap,
    Json(delta): Json<DeltaIngest>,
) -> Response {
    if !check_token(&headers, &state) { return unauthorized(); }
    (state.callbacks.on_delta)(delta);
    StatusCode::NO_CONTENT.into_response()
}

async fn query_path(
    State(state): State<InternalState>,
    headers: axum::http::HeaderMap,
    Path(url_path): Path<String>,
) -> Response {
    if !check_token(&headers, &state) { return unauthorized(); }
    let query = PathQuery::self_path(url_to_sk_path(&url_path));
    match (state.callbacks.on_query)(query) {
        Some(resp) => Json(resp).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"message": "Path not found"})),
        )
            .into_response(),
    }
}

async fn write_path(
    State(state): State<InternalState>,
    headers: axum::http::HeaderMap,
    Path(url_path): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if !check_token(&headers, &state) { return unauthorized(); }
    use signalk_types::{PathValue, Source, Update};
    let delta = DeltaIngest::self_vessel(vec![Update::new(
        Source::plugin("bridge"),
        vec![PathValue::new(url_to_sk_path(&url_path), body["value"].clone())],
    )]);
    (state.callbacks.on_delta)(delta);
    StatusCode::NO_CONTENT.into_response()
}

async fn register_handler(
    State(state): State<InternalState>,
    headers: axum::http::HeaderMap,
    Json(reg): Json<HandlerRegistration>,
) -> Response {
    if !check_token(&headers, &state) { return unauthorized(); }
    state.put_handlers.write().await.insert(reg.path.clone(), reg.plugin_id.clone());
    info!(plugin = %reg.plugin_id, path = %reg.path, "PUT handler registered");
    StatusCode::NO_CONTENT.into_response()
}

async fn register_plugin_routes(
    State(state): State<InternalState>,
    headers: axum::http::HeaderMap,
    Json(reg): Json<PluginRouteRegistration>,
) -> Response {
    if !check_token(&headers, &state) { return unauthorized(); }
    state.plugin_routes.write().await.insert(reg.plugin_id.clone(), reg.path_prefix.clone());
    info!(plugin = %reg.plugin_id, prefix = %reg.path_prefix, "Plugin route registered");
    StatusCode::NO_CONTENT.into_response()
}

async fn register_bridge(
    State(state): State<InternalState>,
    headers: axum::http::HeaderMap,
    Json(reg): Json<BridgeRegistration>,
) -> Response {
    if !check_token(&headers, &state) { return unauthorized(); }
    *state.bridge_version.write().await = Some(reg.version.clone());
    info!(version = %reg.version, "Bridge registered");
    StatusCode::NO_CONTENT.into_response()
}

fn url_to_sk_path(url_path: &str) -> String {
    let parts: Vec<&str> = url_path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 && parts[0] == "vessels" { parts[2..].join(".") } else { parts.join(".") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state() -> InternalState {
        InternalState::new("secret".to_string(), Callbacks {
            on_delta: Box::new(|_| {}),
            on_query: Box::new(|_| None),
        })
    }

    #[test]
    fn url_to_sk_path_strips_vessels_self() {
        assert_eq!(url_to_sk_path("vessels/self/navigation/speedOverGround"), "navigation.speedOverGround");
    }

    #[test]
    fn url_to_sk_path_without_prefix() {
        assert_eq!(url_to_sk_path("navigation/speedOverGround"), "navigation.speedOverGround");
    }

    #[test]
    fn url_to_sk_path_nested() {
        assert_eq!(url_to_sk_path("vessels/self/navigation/position/latitude"), "navigation.position.latitude");
    }

    #[test]
    fn check_token_valid() {
        use axum::http::HeaderMap;
        let mut headers = HeaderMap::new();
        headers.insert("X-Bridge-Token", "secret".parse().unwrap());
        assert!(check_token(&headers, &make_state()));
    }

    #[test]
    fn check_token_invalid() {
        use axum::http::HeaderMap;
        let mut headers = HeaderMap::new();
        headers.insert("X-Bridge-Token", "wrong".parse().unwrap());
        assert!(!check_token(&headers, &make_state()));
    }

    #[test]
    fn check_token_missing() {
        assert!(!check_token(&axum::http::HeaderMap::new(), &make_state()));
    }
}
