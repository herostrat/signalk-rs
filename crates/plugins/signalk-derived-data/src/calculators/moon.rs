/// Derives moon phase information from current time.
///
/// Outputs:
/// - `environment.moon.phase` (ratio 0-1, 0.5 = full moon)
/// - `environment.moon.illumination` (ratio 0-1)
/// - `environment.moon.age` (days in current cycle, 0-29.5)
/// - `environment.moon.phaseName` (string: "New", "Full", "Waxing Crescent", etc.)
///
/// Uses the `moon-phase` crate.
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;
use std::time::{Duration, UNIX_EPOCH};

pub struct Moon;

impl Calculator for Moon {
    fn name(&self) -> &str {
        "moon"
    }

    fn inputs(&self) -> &[&str] {
        &["navigation.datetime"]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let datetime_str = values.get("navigation.datetime")?.as_str()?;
        let unix_ms = parse_iso8601_to_unix_ms(datetime_str)?;

        let time = if unix_ms >= 0 {
            UNIX_EPOCH + Duration::from_millis(unix_ms as u64)
        } else {
            return None;
        };

        let mp = moon_phase::MoonPhase::new(time);

        Some(vec![
            PathValue::new("environment.moon.phase", serde_json::json!(mp.phase)),
            PathValue::new(
                "environment.moon.illumination",
                serde_json::json!(mp.fraction),
            ),
            PathValue::new("environment.moon.age", serde_json::json!(mp.age)),
            PathValue::new(
                "environment.moon.phaseName",
                serde_json::json!(mp.phase_name),
            ),
        ])
    }
}

fn parse_iso8601_to_unix_ms(s: &str) -> Option<i64> {
    let dt = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    Some(dt.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moon_phase_values() {
        let calc = Moon;
        let mut values = HashMap::new();
        values.insert(
            "navigation.datetime".into(),
            serde_json::json!("2026-06-21T12:00:00.000Z"),
        );
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result.len(), 4);

        let phase = result
            .iter()
            .find(|pv| pv.path == "environment.moon.phase")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        assert!(
            (0.0..=1.0).contains(&phase),
            "Phase should be 0-1, got {phase}"
        );

        let illumination = result
            .iter()
            .find(|pv| pv.path == "environment.moon.illumination")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        assert!(
            (0.0..=1.0).contains(&illumination),
            "Illumination should be 0-1"
        );
    }

    #[test]
    fn missing_datetime() {
        let calc = Moon;
        let values = HashMap::new();
        assert!(calc.calculate(&values).is_none());
    }
}
