//! XDR (Transducer Measurement) → SignalK path mapping.
//!
//! XDR sentences carry 1-8 generic transducer measurements per sentence.
//! Each measurement has a type (char), value (f64), units (char), and name (string).
//! This module maps well-known transducer names to SignalK paths.

use crate::sentences::PathValue;
use serde_json::json;

const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;
const KELVIN_OFFSET: f64 = 273.15;
const BAR_TO_PA: f64 = 100_000.0;

/// A mapping rule: transducer type + name pattern → SignalK path + unit conversion.
struct XdrMapping {
    transducer_type: char,
    name_pattern: &'static str,
    path: &'static str,
    convert: fn(f64) -> f64,
}

fn identity(v: f64) -> f64 {
    v
}
fn celsius_to_kelvin(v: f64) -> f64 {
    v + KELVIN_OFFSET
}
fn bar_to_pascal(v: f64) -> f64 {
    v * BAR_TO_PA
}
fn deg_to_rad(v: f64) -> f64 {
    v * DEG_TO_RAD
}
fn percent_to_ratio(v: f64) -> f64 {
    v / 100.0
}

/// Name pattern matching: exact match or prefix match (pattern ends with '*').
fn name_matches(pattern: &str, name: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        name == pattern
    }
}

/// Static mapping table for well-known XDR transducer names.
///
/// Order matters — first match wins. More specific patterns before wildcards.
static MAPPINGS: &[XdrMapping] = &[
    // Pressure (Barometric)
    XdrMapping {
        transducer_type: 'P',
        name_pattern: "BARO*",
        path: "environment.outside.pressure",
        convert: bar_to_pascal,
    },
    XdrMapping {
        transducer_type: 'P',
        name_pattern: "BAR*",
        path: "environment.outside.pressure",
        convert: bar_to_pascal,
    },
    XdrMapping {
        transducer_type: 'P',
        name_pattern: "Barometer",
        path: "environment.outside.pressure",
        convert: bar_to_pascal,
    },
    // Water temperature
    XdrMapping {
        transducer_type: 'C',
        name_pattern: "WTHI",
        path: "environment.water.temperature",
        convert: celsius_to_kelvin,
    },
    XdrMapping {
        transducer_type: 'C',
        name_pattern: "WATER*",
        path: "environment.water.temperature",
        convert: celsius_to_kelvin,
    },
    XdrMapping {
        transducer_type: 'C',
        name_pattern: "ENV_WATER_T",
        path: "environment.water.temperature",
        convert: celsius_to_kelvin,
    },
    // Air temperature
    XdrMapping {
        transducer_type: 'C',
        name_pattern: "ENV_OUTAIR*",
        path: "environment.outside.temperature",
        convert: celsius_to_kelvin,
    },
    XdrMapping {
        transducer_type: 'C',
        name_pattern: "AIR*",
        path: "environment.outside.temperature",
        convert: celsius_to_kelvin,
    },
    XdrMapping {
        transducer_type: 'C',
        name_pattern: "TempAir",
        path: "environment.outside.temperature",
        convert: celsius_to_kelvin,
    },
    // Pitch (Angular displacement)
    XdrMapping {
        transducer_type: 'A',
        name_pattern: "PTCH",
        path: "ATTITUDE_PITCH",
        convert: deg_to_rad,
    },
    XdrMapping {
        transducer_type: 'A',
        name_pattern: "PITCH",
        path: "ATTITUDE_PITCH",
        convert: deg_to_rad,
    },
    XdrMapping {
        transducer_type: 'A',
        name_pattern: "Pitch",
        path: "ATTITUDE_PITCH",
        convert: deg_to_rad,
    },
    // Roll (Angular displacement)
    XdrMapping {
        transducer_type: 'A',
        name_pattern: "ROLL",
        path: "ATTITUDE_ROLL",
        convert: deg_to_rad,
    },
    XdrMapping {
        transducer_type: 'A',
        name_pattern: "Roll",
        path: "ATTITUDE_ROLL",
        convert: deg_to_rad,
    },
    // Humidity
    XdrMapping {
        transducer_type: 'H',
        name_pattern: "ENV_OUTSIDE_H*",
        path: "environment.outside.humidity",
        convert: percent_to_ratio,
    },
    XdrMapping {
        transducer_type: 'H',
        name_pattern: "HUM*",
        path: "environment.outside.humidity",
        convert: percent_to_ratio,
    },
    XdrMapping {
        transducer_type: 'H',
        name_pattern: "Humidity",
        path: "environment.outside.humidity",
        convert: percent_to_ratio,
    },
    // Battery voltage
    XdrMapping {
        transducer_type: 'U',
        name_pattern: "BATT*",
        path: "electrical.batteries.main.voltage",
        convert: identity,
    },
];

