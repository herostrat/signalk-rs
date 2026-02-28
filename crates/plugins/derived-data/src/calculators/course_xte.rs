/// Derives `navigation.courseGreatCircle.crossTrackError` from vessel position,
/// previous point, and next point.
///
/// Cross-track error (XTE) is the signed distance from the vessel to the
/// great-circle path between previous and next waypoints.
///
/// - Negative = vessel is left of track (steer right)
/// - Positive = vessel is right of track (steer left)
///
/// Output in meters.
use super::Calculator;
use signalk_types::PathValue;
use signalk_types::geo::cross_track_error;
use std::collections::HashMap;

pub struct CourseXte;

impl Calculator for CourseXte {
    fn name(&self) -> &str {
        "courseXte"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "navigation.position",
            "navigation.courseGreatCircle.nextPoint.position",
            "navigation.courseGreatCircle.previousPoint.position",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let pos = values.get("navigation.position")?;
        let next = values.get("navigation.courseGreatCircle.nextPoint.position")?;
        let prev = values.get("navigation.courseGreatCircle.previousPoint.position")?;

        let lat = pos.get("latitude")?.as_f64()?;
        let lon = pos.get("longitude")?.as_f64()?;
        let next_lat = next.get("latitude")?.as_f64()?;
        let next_lon = next.get("longitude")?.as_f64()?;
        let prev_lat = prev.get("latitude")?.as_f64()?;
        let prev_lon = prev.get("longitude")?.as_f64()?;

        let xte = cross_track_error((lat, lon), (prev_lat, prev_lon), (next_lat, next_lon));

        Some(vec![PathValue::new(
            "navigation.courseGreatCircle.crossTrackError",
            serde_json::json!(xte),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xte_on_track() {
        let calc = CourseXte;
        let mut values = HashMap::new();
        // Vessel exactly on the track going north along lon=-123
        values.insert(
            "navigation.position".to_string(),
            serde_json::json!({"latitude": 49.15, "longitude": -123.12}),
        );
        values.insert(
            "navigation.courseGreatCircle.previousPoint.position".to_string(),
            serde_json::json!({"latitude": 49.0, "longitude": -123.12}),
        );
        values.insert(
            "navigation.courseGreatCircle.nextPoint.position".to_string(),
            serde_json::json!({"latitude": 49.3, "longitude": -123.12}),
        );

        let result = calc.calculate(&values).unwrap();
        let xte = result[0].value.as_f64().unwrap();
        assert!(
            xte.abs() < 50.0,
            "Expected near-zero XTE on track, got {xte}m"
        );
    }

    #[test]
    fn xte_off_track() {
        let calc = CourseXte;
        let mut values = HashMap::new();
        // Vessel 1° east of track
        values.insert(
            "navigation.position".to_string(),
            serde_json::json!({"latitude": 49.15, "longitude": -122.0}),
        );
        values.insert(
            "navigation.courseGreatCircle.previousPoint.position".to_string(),
            serde_json::json!({"latitude": 49.0, "longitude": -123.0}),
        );
        values.insert(
            "navigation.courseGreatCircle.nextPoint.position".to_string(),
            serde_json::json!({"latitude": 49.3, "longitude": -123.0}),
        );

        let result = calc.calculate(&values).unwrap();
        let xte = result[0].value.as_f64().unwrap();
        assert!(
            xte.abs() > 10_000.0,
            "Expected large XTE off track, got {xte}m"
        );
    }

    #[test]
    fn missing_previous_point() {
        let calc = CourseXte;
        let mut values = HashMap::new();
        values.insert(
            "navigation.position".to_string(),
            serde_json::json!({"latitude": 49.0, "longitude": -123.0}),
        );
        values.insert(
            "navigation.courseGreatCircle.nextPoint.position".to_string(),
            serde_json::json!({"latitude": 50.0, "longitude": -123.0}),
        );
        // No previous point
        assert!(calc.calculate(&values).is_none());
    }
}
