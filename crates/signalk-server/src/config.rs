use serde::{Deserialize, Serialize};

/// Server configuration loaded from TOML file or environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSettings,
    pub vessel: VesselSettings,
    pub auth: AuthSettings,
    pub internal: InternalSettings,
    /// Data directory for persistent storage (applicationData, plugin data, etc.)
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
    /// Path to node_modules directory containing webapps and bridge plugins
    #[serde(default = "default_modules_dir")]
    pub modules_dir: String,
    /// Source priority configuration: source_ref → priority (lower = higher).
    ///
    /// When multiple data sources provide the same path, the one with the
    /// lowest priority number wins. Sources without an entry default to 100.
    ///
    /// ```toml
    /// [source_priorities]
    /// "gps.GP" = 10
    /// "ais" = 50
    /// "simulator" = 200
    /// ```
    #[serde(default)]
    pub source_priorities: std::collections::HashMap<String, u16>,
    /// Plugin configurations — everything is a plugin.
    ///
    /// ```toml
    /// [[plugins]]
    /// id = "nmea0183-tcp"
    /// config = { addr = "0.0.0.0:10110", source_label = "gps" }
    ///
    /// [[plugins]]
    /// id = "nmea0183-serial"
    /// config = { path = "/dev/ttyUSB0", baud_rate = 4800, source_label = "depth" }
    ///
    /// [[plugins]]
    /// id = "anchor-alarm"
    /// enabled = false
    /// config = { radius = 75.0 }
    /// ```
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,
}

/// Configuration for a single plugin instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Plugin identifier, e.g. "nmea0183-tcp", "anchor-alarm".
    pub id: String,
    /// Whether this plugin is enabled (default: true).
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Plugin-specific configuration (passed to `Plugin::start`).
    #[serde(default = "default_plugin_config")]
    pub config: serde_json::Value,
}

fn default_enabled() -> bool {
    true
}

fn default_plugin_config() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSettings {
    /// Public HTTP+WS port (default: 3000)
    pub port: u16,
    /// Bind address (default: "0.0.0.0")
    pub host: String,
    /// Server name reported in discovery
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VesselSettings {
    /// Vessel UUID — generated on first start if empty
    pub uuid: String,
    pub name: Option<String>,
    pub mmsi: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSettings {
    /// JWT signing secret — MUST be set in production
    pub jwt_secret: String,
    /// Token validity in seconds (default: 604800 = 7 days)
    pub token_ttl_secs: u64,
    /// Admin credentials (simplified — real impl would use a user DB)
    pub admin_user: String,
    pub admin_password_hash: String,
}

fn default_data_dir() -> String {
    "/var/lib/signalk-rs".to_string()
}

fn default_modules_dir() -> String {
    "/var/lib/signalk-rs/node_modules".to_string()
}

fn default_http_rs_port() -> u16 {
    3001
}
fn default_http_bridge_port() -> u16 {
    3002
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalSettings {
    /// Transport backend: "uds" or "http"
    pub transport: String,
    /// Path for signalk-rs's UDS socket
    pub uds_rs_socket: String,
    /// Path for bridge's UDS socket
    pub uds_bridge_socket: String,
    /// Internal HTTP port (if transport = "http")
    #[serde(default = "default_http_rs_port")]
    pub http_rs_port: u16,
    /// Bridge HTTP port (if transport = "http")
    #[serde(default = "default_http_bridge_port")]
    pub http_bridge_port: u16,
    /// Shared secret between signalk-rs and bridge.
    /// If empty, a random token is generated at startup and printed to stderr.
    #[serde(default)]
    pub bridge_token: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            server: ServerSettings {
                port: 3000,
                host: "0.0.0.0".to_string(),
                name: "signalk-rs".to_string(),
            },
            vessel: VesselSettings {
                uuid: format!("urn:mrn:signalk:uuid:{}", uuid::Uuid::new_v4()),
                name: None,
                mmsi: None,
            },
            auth: AuthSettings {
                jwt_secret: uuid::Uuid::new_v4().to_string(),
                token_ttl_secs: 604800,
                admin_user: "admin".to_string(),
                admin_password_hash: String::new(), // set by user
            },
            data_dir: default_data_dir(),
            modules_dir: default_modules_dir(),
            internal: InternalSettings {
                transport: "uds".to_string(),
                uds_rs_socket: "/run/signalk/rs.sock".to_string(),
                uds_bridge_socket: "/run/signalk/bridge.sock".to_string(),
                http_rs_port: 3001,
                http_bridge_port: 3002,
                bridge_token: String::new(),
            },
            source_priorities: std::collections::HashMap::new(),
            plugins: Vec::new(),
        }
    }
}
