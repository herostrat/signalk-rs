use serde::{Deserialize, Serialize};

/// Server configuration loaded from TOML file or environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSettings,
    pub vessel: VesselSettings,
    pub auth: AuthSettings,
    pub internal: InternalSettings,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalSettings {
    /// Transport backend: "uds" or "http"
    pub transport: String,
    /// Path for signalk-rs's UDS socket
    pub uds_rs_socket: String,
    /// Path for bridge's UDS socket
    pub uds_bridge_socket: String,
    /// Internal HTTP port (if transport = "http")
    pub http_rs_port: u16,
    /// Bridge HTTP port (if transport = "http")
    pub http_bridge_port: u16,
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
            internal: InternalSettings {
                transport: "uds".to_string(),
                uds_rs_socket: "/run/signalk/rs.sock".to_string(),
                uds_bridge_socket: "/run/signalk/bridge.sock".to_string(),
                http_rs_port: 3001,
                http_bridge_port: 3002,
            },
        }
    }
}
