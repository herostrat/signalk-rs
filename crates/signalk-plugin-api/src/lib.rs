/// Canonical plugin API for signalk-rs.
///
/// This crate defines the traits and types that all plugins implement,
/// regardless of tier (Rust in-process, JS bridge, standalone binary, WASM).
///
/// # Core traits
///
/// - [`Plugin`] — the plugin lifecycle interface (metadata, start, stop)
/// - [`PluginContext`] — the server API surface provided to plugins ("app" object)
/// - [`PluginRouter`] — framework-agnostic HTTP route registration
///
/// # For plugin authors
///
/// ```rust,ignore
/// use signalk_plugin_api::{Plugin, PluginContext, PluginMetadata, PluginError};
/// use signalk_types::Delta;
/// use async_trait::async_trait;
/// use std::sync::Arc;
///
/// pub struct MyPlugin;
///
/// #[async_trait]
/// impl Plugin for MyPlugin {
///     fn metadata(&self) -> PluginMetadata {
///         PluginMetadata::new("my-plugin", "My Plugin", "Does things", "0.1.0")
///     }
///
///     async fn start(&mut self, _config: serde_json::Value, ctx: Arc<dyn PluginContext>)
///         -> Result<(), PluginError>
///     {
///         ctx.set_status("Running");
///         Ok(())
///     }
///
///     async fn stop(&mut self) -> Result<(), PluginError> {
///         Ok(())
///     }
/// }
/// ```
///
/// # Tier overview
///
/// | Tier | Transport | Crate |
/// |------|-----------|-------|
/// | 1: Rust (in-process) | Direct store access | `signalk-server` (`RustPluginContext`) |
/// | 2: JS (bridge) | HTTP over UDS | `bridge/src/app.js` |
/// | 3: Standalone binary | HTTP over UDS | `signalk-plugin-client` (`RemotePluginContext`) |
/// | 4: WASM (future) | Host calls | TBD |
pub mod context;
pub mod error;
pub mod plugin;

#[cfg(feature = "testing")]
pub mod testing;

// Re-export core traits and commonly used types.
pub use context::{
    DeltaCallback, DeltaInputHandler, PluginContext, PluginRequest, PluginResponse, PluginRouter,
    PutCommand, PutHandler, PutHandlerResult, RegisteredRoute, RouteCollector, RouteHandler,
    RouterSetup, SubscriptionHandle, SubscriptionSpec, delta_callback, put_handler, route_handler,
};
pub use error::PluginError;
pub use plugin::{Plugin, PluginMetadata, PluginStatus};
