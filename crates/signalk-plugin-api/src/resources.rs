/// ResourceProvider trait — pluggable resource storage backend.
///
/// Plugins can implement this to provide custom storage for SignalK resources
/// (waypoints, routes, notes, regions, charts). The server uses a default
/// file-based provider; plugins can override per resource type.
///
/// All methods take `resource_type` as a string (e.g. "waypoints", "routes").
use async_trait::async_trait;
use signalk_types::v2::ResourceQueryParams;

use crate::error::PluginError;

/// A pluggable storage backend for SignalK resources.
///
/// Implementations handle persistence for one or more resource types.
/// The server's `ResourceProviderRegistry` routes requests to the
/// appropriate provider.
#[async_trait]
pub trait ResourceProvider: Send + Sync + 'static {
    /// List resources of the given type, optionally filtered.
    async fn list(
        &self,
        resource_type: &str,
        query: &ResourceQueryParams,
    ) -> Result<serde_json::Value, PluginError>;

    /// Get a single resource by ID.
    ///
    /// Returns `Ok(None)` if the resource doesn't exist.
    async fn get(
        &self,
        resource_type: &str,
        id: &str,
    ) -> Result<Option<serde_json::Value>, PluginError>;

    /// Create a new resource. Returns the generated ID.
    async fn create(
        &self,
        resource_type: &str,
        value: serde_json::Value,
    ) -> Result<String, PluginError>;

    /// Update an existing resource by ID.
    ///
    /// Returns `Err` if the resource doesn't exist.
    async fn update(
        &self,
        resource_type: &str,
        id: &str,
        value: serde_json::Value,
    ) -> Result<(), PluginError>;

    /// Delete a resource by ID.
    ///
    /// Returns `Err` if the resource doesn't exist.
    async fn delete(&self, resource_type: &str, id: &str) -> Result<(), PluginError>;
}
