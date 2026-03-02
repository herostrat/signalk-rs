/// Derives `navigation.course.steerError` — steering error indicator.
///
/// steerError = COG − bearing to next waypoint (signed angle in radians).
/// Positive = veering to starboard of the course line.
///
/// Only produces output when actively navigating.
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;
use std::f64::consts::PI;

pub struct SteerError;

impl Calculator for SteerError {
    fn name(&self) -> &str {
        "steerError"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "navigation.courseOverGroundTrue",
            "navigation.course.calcValues.bearingTrackTrue",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let cog = values.get("navigation.courseOverGroundTrue")?.as_f64()?;
        let bearing = values
            .get("navigation.course.calcValues.bearingTrackTrue")?
            .as_f64()?;

        // Signed difference, range [−π, π]
        let mut error = cog - bearing;
        if error > PI {
            error -= 2.0 * PI;
        } else if error < -PI {
            error += 2.0 * PI;
        }

        Some(vec![PathValue::new(
            "navigation.course.steerError",
            serde_json::json!(error),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_course() {
        let calc = SteerError;
        let mut values = HashMap::new();
        values.insert(
            "navigation.courseOverGroundTrue".into(),
            serde_json::json!(1.0),
        );
        values.insert(
            "navigation.course.calcValues.bearingTrackTrue".into(),
            serde_json::json!(1.0),
        );
        let result = calc.calculate(&values).unwrap();
        let error = result[0].value.as_f64().unwrap();
        assert!(error.abs() < 1e-10);
    }

    #[test]
    fn veering_starboard() {
        let calc = SteerError;
        let mut values = HashMap::new();
        // COG 10° more than bearing → positive error
        values.insert(
            "navigation.courseOverGroundTrue".into(),
            serde_json::json!(1.2),
        );
        values.insert(
            "navigation.course.calcValues.bearingTrackTrue".into(),
            serde_json::json!(1.0),
        );
        let result = calc.calculate(&values).unwrap();
        let error = result[0].value.as_f64().unwrap();
        assert!((error - 0.2).abs() < 1e-10);
    }

    #[test]
    fn wraps_correctly() {
        let calc = SteerError;
        let mut values = HashMap::new();
        // COG near 0, bearing near 2π → small positive error
        values.insert(
            "navigation.courseOverGroundTrue".into(),
            serde_json::json!(0.1),
        );
        values.insert(
            "navigation.course.calcValues.bearingTrackTrue".into(),
            serde_json::json!(2.0 * PI - 0.1),
        );
        let result = calc.calculate(&values).unwrap();
        let error = result[0].value.as_f64().unwrap();
        assert!((error - 0.2).abs() < 1e-10, "Expected ~0.2, got {error}");
    }
}
