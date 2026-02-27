/// Plugin error types.
///
/// All fallible plugin API operations return `Result<T, PluginError>`.
/// Plugins should use the appropriate variant to signal what went wrong.
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluginError {
    /// Configuration is invalid or missing required fields.
    #[error("invalid configuration: {0}")]
    Config(String),

    /// An I/O or network operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// A runtime error during plugin execution.
    #[error("runtime error: {0}")]
    Runtime(String),

    /// The plugin is not in the expected state for the requested operation.
    #[error("invalid state: {0}")]
    InvalidState(String),

    /// A subscription or handler registration was rejected.
    #[error("registration rejected: {0}")]
    Registration(String),

    /// Catch-all for other errors.
    #[error("{0}")]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

impl PluginError {
    pub fn config(msg: impl Into<String>) -> Self {
        PluginError::Config(msg.into())
    }

    pub fn runtime(msg: impl Into<String>) -> Self {
        PluginError::Runtime(msg.into())
    }
}
