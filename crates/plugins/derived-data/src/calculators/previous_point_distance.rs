/// Derives `navigation.course.calcValues.previousPoint.distance` from vessel position
/// and previous waypoint position.
///
/// Uses haversine formula. Output in meters.
/// Analogous to `course_distance.rs` but for the previous waypoint.
use super::Calculator;
use signalk_types::PathValue;
use signalk_types::geo::haversine_meters;
use std::collections::HashMap;

pub struct PreviousPointDistance;

impl Calculator for PreviousPointDistance {
    fn name(&self) -> &str {
        "previousPointDistance"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "navigation.position",
            "navigation.course.previousPoint.position",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let pos = values.get("navigation.position")?;
        let prev = values.get("navigation.course.previousPoint.position")?;

        let lat1 = pos.get("latitude")?.as_f64()?;
        let lon1 = pos.get("longitude")?.as_f64()?;
        let lat2 = prev.get("latitude")?.as_f64()?;
        let lon2 = prev.get("longitude")?.as_f64()?;

        let distance = haversine_meters(lat1, lon1, lat2, lon2);

        Some(vec![PathValue::new(
            "navigation.course.calcValues.previousPoint.distance",
            serde_json::json!(distance),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_known() {
        let calc = PreviousPointDistance;
        let mut values = HashMap::new();
        values.insert(
            "navigation.position".to_string(),
            serde_json::json!({"latitude": 49.3200, "longitude": -123.0724}),
        );
        values.insert(
            "navigation.course.previousPoint.position".to_string(),
            serde_json::json!({"latitude": 49.2827, "longitude": -123.1207}),
        );

        let result = calc.calculate(&values).unwrap();
        assert_eq!(
            result[0].path,
            "navigation.course.calcValues.previousPoint.distance"
        );
        let distance = result[0].value.as_f64().unwrap();
        assert!(
            (5000.0..6000.0).contains(&distance),
            "Expected ~5.5km, got {distance}m"
        );
    }

    #[test]
    fn at_previous_point() {
        let calc = PreviousPointDistance;
        let mut values = HashMap::new();
        values.insert(
            "navigation.position".to_string(),
            serde_json::json!({"latitude": 49.0, "longitude": -123.0}),
        );
        values.insert(
            "navigation.course.previousPoint.position".to_string(),
            serde_json::json!({"latitude": 49.0, "longitude": -123.0}),
        );

        let result = calc.calculate(&values).unwrap();
        let distance = result[0].value.as_f64().unwrap();
        assert!(distance < 1.0, "Expected near-zero, got {distance}m");
    }

    #[test]
    fn missing_previous_returns_none() {
        let calc = PreviousPointDistance;
        let mut values = HashMap::new();
        values.insert(
            "navigation.position".to_string(),
            serde_json::json!({"latitude": 49.0, "longitude": -123.0}),
        );
        assert!(calc.calculate(&values).is_none());
    }
}
