/// Derives `tanks.{type}.{id}.currentVolume` from currentLevel × capacity.
///
/// currentLevel is a ratio (0.0–1.0), capacity is in m³.
/// currentVolume = currentLevel × capacity (m³).
///
/// Iterates over all tank types and instances found in the snapshot.
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct TankVolume;

impl Calculator for TankVolume {
    fn name(&self) -> &str {
        "tankVolume"
    }

    fn inputs(&self) -> &[&str] {
        &["tanks"]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let mut results = Vec::new();

        // Find all capacity paths to discover instances
        for (path, _) in values.iter() {
            let Some(prefix) = path.strip_suffix(".capacity") else {
                continue;
            };
            if !prefix.starts_with("tanks.") {
                continue;
            }
            let Some(capacity) = values.get(path).and_then(|v| v.as_f64()) else {
                continue;
            };
            let Some(level) = values
                .get(&format!("{prefix}.currentLevel"))
                .and_then(|v| v.as_f64())
            else {
                continue;
            };
            if capacity <= 0.0 || !(0.0..=1.0).contains(&level) {
                continue;
            }
            results.push(PathValue::new(
                format!("{prefix}.currentVolume"),
                serde_json::json!(level * capacity),
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
    fn fuel_tank() {
        let calc = TankVolume;
        let mut values = HashMap::new();
        values.insert(
            "tanks.fuel.0.capacity".into(),
            serde_json::json!(0.200), // 200L = 0.200 m³
        );
        values.insert("tanks.fuel.0.currentLevel".into(), serde_json::json!(0.75));
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result[0].path, "tanks.fuel.0.currentVolume");
        let volume = result[0].value.as_f64().unwrap();
        assert!((volume - 0.150).abs() < 0.001);
    }

    #[test]
    fn multiple_tanks() {
        let calc = TankVolume;
        let mut values = HashMap::new();
        values.insert("tanks.fuel.0.capacity".into(), serde_json::json!(0.200));
        values.insert("tanks.fuel.0.currentLevel".into(), serde_json::json!(0.5));
        values.insert(
            "tanks.freshWater.0.capacity".into(),
            serde_json::json!(0.150),
        );
        values.insert(
            "tanks.freshWater.0.currentLevel".into(),
            serde_json::json!(0.8),
        );
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn missing_level_skips() {
        let calc = TankVolume;
        let mut values = HashMap::new();
        values.insert("tanks.fuel.0.capacity".into(), serde_json::json!(0.200));
        assert!(calc.calculate(&values).is_none());
    }
}
