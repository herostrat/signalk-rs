/// PluginContext trait — the "app" object for plugins.
///
/// This is the canonical API surface that all plugin tiers implement:
/// - **Tier 1 (Rust):** `RustPluginContext` — direct store access, zero IPC
/// - **Tier 2 (JS/Bridge):** `app.js` — wraps these calls as JS methods
/// - **Tier 3 (Standalone):** `RemotePluginContext` — HTTP-over-UDS calls
///
/// Every method has a 1:1 correspondence with the JS bridge's `app.*` API.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use signalk_types::Delta;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::PluginError;

// ─── Subscription types ─────────────────────────────────────────────────────

/// Specifies what to subscribe to. Mirrors the WebSocket subscribe message
/// but is used internally by plugins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionSpec {
    /// Vessel context, e.g. "vessels.self" or "vessels.*"
    pub context: String,

    /// One or more path subscriptions.
    pub subscribe: Vec<signalk_types::Subscription>,
}

impl SubscriptionSpec {
    /// Subscribe to paths on the self vessel.
    pub fn self_vessel(paths: Vec<signalk_types::Subscription>) -> Self {
        SubscriptionSpec {
            context: "vessels.self".to_string(),
            subscribe: paths,
        }
    }

    /// Subscribe to paths on ALL vessels (wildcard context).
    ///
    /// Used by plugins that need to process data from all vessels,
    /// e.g. AIS target tracking. The host filters with `context == "*"`.
    pub fn all_vessels(paths: Vec<signalk_types::Subscription>) -> Self {
        SubscriptionSpec {
            context: "*".to_string(),
            subscribe: paths,
        }
    }
}

/// Opaque handle for an active subscription. Pass to `unsubscribe()` to cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionHandle {
    pub(crate) id: u64,
}

impl SubscriptionHandle {
    pub fn new(id: u64) -> Self {
        SubscriptionHandle { id }
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

/// Callback invoked for each delta matching a subscription.
pub type DeltaCallback = Box<dyn Fn(Delta) + Send + Sync + 'static>;

/// Convenience constructor for `DeltaCallback`.
pub fn delta_callback(f: impl Fn(Delta) + Send + Sync + 'static) -> DeltaCallback {
    Box::new(f)
}

// ─── PUT handler types ──────────────────────────────────────────────────────

/// A PUT command forwarded to a plugin's handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutCommand {
    /// The full SignalK path, e.g. "steering.autopilot.target.headingTrue"
    pub path: String,

    /// The value to set.
    pub value: serde_json::Value,

    /// Optional source identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Unique request ID for correlation.
    pub request_id: String,
}

/// Result of a PUT handler invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutHandlerResult {
    /// The command was executed successfully.
    Completed,
    /// The command failed with an error message.
    Failed(String),
    /// The command is still being processed (async completion).
    Pending,
}

/// Async PUT handler function type.
pub type PutHandler = Box<
    dyn Fn(
            PutCommand,
        ) -> Pin<Box<dyn Future<Output = Result<PutHandlerResult, PluginError>> + Send>>
        + Send
        + Sync
        + 'static,
>;

/// Convenience constructor for `PutHandler` — avoids Box::new/Box::pin boilerplate.
///
/// ```rust,ignore
/// ctx.register_put_handler("vessels.self", "steering.autopilot.target.headingTrue",
///     put_handler(|cmd| async move {
///         println!("SET {} = {}", cmd.path, cmd.value);
///         Ok(PutHandlerResult::Completed)
///     })
/// ).await?;
/// ```
pub fn put_handler<F, Fut>(f: F) -> PutHandler
where
    F: Fn(PutCommand) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<PutHandlerResult, PluginError>> + Send + 'static,
{
    Box::new(move |cmd| Box::pin(f(cmd)))
}

// ─── Route types ────────────────────────────────────────────────────────────

/// An HTTP request forwarded to a plugin route handler.
/// Framework-agnostic — no axum, hyper, or express types.
#[derive(Debug, Clone)]
pub struct PluginRequest {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl PluginRequest {
    /// Parse the body as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.body)
    }
}

/// An HTTP response from a plugin route handler.
#[derive(Debug, Clone)]
pub struct PluginResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl PluginResponse {
    /// Create a JSON response.
    pub fn json(status: u16, value: &impl Serialize) -> Self {
        let body = serde_json::to_vec(value).unwrap_or_default();
        PluginResponse {
            status,
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body,
        }
    }

