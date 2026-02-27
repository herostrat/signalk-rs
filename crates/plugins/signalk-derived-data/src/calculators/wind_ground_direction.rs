/// Derives `environment.wind.directionGround` from heading + true wind angle over ground.
///
/// directionGround = headingTrue + angleTrueGround (normalized to [0, 2π))
use super::{Calculator, normalize_angle};
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct WindGroundDirection;

impl Calculator for WindGroundDirection {
    fn name(&self) -> &str {
        "windGroundDirection"
    }

    fn inputs(&self) -> &[&str] {
        &["navigation.headingTrue", "environment.wind.angleTrueGround"]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let heading = values.get("navigation.headingTrue")?.as_f64()?;
        let angle = values.get("environment.wind.angleTrueGround")?.as_f64()?;

        let direction = normalize_angle(heading + angle);

        Some(vec![PathValue::new(
            "environment.wind.directionGround",
            serde_json::json!(direction),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn basic() {
        let calc = WindGroundDirection;
        let mut values = HashMap::new();
        values.insert("navigation.headingTrue".into(), serde_json::json!(1.0));
        values.insert(
            "environment.wind.angleTrueGround".into(),
            serde_json::json!(0.5),
        );
        let result = calc.calculate(&values).unwrap();
        let dir = result[0].value.as_f64().unwrap();
        assert!((dir - 1.5).abs() < 1e-10);
    }

    #[test]
    fn wraps() {
        let calc = WindGroundDirection;
        let mut values = HashMap::new();
        values.insert("navigation.headingTrue".into(), serde_json::json!(5.0));
        values.insert(
            "environment.wind.angleTrueGround".into(),
            serde_json::json!(3.0),
        );
        let result = calc.calculate(&values).unwrap();
        let dir = result[0].value.as_f64().unwrap();
        assert!((0.0..2.0 * PI).contains(&dir));
    }
}
