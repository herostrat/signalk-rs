/// Plugin trait and metadata — the core abstraction every plugin implements.
///
/// A plugin is a self-contained unit of functionality. It can be a data source
/// (NMEA 0183 TCP listener), a data processor (anchor alarm), or anything in between.
///
/// # Lifecycle
///
/// ```text
/// new() → metadata() → start(config, ctx) → [running] → stop()
///                           │                    │
///                           │    ctx.subscribe()  │
///                           │    ctx.handle_message()
///                           │    ctx.register_put_handler()
///                           │    ctx.register_routes()
///                           │    ctx.set_status()
///                           │    ...
/// ```
use async_trait::async_trait;
use std::sync::Arc;

use crate::context::PluginContext;
use crate::error::PluginError;

/// Static metadata describing a plugin.
#[derive(Debug, Clone)]
pub struct PluginMetadata {
    /// Unique identifier, e.g. "nmea0183-tcp" or "anchor-alarm".
    /// Used in config, REST paths, logging.
    pub id: String,

    /// Human-readable display name.
    pub name: String,

    /// Short description of what the plugin does.
    pub description: String,

    /// SemVer version string.
    pub version: String,
}

impl PluginMetadata {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        PluginMetadata {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            version: version.into(),
        }
    }
}

/// Runtime status of a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    Stopped,
    Starting,
    Running(String),
    Error(String),
    Stopping,
}

impl std::fmt::Display for PluginStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginStatus::Stopped => write!(f, "Stopped"),
            PluginStatus::Starting => write!(f, "Starting"),
            PluginStatus::Running(msg) => write!(f, "Running: {msg}"),
            PluginStatus::Error(msg) => write!(f, "Error: {msg}"),
            PluginStatus::Stopping => write!(f, "Stopping"),
        }
    }
}

/// The core plugin trait.
///
/// Every plugin — whether it's an input provider (NMEA), a data processor
/// (anchor alarm), or a side-effect producer (InfluxDB writer) — implements
/// this trait.
///
/// Plugins receive an `Arc<dyn PluginContext>` on start, which provides the
/// full server API surface (the Rust equivalent of the JS `app` object).
#[async_trait]
pub trait Plugin: Send + Sync + 'static {
    /// Return static metadata for this plugin.
    fn metadata(&self) -> PluginMetadata;

    /// Return a JSON Schema describing this plugin's configuration.
    /// Returns `None` if the plugin has no configurable options.
    fn schema(&self) -> Option<serde_json::Value> {
        None
    }

    /// Start the plugin with the given configuration.
    ///
    /// The `ctx` provides the full plugin API surface. Plugins should store
    /// the Arc and use it to subscribe, emit deltas, register routes, etc.
    ///
    /// Long-running work (TCP listeners, periodic tasks) should be spawned
    /// as async tasks — `start` should return promptly.
    async fn start(
        &mut self,
        config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError>;

    /// Stop the plugin and release all resources.
    ///
    /// Subscriptions and registered handlers are automatically cleaned up
    /// by the plugin manager after stop returns.
    async fn stop(&mut self) -> Result<(), PluginError>;
}
