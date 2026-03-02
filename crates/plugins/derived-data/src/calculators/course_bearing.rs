/// Derives `navigation.course.calcValues.bearingTrackTrue` from vessel position
/// and next waypoint position.
///
/// Uses the initial bearing (forward azimuth) formula.
/// Input/output in radians.
use super::Calculator;
use signalk_types::PathValue;
use signalk_types::geo::initial_bearing;
use std::collections::HashMap;

pub struct CourseBearing;

impl Calculator for CourseBearing {
    fn name(&self) -> &str {
        "courseBearing"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "navigation.position",
            "navigation.course.nextPoint.position",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let pos = values.get("navigation.position")?;
        let next = values.get("navigation.course.nextPoint.position")?;

        let lat1 = pos.get("latitude")?.as_f64()?;
        let lon1 = pos.get("longitude")?.as_f64()?;
        let lat2 = next.get("latitude")?.as_f64()?;
        let lon2 = next.get("longitude")?.as_f64()?;

        let bearing = initial_bearing(lat1, lon1, lat2, lon2);

        Some(vec![PathValue::new(
            "navigation.course.calcValues.bearingTrackTrue",
            serde_json::json!(bearing),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn bearing_north() {
        let calc = CourseBearing;
        let mut values = HashMap::new();
        values.insert(
            "navigation.position".to_string(),
            serde_json::json!({"latitude": 49.0, "longitude": -123.0}),
        );
        values.insert(
            "navigation.course.nextPoint.position".to_string(),
            serde_json::json!({"latitude": 50.0, "longitude": -123.0}),
        );

        let result = calc.calculate(&values).unwrap();
        assert_eq!(
            result[0].path,
            "navigation.course.calcValues.bearingTrackTrue"
        );
        let value = result[0].value.as_f64().unwrap();
        // Going north: bearing ≈ 0
        assert!(
            !(0.01..=2.0 * PI - 0.01).contains(&value),
            "Expected ~0 rad, got {value}"
        );
    }

    #[test]
    fn bearing_east() {
        let calc = CourseBearing;
        let mut values = HashMap::new();
        values.insert(
            "navigation.position".to_string(),
            serde_json::json!({"latitude": 0.0, "longitude": 0.0}),
        );
        values.insert(
            "navigation.course.nextPoint.position".to_string(),
            serde_json::json!({"latitude": 0.0, "longitude": 1.0}),
        );

        let result = calc.calculate(&values).unwrap();
        let value = result[0].value.as_f64().unwrap();
        assert!(
            (value - PI / 2.0).abs() < 0.01,
            "Expected ~π/2, got {value}"
        );
    }

    #[test]
    fn missing_next_point() {
        let calc = CourseBearing;
        let mut values = HashMap::new();
        values.insert(
            "navigation.position".to_string(),
            serde_json::json!({"latitude": 49.0, "longitude": -123.0}),
        );
        assert!(calc.calculate(&values).is_none());
    }
}
