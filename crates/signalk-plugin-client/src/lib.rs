/// Rust client library for standalone (Tier 3) signalk-rs plugins.
///
/// A standalone plugin runs as its own OS process and communicates with the
/// signalk-rs server via the Internal API over Unix Domain Sockets (or HTTP).
///
/// ```no_run
/// use signalk_plugin_client::RemotePluginContext;
/// use signalk_plugin_api::PluginContext;
///
/// #[tokio::main]
/// async fn main() {
///     let ctx = RemotePluginContext::connect(
///         "/run/signalk/rs.sock",
///         "my-bridge-token",
///         "my-plugin",
///     ).await.unwrap();
///
///     // Query a path
///     let speed = ctx.get_self_path("navigation.speedOverGround").await.unwrap();
///     println!("SOG: {:?}", speed);
/// }
/// ```
use async_trait::async_trait;
use signalk_plugin_api::{
    DeltaCallback, DeltaInputHandler, PluginContext, PluginError, PutHandler, RouterSetup,
    SubscriptionHandle, SubscriptionSpec,
};
use signalk_types::Delta;
use std::path::{Path, PathBuf};
use tracing::debug;

/// Remote plugin context — communicates with signalk-rs over Internal API (UDS).
///
/// Implements a subset of `PluginContext`:
/// - `get_self_path` — query self vessel data
/// - `handle_message` — inject deltas into the store
/// - `set_status` / `set_error` — lifecycle reporting (local only)
///
/// Methods that require server-side state (subscriptions, PUT handlers, routes)
/// are not supported in the remote context and return `PluginError::Runtime`.
pub struct RemotePluginContext {
    socket_path: PathBuf,
    token: String,
    plugin_id: String,
    status: std::sync::Mutex<String>,
    error_msg: std::sync::Mutex<Option<String>>,
}

impl RemotePluginContext {
    /// Connect to a signalk-rs server via UDS.
    ///
    /// The `token` must match the bridge token configured on the server.
    pub async fn connect(
        socket_path: impl AsRef<Path>,
        token: &str,
        plugin_id: &str,
    ) -> Result<Self, PluginError> {
        let path = socket_path.as_ref().to_path_buf();

        // Verify the socket exists
        if !path.exists() {
            return Err(PluginError::config(format!(
                "UDS socket not found: {}",
                path.display()
            )));
        }

        Ok(RemotePluginContext {
            socket_path: path,
            token: token.to_string(),
            plugin_id: plugin_id.to_string(),
            status: std::sync::Mutex::new(String::new()),
            error_msg: std::sync::Mutex::new(None),
        })
    }

    /// Low-level: send an HTTP request over the UDS socket and return the body.
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<(u16, String), PluginError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::UnixStream;

        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(PluginError::Io)?;

        let body_bytes = match body {
            Some(v) => serde_json::to_vec(v)?,
            None => Vec::new(),
        };

        let request = if body_bytes.is_empty() {
            format!(
                "{method} {path} HTTP/1.1\r\n\
                 Host: localhost\r\n\
                 Authorization: Bearer {}\r\n\
                 Connection: close\r\n\
                 \r\n",
                self.token
            )
        } else {
            format!(
                "{method} {path} HTTP/1.1\r\n\
                 Host: localhost\r\n\
                 Authorization: Bearer {}\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n",
                self.token,
                body_bytes.len()
            )
        };

        stream
            .write_all(request.as_bytes())
            .await
            .map_err(PluginError::Io)?;
        if !body_bytes.is_empty() {
            stream
                .write_all(&body_bytes)
                .await
                .map_err(PluginError::Io)?;
        }

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .map_err(PluginError::Io)?;

        let response_str = String::from_utf8_lossy(&response);

        // Parse HTTP status line
        let status = response_str
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(500);

        // Extract body after \r\n\r\n
        let body = response_str
            .find("\r\n\r\n")
            .map(|i| response_str[i + 4..].to_string())
            .unwrap_or_default();

        debug!(method, path, status, "Internal API response");
        Ok((status, body))
    }
}

#[async_trait]
impl PluginContext for RemotePluginContext {
    async fn get_self_path(&self, path: &str) -> Result<Option<serde_json::Value>, PluginError> {
        let api_path = format!("/internal/v1/api/vessels/self/{path}");
        let (status, body) = self.request("GET", &api_path, None).await?;

        match status {
            200 => {
                let response: serde_json::Value = serde_json::from_str(&body)?;
                Ok(response.get("value").cloned())
            }
            404 => Ok(None),
            _ => Err(PluginError::runtime(format!(
                "get_self_path failed: HTTP {status}"
            ))),
        }
    }

