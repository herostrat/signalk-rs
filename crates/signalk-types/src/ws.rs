/// WebSocket message types for the SignalK streaming API.
///
/// Spec: https://signalk.org/specification/1.7.0/doc/streaming_api.html
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Server → Client: sent immediately on WebSocket connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloMessage {
    /// Server software name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// SignalK API version, e.g. "1.7.0"
    pub version: String,

    /// The self vessel URI
    #[serde(rename = "self", skip_serializing_if = "Option::is_none")]
    pub self_uri: Option<String>,

    /// Server roles, e.g. ["master", "main"]
    pub roles: Vec<String>,

    /// Server time at connection
    pub timestamp: DateTime<Utc>,
}

impl HelloMessage {
    pub fn new(version: impl Into<String>, self_uri: Option<String>) -> Self {
        HelloMessage {
            name: Some("signalk-rs".to_string()),
            version: version.into(),
            self_uri,
            roles: vec!["master".to_string(), "main".to_string()],
            timestamp: Utc::now(),
        }
    }
}

/// Client → Server: subscribe to one or more paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeMessage {
    /// Vessel context, e.g. "vessels.self" or "vessels.*"
    pub context: String,

    pub subscribe: Vec<Subscription>,
}

/// A single path subscription with delivery parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    /// Path or pattern, e.g. "navigation.speedOverGround" or "navigation.*"
    pub path: String,

    /// Desired update interval in milliseconds (default: 1000)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<u64>,

    /// Message format: always "delta"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    /// Delivery policy
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<SubscriptionPolicy>,

    /// Minimum milliseconds between messages even for instant policy
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_period: Option<u64>,
}

impl Subscription {
    pub fn path(path: impl Into<String>) -> Self {
        Subscription {
            path: path.into(),
            period: None,
            format: None,
            policy: None,
            min_period: None,
        }
    }

    pub fn with_period(mut self, period_ms: u64) -> Self {
        self.period = Some(period_ms);
        self
    }

    pub fn with_policy(mut self, policy: SubscriptionPolicy) -> Self {
        self.policy = Some(policy);
        self
    }
}

/// How often/when to deliver updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubscriptionPolicy {
    /// Send immediately when value changes, respecting min_period
    Instant,
    /// Send immediately on change; resend at period if no update occurs
    #[default]
    Ideal,
    /// Send at fixed period intervals regardless of changes
    Fixed,
}

/// Client → Server: unsubscribe from paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsubscribeMessage {
    pub context: String,
    pub unsubscribe: Vec<UnsubscribeSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsubscribeSpec {
    /// Path or "*" to unsubscribe from all
    pub path: String,
}

impl UnsubscribeSpec {
    pub fn all() -> Self {
        UnsubscribeSpec {
            path: "*".to_string(),
        }
    }

    pub fn path(path: impl Into<String>) -> Self {
        UnsubscribeSpec { path: path.into() }
    }
}

/// Subscription mode set via query parameter on connection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubscribeMode {
    /// Stream only vessels.self (default)
    #[default]
    Self_,
    /// Stream all vessel updates
    All,
    /// Stream nothing until explicitly subscribed
    None,
}

impl std::str::FromStr for SubscribeMode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "all" => SubscribeMode::All,
            "none" => SubscribeMode::None,
            _ => SubscribeMode::Self_,
        })
    }
}

/// Unified inbound WebSocket message from client.
/// The server must distinguish subscribe/unsubscribe/put by presence of fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InboundMessage {
    Subscribe(SubscribeMessage),
    Unsubscribe(UnsubscribeMessage),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_message_roundtrip() {
        let hello = HelloMessage::new("1.7.0", Some("urn:mrn:signalk:uuid:abc".to_string()));
        let json = serde_json::to_string(&hello).unwrap();
        let back: HelloMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, "1.7.0");
        assert_eq!(back.roles, vec!["master", "main"]);
    }

    #[test]
    fn subscribe_message_deserializes() {
        let json = r#"{
            "context": "vessels.self",
            "subscribe": [
                {
                    "path": "navigation.speedOverGround",
                    "period": 1000,
                    "policy": "instant"
                }
            ]
        }"#;
        let msg: SubscribeMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.context, "vessels.self");
        assert_eq!(msg.subscribe[0].path, "navigation.speedOverGround");
        assert_eq!(msg.subscribe[0].period, Some(1000));
        assert_eq!(msg.subscribe[0].policy, Some(SubscriptionPolicy::Instant));
    }

    #[test]
    fn unsubscribe_all() {
        let msg = UnsubscribeMessage {
            context: "vessels.self".to_string(),
            unsubscribe: vec![UnsubscribeSpec::all()],
        };
        let json: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["unsubscribe"][0]["path"], "*");
    }
}
