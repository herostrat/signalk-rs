/// Derives `navigation.leewayAngleHeel` from heel angle (roll) and speed through water.
///
/// Uses the empirical formula: leeway = K × heel / STW²
/// where K is a boat-specific coefficient (default: 10).
///
/// This is the "heel-based" leeway calculation, output to a separate path from
/// `leeway_angle.rs` (GPS-based, `navigation.leewayAngle`) since the two methods
/// use fundamentally different inputs and may diverge.
///
/// Sign convention: positive heel to starboard → negative leeway (sliding to port).
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

/// Default leeway coefficient (boat-specific, should be calibrated)
const DEFAULT_K: f64 = 10.0;

pub struct Leeway;

impl Calculator for Leeway {
    fn name(&self) -> &str {
        "leeway"
    }

    fn inputs(&self) -> &[&str] {
        &["navigation.attitude", "navigation.speedThroughWater"]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        // attitude is an object with roll/pitch/yaw
        let attitude = values.get("navigation.attitude")?;
        let roll = attitude.get("roll").and_then(|v| v.as_f64())?;
        let stw = values.get("navigation.speedThroughWater")?.as_f64()?;

        if stw < 0.5 {
            // Too slow for meaningful leeway calculation
            return None;
        }

        // Leeway in radians = K × roll / STW²
        let leeway = DEFAULT_K * roll / (stw * stw);

        Some(vec![PathValue::new(
            "navigation.leewayAngleHeel",
            serde_json::json!(leeway),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heel_produces_leeway() {
        let calc = Leeway;
        let mut values = HashMap::new();
        // 15° heel (0.26 rad), 5 m/s STW
        values.insert(
            "navigation.attitude".into(),
            serde_json::json!({"roll": 0.26, "pitch": 0.0, "yaw": 0.0}),
        );
        values.insert(
            "navigation.speedThroughWater".into(),
            serde_json::json!(5.0),
        );
        let result = calc.calculate(&values).unwrap();
        let leeway = result[0].value.as_f64().unwrap();
        // K=10, roll=0.26, stw=5 → 10 * 0.26 / 25 = 0.104 rad ≈ 6°
        assert!(
            (leeway - 0.104).abs() < 0.001,
            "Expected ~0.104 rad, got {leeway}"
        );
    }

    #[test]
    fn too_slow_returns_none() {
        let calc = Leeway;
        let mut values = HashMap::new();
        values.insert(
            "navigation.attitude".into(),
            serde_json::json!({"roll": 0.26}),
        );
        values.insert(
            "navigation.speedThroughWater".into(),
            serde_json::json!(0.1),
        );
        assert!(calc.calculate(&values).is_none());
    }

    #[test]
    fn no_heel_no_leeway() {
        let calc = Leeway;
        let mut values = HashMap::new();
        values.insert(
            "navigation.attitude".into(),
            serde_json::json!({"roll": 0.0}),
        );
        values.insert(
            "navigation.speedThroughWater".into(),
            serde_json::json!(5.0),
        );
        let result = calc.calculate(&values).unwrap();
        let leeway = result[0].value.as_f64().unwrap();
        assert!(leeway.abs() < 1e-10);
    }
}