/// Convert XDR measurements to SignalK path-value pairs.
///
/// Pitch and roll are combined into a single `navigation.attitude` object.
pub fn from_xdr(xdr: &nmea::sentences::XdrData) -> Vec<PathValue> {
    let mut out = Vec::new();
    let mut pitch_rad: Option<f64> = None;
    let mut roll_rad: Option<f64> = None;

    for m in &xdr.measurements {
        let Some(ttype) = m.transducer_type else {
            continue;
        };
        let Some(value) = m.value else { continue };
        let name = m.name.as_str();

        // Find first matching mapping
        let Some(mapping) = MAPPINGS
            .iter()
            .find(|r| r.transducer_type == ttype && name_matches(r.name_pattern, name))
        else {
            continue;
        };

        let converted = (mapping.convert)(value);

        // Collect attitude components for combined output
        match mapping.path {
            "ATTITUDE_PITCH" => pitch_rad = Some(converted),
            "ATTITUDE_ROLL" => roll_rad = Some(converted),
            path => {
                out.push(PathValue::new(path, json!(converted)));
            }
        }
    }

    // Emit combined attitude object if we have pitch or roll
    if pitch_rad.is_some() || roll_rad.is_some() {
        let mut attitude = serde_json::Map::new();
        if let Some(p) = pitch_rad {
            attitude.insert("pitch".into(), json!(p));
        }
        if let Some(r) = roll_rad {
            attitude.insert("roll".into(), json!(r));
        }
        out.push(PathValue::new(
            "navigation.attitude",
            serde_json::Value::Object(attitude),
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nmea::sentences::xdr::{XdrData, XdrMeasurement};

    fn make_measurement(ttype: char, value: f64, units: char, name: &str) -> XdrMeasurement {
        XdrMeasurement {
            transducer_type: Some(ttype),
            value: Some(value),
            units: Some(units),
            name: name.parse().unwrap(),
        }
    }

    #[test]
    fn barometric_pressure() {
        let xdr = XdrData {
            measurements: [make_measurement('P', 1.013, 'B', "BARO")].into_iter().collect(),
        };
        let values = from_xdr(&xdr);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].path, "environment.outside.pressure");
        assert!((values[0].value.as_f64().unwrap() - 101_300.0).abs() < 1.0);
    }

    #[test]
    fn water_temperature() {
        let xdr = XdrData {
            measurements: [make_measurement('C', 18.5, 'C', "WTHI")].into_iter().collect(),
        };
        let values = from_xdr(&xdr);
        assert_eq!(values[0].path, "environment.water.temperature");
        assert!((values[0].value.as_f64().unwrap() - 291.65).abs() < 1e-6);
    }

    #[test]
    fn air_temperature() {
        let xdr = XdrData {
            measurements: [make_measurement('C', 22.0, 'C', "TempAir")].into_iter().collect(),
        };
        let values = from_xdr(&xdr);
        assert_eq!(values[0].path, "environment.outside.temperature");
        assert!((values[0].value.as_f64().unwrap() - 295.15).abs() < 1e-6);
    }

    #[test]
    fn pitch_and_roll_combined() {
        let xdr = XdrData {
            measurements: [
                make_measurement('A', 5.2, 'D', "PTCH"),
                make_measurement('A', -1.3, 'D', "ROLL"),
            ]
            .into_iter()
            .collect(),
        };
        let values = from_xdr(&xdr);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].path, "navigation.attitude");
        let pitch = values[0].value["pitch"].as_f64().unwrap();
        assert!((pitch - 5.2 * DEG_TO_RAD).abs() < 1e-8);
        let roll = values[0].value["roll"].as_f64().unwrap();
        assert!((roll - (-1.3) * DEG_TO_RAD).abs() < 1e-8);
    }

    #[test]
    fn pitch_only() {
        let xdr = XdrData {
            measurements: [make_measurement('A', 3.0, 'D', "PITCH")].into_iter().collect(),
        };
        let values = from_xdr(&xdr);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].path, "navigation.attitude");
        assert!(values[0].value.get("pitch").is_some());
        assert!(values[0].value.get("roll").is_none());
    }

    #[test]
    fn humidity() {
        let xdr = XdrData {
            measurements: [make_measurement('H', 65.0, 'P', "Humidity")].into_iter().collect(),
        };
        let values = from_xdr(&xdr);
        assert_eq!(values[0].path, "environment.outside.humidity");
        assert!((values[0].value.as_f64().unwrap() - 0.65).abs() < 1e-6);
    }

    #[test]
    fn multiple_measurements() {
        let xdr = XdrData {
            measurements: [
                make_measurement('P', 1.013, 'B', "BARO"),
                make_measurement('C', 22.0, 'C', "TempAir"),
                make_measurement('H', 55.0, 'P', "Humidity"),
            ]
            .into_iter()
            .collect(),
        };
        let values = from_xdr(&xdr);
        assert_eq!(values.len(), 3);
        assert!(values.iter().any(|v| v.path == "environment.outside.pressure"));
        assert!(values.iter().any(|v| v.path == "environment.outside.temperature"));
        assert!(values.iter().any(|v| v.path == "environment.outside.humidity"));
    }

    #[test]
    fn unknown_transducer_ignored() {
        let xdr = XdrData {
            measurements: [make_measurement('Z', 42.0, 'X', "UNKNOWN")]
                .into_iter()
                .collect(),
        };
        let values = from_xdr(&xdr);
        assert!(values.is_empty());
    }

    #[test]
    fn wildcard_name_matching() {
        assert!(name_matches("BARO*", "BARO"));
        assert!(name_matches("BARO*", "BAROMETER"));
        assert!(!name_matches("BARO*", "BAR"));
        assert!(name_matches("WTHI", "WTHI"));
        assert!(!name_matches("WTHI", "WTHIX"));
    }

    #[test]
    fn battery_voltage() {
        let xdr = XdrData {
            measurements: [make_measurement('U', 12.6, 'V', "BATT1")]
                .into_iter()
                .collect(),
        };
        let values = from_xdr(&xdr);
        assert_eq!(values[0].path, "electrical.batteries.main.voltage");
        assert!((values[0].value.as_f64().unwrap() - 12.6).abs() < 1e-6);
    }
}
