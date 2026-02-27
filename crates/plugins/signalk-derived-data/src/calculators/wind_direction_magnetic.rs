/// Derives `environment.wind.directionMagnetic` from true wind direction minus variation.
///
/// directionMagnetic = directionTrue − variation
///
/// Matches upstream windDirectionMagnetic.js.
use super::{Calculator, normalize_angle};
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct WindDirectionMagnetic;

impl Calculator for WindDirectionMagnetic {
    fn name(&self) -> &str {
        "windDirectionMagnetic"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "environment.wind.directionTrue",
            "navigation.magneticVariation",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let dir_true = values.get("environment.wind.directionTrue")?.as_f64()?;
        let variation = values.get("navigation.magneticVariation")?.as_f64()?;

        let dir_mag = normalize_angle(dir_true - variation);

        Some(vec![PathValue::new(
            "environment.wind.directionMagnetic",
            serde_json::json!(dir_mag),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn basic() {
        let calc = WindDirectionMagnetic;
        let mut values = HashMap::new();
        values.insert(
            "environment.wind.directionTrue".into(),
            serde_json::json!(1.5),
        );
        values.insert(
            "navigation.magneticVariation".into(),
            serde_json::json!(0.1),
        );
        let result = calc.calculate(&values).unwrap();
        let dir = result[0].value.as_f64().unwrap();
        assert!((dir - 1.4).abs() < 1e-10);
    }

    #[test]
    fn wraps_negative() {
        let calc = WindDirectionMagnetic;
        let mut values = HashMap::new();
        values.insert(
            "environment.wind.directionTrue".into(),
            serde_json::json!(0.05),
        );
        values.insert(
            "navigation.magneticVariation".into(),
            serde_json::json!(0.1),
        );
        let result = calc.calculate(&values).unwrap();
        let dir = result[0].value.as_f64().unwrap();
        assert!(
            (0.0..2.0 * PI).contains(&dir),
            "Should wrap to [0, 2π), got {dir}"
        );
        // 0.05 - 0.1 = -0.05 → 2π - 0.05
        assert!((dir - (2.0 * PI - 0.05)).abs() < 1e-10);
    }
}
