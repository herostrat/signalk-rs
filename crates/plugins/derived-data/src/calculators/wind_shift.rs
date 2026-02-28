/// Detects significant wind direction shifts and raises a notification.
///
/// Tracks the previous true wind direction and alerts when the direction
/// changes by more than a threshold (default: 15°/0.26 rad).
///
/// Uses interior mutability to track state across invocations.
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;
use std::sync::Mutex;

/// Wind shift threshold in radians (~15°)
const SHIFT_THRESHOLD: f64 = 0.26;

pub struct WindShift {
    prev_direction: Mutex<Option<f64>>,
}

impl Default for WindShift {
    fn default() -> Self {
        Self::new()
    }
}

impl WindShift {
    pub fn new() -> Self {
        WindShift {
            prev_direction: Mutex::new(None),
        }
    }
}

impl Calculator for WindShift {
    fn name(&self) -> &str {
        "windShift"
    }

    fn inputs(&self) -> &[&str] {
        &["environment.wind.directionTrue"]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let direction = values.get("environment.wind.directionTrue")?.as_f64()?;

        let mut prev = self.prev_direction.lock().unwrap();
        let shift = if let Some(prev_dir) = *prev {
            let mut diff = (direction - prev_dir).abs();
            if diff > std::f64::consts::PI {
                diff = 2.0 * std::f64::consts::PI - diff;
            }
            diff
        } else {
            0.0
        };

        *prev = Some(direction);

        if shift > SHIFT_THRESHOLD {
            let shift_deg = shift.to_degrees();
            Some(vec![PathValue::new(
                "notifications.environment.wind.directionChange",
                serde_json::json!({
                    "state": "alert",
                    "method": ["visual"],
                    "message": format!("Wind shift detected: {shift_deg:.0}° change")
                }),
            )])
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_shift_on_first_reading() {
        let calc = WindShift::new();
        let mut values = HashMap::new();
        values.insert(
            "environment.wind.directionTrue".into(),
            serde_json::json!(1.0),
        );
        assert!(calc.calculate(&values).is_none());
    }

    #[test]
    fn small_change_no_notification() {
        let calc = WindShift::new();
        let mut values = HashMap::new();

        values.insert(
            "environment.wind.directionTrue".into(),
            serde_json::json!(1.0),
        );
        calc.calculate(&values);

        values.insert(
            "environment.wind.directionTrue".into(),
            serde_json::json!(1.1), // 0.1 rad < 0.26 threshold
        );
        assert!(calc.calculate(&values).is_none());
    }

    #[test]
    fn large_shift_triggers_notification() {
        let calc = WindShift::new();
        let mut values = HashMap::new();

        values.insert(
            "environment.wind.directionTrue".into(),
            serde_json::json!(1.0),
        );
        calc.calculate(&values);

        values.insert(
            "environment.wind.directionTrue".into(),
            serde_json::json!(1.5), // 0.5 rad > 0.26 threshold
        );
        let result = calc.calculate(&values).unwrap();
        assert!(result[0].path.starts_with("notifications.environment.wind"));
    }

    #[test]
    fn wraps_around() {
        let calc = WindShift::new();
        let mut values = HashMap::new();

        // Direction near 2π
        values.insert(
            "environment.wind.directionTrue".into(),
            serde_json::json!(6.0),
        );
        calc.calculate(&values);

        // Direction near 0 — small actual change, not a shift
        values.insert(
            "environment.wind.directionTrue".into(),
            serde_json::json!(0.1), // actual diff ≈ 0.38 rad
        );
        let result = calc.calculate(&values);
        // 6.28 - 6.0 + 0.1 ≈ 0.38 rad > 0.26 → notification
        assert!(result.is_some());
    }
}
