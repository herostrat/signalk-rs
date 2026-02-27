/// Derives sun position from vessel position and current time.
///
/// Outputs:
/// - `environment.sunlight.azimuth` (rad)
/// - `environment.sunlight.altitude` (rad)
///
/// Uses the `sun` crate (port of suncalc.js).
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct SunCalc;

impl Calculator for SunCalc {
    fn name(&self) -> &str {
        "suncalc"
    }

    fn inputs(&self) -> &[&str] {
        &["navigation.position", "navigation.datetime"]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let position = values.get("navigation.position")?;
        let lat = position.get("latitude").and_then(|v| v.as_f64())?;
        let lon = position.get("longitude").and_then(|v| v.as_f64())?;

        let datetime_str = values.get("navigation.datetime")?.as_str()?;
        let unix_ms = parse_iso8601_to_unix_ms(datetime_str)?;

        let pos = sun::pos(unix_ms, lat, lon);

        Some(vec![
            PathValue::new(
                "environment.sunlight.azimuth",
                serde_json::json!(pos.azimuth),
            ),
            PathValue::new(
                "environment.sunlight.altitude",
                serde_json::json!(pos.altitude),
            ),
        ])
    }
}

/// Parse ISO 8601 datetime string to unix time in milliseconds.
fn parse_iso8601_to_unix_ms(s: &str) -> Option<i64> {
    // Handle "2026-02-27T12:00:00.000Z" format
    let dt = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    Some(dt.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sun_position_at_noon() {
        let calc = SunCalc;
        let mut values = HashMap::new();
        // Somewhere in the tropics at noon UTC
        values.insert(
            "navigation.position".into(),
            serde_json::json!({"latitude": 20.0, "longitude": 0.0}),
        );
        values.insert(
            "navigation.datetime".into(),
            serde_json::json!("2026-06-21T12:00:00.000Z"),
        );
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result.len(), 2);

        let alt = result
            .iter()
            .find(|pv| pv.path == "environment.sunlight.altitude")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        // At noon near summer solstice, sun should be high in the sky
        assert!(alt > 0.5, "Sun altitude should be high at noon, got {alt}");
    }

    #[test]
    fn missing_position() {
        let calc = SunCalc;
        let mut values = HashMap::new();
        values.insert(
            "navigation.datetime".into(),
            serde_json::json!("2026-06-21T12:00:00.000Z"),
        );
        assert!(calc.calculate(&values).is_none());
    }
}