    /// Create a plain text response.
    pub fn text(status: u16, text: &str) -> Self {
        PluginResponse {
            status,
            headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
            body: text.as_bytes().to_vec(),
        }
    }

    /// Create an empty response with just a status code.
    pub fn empty(status: u16) -> Self {
        PluginResponse {
            status,
            headers: vec![],
            body: vec![],
        }
    }
}

/// Async route handler function type.
pub type RouteHandler = Arc<
    dyn Fn(PluginRequest) -> Pin<Box<dyn Future<Output = PluginResponse> + Send>>
        + Send
        + Sync
        + 'static,
>;

/// Convenience constructor for `RouteHandler`.
///
/// ```rust,ignore
/// router.get("/status", route_handler(|_req| async move {
///     PluginResponse::json(200, &serde_json::json!({"ok": true}))
/// }));
/// ```
pub fn route_handler<F, Fut>(f: F) -> RouteHandler
where
    F: Fn(PluginRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = PluginResponse> + Send + 'static,
{
    Arc::new(move |req| Box::pin(f(req)))
}

/// Route setup callback — called once during `register_routes`.
pub type RouterSetup = Box<dyn FnOnce(&mut dyn PluginRouter) + Send + 'static>;

/// A registered route entry (method + path + handler).
pub struct RegisteredRoute {
    pub method: String,
    pub path: String,
    pub handler: RouteHandler,
}

impl std::fmt::Debug for RegisteredRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredRoute")
            .field("method", &self.method)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Abstract HTTP router for plugin route registration.
///
/// Plugins use this to register REST endpoints without depending on
/// any HTTP framework (no axum, express, etc.).
pub trait PluginRouter: Send {
    fn get(&mut self, path: &str, handler: RouteHandler);
    fn post(&mut self, path: &str, handler: RouteHandler);
    fn put(&mut self, path: &str, handler: RouteHandler);
    fn delete(&mut self, path: &str, handler: RouteHandler);
}

/// Default `PluginRouter` implementation that collects routes into a `Vec`.
///
/// Used by `PluginContext` implementations to capture routes during setup.
#[derive(Debug)]
pub struct RouteCollector {
    routes: Vec<RegisteredRoute>,
}

impl RouteCollector {
    pub fn new() -> Self {
        RouteCollector { routes: vec![] }
    }

    /// Consume the collector and return all registered routes.
    pub fn into_routes(self) -> Vec<RegisteredRoute> {
        self.routes
    }
}

impl Default for RouteCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRouter for RouteCollector {
    fn get(&mut self, path: &str, handler: RouteHandler) {
        self.routes.push(RegisteredRoute {
            method: "GET".into(),
            path: path.into(),
            handler,
        });
    }

    fn post(&mut self, path: &str, handler: RouteHandler) {
        self.routes.push(RegisteredRoute {
            method: "POST".into(),
            path: path.into(),
            handler,
        });
    }

    fn put(&mut self, path: &str, handler: RouteHandler) {
        self.routes.push(RegisteredRoute {
            method: "PUT".into(),
            path: path.into(),
            handler,
        });
    }

    fn delete(&mut self, path: &str, handler: RouteHandler) {
        self.routes.push(RegisteredRoute {
            method: "DELETE".into(),
            path: path.into(),
            handler,
        });
    }
}

// ─── Delta input handler ────────────────────────────────────────────────────

/// Pre-store delta filter. Returns `Some(delta)` to pass through (possibly
/// modified), or `None` to drop the delta before it enters the store.
pub type DeltaInputHandler = Box<dyn Fn(Delta) -> Option<Delta> + Send + Sync + 'static>;

// ─── Webapp registration ────────────────────────────────────────────────────

/// Info needed to register a webapp from a plugin.
#[derive(Debug, Clone)]
pub struct WebAppRegistration {
    /// Human-readable display name for the webapp.
    pub display_name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Path to the directory containing static files (index.html, etc.).
    pub public_dir: PathBuf,
}

// ─── PluginContext trait ────────────────────────────────────────────────────

/// The plugin API surface — Rust equivalent of the JS bridge's `app` object.
///
/// Passed to `Plugin::start()` as `Arc<dyn PluginContext>`. All methods use
/// `&self` — the context is safe to share across spawned async tasks.
///
/// ## Method mapping (Rust ↔ JS Bridge ↔ Internal API)
///
/// | Rust method               | JS `app.*`                    | Internal API endpoint              |
/// |---------------------------|-------------------------------|------------------------------------|
/// | `get_self_path`           | `getSelfPath`                 | `GET /internal/v1/api/vessels/self/{path}` |
/// | `get_path`                | `getPath`                     | `GET /internal/v1/api/{path}`      |
/// | `handle_message`          | `handleMessage`               | `POST /internal/v1/delta`          |
/// | `subscribe`               | `subscriptionmanager.subscribe`| WebSocket subscription             |
/// | `register_put_handler`    | `registerPutHandler`          | `POST /internal/v1/handlers`       |
/// | `register_routes`         | `registerWithRouter`          | `POST /internal/v1/plugin-routes`  |
/// | `set_status` / `set_error`| `setPluginStatus`             | Plugin status reporting            |
/// | `save_options`            | `savePluginOptions`           | Config persistence                 |
/// | `read_options`            | `readPluginOptions`           | Config persistence                 |
/// | `raise_notification`      | `notify`                      | via `handle_message` (delta)       |
/// | `clear_notification`      | `notify` (state=normal)       | via `handle_message` (delta)       |
#[async_trait]
pub trait PluginContext: Send + Sync {
    // ── Data read ───────────────────────────────────────────────────────

    /// Query a path value on the self vessel.
    ///
    /// Returns the JSON value at the given path, or `None` if the path
    /// doesn't exist in the store.
    ///
    /// Equivalent to JS `app.getSelfPath('navigation.speedOverGround')`.
    async fn get_self_path(&self, path: &str) -> Result<Option<serde_json::Value>, PluginError>;

    /// Query a path value with full context.
    ///
    /// The path includes the context prefix, e.g.
    /// `"vessels.urn:mrn:signalk:uuid:abc.navigation.speedOverGround"`.
    async fn get_path(&self, full_path: &str) -> Result<Option<serde_json::Value>, PluginError>;

    /// Get all source values for a path on the self vessel.
    ///
    /// Returns a map from source_ref → value (JSON), or `None` if no data
    /// exists for the path. Useful when multiple sensors provide the same
    /// measurement (e.g. multiple GPS receivers).
    ///
    /// Equivalent to JS `app.getSelfPathSources('navigation.speedOverGround')`.
    async fn get_self_path_sources(
        &self,
        path: &str,
    ) -> Result<Option<std::collections::HashMap<String, serde_json::Value>>, PluginError> {
        // Default: not supported
        let _ = path;
        Err(PluginError::runtime(
            "get_self_path_sources not supported by this context",
        ))
    }

    // ── Data write ──────────────────────────────────────────────────────

    /// Inject a delta into the store (full round-trip: store → broadcast → subscriptions).
    ///
    /// Equivalent to JS `app.handleMessage(pluginId, delta)`.
    async fn handle_message(&self, delta: Delta) -> Result<(), PluginError>;

    // ── Subscriptions ───────────────────────────────────────────────────

    /// Subscribe to delta updates matching the given spec.
    ///
    /// The callback is invoked for each matching delta. Returns a handle
    /// that can be passed to `unsubscribe()` to cancel.
    ///
    /// Equivalent to JS `app.subscriptionmanager.subscribe(spec, [], callback, pluginId)`.
    async fn subscribe(
        &self,
        spec: SubscriptionSpec,
        callback: DeltaCallback,
    ) -> Result<SubscriptionHandle, PluginError>;

    /// Cancel an active subscription.
    async fn unsubscribe(&self, handle: SubscriptionHandle) -> Result<(), PluginError>;

    // ── PUT handlers ────────────────────────────────────────────────────

    /// Register a handler for PUT commands on a specific path.
    ///
    /// When a client PUTs to the matching path, the handler is called with
    /// a `PutCommand` and should return `PutHandlerResult`.
    ///
    /// Equivalent to JS `app.registerPutHandler(context, path, handler, pluginId)`.
    async fn register_put_handler(
        &self,
        context: &str,
        path: &str,
        handler: PutHandler,
    ) -> Result<(), PluginError>;

    // ── REST routes ─────────────────────────────────────────────────────

    /// Register custom REST endpoints under `/plugins/{plugin_id}/`.
    ///
    /// The setup callback receives a `PluginRouter` to register GET/POST/PUT/DELETE
    /// handlers. Routes are served by the main HTTP server.
    ///
    /// Equivalent to JS `app.registerWithRouter((router) => { ... })`.
    async fn register_routes(&self, setup: RouterSetup) -> Result<(), PluginError>;

    // ── Config persistence ──────────────────────────────────────────────

    /// Persist plugin options to disk (survives restarts).
    async fn save_options(&self, opts: serde_json::Value) -> Result<(), PluginError>;

    /// Read previously saved plugin options.
    async fn read_options(&self) -> Result<serde_json::Value, PluginError>;

    /// Path to the plugin's data directory (for files, caches, etc.).
    fn data_dir(&self) -> PathBuf;

    // ── Database ──────────────────────────────────────────────────────

    /// Shared SQLite database connection for persistent plugin storage.
    ///
    /// Returns the server-wide `signalk-rs.db` connection. Plugins use this
    /// to read/write their tables (e.g. `track_points`). The connection is
    /// wrapped in a `Mutex` because `rusqlite::Connection` is Send but not Sync.
    fn database(&self) -> Option<Arc<std::sync::Mutex<signalk_sqlite::rusqlite::Connection>>>;

    // ── Status ──────────────────────────────────────────────────────────

    /// Report the plugin's current status (shown in admin UI).
    fn set_status(&self, msg: &str);

    /// Report a plugin error (shown in admin UI, may trigger alerts).
    fn set_error(&self, msg: &str);

    // ── Delta pre-filtering ─────────────────────────────────────────────

    /// Register a handler that intercepts deltas before they enter the store.
    ///
    /// The handler receives each delta and returns `Some(delta)` to pass it
    /// through (possibly modified), or `None` to drop it.
    async fn register_delta_input_handler(
        &self,
        handler: DeltaInputHandler,
    ) -> Result<(), PluginError>;

    // ── Notifications ────────────────────────────────────────────────────

    /// Raise a notification at the given path.
    ///
    /// Creates a delta with `notifications.{path}` and calls `handle_message()`.
    /// Path should NOT include the `notifications.` prefix.
    ///
    /// Equivalent to JS `app.notify(path, { state, method, message })`.
    async fn raise_notification(
        &self,
        path: &str,
        notification: signalk_types::Notification,
        plugin_id: &str,
    ) -> Result<(), PluginError> {
        use signalk_types::{PathValue, Source, Update};
        let value = serde_json::to_value(&notification)
            .map_err(|e| PluginError::runtime(format!("Failed to serialize notification: {e}")))?;
        let delta = Delta::self_vessel(vec![Update::new(
            Source::plugin(plugin_id),
            vec![PathValue::new(format!("notifications.{path}"), value)],
        )]);
        self.handle_message(delta).await
    }

    /// Clear a notification (sets state to Normal with empty methods).
    ///
    /// Equivalent to raising a notification with state `Normal`.
    async fn clear_notification(&self, path: &str, plugin_id: &str) -> Result<(), PluginError> {
        self.raise_notification(
            path,
            signalk_types::Notification {
                state: signalk_types::NotificationState::Normal,
                method: vec![],
                message: String::new(),
                status: None,
            },
            plugin_id,
        )
        .await
    }

    // ── Autopilot providers ─────────────────────────────────────────────

    /// Register an autopilot provider with the server's autopilot registry.
    ///
    /// The provider becomes available via the V2 autopilot API at
    /// `/signalk/v2/api/vessels/self/autopilots/{device_id}/`. The first
    /// registered provider is automatically set as the default.
    ///
    /// Not all tiers support this — the default returns an error.
    async fn register_autopilot_provider(
        &self,
        _provider: std::sync::Arc<dyn crate::autopilot::AutopilotProvider>,
    ) -> Result<(), PluginError> {
        Err(PluginError::runtime(
            "register_autopilot_provider not supported by this context",
        ))
    }

    // ── Resource providers ─────────────────────────────────────────────

    /// Register a custom resource provider for a specific resource type.
    ///
    /// Plugins can override the default file-based storage for any of the
    /// standard resource types (waypoints, routes, notes, regions, charts).
    ///
    /// Not all tiers support this — the default returns an error.
    async fn register_resource_provider(
        &self,
        _resource_type: &str,
        _provider: Box<dyn crate::resources::ResourceProvider>,
    ) -> Result<(), PluginError> {
        Err(PluginError::runtime(
            "register_resource_provider not supported by this context",
        ))
    }

    // ── Webapp registration ──────────────────────────────────────────────

    /// Register a webapp to be served at a URL path.
    ///
    /// Tier 1 (Rust) plugins can expose a web UI by providing a directory
    /// of static files. The server will serve these at `/plugins/{plugin_id}/`.
    ///
    /// Not all tiers support this — the default returns an error.
    async fn register_webapp(&self, _info: WebAppRegistration) -> Result<(), PluginError> {
        Err(PluginError::runtime(
            "register_webapp not supported by this context",
        ))
    }
}
