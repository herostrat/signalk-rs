/// Derives `navigation.leewayAngle` from heading true and course over ground true.
///
/// leewayAngle = headingTrue − courseOverGroundTrue
///
/// Positive leeway = bow points to windward of track (boat sliding leeward).
/// Result is in radians, range [−π, π].
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;
use std::f64::consts::PI;

pub struct LeewayAngle;

impl Calculator for LeewayAngle {
    fn name(&self) -> &str {
        "leewayAngle"
    }

    fn inputs(&self) -> &[&str] {
        &["navigation.headingTrue", "navigation.courseOverGroundTrue"]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let heading_true = values.get("navigation.headingTrue")?.as_f64()?;
        let cog_true = values.get("navigation.courseOverGroundTrue")?.as_f64()?;

        // Signed angle difference, range [−π, π]
        let mut leeway = heading_true - cog_true;
        if leeway > PI {
            leeway -= 2.0 * PI;
        } else if leeway < -PI {
            leeway += 2.0 * PI;
        }

        Some(vec![PathValue::new(
            "navigation.leewayAngle",
            serde_json::json!(leeway),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn no_leeway() {
        let calc = LeewayAngle;
        let mut values = HashMap::new();
        values.insert("navigation.headingTrue".into(), serde_json::json!(1.0));
        values.insert(
            "navigation.courseOverGroundTrue".into(),
            serde_json::json!(1.0),
        );
        let result = calc.calculate(&values).unwrap();
        let leeway = result[0].value.as_f64().unwrap();
        assert!(leeway.abs() < 1e-10);
    }

    #[test]
    fn positive_leeway() {
        let calc = LeewayAngle;
        let mut values = HashMap::new();
        // Heading 10° more than COG → positive leeway (sliding leeward)
        values.insert("navigation.headingTrue".into(), serde_json::json!(1.1));
        values.insert(
            "navigation.courseOverGroundTrue".into(),
            serde_json::json!(1.0),
        );
        let result = calc.calculate(&values).unwrap();
        let leeway = result[0].value.as_f64().unwrap();
        assert!((leeway - 0.1).abs() < 1e-10);
    }

    #[test]
    fn wraps_correctly() {
        let calc = LeewayAngle;
        let mut values = HashMap::new();
        // Heading near 0, COG near 2π → small positive leeway
        values.insert("navigation.headingTrue".into(), serde_json::json!(0.1));
        values.insert(
            "navigation.courseOverGroundTrue".into(),
            serde_json::json!(2.0 * PI - 0.1),
        );
        let result = calc.calculate(&values).unwrap();
        let leeway = result[0].value.as_f64().unwrap();
        assert!(
            (leeway - 0.2).abs() < 1e-10,
            "Expected ~0.2 rad, got {leeway}"
        );
    }
}
