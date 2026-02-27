/// Derives true wind from apparent wind, heading, and speed through water.
///
/// Decomposes apparent wind into Cartesian, subtracts vessel STW from forward
/// component, then reconstructs magnitude and angle.
///
/// Inputs:  headingTrue, speedThroughWater, wind.speedApparent, wind.angleApparent
/// Outputs: wind.directionTrue, wind.angleTrueWater, wind.speedTrue
///
/// All angles in radians, speeds in m/s (SignalK SI).
use super::{Calculator, normalize_angle};
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct TrueWind;

impl Calculator for TrueWind {
    fn name(&self) -> &str {
        "trueWind"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "navigation.headingTrue",
            "navigation.speedThroughWater",
            "environment.wind.speedApparent",
            "environment.wind.angleApparent",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let heading_true = values.get("navigation.headingTrue")?.as_f64()?;
        let stw = values.get("navigation.speedThroughWater")?.as_f64()?;
        let aws = values.get("environment.wind.speedApparent")?.as_f64()?;
        let awa = values.get("environment.wind.angleApparent")?.as_f64()?;

        if !heading_true.is_finite() || !stw.is_finite() || !aws.is_finite() || !awa.is_finite() {
            return None;
        }

        // Decompose apparent wind into Cartesian (vessel frame)
        let apparent_x = awa.cos() * aws;
        let apparent_y = awa.sin() * aws;

        // True wind angle relative to vessel (subtract vessel speed from forward component)
        let angle = if aws < 1e-9 {
            // Degenerate: no wind — copy apparent angle
            awa
        } else {
            apparent_y.atan2(-stw + apparent_x)
        };

        let speed = (apparent_y.powi(2) + (-stw + apparent_x).powi(2)).sqrt();

        // Absolute true wind direction
        let direction = normalize_angle(heading_true + angle);

        Some(vec![
            PathValue::new(
                "environment.wind.directionTrue",
                serde_json::json!(direction),
            ),
            PathValue::new("environment.wind.angleTrueWater", serde_json::json!(angle)),
            PathValue::new("environment.wind.speedTrue", serde_json::json!(speed)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn make_values(
        heading: f64,
        stw: f64,
        aws: f64,
        awa: f64,
    ) -> HashMap<String, serde_json::Value> {
        let mut v = HashMap::new();
        v.insert("navigation.headingTrue".into(), serde_json::json!(heading));
        v.insert(
            "navigation.speedThroughWater".into(),
            serde_json::json!(stw),
        );
        v.insert(
            "environment.wind.speedApparent".into(),
            serde_json::json!(aws),
        );
        v.insert(
            "environment.wind.angleApparent".into(),
            serde_json::json!(awa),
        );
        v
    }

    #[test]
    fn headwind_equals_apparent_plus_stw() {
        // Heading north, motoring at 5 m/s into a 10 m/s headwind (AWA = π)
        let values = make_values(0.0, 5.0, 10.0, PI);
        let result = TrueWind.calculate(&values).unwrap();

        let speed_true = result
            .iter()
            .find(|pv| pv.path == "environment.wind.speedTrue")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        // True wind speed = apparent + STW when dead ahead (vectors add)
        assert!(
            (speed_true - 15.0).abs() < 0.01,
            "expected ~15, got {speed_true}"
        );

        let angle_true = result
            .iter()
            .find(|pv| pv.path == "environment.wind.angleTrueWater")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        // True wind angle should be ~π (from ahead)
        assert!(
            (angle_true - PI).abs() < 0.01,
            "expected ~π, got {angle_true}"
        );
    }

    #[test]
    fn beam_wind() {
        // Heading north, STW 5 m/s, apparent wind 7 m/s from starboard (AWA = π/2)
        let values = make_values(0.0, 5.0, 7.0, PI / 2.0);
        let result = TrueWind.calculate(&values).unwrap();

        let speed_true = result
            .iter()
            .find(|pv| pv.path == "environment.wind.speedTrue")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        // True wind: apparent_x = 0, apparent_y = 7
        // true_x = 0 - 5 = -5, true_y = 7
        // speed = sqrt(25 + 49) = sqrt(74) ≈ 8.602
        assert!(
            (speed_true - 8.602).abs() < 0.01,
            "expected ~8.602, got {speed_true}"
        );
    }

    #[test]
    fn zero_wind_degenerate() {
        let values = make_values(1.0, 5.0, 0.0, 0.5);
        let result = TrueWind.calculate(&values).unwrap();

        let angle = result
            .iter()
            .find(|pv| pv.path == "environment.wind.angleTrueWater")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        // With zero apparent wind speed, angle should be copied from AWA
        assert!((angle - 0.5).abs() < 0.001);
    }

    #[test]
    fn missing_input_returns_none() {
        let mut values = HashMap::new();
        values.insert("navigation.headingTrue".into(), serde_json::json!(1.0));
        // Missing other inputs
        assert!(TrueWind.calculate(&values).is_none());
    }

    #[test]
    fn direction_true_normalized() {
        // Large heading + angle should wrap to [0, 2π)
        let values = make_values(6.0, 2.0, 5.0, PI);
        let result = TrueWind.calculate(&values).unwrap();

        let dir = result
            .iter()
            .find(|pv| pv.path == "environment.wind.directionTrue")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        assert!(dir >= 0.0 && dir < 2.0 * PI, "direction={dir} out of range");
    }
}
