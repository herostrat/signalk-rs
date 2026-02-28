/// Derives `navigation.course.estimatedTimeOfArrival` from distance and VMG.
///
/// ETA seconds = distance / VMG
///
/// Only produces output when actively navigating to a waypoint
/// (distance and bearing to next point are available).
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct Eta;

impl Calculator for Eta {
    fn name(&self) -> &str {
        "eta"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "navigation.courseGreatCircle.nextPoint.distance",
            "navigation.courseGreatCircle.nextPoint.velocityMadeGood",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let distance = values
            .get("navigation.courseGreatCircle.nextPoint.distance")?
            .as_f64()?;
        let vmg = values
            .get("navigation.courseGreatCircle.nextPoint.velocityMadeGood")?
            .as_f64()?;

        if vmg <= 0.1 || distance < 0.0 {
            // Not making progress toward waypoint
            return None;
        }

        let eta_seconds = distance / vmg;

        Some(vec![PathValue::new(
            "navigation.course.estimatedTimeOfArrival",
            serde_json::json!(eta_seconds),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_eta() {
        let calc = Eta;
        let mut values = HashMap::new();
        // 10 NM = 18520m, 5 m/s VMG → 3704 seconds ≈ 62 min
        values.insert(
            "navigation.courseGreatCircle.nextPoint.distance".into(),
            serde_json::json!(18520.0),
        );
        values.insert(
            "navigation.courseGreatCircle.nextPoint.velocityMadeGood".into(),
            serde_json::json!(5.0),
        );
        let result = calc.calculate(&values).unwrap();
        let eta_s = result[0].value.as_f64().unwrap();
        assert!((eta_s - 3704.0).abs() < 1.0);
    }

    #[test]
    fn no_progress_returns_none() {
        let calc = Eta;
        let mut values = HashMap::new();
        values.insert(
            "navigation.courseGreatCircle.nextPoint.distance".into(),
            serde_json::json!(18520.0),
        );
        values.insert(
            "navigation.courseGreatCircle.nextPoint.velocityMadeGood".into(),
            serde_json::json!(0.0),
        );
        assert!(calc.calculate(&values).is_none());
    }
}
