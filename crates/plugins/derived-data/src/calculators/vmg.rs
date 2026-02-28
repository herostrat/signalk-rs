/// Derives velocity made good (VMG) to wind.
///
/// Formula: VMG = cos(angleTrueWater) × speedOverGround
///
/// Positive VMG means sailing toward the wind, negative means away.
/// At beam reach (90°), VMG = 0.
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct VmgWind;

impl Calculator for VmgWind {
    fn name(&self) -> &str {
        "vmgWind"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "environment.wind.angleTrueWater",
            "navigation.speedOverGround",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let twa = values.get("environment.wind.angleTrueWater")?.as_f64()?;
        let sog = values.get("navigation.speedOverGround")?.as_f64()?;

        if !twa.is_finite() || !sog.is_finite() {
            return None;
        }

        let vmg = twa.cos() * sog;

        Some(vec![PathValue::new(
            "performance.velocityMadeGood",
            serde_json::json!(vmg),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn vmg_dead_ahead() {
        let mut values = HashMap::new();
        values.insert(
            "environment.wind.angleTrueWater".into(),
            serde_json::json!(0.0),
        );
        values.insert("navigation.speedOverGround".into(), serde_json::json!(5.0));

        let result = VmgWind.calculate(&values).unwrap();
        let vmg = result[0].value.as_f64().unwrap();
        assert!((vmg - 5.0).abs() < 0.001, "expected 5.0, got {vmg}");
    }

    #[test]
    fn vmg_beam_reach() {
        let mut values = HashMap::new();
        values.insert(
            "environment.wind.angleTrueWater".into(),
            serde_json::json!(PI / 2.0),
        );
        values.insert("navigation.speedOverGround".into(), serde_json::json!(5.0));

        let result = VmgWind.calculate(&values).unwrap();
        let vmg = result[0].value.as_f64().unwrap();
        assert!(vmg.abs() < 0.001, "expected ~0, got {vmg}");
    }

    #[test]
    fn vmg_downwind() {
        let mut values = HashMap::new();
        values.insert(
            "environment.wind.angleTrueWater".into(),
            serde_json::json!(PI),
        );
        values.insert("navigation.speedOverGround".into(), serde_json::json!(5.0));

        let result = VmgWind.calculate(&values).unwrap();
        let vmg = result[0].value.as_f64().unwrap();
        assert!((vmg - (-5.0)).abs() < 0.001, "expected -5.0, got {vmg}");
    }

    #[test]
    fn vmg_missing_input() {
        let mut values = HashMap::new();
        values.insert(
            "environment.wind.angleTrueWater".into(),
            serde_json::json!(0.5),
        );
        assert!(VmgWind.calculate(&values).is_none());
    }
}
