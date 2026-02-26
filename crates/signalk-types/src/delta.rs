use crate::source::Source;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A SignalK delta message — the primary format for streaming data updates.
///
/// Represents incremental changes to the data model. The most commonly
/// produced SignalK format.
///
/// Spec: https://signalk.org/specification/1.7.0/doc/delta_format.html
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    /// Path to the data location, e.g. "vessels.urn:mrn:signalk:uuid:..."
    /// Defaults to "vessels.self" when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,

    /// One or more update objects from potentially different sources
    pub updates: Vec<Update>,
}

impl Delta {
    /// Create a delta for the self vessel context
    pub fn self_vessel(updates: Vec<Update>) -> Self {
        Delta {
            context: Some("vessels.self".to_string()),
            updates,
        }
    }

    /// Create a delta with explicit context
    pub fn with_context(context: impl Into<String>, updates: Vec<Update>) -> Self {
        Delta {
            context: Some(context.into()),
            updates,
        }
    }
}

/// A single update from one source at one point in time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Update {
    /// The data source that produced these values
    pub source: Source,

    /// RFC 3339 timestamp when these values were recorded
    pub timestamp: DateTime<Utc>,

    /// The actual value changes
    pub values: Vec<PathValue>,
}

impl Update {
    pub fn new(source: Source, values: Vec<PathValue>) -> Self {
        Update {
            source,
            timestamp: Utc::now(),
            values,
        }
    }

    pub fn with_timestamp(source: Source, timestamp: DateTime<Utc>, values: Vec<PathValue>) -> Self {
        Update { source, timestamp, values }
    }
}

/// A leaf-node path and its new value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathValue {
    /// Dot-separated path from context root, e.g. "navigation.speedOverGround"
    pub path: String,

    /// The value — scalar (number, string, bool, null) or object
    pub value: serde_json::Value,
}

impl PathValue {
    pub fn new(path: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        PathValue {
            path: path.into(),
            value: value.into(),
        }
    }

    pub fn null(path: impl Into<String>) -> Self {
        PathValue {
            path: path.into(),
            value: serde_json::Value::Null,
        }
    }
}

/// A PUT request sent via WebSocket (v2 style)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutRequest {
    pub context: String,
    pub request_id: String,
    pub put: PutSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PutSpec {
    pub path: String,
    pub value: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Response to a PUT request
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutResponse {
    pub context: String,
    pub request_id: String,
    pub state: PutState,
    pub status_code: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PutState {
    Completed,
    Failed,
    Pending,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Source;

    fn make_delta() -> Delta {
        Delta::self_vessel(vec![Update::with_timestamp(
            Source::nmea0183("ttyUSB0", "GP"),
            "2024-02-26T12:34:56.789Z".parse().unwrap(),
            vec![
                PathValue::new("navigation.speedOverGround", serde_json::json!(3.85)),
                PathValue::new("navigation.courseOverGroundTrue", serde_json::json!(2.971)),
            ],
        )])
    }

    #[test]
    fn delta_roundtrip() {
        let delta = make_delta();
        let json = serde_json::to_string(&delta).unwrap();
        let back: Delta = serde_json::from_str(&json).unwrap();
        assert_eq!(delta, back);
    }

    #[test]
    fn delta_matches_spec_format() {
        // Verify JSON structure matches the SignalK spec example
        let delta = make_delta();
        let json: serde_json::Value = serde_json::to_value(&delta).unwrap();

        assert_eq!(json["context"], "vessels.self");
        assert!(json["updates"].is_array());
        let update = &json["updates"][0];
        assert_eq!(update["source"]["type"], "NMEA0183");
        assert_eq!(update["source"]["talker"], "GP");
        assert!(update["values"].is_array());
        let val = &update["values"][0];
        assert_eq!(val["path"], "navigation.speedOverGround");
        assert_eq!(val["value"], 3.85);
    }

    #[test]
    fn delta_without_context_is_valid() {
        let delta = Delta {
            context: None,
            updates: vec![],
        };
        let json = serde_json::to_string(&delta).unwrap();
        assert!(!json.contains("context"));
    }

    #[test]
    fn put_state_uppercase() {
        assert_eq!(
            serde_json::to_string(&PutState::Completed).unwrap(),
            "\"COMPLETED\""
        );
    }
}
