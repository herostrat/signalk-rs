use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Identifies the origin of a data value.
///
/// The `label` and `type_` fields are always present.
/// Additional fields depend on the source type:
/// - NMEA0183: `talker` (e.g. "GP")
/// - NMEA2000: `src` (source address), `pgn` (parameter group number)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// Human-readable label for the source (e.g. "ttyUSB0", "N2000-01")
    pub label: String,

    /// Source type: "NMEA0183", "NMEA2000", "Plugin", "SignalK", "Internal"
    #[serde(rename = "type")]
    pub type_: String,

    /// Additional type-specific fields (talker, src, pgn, etc.)
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Source {
    pub fn nmea0183(label: impl Into<String>, talker: impl Into<String>) -> Self {
        let mut extra = HashMap::new();
        extra.insert(
            "talker".to_string(),
            serde_json::Value::String(talker.into()),
        );
        Source {
            label: label.into(),
            type_: "NMEA0183".to_string(),
            extra,
        }
    }

    pub fn nmea2000(label: impl Into<String>, src: u8, pgn: u32) -> Self {
        let mut extra = HashMap::new();
        extra.insert(
            "src".to_string(),
            serde_json::Value::String(src.to_string()),
        );
        extra.insert("pgn".to_string(), serde_json::Value::Number(pgn.into()));
        Source {
            label: label.into(),
            type_: "NMEA2000".to_string(),
            extra,
        }
    }

    pub fn plugin(plugin_id: impl Into<String>) -> Self {
        Source {
            label: plugin_id.into(),
            type_: "Plugin".to_string(),
            extra: HashMap::new(),
        }
    }

    pub fn internal() -> Self {
        Source {
            label: "signalk-rs".to_string(),
            type_: "Internal".to_string(),
            extra: HashMap::new(),
        }
    }
}

/// Registry key for a source, used in the full data model's `$source` field.
/// Format: "{label}.{type_specific}" e.g. "ttyUSB0.GP"
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceRef(pub String);

impl SourceRef {
    pub fn new(s: impl Into<String>) -> Self {
        SourceRef(s.into())
    }
}

impl std::fmt::Display for SourceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_nmea0183_roundtrip() {
        let src = Source::nmea0183("ttyUSB0", "GP");
        let json = serde_json::to_string(&src).unwrap();
        let back: Source = serde_json::from_str(&json).unwrap();
        assert_eq!(src, back);
        assert_eq!(back.type_, "NMEA0183");
        assert_eq!(back.extra["talker"], "GP");
    }

    #[test]
    fn source_nmea2000_roundtrip() {
        let src = Source::nmea2000("can0", 115, 128267);
        let json = serde_json::to_string(&src).unwrap();
        let back: Source = serde_json::from_str(&json).unwrap();
        assert_eq!(back.type_, "NMEA2000");
        assert_eq!(back.extra["pgn"], 128267);
    }
}
