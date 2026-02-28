/// Derives `navigation.courseOverGroundTrue` from COG magnetic + magnetic variation.
///
/// cogTrue = cogMagnetic + variation
///
/// Inverse of `courseOverGroundMagnetic` calculator.
use super::{Calculator, normalize_angle};
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct CourseOverGroundTrue;

impl Calculator for CourseOverGroundTrue {
    fn name(&self) -> &str {
        "courseOverGroundTrue"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "navigation.courseOverGroundMagnetic",
            "navigation.magneticVariation",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let cog_mag = values
            .get("navigation.courseOverGroundMagnetic")?
            .as_f64()?;
        let variation = values.get("navigation.magneticVariation")?.as_f64()?;

        let cog_true = normalize_angle(cog_mag + variation);

        Some(vec![PathValue::new(
            "navigation.courseOverGroundTrue",
            serde_json::json!(cog_true),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn basic_east_variation() {
        let calc = CourseOverGroundTrue;
        let mut values = HashMap::new();
        values.insert(
            "navigation.courseOverGroundMagnetic".into(),
            serde_json::json!(1.0),
        );
        values.insert(
            "navigation.magneticVariation".into(),
            serde_json::json!(0.1),
        );
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result[0].path, "navigation.courseOverGroundTrue");
        let cog = result[0].value.as_f64().unwrap();
        assert!((cog - 1.1).abs() < 1e-10);
    }

    #[test]
    fn wraps_around() {
        let calc = CourseOverGroundTrue;
        let mut values = HashMap::new();
        values.insert(
            "navigation.courseOverGroundMagnetic".into(),
            serde_json::json!(6.0),
        );
        values.insert(
            "navigation.magneticVariation".into(),
            serde_json::json!(1.0),
        );
        let result = calc.calculate(&values).unwrap();
        let cog = result[0].value.as_f64().unwrap();
        assert!((0.0..2.0 * PI).contains(&cog));
    }
}
