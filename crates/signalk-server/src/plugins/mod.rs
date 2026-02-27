/// Plugin infrastructure for signalk-rs.
///
/// This module provides the Tier 1 (in-process Rust) plugin host:
///
/// - [`host::RustPluginContext`] — implements `PluginContext` with direct store access
/// - [`manager::PluginManager`] — orchestrates plugin lifecycle (register, start, stop)
/// - [`routes::PluginRouteTable`] — dynamic REST route dispatch for plugin endpoints
/// - [`isolation::guarded`] — panic isolation for plugin futures
pub mod delta_filter;
pub mod host;
pub mod isolation;
pub mod manager;
pub mod routes;
