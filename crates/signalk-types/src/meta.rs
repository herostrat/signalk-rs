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

/// Return default metadata for well-known SignalK paths.
///
/// These defaults come from the SignalK specification and provide units,
/// descriptions, and display names for the most common sensor paths.
/// Returns `None` for unknown paths.
pub fn default_metadata(path: &str) -> Option<Metadata> {
    let (units, description, display_name) = match path {
        // Navigation
        "navigation.speedOverGround" => ("m/s", "Speed over ground", "SOG"),
        "navigation.speedThroughWater" => ("m/s", "Speed through water", "STW"),
        "navigation.courseOverGroundTrue" => ("rad", "Course over ground (true)", "COG(T)"),
        "navigation.courseOverGroundMagnetic" => ("rad", "Course over ground (magnetic)", "COG(M)"),
        "navigation.headingTrue" => ("rad", "Heading true", "HDG(T)"),
        "navigation.headingMagnetic" => ("rad", "Heading magnetic", "HDG(M)"),
        "navigation.magneticVariation" => ("rad", "Magnetic variation", "VAR"),
        "navigation.leewayAngle" => ("rad", "Leeway angle", "Leeway"),
        "navigation.attitude" => ("", "Vessel attitude (roll/pitch/yaw)", "Attitude"),
        // Depth
        "environment.depth.belowTransducer" => ("m", "Depth below transducer", "DBT"),
        "environment.depth.belowKeel" => ("m", "Depth below keel", "DBK"),
        "environment.depth.belowSurface" => ("m", "Depth below surface", "DBS"),
        "environment.depth.surfaceToTransducer" => ("m", "Surface to transducer", "S→T"),
        "environment.depth.transducerToKeel" => ("m", "Transducer to keel", "T→K"),
        // Wind
        "environment.wind.speedApparent" => ("m/s", "Apparent wind speed", "AWS"),
        "environment.wind.angleApparent" => ("rad", "Apparent wind angle", "AWA"),
        "environment.wind.speedTrue" => ("m/s", "True wind speed (ground ref)", "TWS"),
        "environment.wind.angleTrueGround" => ("rad", "True wind angle (ground ref)", "TWA"),
        "environment.wind.angleTrueWater" => ("rad", "True wind angle (water ref)", "TWA(W)"),
        "environment.wind.directionTrue" => ("rad", "True wind direction", "TWD"),
        "environment.wind.directionMagnetic" => ("rad", "Wind direction (magnetic)", "MWD"),
        "environment.wind.speedOverGround" => ("m/s", "True wind speed (ground ref)", "GWS"),
        "environment.wind.directionGround" => ("rad", "True wind direction (ground ref)", "GWD"),
        // Temperature & pressure
        "environment.outside.temperature" => ("K", "Outside air temperature", "Air"),
        "environment.outside.pressure" => ("Pa", "Atmospheric pressure", "Baro"),
        "environment.outside.humidity" => ("ratio", "Relative humidity", "Hum"),
        "environment.outside.dewPointTemperature" => ("K", "Dew point temperature", "Dew"),
        "environment.outside.airDensity" => ("kg/m³", "Air density", "ρ"),
        "environment.outside.heatIndexTemperature" => ("K", "Heat index temperature", "HI"),
        "environment.outside.apparentWindChillTemperature" => ("K", "Wind chill temperature", "WC"),
        "environment.water.temperature" => ("K", "Water temperature", "Water"),
        // Current
        "environment.current.setTrue" => ("rad", "Current set (true)", "Set(T)"),
        "environment.current.setMagnetic" => ("rad", "Current set (magnetic)", "Set(M)"),
        "environment.current.drift" => ("m/s", "Current drift", "Drift"),
        "environment.current.driftImpact" => ("ratio", "Current drift impact", "Impact"),
        // Navigation / Course
        "navigation.course.estimatedTimeOfArrival" => ("s", "ETA in seconds", "ETA"),
        "navigation.course.steerError" => ("rad", "Steering error", "XTE"),
        // VMG / Performance
        "performance.velocityMadeGood" => ("m/s", "Velocity made good", "VMG"),
        "performance.velocityMadeGoodToWaypoint" => ("m/s", "VMG to waypoint (STW)", "VMG(W)"),
        _ => return None,
    };

    Some(Metadata {
        units: Some(units.to_string()),
        description: Some(description.to_string()),
        display_name: Some(display_name.to_string()),
        ..Default::default()
    })
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
    fn default_metadata_known_path() {
        let meta = default_metadata("navigation.speedOverGround").unwrap();
        assert_eq!(meta.units.as_deref(), Some("m/s"));
        assert_eq!(meta.display_name.as_deref(), Some("SOG"));
        assert!(meta.description.is_some());
    }

    #[test]
    fn default_metadata_unknown_path() {
        assert!(default_metadata("some.unknown.path").is_none());
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
