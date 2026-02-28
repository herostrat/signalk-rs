/// Derives `environment.wind.directionMagneticApparent` from heading magnetic + apparent wind angle.
///
/// directionMagneticApparent = headingMagnetic + angleApparent
///
/// Fallback method for when true wind direction is not available. Uses apparent
/// wind angle instead of true, so the result includes vessel speed effects.
/// Output on a separate path from `wind_direction_magnetic.rs` (which uses
/// true wind direction) since the two methods may produce different values.
use super::{Calculator, normalize_angle};
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct WindDirectionMagnetic2;

impl Calculator for WindDirectionMagnetic2 {
    fn name(&self) -> &str {
        "windDirectionMagnetic2"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "navigation.headingMagnetic",
            "environment.wind.angleApparent",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let heading = values.get("navigation.headingMagnetic")?.as_f64()?;
        let angle_apparent = values.get("environment.wind.angleApparent")?.as_f64()?;

        let dir_mag = normalize_angle(heading + angle_apparent);

        Some(vec![PathValue::new(
            "environment.wind.directionMagneticApparent",
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
        let calc = WindDirectionMagnetic2;
        let mut values = HashMap::new();
        // Heading north (0), wind from starboard beam (π/2)
        values.insert("navigation.headingMagnetic".into(), serde_json::json!(0.0));
        values.insert(
            "environment.wind.angleApparent".into(),
            serde_json::json!(std::f64::consts::FRAC_PI_2),
        );
        let result = calc.calculate(&values).unwrap();
        let dir = result[0].value.as_f64().unwrap();
        assert!((dir - std::f64::consts::FRAC_PI_2).abs() < 1e-10);
    }

    #[test]
    fn wraps_around() {
        let calc = WindDirectionMagnetic2;
        let mut values = HashMap::new();
        values.insert("navigation.headingMagnetic".into(), serde_json::json!(5.0));
        values.insert(
            "environment.wind.angleApparent".into(),
            serde_json::json!(3.0),
        );
        let result = calc.calculate(&values).unwrap();
        let dir = result[0].value.as_f64().unwrap();
        assert!(
            (0.0..2.0 * PI).contains(&dir),
            "Should wrap to [0, 2π), got {dir}"
        );
    }
}
