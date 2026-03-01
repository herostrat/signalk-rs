/// Autopilot provider registry and V2 API support.
///
/// The `AutopilotManager` holds all registered `AutopilotProvider` implementations.
/// Provider plugins register via `PluginContext::register_autopilot_provider()`.
/// The server's V2 autopilot routes delegate commands to the manager.
pub mod manager;

pub use manager::AutopilotManager;
