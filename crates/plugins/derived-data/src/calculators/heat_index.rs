/// Derives `environment.outside.heatIndexTemperature` from temperature and humidity.
///
/// Uses the Steadman/Rothfusz regression (same as NWS):
///   HI = −42.379 + 2.04901523T + 10.14333127R − 0.22475541TR
///        − 6.83783e-3 T² − 5.481717e-2 R² + 1.22874e-3 T²R
///        + 8.5282e-4 TR² − 1.99e-6 T²R²
///
/// Valid for T ≥ 80°F (26.7°C / 299.8K) and RH ≥ 40%.
/// Below that threshold, returns the simple Steadman formula:
///   HI = 0.5 · (T + 61.0 + (T − 68.0) · 1.2 + RH · 0.094)
///
/// All inputs/outputs in SI (Kelvin, ratio).
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

const KELVIN_OFFSET: f64 = 273.15;

pub struct HeatIndex;

impl Calculator for HeatIndex {
    fn name(&self) -> &str {
        "heatIndex"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "environment.outside.temperature",
            "environment.outside.humidity",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let temp_k = values.get("environment.outside.temperature")?.as_f64()?;
        let humidity = values.get("environment.outside.humidity")?.as_f64()?;

        // Reject non-finite values (NaN passes ordinary range checks)
        if !temp_k.is_finite()
            || !humidity.is_finite()
            || temp_k <= 0.0
            || !(0.0..=1.0).contains(&humidity)
        {
            return None;
        }

        // Convert to Fahrenheit for the NWS formula
        let temp_c = temp_k - KELVIN_OFFSET;
        let temp_f = temp_c * 9.0 / 5.0 + 32.0;
        let rh = humidity * 100.0; // percent

        // Simple Steadman formula first
        let hi_simple = 0.5 * (temp_f + 61.0 + (temp_f - 68.0) * 1.2 + rh * 0.094);

        let hi_f = if hi_simple >= 80.0 {
            // Full Rothfusz regression
            let t = temp_f;
            let r = rh;
            let mut hi = -42.379 + 2.049_015_23 * t + 10.143_331_27 * r
                - 0.224_755_41 * t * r
                - 6.837_83e-3 * t * t
                - 5.481_717e-2 * r * r
                + 1.228_74e-3 * t * t * r
                + 8.528_2e-4 * t * r * r
                - 1.99e-6 * t * t * r * r;

            // Low-humidity adjustment
            if rh < 13.0 && (80.0..=112.0).contains(&t) {
                hi -= ((13.0 - r) / 4.0) * ((17.0 - (t - 95.0).abs()) / 17.0).sqrt();
            }
            // High-humidity adjustment
            if rh > 85.0 && (80.0..=87.0).contains(&t) {
                hi += ((r - 85.0) / 10.0) * ((87.0 - t) / 5.0);
            }
            hi
        } else {
            hi_simple
        };

        // Convert back to Kelvin
        let hi_c = (hi_f - 32.0) * 5.0 / 9.0;
        let hi_k = hi_c + KELVIN_OFFSET;

        Some(vec![PathValue::new(
            "environment.outside.heatIndexTemperature",
            serde_json::json!(hi_k),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_humid_day() {
        let calc = HeatIndex;
        let mut values = HashMap::new();
        // 35°C = 308.15K, 70% humidity → heat index should be > 35°C
        values.insert(
            "environment.outside.temperature".into(),
            serde_json::json!(308.15),
        );
        values.insert(
            "environment.outside.humidity".into(),
            serde_json::json!(0.7),
        );
        let result = calc.calculate(&values).unwrap();
        let hi_k = result[0].value.as_f64().unwrap();
        let hi_c = hi_k - 273.15;
        assert!(
            hi_c > 35.0,
            "Heat index should exceed air temp at high humidity, got {hi_c}°C"
        );
    }

    #[test]
    fn cool_day_returns_near_temp() {
        let calc = HeatIndex;
        let mut values = HashMap::new();
        // 20°C = 293.15K, 50% humidity → heat index ≈ air temp
        values.insert(
            "environment.outside.temperature".into(),
            serde_json::json!(293.15),
        );
        values.insert(
            "environment.outside.humidity".into(),
            serde_json::json!(0.5),
        );
        let result = calc.calculate(&values).unwrap();
        let hi_k = result[0].value.as_f64().unwrap();
        assert!(
            (hi_k - 293.15).abs() < 5.0,
            "At cool temps, heat index should be close to air temp"
        );
    }
}
