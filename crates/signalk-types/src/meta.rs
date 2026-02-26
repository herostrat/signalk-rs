use serde::{Deserialize, Serialize};

/// Metadata for a SignalK value — drives UI display and alarm zones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    /// Human-readable description of what this value represents
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// SI unit string (e.g. "m/s", "K", "Pa", "rad", "ratio")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,

    /// Short label for gauges (e.g. "SOG")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,

    /// Long descriptive name (e.g. "Speed over Ground")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_name: Option<String>,

    /// How long a value is valid after its timestamp, in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,

    /// Expected minimum operational value (for gauge display)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_value: Option<f64>,

    /// Expected maximum operational value (for gauge display)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_value: Option<f64>,

    /// Alarm zones — trigger notifications when value falls within a zone
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<Zone>,
}

/// An alarm zone with lower/upper bounds and severity state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Zone {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lower: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub upper: Option<f64>,

    pub state: ZoneState,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZoneState {
    Nominal,
    Alert,
    Warn,
    Alarm,
    Emergency,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_roundtrip() {
        let meta = Metadata {
            description: Some("Speed over ground".to_string()),
            units: Some("m/s".to_string()),
            display_name: Some("SOG".to_string()),
            min_value: Some(0.0),
            max_value: Some(30.0),
            zones: vec![
                Zone {
                    lower: None,
                    upper: Some(20.0),
                    state: ZoneState::Nominal,
                    message: None,
                },
                Zone {
                    lower: Some(20.0),
                    upper: Some(25.0),
                    state: ZoneState::Alert,
                    message: None,
                },
                Zone {
                    lower: Some(25.0),
                    upper: None,
                    state: ZoneState::Alarm,
                    message: Some("Over speed!".to_string()),
                },
            ],
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&meta).unwrap();
        let back: Metadata = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, back);
    }

    #[test]
    fn zone_state_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&ZoneState::Nominal).unwrap(),
            "\"nominal\""
        );
        assert_eq!(
            serde_json::to_string(&ZoneState::Emergency).unwrap(),
            "\"emergency\""
        );
    }
}