    async fn get_path(&self, full_path: &str) -> Result<Option<serde_json::Value>, PluginError> {
        // Convert dot notation to URL path: "vessels.self.nav.speed" → "vessels/self/nav/speed"
        let url_path = full_path.replace('.', "/");
        let api_path = format!("/internal/v1/api/{url_path}");
        let (status, body) = self.request("GET", &api_path, None).await?;

        match status {
            200 => {
                let response: serde_json::Value = serde_json::from_str(&body)?;
                Ok(response.get("value").cloned())
            }
            404 => Ok(None),
            _ => Err(PluginError::runtime(format!(
                "get_path failed: HTTP {status}"
            ))),
        }
    }

    async fn handle_message(&self, delta: Delta) -> Result<(), PluginError> {
        let body = serde_json::to_value(&delta)?;
        let (status, _) = self
            .request("POST", "/internal/v1/delta", Some(&body))
            .await?;

        if status == 200 || status == 204 {
            Ok(())
        } else {
            Err(PluginError::runtime(format!(
                "handle_message failed: HTTP {status}"
            )))
        }
    }

    async fn subscribe(
        &self,
        _spec: SubscriptionSpec,
        _callback: DeltaCallback,
    ) -> Result<SubscriptionHandle, PluginError> {
        Err(PluginError::runtime(
            "subscribe not supported in remote context — use WebSocket client instead",
        ))
    }

    async fn unsubscribe(&self, _handle: SubscriptionHandle) -> Result<(), PluginError> {
        Err(PluginError::runtime(
            "unsubscribe not supported in remote context",
        ))
    }

    async fn register_put_handler(
        &self,
        _context: &str,
        _path: &str,
        _handler: PutHandler,
    ) -> Result<(), PluginError> {
        Err(PluginError::runtime(
            "register_put_handler not supported in remote context — use Internal API POST /internal/v1/handlers",
        ))
    }

    async fn register_routes(&self, _setup: RouterSetup) -> Result<(), PluginError> {
        Err(PluginError::runtime(
            "register_routes not supported in remote context — serve your own HTTP endpoints",
        ))
    }

    async fn save_options(&self, _opts: serde_json::Value) -> Result<(), PluginError> {
        Err(PluginError::runtime(
            "save_options not supported in remote context — use local file storage",
        ))
    }

    async fn read_options(&self) -> Result<serde_json::Value, PluginError> {
        Err(PluginError::runtime(
            "read_options not supported in remote context — use local file storage",
        ))
    }

    fn data_dir(&self) -> PathBuf {
        PathBuf::from(format!(
            "/var/lib/signalk-rs/plugin-data/{}",
            self.plugin_id
        ))
    }

    fn set_status(&self, msg: &str) {
        *self.status.lock().unwrap() = msg.to_string();
        *self.error_msg.lock().unwrap() = None;
    }

    fn set_error(&self, msg: &str) {
        *self.error_msg.lock().unwrap() = Some(msg.to_string());
    }

    async fn register_delta_input_handler(
        &self,
        _handler: DeltaInputHandler,
    ) -> Result<(), PluginError> {
        Err(PluginError::runtime(
            "register_delta_input_handler not supported in remote context",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_fails_for_missing_socket() {
        let result =
            RemotePluginContext::connect("/tmp/nonexistent-signalk-test.sock", "token", "test")
                .await;
        assert!(result.is_err());
    }

    #[test]
    fn set_status_works() {
        let ctx = RemotePluginContext {
            socket_path: PathBuf::from("/tmp/test.sock"),
            token: "test".to_string(),
            plugin_id: "test-plugin".to_string(),
            status: std::sync::Mutex::new(String::new()),
            error_msg: std::sync::Mutex::new(None),
        };

        ctx.set_status("Running");
        assert_eq!(*ctx.status.lock().unwrap(), "Running");

        ctx.set_error("Connection lost");
        assert_eq!(
            *ctx.error_msg.lock().unwrap(),
            Some("Connection lost".to_string())
        );
    }

    #[test]
    fn data_dir_uses_plugin_id() {
        let ctx = RemotePluginContext {
            socket_path: PathBuf::from("/tmp/test.sock"),
            token: "test".to_string(),
            plugin_id: "my-plugin".to_string(),
            status: std::sync::Mutex::new(String::new()),
            error_msg: std::sync::Mutex::new(None),
        };

        assert_eq!(
            ctx.data_dir(),
            PathBuf::from("/var/lib/signalk-rs/plugin-data/my-plugin")
        );
    }
}
