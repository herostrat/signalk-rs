/// Derives `navigation.courseGreatCircle.nextPoint.velocityMadeGood` from SOG, COG, and bearing.
///
/// VMG to waypoint = SOG × cos(COG − bearing)
///
/// This is the velocity component toward the active waypoint, distinct from
/// wind-based VMG (`vmg.rs` / `vmg_stw.rs`). Feeds into the ETA calculator.
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct VmgWaypoint;

impl Calculator for VmgWaypoint {
    fn name(&self) -> &str {
        "vmgWaypoint"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "navigation.speedOverGround",
            "navigation.courseOverGroundTrue",
            "navigation.courseGreatCircle.bearingTrackTrue",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let sog = values.get("navigation.speedOverGround")?.as_f64()?;
        let cog = values.get("navigation.courseOverGroundTrue")?.as_f64()?;
        let bearing = values
            .get("navigation.courseGreatCircle.bearingTrackTrue")?
            .as_f64()?;

        if !sog.is_finite() || !cog.is_finite() || !bearing.is_finite() {
            return None;
        }

        let vmg = sog * (cog - bearing).cos();

        Some(vec![PathValue::new(
            "navigation.courseGreatCircle.nextPoint.velocityMadeGood",
            serde_json::json!(vmg),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    #[test]
    fn heading_straight_to_waypoint() {
        let mut values = HashMap::new();
        values.insert("navigation.speedOverGround".into(), serde_json::json!(5.0));
        values.insert(
            "navigation.courseOverGroundTrue".into(),
            serde_json::json!(1.0), // COG = 1.0 rad
        );
        values.insert(
            "navigation.courseGreatCircle.bearingTrackTrue".into(),
            serde_json::json!(1.0), // bearing = 1.0 rad (same direction)
        );

        let result = VmgWaypoint.calculate(&values).unwrap();
        let vmg = result[0].value.as_f64().unwrap();
        assert!((vmg - 5.0).abs() < 0.001, "Expected ~5.0 m/s, got {vmg}");
    }

    #[test]
    fn perpendicular_to_waypoint() {
        let mut values = HashMap::new();
        values.insert("navigation.speedOverGround".into(), serde_json::json!(5.0));
        values.insert(
            "navigation.courseOverGroundTrue".into(),
            serde_json::json!(0.0),
        );
        values.insert(
            "navigation.courseGreatCircle.bearingTrackTrue".into(),
            serde_json::json!(FRAC_PI_2), // 90° off
        );

        let result = VmgWaypoint.calculate(&values).unwrap();
        let vmg = result[0].value.as_f64().unwrap();
        assert!(vmg.abs() < 0.001, "Expected ~0 m/s, got {vmg}");
    }

    #[test]
    fn away_from_waypoint() {
        let mut values = HashMap::new();
        values.insert("navigation.speedOverGround".into(), serde_json::json!(5.0));
        values.insert(
            "navigation.courseOverGroundTrue".into(),
            serde_json::json!(0.0),
        );
        values.insert(
            "navigation.courseGreatCircle.bearingTrackTrue".into(),
            serde_json::json!(PI), // 180° off — sailing away
        );

        let result = VmgWaypoint.calculate(&values).unwrap();
        let vmg = result[0].value.as_f64().unwrap();
        assert!(
            (vmg - (-5.0)).abs() < 0.001,
            "Expected ~-5.0 m/s, got {vmg}"
        );
    }

    #[test]
    fn sog_zero() {
        let mut values = HashMap::new();
        values.insert("navigation.speedOverGround".into(), serde_json::json!(0.0));
        values.insert(
            "navigation.courseOverGroundTrue".into(),
            serde_json::json!(1.0),
        );
        values.insert(
            "navigation.courseGreatCircle.bearingTrackTrue".into(),
            serde_json::json!(2.0),
        );

        let result = VmgWaypoint.calculate(&values).unwrap();
        let vmg = result[0].value.as_f64().unwrap();
        assert!(vmg.abs() < 0.001, "Expected 0 m/s when SOG=0, got {vmg}");
    }

    #[test]
    fn missing_bearing_returns_none() {
        let mut values = HashMap::new();
        values.insert("navigation.speedOverGround".into(), serde_json::json!(5.0));
        values.insert(
            "navigation.courseOverGroundTrue".into(),
            serde_json::json!(1.0),
        );
        // No bearing → no active navigation
        assert!(VmgWaypoint.calculate(&values).is_none());
    }
}
