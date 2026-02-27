/// Internal API protocol types — transport-agnostic message definitions.
///
/// These types define WHAT is communicated between signalk-rs and the Node.js Bridge.
/// HOW they're transported is determined by the InternalTransport implementation.
use serde::{Deserialize, Serialize};
use signalk_types::Delta;

// ─── Bridge → signalk-rs ─────────────────────────────────────────────────────

/// Bridge sends a delta to inject into the store (plugin's handleMessage).
/// This is a thin wrapper — the Delta type is the canonical wire format.
pub type DeltaIngest = Delta;

/// Bridge queries a path value (plugin's getSelfPath / getPath).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathQuery {
    /// Dot-separated path, e.g. "navigation.speedOverGround"
    pub path: String,
    /// Context — defaults to "vessels.self"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

impl PathQuery {
    pub fn self_path(path: impl Into<String>) -> Self {
        PathQuery {
            path: path.into(),
            context: Some("vessels.self".to_string()),
        }
    }
}

/// Response to a path query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathQueryResponse {
    pub path: String,
    pub value: Option<serde_json::Value>,
    #[serde(rename = "$source", skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// Bridge registers a PUT handler for a path pattern (plugin's registerPutHandler).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandlerRegistration {
    pub plugin_id: String,
    /// Path or pattern: "steering.autopilot.target.*"
    pub path: String,
}

/// Bridge registers custom REST routes (plugin's registerWithRouter).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRouteRegistration {
    pub plugin_id: String,
    /// URL prefix that the bridge handles: "/plugins/my-plugin"
    pub path_prefix: String,
}

/// Bridge registers itself on startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeRegistration {
    /// Shared secret token for authenticating bridge ↔ rs calls
    pub bridge_token: String,
    /// Bridge version for compatibility checks
    pub version: String,
}

// ─── signalk-rs → Bridge ─────────────────────────────────────────────────────

/// signalk-rs forwards a PUT command to the bridge (for registered handlers).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutForwardRequest {
    pub request_id: String,
    pub plugin_id: String,
    pub path: String,
    pub value: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Bridge's response to a PUT forward.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutForwardResponse {
    pub request_id: String,
    pub state: PutForwardState,
    pub status_code: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PutForwardState {
    Completed,
    Failed,
    Pending,
}

/// Plugin lifecycle event sent from signalk-rs to bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleEvent {
    pub event: LifecycleEventType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LifecycleEventType {
    Start,
    Stop,
    Restart,
}

/// Bridge reports its loaded plugins to signalk-rs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgePluginReport {
    pub plugins: Vec<BridgePluginEntry>,
}

/// A single bridge plugin entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgePluginEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub has_webapp: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_forward_state_uppercase() {
        assert_eq!(
            serde_json::to_string(&PutForwardState::Completed).unwrap(),
            "\"COMPLETED\""
        );
        assert_eq!(
            serde_json::to_string(&PutForwardState::Failed).unwrap(),
            "\"FAILED\""
        );
    }

    #[test]
    fn path_query_roundtrip() {
        let q = PathQuery::self_path("navigation.speedOverGround");
        let json = serde_json::to_string(&q).unwrap();
        let back: PathQuery = serde_json::from_str(&json).unwrap();
        assert_eq!(back.path, "navigation.speedOverGround");
        assert_eq!(back.context.as_deref(), Some("vessels.self"));
    }

    #[test]
    fn lifecycle_event_lowercase() {
        assert_eq!(
            serde_json::to_string(&LifecycleEventType::Stop).unwrap(),
            "\"stop\""
        );
    }
}
