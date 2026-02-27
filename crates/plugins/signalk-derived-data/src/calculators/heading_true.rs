/// Derives `navigation.headingTrue` from magnetic heading + variation.
///
/// Formula: headingTrue = headingMagnetic + magneticVariation
/// All values in radians, result normalized to [0, 2π).
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;
use std::f64::consts::PI;

pub struct HeadingTrue;

impl Calculator for HeadingTrue {
    fn name(&self) -> &str {
        "headingTrue"
    }

    fn inputs(&self) -> &[&str] {
        &["navigation.headingMagnetic", "navigation.magneticVariation"]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let heading_mag = values.get("navigation.headingMagnetic")?.as_f64()?;
        let variation = values.get("navigation.magneticVariation")?.as_f64()?;

        let heading_true = (heading_mag + variation).rem_euclid(2.0 * PI);

        Some(vec![PathValue::new(
            "navigation.headingTrue",
            serde_json::json!(heading_true),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_true_basic() {
        let calc = HeadingTrue;
        let mut values = HashMap::new();
        values.insert(
            "navigation.headingMagnetic".to_string(),
            serde_json::json!(1.5),
        );
        values.insert(
            "navigation.magneticVariation".to_string(),
            serde_json::json!(0.05),
        );

        let result = calc.calculate(&values).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path, "navigation.headingTrue");
        let value = result[0].value.as_f64().unwrap();
        assert!((value - 1.55).abs() < 0.001);
    }

    #[test]
    fn heading_true_wraps_around() {
        let calc = HeadingTrue;
        let mut values = HashMap::new();
        // heading near 2π + positive variation → should wrap
        values.insert(
            "navigation.headingMagnetic".to_string(),
            serde_json::json!(6.2),
        );
        values.insert(
            "navigation.magneticVariation".to_string(),
            serde_json::json!(0.2),
        );

        let result = calc.calculate(&values).unwrap();
        let value = result[0].value.as_f64().unwrap();
        assert!(
            (0.0..2.0 * PI).contains(&value),
            "value={value} out of range"
        );
    }

    #[test]
    fn heading_true_missing_input() {
        let calc = HeadingTrue;
        let mut values = HashMap::new();
        values.insert(
            "navigation.headingMagnetic".to_string(),
            serde_json::json!(1.5),
        );
        // Missing magneticVariation
        assert!(calc.calculate(&values).is_none());
    }
}
