/// Derives `navigation.courseOverGroundMagnetic` from true COG - variation.
///
/// Formula: cogMagnetic = courseOverGroundTrue - magneticVariation
/// All values in radians, result normalized to [0, 2π).
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;
use std::f64::consts::PI;

pub struct CourseOverGroundMagnetic;

impl Calculator for CourseOverGroundMagnetic {
    fn name(&self) -> &str {
        "courseOverGroundMagnetic"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "navigation.courseOverGroundTrue",
            "navigation.magneticVariation",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let cog_true = values.get("navigation.courseOverGroundTrue")?.as_f64()?;
        let variation = values.get("navigation.magneticVariation")?.as_f64()?;

        let cog_mag = (cog_true - variation).rem_euclid(2.0 * PI);

        Some(vec![PathValue::new(
            "navigation.courseOverGroundMagnetic",
            serde_json::json!(cog_mag),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cog_magnetic_basic() {
        let calc = CourseOverGroundMagnetic;
        let mut values = HashMap::new();
        values.insert(
            "navigation.courseOverGroundTrue".to_string(),
            serde_json::json!(1.5),
        );
        values.insert(
            "navigation.magneticVariation".to_string(),
            serde_json::json!(0.05),
        );

        let result = calc.calculate(&values).unwrap();
        assert_eq!(result[0].path, "navigation.courseOverGroundMagnetic");
        let value = result[0].value.as_f64().unwrap();
        assert!((value - 1.45).abs() < 0.001);
    }

    #[test]
    fn cog_magnetic_wraps_negative() {
        let calc = CourseOverGroundMagnetic;
        let mut values = HashMap::new();
        values.insert(
            "navigation.courseOverGroundTrue".to_string(),
            serde_json::json!(0.02),
        );
        values.insert(
            "navigation.magneticVariation".to_string(),
            serde_json::json!(0.1),
        );

        let result = calc.calculate(&values).unwrap();
        let value = result[0].value.as_f64().unwrap();
        // Should wrap to near 2π
        assert!(value > 6.0, "expected near 2π, got {value}");
    }
}
