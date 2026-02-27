/// Panic isolation for Tier 1 (in-process) Rust plugins.
///
/// Spawns the future as a separate tokio task. If it panics, the panic is
/// caught by the runtime and returned as a `PluginError::Runtime` instead
/// of bringing down the entire server.
use signalk_plugin_api::PluginError;
use std::future::Future;

/// Run a plugin future with panic isolation.
///
/// If the future panics, the server continues and the error is returned
/// as `PluginError::Runtime`.
pub async fn guarded<F, T>(plugin_id: &str, fut: F) -> Result<T, PluginError>
where
    F: Future<Output = Result<T, PluginError>> + Send + 'static,
    T: Send + 'static,
{
    match tokio::spawn(fut).await {
        Ok(result) => result,
        Err(join_err) if join_err.is_panic() => {
            tracing::error!(plugin = %plugin_id, "Plugin panicked — disabled");
            Err(PluginError::Runtime(format!(
                "plugin '{}' panicked",
                plugin_id
            )))
        }
        Err(join_err) => Err(PluginError::Runtime(format!(
            "plugin '{}' task failed: {}",
            plugin_id, join_err
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn guarded_ok_result_passes_through() {
        let result: Result<i32, PluginError> =
            guarded("test", async { Ok(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn guarded_err_result_passes_through() {
        let result: Result<i32, PluginError> =
            guarded("test", async { Err(PluginError::config("bad")) }).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("bad"));
    }

    #[tokio::test]
    async fn guarded_catches_panic() {
        let result: Result<i32, PluginError> = guarded("crashy-plugin", async {
            panic!("intentional test panic");
        })
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("crashy-plugin"));
        assert!(err.contains("panicked"));
    }
}
