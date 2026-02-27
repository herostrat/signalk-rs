/// Derives `propulsion.{id}.slip` from revolutions and speed through water.
///
/// Propeller slip = 1 − (STW / theoretical_speed)
/// theoretical_speed = revolutions × pitch_factor
///
/// Since we don't know pitch, we use a simpler form:
/// slip ratio = 1 − (STW / (revolutions × K))
/// where K is a constant that would need to be calibrated per boat.
///
/// However, the upstream signalk-derived-data uses a different approach:
/// slip = (theoretical_speed − STW) / theoretical_speed
/// theoretical_speed is derived from gear ratio and prop pitch.
///
/// For now, we output a simple comparison metric:
/// If STW and revolutions are both available, we compute an indicative ratio.
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct PropSlip;

impl Calculator for PropSlip {
    fn name(&self) -> &str {
        "propSlip"
    }

    fn inputs(&self) -> &[&str] {
        &["navigation.speedThroughWater", "propulsion"]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let stw = values.get("navigation.speedThroughWater")?.as_f64()?;

        let mut results = Vec::new();

        for (path, value) in values.iter() {
            let Some(prefix) = path.strip_suffix(".revolutions") else {
                continue;
            };
            if !prefix.starts_with("propulsion.") {
                continue;
            }
            let Some(revolutions) = value.as_f64() else {
                continue;
            };
            if revolutions <= 0.0 {
                continue;
            }
            let ratio = stw / revolutions;
            results.push(PathValue::new(
                format!("{prefix}.drive.propeller.slip"),
                serde_json::json!(ratio),
            ));
        }

        if results.is_empty() {
            None
        } else {
            Some(results)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_ratio() {
        let calc = PropSlip;
        let mut values = HashMap::new();
        values.insert(
            "navigation.speedThroughWater".into(),
            serde_json::json!(5.0),
        );
        values.insert(
            "propulsion.main.revolutions".into(),
            serde_json::json!(25.0), // 25 rev/s = 1500 RPM
        );
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result[0].path, "propulsion.main.drive.propeller.slip");
        let ratio = result[0].value.as_f64().unwrap();
        assert!((ratio - 0.2).abs() < 0.001); // 5.0 / 25.0
    }

    #[test]
    fn engine_off_skips() {
        let calc = PropSlip;
        let mut values = HashMap::new();
        values.insert(
            "navigation.speedThroughWater".into(),
            serde_json::json!(5.0),
        );
        values.insert("propulsion.main.revolutions".into(), serde_json::json!(0.0));
        assert!(calc.calculate(&values).is_none());
    }
}
