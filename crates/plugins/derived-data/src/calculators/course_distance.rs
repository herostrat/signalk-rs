/// Derives `navigation.courseGreatCircle.nextPoint.distance` from vessel position
/// and next waypoint position.
///
/// Uses haversine formula. Output in meters.
use super::Calculator;
use signalk_types::PathValue;
use signalk_types::geo::haversine_meters;
use std::collections::HashMap;

pub struct CourseDistance;

impl Calculator for CourseDistance {
    fn name(&self) -> &str {
        "courseDistance"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "navigation.position",
            "navigation.courseGreatCircle.nextPoint.position",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let pos = values.get("navigation.position")?;
        let next = values.get("navigation.courseGreatCircle.nextPoint.position")?;

        let lat1 = pos.get("latitude")?.as_f64()?;
        let lon1 = pos.get("longitude")?.as_f64()?;
        let lat2 = next.get("latitude")?.as_f64()?;
        let lon2 = next.get("longitude")?.as_f64()?;

        let distance = haversine_meters(lat1, lon1, lat2, lon2);

        Some(vec![PathValue::new(
            "navigation.courseGreatCircle.nextPoint.distance",
            serde_json::json!(distance),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_known() {
        let calc = CourseDistance;
        let mut values = HashMap::new();
        values.insert(
            "navigation.position".to_string(),
            serde_json::json!({"latitude": 49.2827, "longitude": -123.1207}),
        );
        values.insert(
            "navigation.courseGreatCircle.nextPoint.position".to_string(),
            serde_json::json!({"latitude": 49.3200, "longitude": -123.0724}),
        );

        let result = calc.calculate(&values).unwrap();
        assert_eq!(
            result[0].path,
            "navigation.courseGreatCircle.nextPoint.distance"
        );
        let distance = result[0].value.as_f64().unwrap();
        // ~5.5 km
        assert!(
            (5000.0..6000.0).contains(&distance),
            "Expected ~5.5km, got {distance}m"
        );
    }

    #[test]
    fn distance_zero() {
        let calc = CourseDistance;
        let mut values = HashMap::new();
        values.insert(
            "navigation.position".to_string(),
            serde_json::json!({"latitude": 49.0, "longitude": -123.0}),
        );
        values.insert(
            "navigation.courseGreatCircle.nextPoint.position".to_string(),
            serde_json::json!({"latitude": 49.0, "longitude": -123.0}),
        );

        let result = calc.calculate(&values).unwrap();
        let distance = result[0].value.as_f64().unwrap();
        assert!(
            distance < 1.0,
            "Expected near-zero distance, got {distance}m"
        );
    }

    #[test]
    fn missing_position() {
        let calc = CourseDistance;
        let values = HashMap::new();
        assert!(calc.calculate(&values).is_none());
    }
}
