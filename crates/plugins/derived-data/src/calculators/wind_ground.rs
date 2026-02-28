/// Derives ground-referenced true wind (speed + angle) using SOG instead of STW.
///
/// Same vector math as `true_wind.rs` but using speed over ground, giving
/// "true wind over ground" — useful for racing and weather routing.
///
/// Outputs:
/// - `environment.wind.angleTrueGround` (rad, relative to heading)
/// - `environment.wind.speedOverGround` (m/s)
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct WindGround;

impl Calculator for WindGround {
    fn name(&self) -> &str {
        "windGround"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "environment.wind.speedApparent",
            "environment.wind.angleApparent",
            "navigation.speedOverGround",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let aws = values.get("environment.wind.speedApparent")?.as_f64()?;
        let awa = values.get("environment.wind.angleApparent")?.as_f64()?;
        let sog = values.get("navigation.speedOverGround")?.as_f64()?;

        // Vector decomposition: true wind = apparent wind − boat motion
        let u = aws * awa.cos() - sog;
        let v = aws * awa.sin();

        let tws = (u * u + v * v).sqrt();
        let twa = v.atan2(u);

        Some(vec![
            PathValue::new("environment.wind.angleTrueGround", serde_json::json!(twa)),
            PathValue::new("environment.wind.speedOverGround", serde_json::json!(tws)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headwind() {
        let calc = WindGround;
        let mut values = HashMap::new();
        // 10 m/s apparent from dead ahead (0 rad), SOG 5 m/s
        // True wind = 10 - 5 = 5 m/s from ahead
        values.insert(
            "environment.wind.speedApparent".into(),
            serde_json::json!(10.0),
        );
        values.insert(
            "environment.wind.angleApparent".into(),
            serde_json::json!(0.0),
        );
        values.insert("navigation.speedOverGround".into(), serde_json::json!(5.0));
        let result = calc.calculate(&values).unwrap();
        let tws = result
            .iter()
            .find(|pv| pv.path == "environment.wind.speedOverGround")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        assert!((tws - 5.0).abs() < 0.01);
    }

    #[test]
    fn beam_wind() {
        let calc = WindGround;
        let mut values = HashMap::new();
        values.insert(
            "environment.wind.speedApparent".into(),
            serde_json::json!(10.0),
        );
        values.insert(
            "environment.wind.angleApparent".into(),
            serde_json::json!(std::f64::consts::FRAC_PI_2),
        );
        values.insert("navigation.speedOverGround".into(), serde_json::json!(5.0));
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result.len(), 2);
    }
}
