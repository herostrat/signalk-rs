/// Transport abstraction for the signalk-rs ↔ Bridge internal API.
///
/// Implementations:
/// - `UdsTransport` (default): HTTP over Unix Domain Sockets
/// - `HttpTransport` (feature "http"): plain TCP HTTP (for Docker/remote bridge)
/// - Future: `ShmTransport`, `IoUringTransport`
///
/// Both sides implement this trait with their own server+client halves.
use async_trait::async_trait;
use signalk_types::Delta;

use crate::protocol::{
    BridgeRegistration, HandlerRegistration, LifecycleEvent, PathQuery, PathQueryResponse,
    PluginRouteRegistration, PutForwardRequest, PutForwardResponse,
};

/// The server-side transport (signalk-rs side).
///
/// Listens for incoming requests from the bridge and provides
/// methods to push events back to the bridge.
#[async_trait]
pub trait ServerTransport: Send + Sync + 'static {
    /// Accept and process one request from the bridge.
    /// Implementations should loop on this in a background task.
    async fn accept(&self) -> anyhow::Result<InboundRequest>;

    /// Send a PUT forward request to the bridge.
    async fn send_put(&self, req: PutForwardRequest) -> anyhow::Result<PutForwardResponse>;

    /// Send a lifecycle event to the bridge.
    async fn send_lifecycle(&self, event: LifecycleEvent) -> anyhow::Result<()>;

    /// Forward a plugin HTTP request to the bridge (reverse proxy).
    async fn proxy_plugin_request(
        &self,
        plugin_id: &str,
        method: &str,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> anyhow::Result<ProxyResponse>;
}

/// The client-side transport (bridge side, used by the Node.js bridge via FFI or HTTP).
///
/// For the Node.js bridge this is implemented in TypeScript using the
/// same socket paths. This Rust trait documents the expected interface.
#[async_trait]
pub trait ClientTransport: Send + Sync + 'static {
    /// Send a delta to signalk-rs (plugin's handleMessage).
    async fn send_delta(&self, delta: Delta) -> anyhow::Result<()>;

    /// Query a path value from signalk-rs (plugin's getSelfPath).
    async fn query_path(&self, query: PathQuery) -> anyhow::Result<PathQueryResponse>;

    /// Register a PUT handler with signalk-rs.
    async fn register_handler(&self, reg: HandlerRegistration) -> anyhow::Result<()>;

    /// Register custom plugin REST routes with signalk-rs.
    async fn register_plugin_routes(&self, reg: PluginRouteRegistration) -> anyhow::Result<()>;

    /// Register the bridge itself with signalk-rs.
    async fn register_bridge(&self, reg: BridgeRegistration) -> anyhow::Result<()>;
}

/// Inbound request received by the server transport.
#[derive(Debug)]
pub enum InboundRequest {
    /// Bridge injects a delta into the store
    Delta(Delta),
    /// Bridge queries a path
    Query(PathQuery),
    /// Bridge registers a PUT handler
    RegisterHandler(HandlerRegistration),
    /// Bridge registers plugin routes
    RegisterRoutes(PluginRouteRegistration),
    /// Bridge registers itself on startup
    RegisterBridge(BridgeRegistration),
}

/// Response from a plugin HTTP proxy call.
#[derive(Debug)]
pub struct ProxyResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

/// Configuration for the transport layer.
#[derive(Debug, Clone)]
pub struct TransportConfig {
    pub backend: TransportBackend,
    pub uds: UdsConfig,
    pub http: HttpConfig,
    /// Shared secret token for authenticating bridge ↔ rs calls
    pub bridge_token: String,
}

#[derive(Debug, Clone, Default)]
pub enum TransportBackend {
    #[default]
    Uds,
    Http,
}

#[derive(Debug, Clone)]
pub struct UdsConfig {
    /// signalk-rs listens here for bridge requests
    pub rs_socket: std::path::PathBuf,
    /// Bridge listens here for rs requests
    pub bridge_socket: std::path::PathBuf,
}

impl Default for UdsConfig {
    fn default() -> Self {
        UdsConfig {
            rs_socket: "/run/signalk/rs.sock".into(),
            bridge_socket: "/run/signalk/bridge.sock".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub rs_port: u16,
    pub bridge_port: u16,
    pub host: String,
}

impl Default for HttpConfig {
    fn default() -> Self {
        HttpConfig {
            rs_port: 3001,
            bridge_port: 3002,
            host: "127.0.0.1".to_string(),
        }
    }
}

impl Default for TransportConfig {
    fn default() -> Self {
        TransportConfig {
            backend: TransportBackend::Uds,
            uds: UdsConfig::default(),
            http: HttpConfig::default(),
            bridge_token: uuid::Uuid::new_v4().to_string(),
        }
    }
}
