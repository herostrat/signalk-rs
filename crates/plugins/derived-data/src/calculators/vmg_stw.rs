/// Derives `performance.velocityMadeGoodWater` from true wind angle + STW.
///
/// VMG (upwind/downwind using STW) = STW · cos(angleTrueWater)
///
/// This is the through-water VMG, complementing `vmg.rs` (SOG-based wind VMG).
/// Useful for polar performance analysis. Not to be confused with VMG to waypoint
/// (`vmg_waypoint.rs`), which uses COG and bearing instead of wind angle.
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct VmgStw;

impl Calculator for VmgStw {
    fn name(&self) -> &str {
        "vmgStw"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "environment.wind.angleTrueWater",
            "navigation.speedThroughWater",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let angle = values.get("environment.wind.angleTrueWater")?.as_f64()?;
        let stw = values.get("navigation.speedThroughWater")?.as_f64()?;

        let vmg = stw * angle.cos();

        Some(vec![PathValue::new(
            "performance.velocityMadeGoodWater",
            serde_json::json!(vmg),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    #[test]
    fn dead_ahead() {
        let calc = VmgStw;
        let mut values = HashMap::new();
        values.insert(
            "environment.wind.angleTrueWater".into(),
            serde_json::json!(0.0), // wind from ahead
        );
        values.insert(
            "navigation.speedThroughWater".into(),
            serde_json::json!(5.0),
        );
        let result = calc.calculate(&values).unwrap();
        let vmg = result[0].value.as_f64().unwrap();
        assert!((vmg - 5.0).abs() < 1e-10);
    }

    #[test]
    fn beam_reach() {
        let calc = VmgStw;
        let mut values = HashMap::new();
        values.insert(
            "environment.wind.angleTrueWater".into(),
            serde_json::json!(FRAC_PI_2),
        );
        values.insert(
            "navigation.speedThroughWater".into(),
            serde_json::json!(5.0),
        );
        let result = calc.calculate(&values).unwrap();
        let vmg = result[0].value.as_f64().unwrap();
        assert!(vmg.abs() < 1e-10, "Beam reach VMG should be ~0");
    }

    #[test]
    fn close_hauled() {
        let calc = VmgStw;
        let mut values = HashMap::new();
        values.insert(
            "environment.wind.angleTrueWater".into(),
            serde_json::json!(FRAC_PI_4), // 45°
        );
        values.insert(
            "navigation.speedThroughWater".into(),
            serde_json::json!(6.0),
        );
        let result = calc.calculate(&values).unwrap();
        let vmg = result[0].value.as_f64().unwrap();
        // cos(45°) ≈ 0.707 → VMG ≈ 4.24
        assert!((vmg - 6.0 * FRAC_PI_4.cos()).abs() < 1e-10);
    }

    #[test]
    fn downwind() {
        let calc = VmgStw;
        let mut values = HashMap::new();
        values.insert(
            "environment.wind.angleTrueWater".into(),
            serde_json::json!(PI),
        );
        values.insert(
            "navigation.speedThroughWater".into(),
            serde_json::json!(5.0),
        );
        let result = calc.calculate(&values).unwrap();
        let vmg = result[0].value.as_f64().unwrap();
        assert!((vmg - (-5.0)).abs() < 1e-10);
    }
}
