/// Derives `electrical.batteries.{id}.power` from voltage × current.
///
/// Iterates over all battery instances found in the snapshot.
/// Power in watts = voltage (V) × current (A).
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct BatteryPower;

impl Calculator for BatteryPower {
    fn name(&self) -> &str {
        "batteryPower"
    }

    fn inputs(&self) -> &[&str] {
        &["electrical.batteries"]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let mut results = Vec::new();

        // Discover battery instances by scanning for voltage paths
        for (path, _) in values.iter() {
            let Some(prefix) = path.strip_suffix(".voltage") else {
                continue;
            };
            if !prefix.starts_with("electrical.batteries.") {
                continue;
            }
            let Some(voltage) = values.get(path).and_then(|v| v.as_f64()) else {
                continue;
            };
            let Some(current) = values
                .get(&format!("{prefix}.current"))
                .and_then(|v| v.as_f64())
            else {
                continue;
            };
            results.push(PathValue::new(
                format!("{prefix}.power"),
                serde_json::json!(voltage * current),
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
    fn single_battery() {
        let calc = BatteryPower;
        let mut values = HashMap::new();
        values.insert(
            "electrical.batteries.0.voltage".into(),
            serde_json::json!(13.2),
        );
        values.insert(
            "electrical.batteries.0.current".into(),
            serde_json::json!(5.0),
        );
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result[0].path, "electrical.batteries.0.power");
        let power = result[0].value.as_f64().unwrap();
        assert!((power - 66.0).abs() < 0.01);
    }

    #[test]
    fn multiple_batteries() {
        let calc = BatteryPower;
        let mut values = HashMap::new();
        values.insert(
            "electrical.batteries.0.voltage".into(),
            serde_json::json!(12.8),
        );
        values.insert(
            "electrical.batteries.0.current".into(),
            serde_json::json!(10.0),
        );
        values.insert(
            "electrical.batteries.house.voltage".into(),
            serde_json::json!(13.5),
        );
        values.insert(
            "electrical.batteries.house.current".into(),
            serde_json::json!(-3.0),
        );
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn missing_current_skips() {
        let calc = BatteryPower;
        let mut values = HashMap::new();
        values.insert(
            "electrical.batteries.0.voltage".into(),
            serde_json::json!(13.2),
        );
        assert!(calc.calculate(&values).is_none());
    }
}
