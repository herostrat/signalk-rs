/// Derives current set and drift from vessel motion vectors.
///
/// Compares course-over-ground (GPS) with heading + speed-through-water (log)
/// to determine the ocean current vector.
///
/// Inputs:  headingMagnetic, courseOverGroundTrue, speedThroughWater,
///          speedOverGround, magneticVariation
/// Outputs: environment.current.drift, current.setTrue, current.setMagnetic
///
/// Uses the law of cosines for drift magnitude and atan2 for set direction.
/// All angles in radians, speeds in m/s.
use super::{Calculator, normalize_angle};
use signalk_types::PathValue;
use std::collections::HashMap;
use std::f64::consts::PI;

pub struct SetDrift;

impl Calculator for SetDrift {
    fn name(&self) -> &str {
        "setDrift"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "navigation.headingMagnetic",
            "navigation.courseOverGroundTrue",
            "navigation.speedThroughWater",
            "navigation.speedOverGround",
            "navigation.magneticVariation",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let heading_mag = values.get("navigation.headingMagnetic")?.as_f64()?;
        let cog_true = values.get("navigation.courseOverGroundTrue")?.as_f64()?;
        let stw = values.get("navigation.speedThroughWater")?.as_f64()?;
        let sog = values.get("navigation.speedOverGround")?.as_f64()?;
        let variation = values.get("navigation.magneticVariation")?.as_f64()?;

        if !heading_mag.is_finite()
            || !cog_true.is_finite()
            || !stw.is_finite()
            || !sog.is_finite()
            || !variation.is_finite()
        {
            return None;
        }

        // Both stationary — no current can be determined
        if sog.abs() < 1e-9 && stw.abs() < 1e-9 {
            return Some(vec![
                PathValue::new("environment.current.drift", serde_json::json!(0.0)),
                PathValue::new("environment.current.setTrue", serde_json::json!(0.0)),
                PathValue::new("environment.current.setMagnetic", serde_json::json!(0.0)),
            ]);
        }

        let delta = cog_true - heading_mag;

        // Drift magnitude via law of cosines
        let drift = (sog.powi(2) + stw.powi(2) - 2.0 * stw * sog * delta.cos())
            .max(0.0)
            .sqrt();

        // Set direction (magnetic) — direction current flows toward
        let set_magnetic = normalize_angle((sog * delta.sin()).atan2(stw - sog * delta.cos()) + PI);

        // Set direction (true) = magnetic + variation
        let set_true = normalize_angle(set_magnetic + variation);

        // Drift impact: ratio of current speed to boat speed through water.
        // Undefined (omitted) when STW is near zero.
        let mut results = vec![
            PathValue::new("environment.current.drift", serde_json::json!(drift)),
            PathValue::new("environment.current.setTrue", serde_json::json!(set_true)),
            PathValue::new(
                "environment.current.setMagnetic",
                serde_json::json!(set_magnetic),
            ),
        ];
        if stw > 0.01 {
            results.push(PathValue::new(
                "environment.current.driftImpact",
                serde_json::json!(drift / stw),
            ));
        }

        Some(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_values(
        heading_mag: f64,
        cog_true: f64,
        stw: f64,
        sog: f64,
        variation: f64,
    ) -> HashMap<String, serde_json::Value> {
        let mut v = HashMap::new();
        v.insert(
            "navigation.headingMagnetic".into(),
            serde_json::json!(heading_mag),
        );
        v.insert(
            "navigation.courseOverGroundTrue".into(),
            serde_json::json!(cog_true),
        );
        v.insert(
            "navigation.speedThroughWater".into(),
            serde_json::json!(stw),
        );
        v.insert("navigation.speedOverGround".into(), serde_json::json!(sog));
        v.insert(
            "navigation.magneticVariation".into(),
            serde_json::json!(variation),
        );
        v
    }

    #[test]
    fn no_current_same_heading_and_speed() {
        // Heading north (mag), COG north (true), no variation, same speed → no current
        let values = make_values(0.0, 0.0, 5.0, 5.0, 0.0);
        let result = SetDrift.calculate(&values).unwrap();

        let drift = result
            .iter()
            .find(|pv| pv.path == "environment.current.drift")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        assert!(drift.abs() < 0.01, "expected ~0 drift, got {drift}");
    }

    #[test]
    fn current_from_beam() {
        // Heading north (mag=0), variation=0, STW=5
        // COG pushed 30° east by current, SOG=6
        let values = make_values(0.0, std::f64::consts::FRAC_PI_6, 5.0, 6.0, 0.0);
        let result = SetDrift.calculate(&values).unwrap();

        let drift = result
            .iter()
            .find(|pv| pv.path == "environment.current.drift")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        // Should be positive and plausible
        assert!(drift > 0.5, "expected positive drift, got {drift}");
        assert!(drift < 5.0, "drift too high: {drift}");

        // driftImpact should be drift/stw
        let impact = result
            .iter()
            .find(|pv| pv.path == "environment.current.driftImpact")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        assert!(
            (impact - drift / 5.0).abs() < 0.001,
            "expected driftImpact = drift/stw, got {impact}"
        );
    }

    #[test]
    fn drift_impact_omitted_when_stationary() {
        let values = make_values(0.0, 0.0, 0.0, 0.0, 0.05);
        let result = SetDrift.calculate(&values).unwrap();
        assert!(
            !result
                .iter()
                .any(|pv| pv.path == "environment.current.driftImpact"),
            "driftImpact should be omitted when STW is zero"
        );
    }

    #[test]
    fn both_stationary() {
        let values = make_values(0.0, 0.0, 0.0, 0.0, 0.05);
        let result = SetDrift.calculate(&values).unwrap();

        let drift = result
            .iter()
            .find(|pv| pv.path == "environment.current.drift")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        assert!((drift - 0.0).abs() < 0.001);
    }

    #[test]
    fn set_true_includes_variation() {
        let values = make_values(1.0, 1.2, 5.0, 5.5, 0.1);
        let result = SetDrift.calculate(&values).unwrap();

        let set_mag = result
            .iter()
            .find(|pv| pv.path == "environment.current.setMagnetic")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        let set_true = result
            .iter()
            .find(|pv| pv.path == "environment.current.setTrue")
            .unwrap()
            .value
            .as_f64()
            .unwrap();

        // setTrue should differ from setMagnetic by approximately the variation
        let diff = normalize_angle(set_true - set_mag);
        assert!(
            (diff - 0.1).abs() < 0.01 || (diff - (2.0 * PI - 0.1)).abs() < 0.01,
            "expected ~0.1 difference, got {diff}"
        );
    }

    #[test]
    fn missing_input_returns_none() {
        let mut values = HashMap::new();
        values.insert("navigation.headingMagnetic".into(), serde_json::json!(1.0));
        values.insert(
            "navigation.courseOverGroundTrue".into(),
            serde_json::json!(1.0),
        );
        // Missing STW, SOG, variation
        assert!(SetDrift.calculate(&values).is_none());
    }

    #[test]
    fn angles_normalized() {
        let values = make_values(5.5, 6.0, 4.0, 4.5, 0.05);
        let result = SetDrift.calculate(&values).unwrap();

        for pv in &result {
            if pv.path.contains("set") {
                let val = pv.value.as_f64().unwrap();
                assert!(
                    (0.0..2.0 * PI).contains(&val),
                    "{}: {val} out of range",
                    pv.path
                );
            }
        }
    }
}
