/// Derives `navigation.course.calcValues.bearingTrackMagnetic` from true bearing
/// and magnetic variation.
///
/// bearingMagnetic = bearingTrue − magneticVariation
///
/// Output normalized to [0, 2π).
use super::{Calculator, normalize_angle};
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct BearingMagnetic;

impl Calculator for BearingMagnetic {
    fn name(&self) -> &str {
        "bearingMagnetic"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "navigation.course.calcValues.bearingTrackTrue",
            "navigation.magneticVariation",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let bearing_true = values
            .get("navigation.course.calcValues.bearingTrackTrue")?
            .as_f64()?;
        let variation = values.get("navigation.magneticVariation")?.as_f64()?;

        if !bearing_true.is_finite() || !variation.is_finite() {
            return None;
        }

        let bearing_mag = normalize_angle(bearing_true - variation);

        Some(vec![PathValue::new(
            "navigation.course.calcValues.bearingTrackMagnetic",
            serde_json::json!(bearing_mag),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn east_variation() {
        let calc = BearingMagnetic;
        let mut values = HashMap::new();
        // True bearing 90° (π/2), east variation 10° (0.174 rad)
        values.insert(
            "navigation.course.calcValues.bearingTrackTrue".into(),
            serde_json::json!(PI / 2.0),
        );
        values.insert(
            "navigation.magneticVariation".into(),
            serde_json::json!(0.174),
        );

        let result = calc.calculate(&values).unwrap();
        assert_eq!(
            result[0].path,
            "navigation.course.calcValues.bearingTrackMagnetic"
        );
        let bearing = result[0].value.as_f64().unwrap();
        let expected = PI / 2.0 - 0.174;
        assert!(
            (bearing - expected).abs() < 0.001,
            "Expected ~{expected}, got {bearing}"
        );
    }

    #[test]
    fn wraps_around() {
        let calc = BearingMagnetic;
        let mut values = HashMap::new();
        // True bearing near 0 (0.05 rad), west variation -0.2 rad
        // Result: 0.05 - (-0.2) = 0.25 rad
        values.insert(
            "navigation.course.calcValues.bearingTrackTrue".into(),
            serde_json::json!(0.05),
        );
        values.insert(
            "navigation.magneticVariation".into(),
            serde_json::json!(-0.2),
        );

        let result = calc.calculate(&values).unwrap();
        let bearing = result[0].value.as_f64().unwrap();
        assert!(
            (0.0..2.0 * PI).contains(&bearing),
            "Should be in [0, 2π), got {bearing}"
        );
        assert!(
            (bearing - 0.25).abs() < 0.001,
            "Expected ~0.25, got {bearing}"
        );
    }

    #[test]
    fn missing_variation_returns_none() {
        let calc = BearingMagnetic;
        let mut values = HashMap::new();
        values.insert(
            "navigation.course.calcValues.bearingTrackTrue".into(),
            serde_json::json!(1.0),
        );
        assert!(calc.calculate(&values).is_none());
    }
}
