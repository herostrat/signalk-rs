use serde::{Deserialize, Serialize};

use crate::history::HistoryConfig;

/// Server configuration — bootstrap settings that stay constant at runtime.
///
/// Does NOT contain vessel, plugins, source_priorities, or source_ttls.
/// Those live in `SeedConfig` and are persisted to SQLite on first start.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(default)]
pub struct ServerConfig {
    pub server: ServerSettings,
    pub auth: AuthSettings,
    pub internal: InternalSettings,
    /// History subsystem configuration (time-series recording + retention).
    #[serde(default)]
    pub history: HistoryConfig,
    /// Data directory for persistent storage (applicationData, plugin data, etc.)
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
    /// Path to node_modules directory containing webapps and bridge plugins
    #[serde(default = "default_modules_dir")]
    pub modules_dir: String,
}

/// Optional seed values — written to SQLite on first start, then ignored.
///
/// Remove from TOML after first start, or leave them (they won't overwrite DB).
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SeedConfig {
    #[serde(default)]
    pub vessel: VesselSettings,
    /// Source priority configuration: source_ref → priority (lower = higher).
    #[serde(default)]
    pub source_priorities: std::collections::HashMap<String, u16>,
    /// Source TTL configuration: source_ref → max value age in seconds.
    #[serde(default)]
    pub source_ttls: std::collections::HashMap<String, u64>,
    /// Plugin configurations — everything is a plugin.
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,
}

/// Configuration for a single plugin instance.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ServerSettings {
    /// Public HTTP+WS port (default: 3000)
    pub port: u16,
    /// Bind address (default: "0.0.0.0")
    pub host: String,
    /// Server name reported in discovery
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VesselSettings {
    /// Vessel UUID — generated on first start if empty.
    /// Only used as seed; the authoritative UUID lives in ConfigStore (SQLite).
    #[serde(default)]
    pub uuid: String,
    pub name: Option<String>,
    pub mmsi: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
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
            history: HistoryConfig::default(),
        }
    }
}

/// Combined struct used for TOML parsing and config reference schema generation.
/// Deserializes into both `ServerConfig` (bootstrap) and `SeedConfig` (first-start seed data).
#[derive(Deserialize, schemars::JsonSchema)]
pub struct RawConfig {
    #[serde(flatten)]
    pub server: ServerConfig,
    #[serde(flatten)]
    pub seed: SeedConfig,
}
