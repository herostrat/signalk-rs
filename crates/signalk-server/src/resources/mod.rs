/// Resource provider infrastructure.
///
/// Manages resource storage backends (file-based by default, plugin-overridable).
pub mod file_provider;
pub mod registry;

pub use file_provider::FileResourceProvider;
pub use registry::ResourceProviderRegistry;
