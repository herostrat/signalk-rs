/// Derives sunrise and sunset times from vessel position and date.
///
/// Outputs:
/// - `environment.sunlight.times.sunrise` (ISO 8601 string)
/// - `environment.sunlight.times.sunset` (ISO 8601 string)
/// - `environment.sunlight.times.dawn` (ISO 8601 string, civil twilight)
/// - `environment.sunlight.times.dusk` (ISO 8601 string, civil twilight)
///
/// Uses the `sun` crate (port of suncalc.js).
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct SunTime;

impl Calculator for SunTime {
    fn name(&self) -> &str {
        "suntime"
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

        let sunrise_ms = sun::time_at_phase(unix_ms, sun::SunPhase::Sunrise, lat, lon, 0.0);
        let sunset_ms = sun::time_at_phase(unix_ms, sun::SunPhase::Sunset, lat, lon, 0.0);
        let dawn_ms = sun::time_at_phase(unix_ms, sun::SunPhase::Dawn, lat, lon, 0.0);
        let dusk_ms = sun::time_at_phase(unix_ms, sun::SunPhase::Dusk, lat, lon, 0.0);

        Some(vec![
            PathValue::new(
                "environment.sunlight.times.sunrise",
                serde_json::json!(unix_ms_to_iso8601(sunrise_ms)),
            ),
            PathValue::new(
                "environment.sunlight.times.sunset",
                serde_json::json!(unix_ms_to_iso8601(sunset_ms)),
            ),
            PathValue::new(
                "environment.sunlight.times.dawn",
                serde_json::json!(unix_ms_to_iso8601(dawn_ms)),
            ),
            PathValue::new(
                "environment.sunlight.times.dusk",
                serde_json::json!(unix_ms_to_iso8601(dusk_ms)),
            ),
        ])
    }
}

fn parse_iso8601_to_unix_ms(s: &str) -> Option<i64> {
    let dt = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    Some(dt.timestamp_millis())
}

fn unix_ms_to_iso8601(ms: i64) -> String {
    let dt = chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default();
    dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sunrise_before_sunset() {
        let calc = SunTime;
        let mut values = HashMap::new();
        values.insert(
            "navigation.position".into(),
            serde_json::json!({"latitude": 48.0, "longitude": 9.0}),
        );
        values.insert(
            "navigation.datetime".into(),
            serde_json::json!("2026-06-21T12:00:00.000Z"),
        );
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result.len(), 4);

        let sunrise = result
            .iter()
            .find(|pv| pv.path == "environment.sunlight.times.sunrise")
            .unwrap()
            .value
            .as_str()
            .unwrap();
        let sunset = result
            .iter()
            .find(|pv| pv.path == "environment.sunlight.times.sunset")
            .unwrap()
            .value
            .as_str()
            .unwrap();
        // Sunrise should come before sunset lexicographically (same day)
        assert!(
            sunrise < sunset,
            "sunrise {sunrise} should be before sunset {sunset}"
        );
    }
}
